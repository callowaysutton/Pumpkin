use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Potion;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        Controls, Goal, active_target::ActiveTargetGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, ranged_attack::RangedAttackGoal, revenge::RevengeGoal,
        swim::SwimGoal, to_goal_ticks, wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity, RangedAttackMob,
        patrol::{PatrolData, PatrollingMonster},
        raider::{
            PathfindToRaidGoal, Raider, RaiderCelebrationGoal, RaiderData,
            RaiderMoveThroughVillageGoal,
        },
    },
    projectile::splash_potion::SplashPotionEntity,
};

fn create_potion_stack(item: &'static Item, potion: &'static Potion) -> ItemStack {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{DataComponentImpl, PotionContentsImpl};
    let mut stack = ItemStack::new(1, item);
    stack.patch.push((
        DataComponent::PotionContents,
        Some(
            PotionContentsImpl {
                potion_id: Some(i32::from(potion.id)),
                custom_color: None,
                custom_effects: Vec::new(),
                custom_name: None,
            }
            .to_dyn(),
        ),
    ));
    stack
}

/// Vanilla `NearestHealableRaiderTargetGoal`: finds a raider to throw a healing potion at.
///
/// Vanilla overrides `canUse` and drops the 500 tick reciprocal interval of
/// `NearestAttackableTargetGoal` entirely; only the coin flip below rate limits the search.
/// The search therefore stays active while the mob has a raid, and starts a 200 tick cooldown so
/// the witch does not immediately pick a new patient. While that cooldown runs the witch is not
/// allowed to target players, see [`WitchAttackPlayersGoal`].
pub struct WitchHealRaidersGoal {
    inner: Box<ActiveTargetGoal>,
    cooldown: Arc<AtomicI32>,
}

impl WitchHealRaidersGoal {
    const COOLDOWN_TICKS: i32 = 200;

    #[must_use]
    pub fn new(mob: &MobEntity, cooldown: Arc<AtomicI32>) -> Self {
        Self {
            // Vanilla selects `Raider.class` with `!target.is(EntityTypes.WITCH)` and no reciprocal
            // interval (`NearestHealableRaiderTargetGoal.canUse` bypasses `super.canUse`). A
            // reciprocal chance of 0 disables that gate; the coin flip in `can_start` is the only
            // rate limiting, exactly like vanilla. Raiders are several entity types, so filter on
            // the tag instead of a single class.
            inner: ActiveTargetGoal::predicated(mob, 0, true, |target, _world| {
                target
                    .entity
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
                    && target.entity.entity_type != &EntityType::WITCH
            }),
            cooldown,
        }
    }
}

impl Goal for WitchHealRaidersGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.cooldown.load(Ordering::Relaxed) > 0 {
            return false;
        }
        // Vanilla `NearestHealableRaiderTargetGoal.canUse` flips a coin before searching.
        if !mob.get_random().random_bool(0.5) {
            return false;
        }
        if mob
            .as_raider()
            .is_none_or(|raider| !raider.has_active_raid())
        {
            return false;
        }
        self.inner.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
        self.cooldown
            .store(to_goal_ticks(Self::COOLDOWN_TICKS), Ordering::Relaxed);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}

/// Vanilla `NearestAttackableWitchTargetGoal`: the witch's player target, suppressed while the
/// heal cooldown of [`WitchHealRaidersGoal`] is ticking down.
pub struct WitchAttackPlayersGoal {
    inner: Box<ActiveTargetGoal>,
    can_attack: Arc<AtomicBool>,
}

impl WitchAttackPlayersGoal {
    #[must_use]
    pub fn new(mob: &MobEntity, can_attack: Arc<AtomicBool>) -> Self {
        Self {
            inner: ActiveTargetGoal::with_default(mob, &EntityType::PLAYER, true),
            can_attack,
        }
    }
}

impl Goal for WitchAttackPlayersGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.can_attack.load(Ordering::Relaxed) && self.inner.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        // Vanilla `TargetGoal.canContinueToUse` does not re-check the attack flag.
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}

/// Represents a Witch, a hostile ranged mob that throws splash potions and drinks restorative potions.
///
/// Wiki: <https://minecraft.wiki/w/Witch>
pub struct WitchEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    drinking_potion: AtomicBool,
    using_time: AtomicI32,
    /// Mirrors vanilla `Witch.healRaidersGoal.getCooldown()`.
    heal_raiders_cooldown: Arc<AtomicI32>,
    /// Mirrors vanilla `Witch.attackPlayersGoal.canAttack`.
    can_attack_players: Arc<AtomicBool>,
}

impl WitchEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let heal_raiders_cooldown = Arc::new(AtomicI32::new(0));
        let can_attack_players = Arc::new(AtomicBool::new(true));
        let witch = Self {
            mob_entity,
            raider_data: RaiderData::default(),
            drinking_potion: AtomicBool::new(false),
            using_time: AtomicI32::new(0),
            heal_raiders_cooldown: heal_raiders_cooldown.clone(),
            can_attack_players: can_attack_players.clone(),
        };
        let mob_arc = Arc::new(witch);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let ranged_weak: Weak<dyn RangedAttackMob> = {
            let ranged_arc: Arc<dyn RangedAttackMob> = mob_arc.clone();
            Arc::downgrade(&ranged_arc)
        };

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(1, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                2,
                Box::new(RangedAttackGoal::new(ranged_weak, 1.0, 60, 10.0)),
            );
            goal_selector.add_goal(3, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(3, Box::new(PathfindToRaidGoal::default()));
            goal_selector.add_goal(4, Box::new(RaiderCelebrationGoal));
            goal_selector.add_goal(4, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                5,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(6, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(
                1,
                Box::new(RevengeGoal::new(true).ignoring(|entity_type| {
                    entity_type.has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
                })),
            );
            // Vanilla: heal raiders at 2, attack players at 3. Both claim TARGET, so the
            // lower priority heal goal wins while it has a patient.
            target_selector.add_goal(
                2,
                Box::new(WitchHealRaidersGoal::new(
                    &mob_arc.mob_entity,
                    heal_raiders_cooldown,
                )),
            );
            target_selector.add_goal(
                3,
                Box::new(WitchAttackPlayersGoal::new(
                    &mob_arc.mob_entity,
                    can_attack_players,
                )),
            );
        };

        mob_arc
    }

    pub fn set_drinking_potion(&self, drinking: bool) {
        self.drinking_potion.store(drinking, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(tracked_data::witch::DATA_USING_ITEM, drinking);
    }

    #[must_use]
    pub fn is_drinking_potion(&self) -> bool {
        self.drinking_potion.load(Ordering::Relaxed)
    }

    pub fn throw_potion(&self, target: &Arc<dyn EntityBase>) {
        if self.is_drinking_potion() {
            return;
        }

        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load_full();

        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        let target_vel = target_entity.velocity.load();
        let witch_pos = entity.pos.load();

        let xd = target_pos.x + target_vel.x - witch_pos.x;
        let yd = target_pos.y + target_entity.get_eye_height() - 1.1 - witch_pos.y;
        let zd = target_pos.z + target_vel.z - witch_pos.z;
        let dist = xd.hypot(zd);

        let mut potion = &Potion::HARMING;

        if let Some(target_living) = target.get_living_entity() {
            let r: f32 = rand::random();
            if target.get_mob().and_then(|mob| mob.as_raider()).is_some() {
                // Vanilla `Witch.performRangedAttack` heals raiders and gives up the target.
                potion = if target_living.health.load() <= 4.0 {
                    &Potion::HEALING
                } else {
                    &Potion::REGENERATION
                };
                self.mob_entity.set_target(None);
            } else if dist >= 8.0 && !target_living.has_effect(&StatusEffect::SLOWNESS) {
                potion = &Potion::SLOWNESS;
            } else if target_living.health.load() >= 8.0
                && !target_living.has_effect(&StatusEffect::POISON)
            {
                potion = &Potion::POISON;
            } else if dist <= 3.0 && !target_living.has_effect(&StatusEffect::WEAKNESS) && r < 0.25
            {
                potion = &Potion::WEAKNESS;
            }
        }

        let potion_stack = create_potion_stack(&Item::SPLASH_POTION, potion);

        let splash_entity = Entity::new(world.clone(), witch_pos, &EntityType::SPLASH_POTION);
        let splash = SplashPotionEntity::new_shot(splash_entity, entity);
        splash.set_item_stack(potion_stack);

        let speed = if dist <= 2.0 { 0.45 } else { 0.75 };
        let yo = dist * 0.2;

        splash.thrown.set_velocity(xd, yd + yo, zd, speed, 8.0);

        if !entity.silent.load(Ordering::Relaxed) {
            world.play_sound(Sound::EntityWitchThrow, SoundCategory::Hostile, &witch_pos);
        }

        let splash_arc: Arc<dyn EntityBase> = Arc::new(splash);
        world.spawn_entity(splash_arc);
    }
}

impl Mob for WitchEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut pumpkin_nbt::compound::NbtCompound) {
        self.write_raider_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &pumpkin_nbt::compound::NbtCompound) {
        self.read_raider_nbt(nbt);
    }

    fn pre_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) -> bool {
        if let Some(src) = source
            && src.get_entity().entity_id == self.mob_entity.living_entity.entity.entity_id
        {
            return false;
        }
        true
    }

    fn modify_incoming_damage(&self, mut amount: f32, damage_type: DamageType) -> f32 {
        // Vanilla `Witch.getDamageAfterMagicAbsorb`.
        if damage_type.has_tag(&tag::DamageType::MINECRAFT_WITCH_RESISTANT_TO) {
            amount *= 0.15;
        }
        amount
    }

    fn mob_tick(&self, caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        let living = &self.mob_entity.living_entity;
        let world = entity.world.load();

        // Vanilla `Witch.aiStep`: the heal cooldown doubles as the gate on player targeting.
        // Vanilla `decrementCooldown` is a plain `cooldown--`; clamp at 0 so a long-lived witch
        // cannot wrap the counter around to positive after 2^31 ticks. Both the tick and the
        // goal run on the entity's tick thread, so a plain load/store is enough.
        let cooldown = self.heal_raiders_cooldown.load(Ordering::Relaxed);
        let cooldown = (cooldown - 1).max(0);
        self.heal_raiders_cooldown
            .store(cooldown, Ordering::Relaxed);
        self.can_attack_players
            .store(cooldown <= 0, Ordering::Relaxed);

        if self.is_drinking_potion() {
            let remaining = self.using_time.fetch_sub(1, Ordering::Relaxed) - 1;
            if remaining <= 0 {
                self.set_drinking_potion(false);
                if let Some(witch) = caller.cast_any().downcast_ref::<Self>() {
                    let living = &witch.mob_entity.living_entity;
                    let mut equipment = living
                        .entity_equipment
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let stack = equipment.get(&EquipmentSlot::MAIN_HAND);
                    equipment.put(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
                    drop(equipment);
                    living.send_equipment_changes(&[(
                        EquipmentSlot::MAIN_HAND,
                        ItemStack::EMPTY.clone(),
                    )]);

                    let effects = crate::item::potion::PotionContents::read_potion_effects(&stack);
                    crate::item::potion::PotionContents::apply_effects_to(
                        living,
                        effects,
                        1.0,
                        crate::item::potion::PotionApplicationSource::Normal,
                    );
                }
            }
        } else {
            let mut potion: Option<&'static Potion> = None;
            let r: f32 = rand::random();

            if r < 0.15
                && entity.touching_water.load(Ordering::Relaxed)
                && !living.has_effect(&StatusEffect::WATER_BREATHING)
            {
                potion = Some(&Potion::WATER_BREATHING);
            } else if r < 0.15
                && entity.fire_ticks.load(Ordering::Relaxed) > 0
                && !living.has_effect(&StatusEffect::FIRE_RESISTANCE)
            {
                potion = Some(&Potion::FIRE_RESISTANCE);
            } else if r < 0.05 && living.health.load() < living.get_max_health() {
                potion = Some(&Potion::HEALING);
            } else if r < 0.5
                && let Some(target) = self.mob_entity.get_target()
                && !living.has_effect(&StatusEffect::SPEED)
            {
                let target_pos = target.get_entity().pos.load();
                let self_pos = entity.pos.load();
                if self_pos.squared_distance_to_vec(&target_pos) > 121.0 {
                    potion = Some(&Potion::SWIFTNESS);
                }
            }

            if let Some(potion) = potion {
                let stack = create_potion_stack(&Item::POTION, potion);
                if let Some(witch) = caller.cast_any().downcast_ref::<Self>() {
                    let living = &witch.mob_entity.living_entity;
                    let mut equipment = living
                        .entity_equipment
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    equipment.put(&EquipmentSlot::MAIN_HAND, stack.clone());
                    drop(equipment);
                    living.send_equipment_changes(&[(EquipmentSlot::MAIN_HAND, stack)]);
                }

                self.using_time.store(32, Ordering::Relaxed);
                self.set_drinking_potion(true);

                if !entity.silent.load(Ordering::Relaxed) {
                    let pos = entity.pos.load();
                    world.play_sound(Sound::EntityWitchDrink, SoundCategory::Hostile, &pos);
                }
            }
        }
    }
}

impl RangedAttackMob for WitchEntity {
    fn perform_ranged_attack(&self, target: &Arc<dyn EntityBase>, _power: f32) {
        self.throw_potion(target);
    }
}

impl PatrollingMonster for WitchEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }

    fn can_be_leader(&self) -> bool {
        false
    }
}

impl Raider for WitchEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityWitchCelebrate
    }
}

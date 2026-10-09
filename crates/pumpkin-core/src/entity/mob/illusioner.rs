use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, AtomicU8, Ordering},
};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::difficulty::Difficulty;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        Controls, Goal, active_target::ActiveTargetGoal, avoid_entity::AvoidEntityGoal,
        bow_attack::BowAttackGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, revenge::RevengeGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        equipment::RegionalDifficulty,
        evoker::IllagerSpell,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            ObtainRaidLeaderBannerGoal, PathfindToRaidGoal, Raider, RaiderCelebrationGoal,
            RaiderData, RaiderMoveThroughVillageGoal,
        },
        spawn::SpawnGroupData,
    },
};

use crate::world::World;

pub struct IllusionerEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    spell_casting_tick_count: AtomicI32,
    current_spell: AtomicU8,
}

impl IllusionerEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let illusioner = Self {
            mob_entity,
            raider_data: RaiderData::default(),
            spell_casting_tick_count: AtomicI32::new(0),
            current_spell: AtomicU8::new(IllagerSpell::None as u8),
        };
        let mob_arc = Arc::new(illusioner);
        let illusioner_weak: Weak<Self> = Arc::downgrade(&mob_arc);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            // Insertion order mirrors vanilla's `registerGoals` call chain
            // (Raider → PatrollingMonster → Illusioner) because ties break by insertion.
            goal_selector.add_goal(1, Box::new(ObtainRaidLeaderBannerGoal));
            goal_selector.add_goal(
                1,
                Box::new(IllusionerCastingSpellGoal::new(illusioner_weak.clone())),
            );
            goal_selector.add_goal(3, Box::new(PathfindToRaidGoal::default()));
            goal_selector.add_goal(
                3,
                Box::new(AvoidEntityGoal::new(&EntityType::CREAKING, 8.0, 1.0, 1.2)),
            );
            goal_selector.add_goal(4, Box::new(LongDistancePatrolGoal::new(0.7, 0.595)));
            goal_selector.add_goal(4, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(
                4,
                Box::new(IllusionerMirrorSpellGoal::new(illusioner_weak.clone())),
            );
            goal_selector.add_goal(5, Box::new(RaiderCelebrationGoal));
            goal_selector.add_goal(
                5,
                Box::new(IllusionerBlindnessSpellGoal::new(illusioner_weak)),
            );
            goal_selector.add_goal(6, Box::new(BowAttackGoal::new(0.5, 20, 15.0)));
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(0.6)));
            goal_selector.add_goal(
                9,
                Box::new(LookAtEntityGoal::new(
                    mob_weak,
                    &EntityType::PLAYER,
                    3.0,
                    1.0,
                    true,
                )),
            );
            // Vanilla goal 10 is `LookAtPlayerGoal(Mob.class, 8.0)`, which the shared
            // look-at goal can't express yet; the random look stands in like the evoker.
            goal_selector.add_goal(10, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            target_selector.add_goal(
                1,
                Box::new(
                    RevengeGoal::new(true)
                        .ignoring(|entity_type| {
                            entity_type.has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
                        })
                        .alerting_others(),
                ),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true)
                    .set_unseen_memory_ticks(300),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::VILLAGER, false)
                    .set_unseen_memory_ticks(300),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::IRON_GOLEM, false)
                    .set_unseen_memory_ticks(300),
            );
        };

        mob_arc
    }

    pub fn is_casting_spell(&self) -> bool {
        self.spell_casting_tick_count.load(Ordering::Relaxed) > 0
    }

    pub fn set_is_casting_spell(&self, spell: IllagerSpell) {
        self.current_spell.store(spell as u8, Ordering::Relaxed);
        let entity = &self.mob_entity.living_entity.entity;
        entity.set_synced_data(
            tracked_data::illusioner::SPELL_CASTING_ID,
            spell as u8 as i8,
        );
    }

    pub fn get_current_spell(&self) -> IllagerSpell {
        IllagerSpell::from_u8(self.current_spell.load(Ordering::Relaxed))
    }

    pub fn get_spell_casting_time(&self) -> i32 {
        self.spell_casting_tick_count.load(Ordering::Relaxed)
    }

    pub fn set_spell_casting_time(&self, ticks: i32) {
        self.spell_casting_tick_count
            .store(ticks, Ordering::Relaxed);
    }
}

impl Mob for IllusionerEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }

    /// Vanilla `Illusioner.finalizeSpawn`: a bow in the main hand before the shared finalize step.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let living = &self.mob_entity.living_entity;
        if let Ok(mut equipment) = living.entity_equipment.lock() {
            equipment.put(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, &Item::BOW));
        }
        self.mob_entity.finalize_spawn_base();
        group_data
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let ticks = self.spell_casting_tick_count.load(Ordering::Relaxed);
        if ticks > 0 {
            self.spell_casting_tick_count
                .store(ticks - 1, Ordering::Relaxed);
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);
        nbt.put_int("SpellTicks", self.get_spell_casting_time());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);
        if let Some(ticks) = nbt.get_int("SpellTicks") {
            self.set_spell_casting_time(ticks);
        }
    }
}

impl PatrollingMonster for IllusionerEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }
}

impl Raider for IllusionerEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityIllusionerAmbient
    }
}

/// Vanilla `SpellcasterIllager.SpellcasterCastingSpellGoal` (shared by the evoker, which
/// keeps its own copy): hold still and watch the target while the spell casting ticks run.
pub struct IllusionerCastingSpellGoal {
    illusioner: Weak<IllusionerEntity>,
}

impl IllusionerCastingSpellGoal {
    #[must_use]
    pub const fn new(illusioner: Weak<IllusionerEntity>) -> Self {
        Self { illusioner }
    }
}

impl Goal for IllusionerCastingSpellGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return false;
        };
        illusioner.is_casting_spell()
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return false;
        };
        illusioner.is_casting_spell()
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        if let Some(illusioner) = self.illusioner.upgrade() {
            illusioner.set_is_casting_spell(IllagerSpell::None);
        }
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return;
        };
        let Some(target) = illusioner.mob_entity.get_target() else {
            return;
        };
        // Vanilla looks with `getMaxHeadYRot()`/`getMaxHeadXRot()` (75/40).
        illusioner
            .mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 75.0, 40.0);
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// Vanilla `Illusioner.IllusionerMirrorSpellGoal` (a `SpellcasterUseSpellGoal`): cast
/// `DISAPPEAR` to turn invisible.
pub struct IllusionerMirrorSpellGoal {
    illusioner: Weak<IllusionerEntity>,
    warmup_delay: i32,
    next_attack_tick: i32,
}

impl IllusionerMirrorSpellGoal {
    #[must_use]
    pub const fn new(illusioner: Weak<IllusionerEntity>) -> Self {
        Self {
            illusioner,
            warmup_delay: 0,
            next_attack_tick: 0,
        }
    }
}

impl Goal for IllusionerMirrorSpellGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return false;
        };
        if illusioner.is_casting_spell() {
            return false;
        }
        let entity = &illusioner.mob_entity.living_entity.entity;
        if entity.age.load(Ordering::Relaxed) < self.next_attack_tick {
            return false;
        }
        let Some(target) = illusioner.mob_entity.get_target() else {
            return false;
        };
        if !target.get_entity().is_alive() {
            return false;
        }
        !illusioner
            .mob_entity
            .living_entity
            .has_effect(&StatusEffect::INVISIBILITY)
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return false;
        };
        self.warmup_delay > 0
            && illusioner
                .mob_entity
                .get_target()
                .is_some_and(|target| target.get_entity().is_alive())
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.warmup_delay = 20;
        let Some(illusioner) = self.illusioner.upgrade() else {
            return;
        };
        let entity = &illusioner.mob_entity.living_entity.entity;
        illusioner.set_spell_casting_time(20);
        self.next_attack_tick = entity.age.load(Ordering::Relaxed) + 340;
        illusioner.set_is_casting_spell(IllagerSpell::Disappear);
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityIllusionerPrepareMirror,
            SoundCategory::Hostile,
            &entity.pos.load(),
        );
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        self.warmup_delay -= 1;
        if self.warmup_delay != 0 {
            return;
        }
        let Some(illusioner) = self.illusioner.upgrade() else {
            return;
        };
        let entity = &illusioner.mob_entity.living_entity.entity;
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityIllusionerCastSpell,
            SoundCategory::Hostile,
            &entity.pos.load(),
        );

        illusioner.mob_entity.living_entity.add_effect(Effect {
            effect_type: &StatusEffect::INVISIBILITY,
            duration: 1200,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        });
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

/// Vanilla `Illusioner.IllusionerBlindnessSpellGoal` (a `SpellcasterUseSpellGoal`): cast
/// `BLINDNESS` on the current target, once per target.
pub struct IllusionerBlindnessSpellGoal {
    illusioner: Weak<IllusionerEntity>,
    warmup_delay: i32,
    next_attack_tick: i32,
    last_target_id: i32,
}

impl IllusionerBlindnessSpellGoal {
    #[must_use]
    pub const fn new(illusioner: Weak<IllusionerEntity>) -> Self {
        Self {
            illusioner,
            warmup_delay: 0,
            next_attack_tick: 0,
            last_target_id: 0,
        }
    }
}

impl Goal for IllusionerBlindnessSpellGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return false;
        };
        if illusioner.is_casting_spell() {
            return false;
        }
        let entity = &illusioner.mob_entity.living_entity.entity;
        if entity.age.load(Ordering::Relaxed) < self.next_attack_tick {
            return false;
        }
        let Some(target) = illusioner.mob_entity.get_target() else {
            return false;
        };
        if target.get_entity().entity_id == self.last_target_id {
            return false;
        }
        // Vanilla: the effective difficulty at the block position must beat
        // `Difficulty.NORMAL.ordinal()` (2.0).
        let world = entity.world.load();
        let difficulty = RegionalDifficulty::at(&world, entity.pos.load());
        difficulty.effective_difficulty > f32::from(Difficulty::Normal as u8)
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return false;
        };
        self.warmup_delay > 0
            && illusioner
                .mob_entity
                .get_target()
                .is_some_and(|target| target.get_entity().is_alive())
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.warmup_delay = 20;
        let Some(illusioner) = self.illusioner.upgrade() else {
            return;
        };
        let entity = &illusioner.mob_entity.living_entity.entity;
        illusioner.set_spell_casting_time(20);
        let age = entity.age.load(Ordering::Relaxed);
        self.next_attack_tick = age + 180;
        if let Some(target) = illusioner.mob_entity.get_target() {
            self.last_target_id = target.get_entity().entity_id;
        }
        illusioner.set_is_casting_spell(IllagerSpell::Blindness);
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityIllusionerPrepareBlindness,
            SoundCategory::Hostile,
            &entity.pos.load(),
        );
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        self.warmup_delay -= 1;
        if self.warmup_delay != 0 {
            return;
        }
        let Some(illusioner) = self.illusioner.upgrade() else {
            return;
        };
        let entity = &illusioner.mob_entity.living_entity.entity;
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityIllusionerCastSpell,
            SoundCategory::Hostile,
            &entity.pos.load(),
        );

        if let Some(target) = illusioner.mob_entity.get_target()
            && let Some(living) = target.get_living_entity()
        {
            living.add_effect(Effect {
                effect_type: &StatusEffect::BLINDNESS,
                duration: 400,
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: true,
            });
        }
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

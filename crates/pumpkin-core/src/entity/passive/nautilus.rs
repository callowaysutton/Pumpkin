use crossbeam::atomic::AtomicCell;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};
use uuid::Uuid;

use pumpkin_data::{
    effect::StatusEffect,
    entity::EntityStatus,
    item::Item,
    item_stack::ItemStack,
    particle::Particle,
    potion::Effect,
    sound::{Sound, SoundCategory},
    tag::{self, Taggable},
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, escape_danger::EscapeDangerGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, revenge::RevengeGoal,
        swim::SwimGoal, tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    custom_sound::CustomSound,
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

/// `minecraft:nautilus_food`, the items a nautilus follows.
///
/// Item lists are static in Pumpkin's goal system; the tag stays the source of truth for
/// [`Animal::is_food`].
const TEMPT_ITEMS: &[&Item] = &[
    &Item::COD,
    &Item::COOKED_COD,
    &Item::SALMON,
    &Item::COOKED_SALMON,
    &Item::PUFFERFISH,
    &Item::TROPICAL_FISH,
    &Item::PUFFERFISH_BUCKET,
    &Item::COD_BUCKET,
    &Item::SALMON_BUCKET,
    &Item::TROPICAL_FISH_BUCKET,
];

pub struct NautilusEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub is_tame: AtomicBool,
    pub owner: AtomicCell<Option<Uuid>>,
    pub is_dashing: AtomicBool,
    pub dash_cooldown: AtomicI32,
    pub is_saddled: AtomicBool,
    pub inventory: Mutex<Vec<ItemStack>>,
}

impl NautilusEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let nautilus = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            is_tame: AtomicBool::new(false),
            owner: AtomicCell::new(None),
            is_dashing: AtomicBool::new(false),
            dash_cooldown: AtomicI32::new(0),
            is_saddled: AtomicBool::new(false),
            inventory: Mutex::new(vec![ItemStack::new(0, &pumpkin_data::item::Item::AIR); 9]),
        };
        let mob_arc = Arc::new(nautilus);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        // Ported from `NautilusAi`. The brain activities map onto Pumpkin's goal system:
        // CORE -> Swim/EscapeDanger, IDLE -> Breed/Tempt/Wander/LookAround, FIGHT -> target
        // selector (Revenge) plus the vanilla melee attack tick.
        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.6));
            goal_selector.add_goal(2, BreedGoal::new(0.4));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.3, TEMPT_ITEMS, false)));
            // Vanilla's FIGHT activity uses a ChargeAttack; Pumpkin has no charge primitive yet,
            // so the closest equivalent is the standard melee goal at the same 0.6 speed.
            goal_selector.add_goal(4, Box::new(MeleeAttackGoal::new(0.6, true)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(
                    mob_weak,
                    &pumpkin_data::entity::EntityType::PLAYER,
                    6.0,
                ),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
        };

        {
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
        };

        mob_arc
    }

    pub fn is_dashing(&self) -> bool {
        self.is_dashing.load(Ordering::Relaxed)
    }

    pub fn set_dashing(&self, dashing: bool) {
        self.is_dashing.store(dashing, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::nautilus::DASH, dashing);
    }

    pub fn is_tame(&self) -> bool {
        self.is_tame.load(Ordering::Relaxed)
    }

    pub fn set_tame(&self, tame: bool, owner: Option<Uuid>) {
        self.is_tame.store(tame, Ordering::Relaxed);
        self.owner.store(owner);
    }

    pub fn get_ambient_sound(&self) -> Sound {
        let is_baby = self
            .mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0;
        let is_water = self
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed);
        if is_baby {
            if is_water {
                Sound::EntityBabyNautilusAmbient
            } else {
                Sound::EntityBabyNautilusAmbientLand
            }
        } else if is_water {
            Sound::EntityNautilusAmbient
        } else {
            Sound::EntityNautilusAmbientLand
        }
    }

    pub fn get_dash_sound(&self) -> Sound {
        let is_water = self
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed);
        if is_water {
            Sound::EntityNautilusDash
        } else {
            Sound::EntityNautilusDashLand
        }
    }

    pub fn get_dash_ready_sound(&self) -> Sound {
        let is_water = self
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed);
        if is_water {
            Sound::EntityNautilusDashReady
        } else {
            Sound::EntityNautilusDashReadyLand
        }
    }

    pub fn get_eat_sound(&self) -> Sound {
        let is_baby = self
            .mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0;
        if is_baby {
            Sound::EntityBabyNautilusEat
        } else {
            Sound::EntityNautilusEat
        }
    }

    pub fn get_swim_sound(&self) -> Sound {
        let is_baby = self
            .mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0;
        if is_baby {
            Sound::EntityBabyNautilusSwim
        } else {
            Sound::EntityNautilusSwim
        }
    }
}

impl AgeableMob for NautilusEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for NautilusEntity {
    /// `AbstractNautilus.isFood`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        if !self.is_tame() && !self.is_baby() {
            item_stack
                .item
                .has_tag(&tag::Item::MINECRAFT_NAUTILUS_TAMING_ITEMS)
        } else {
            item_stack.item.has_tag(&tag::Item::MINECRAFT_NAUTILUS_FOOD)
        }
    }
}

impl CustomSound for NautilusEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        let is_water = entity.touching_water.load(Ordering::Relaxed);
        Some(if is_baby {
            if is_water {
                Sound::EntityBabyNautilusHurt
            } else {
                Sound::EntityBabyNautilusHurtLand
            }
        } else if is_water {
            Sound::EntityNautilusHurt
        } else {
            Sound::EntityNautilusHurtLand
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        let is_water = entity.touching_water.load(Ordering::Relaxed);
        Some(if is_baby {
            if is_water {
                Sound::EntityBabyNautilusDeath
            } else {
                Sound::EntityBabyNautilusDeathLand
            }
        } else if is_water {
            Sound::EntityNautilusDeath
        } else {
            Sound::EntityNautilusDeathLand
        })
    }
}

impl Mob for NautilusEntity {
    fn as_custom_sound(&self) -> Option<&dyn crate::entity::custom_sound::CustomSound> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_bool("IsTame", self.is_tame.load(Ordering::Relaxed));
        nbt.put_bool("Saddled", self.is_saddled.load(Ordering::Relaxed));
        nbt.put_int("DashCooldown", self.dash_cooldown.load(Ordering::Relaxed));
        if let Some(owner) = self.owner.load() {
            nbt.put_uuid("Owner", owner);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(is_tame) = nbt.get_bool("IsTame") {
            self.is_tame.store(is_tame, Ordering::Relaxed);
        }
        if let Some(saddled) = nbt.get_bool("Saddled") {
            self.is_saddled.store(saddled, Ordering::Relaxed);
        }
        if let Some(dash) = nbt.get_int("DashCooldown") {
            self.dash_cooldown.store(dash, Ordering::Relaxed);
        }
        if let Some(owner) = nbt.get_uuid("Owner") {
            self.owner.store(Some(owner));
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        self.mob_entity.living_entity.entity.set_synced_data(
            pumpkin_data::tracked_data::nautilus::DASH,
            self.is_dashing(),
        );
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
        let entity = &self.mob_entity.living_entity.entity;

        let Ok(passengers) = entity.passengers.try_lock() else {
            return;
        };
        if let Some(passenger) = passengers.first()
            && let Some(player) = passenger.cast_any().downcast_ref::<Player>()
        {
            let world = entity.world.load();
            let game_time = world.get_world_age();
            if game_time % 40 == 0 {
                let player_arc = world.get_player_by_uuid(player.gameprofile.id);
                if let Some(p) = player_arc {
                    p.add_effect(Effect {
                        effect_type: &StatusEffect::BREATH_OF_THE_NAUTILUS,
                        duration: 60,
                        amplifier: 0,
                        ambient: true,
                        show_particles: true,
                        show_icon: true,
                        blend: true,
                    });
                }
            }
        }

        if self.is_dashing() && self.dash_cooldown.load(Ordering::Relaxed) < 35 {
            self.set_dashing(false);
        }

        let cooldown = self.dash_cooldown.load(Ordering::Relaxed);
        if cooldown > 0 {
            let next = cooldown - 1;
            self.dash_cooldown.store(next, Ordering::Relaxed);
            if next == 0 {
                let world = entity.world.load();
                world.play_sound(
                    self.get_dash_ready_sound(),
                    SoundCategory::Neutral,
                    &entity.pos.load(),
                );
            }
        }

        if entity.touching_water.load(Ordering::Relaxed) {
            let velo = entity.velocity.load();
            let speed = velo.length();
            let prob = (speed * 2.0).clamp(0.15, 1.0);
            if rand::random::<f64>() < prob {
                let world = entity.world.load();
                let pos = entity.pos.load();
                world.spawn_particle(
                    pos + Vector3::new(0.0, 0.25, 0.0),
                    Vector3::new(0.4, 0.4, 0.4),
                    0.5,
                    2,
                    Particle::Bubble,
                );
            }
        }
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let mob_entity = &self.mob_entity;
        let entity = &mob_entity.living_entity.entity;

        // Babies are only fed through the shared animal interaction, like vanilla's early return
        // into `Animal.mobInteract` (which ages them up).
        if self.is_baby() {
            return self.animal_interact(player, item_stack, self.get_ambient_sound());
        }

        // Taming has priority over riding, and only applies to untamed adults.
        if !self.is_tame() && self.is_food(item_stack) {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            if rand::random::<u32>().is_multiple_of(3) {
                self.set_tame(true, Some(player.gameprofile.id));
                mob_entity
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stop();
                let world = entity.world.load();
                world.send_entity_status(entity, EntityStatus::TamingSucceeded, None);
            } else {
                let world = entity.world.load();
                world.send_entity_status(entity, EntityStatus::TamingFailed, None);
            }
            let world = entity.world.load();
            world.play_sound(
                self.get_eat_sound(),
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
            return true;
        }

        // Tamed adults eat their food to heal, then can be saddled and ridden.
        if self.is_tame() && self.is_food(item_stack) && entity.is_alive() {
            let living = &mob_entity.living_entity;
            if living.health.load() < living.get_max_health() {
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                living.heal(2.0);
                self.play_eating_sound(self.get_eat_sound());
                return true;
            }
        }

        if self.is_tame() && !player.get_entity().is_sneaking() {
            if !self.is_saddled.load(Ordering::Relaxed)
                && item_stack.item == &pumpkin_data::item::Item::SADDLE
            {
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                self.is_saddled.store(true, Ordering::Relaxed);
                let world = entity.world.load();
                world.play_sound(
                    Sound::ItemNautilusSaddleEquip,
                    SoundCategory::Neutral,
                    &entity.pos.load(),
                );
                return true;
            }

            if !self.is_food(item_stack) {
                let world = player.world();
                if let Some(vehicle) = world.get_entity_by_id(entity.entity_id)
                    && let Some(passenger) = world.get_player_by_id(player.entity_id())
                {
                    entity.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
                    return true;
                }
            }
        }

        self.animal_interact(player, item_stack, self.get_ambient_sound())
    }

    fn is_saddled(&self) -> bool {
        self.is_saddled.load(Ordering::Relaxed)
    }

    fn can_be_saddled(&self) -> bool {
        self.mob_entity.living_entity.entity.is_alive()
    }

    fn set_saddled(&self, saddled: bool) {
        self.is_saddled.store(saddled, Ordering::Relaxed);
    }
}

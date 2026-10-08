use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::{
    entity::EntityType,
    item_stack::ItemStack,
    sound::{Sound, SoundCategory},
    tag::Taggable,
    zombie_nautilus_variant::ZombieNautilusVariant,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        active_target::ActiveTargetGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    custom_sound::CustomSound,
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

/// A hostile nautilus variant introduced in 26.2.
///
/// Wiki: <https://minecraft.wiki/w/Zombie_Nautilus>
pub struct ZombieNautilusEntity {
    pub mob_entity: MobEntity,
    pub variant: AtomicI32,
    pub is_tame: AtomicBool,
    pub is_saddled: AtomicBool,
    pub is_dashing: AtomicBool,
    pub dash_cooldown: AtomicI32,
}

impl ZombieNautilusEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let world = entity.world.load();
        let biome = world.get_biome(&entity.block_pos.load());
        let variant = ZombieNautilusVariant::select_for_biome(biome.registry_id);
        let mob_entity = MobEntity::new(entity);
        let zombie_nautilus = Self {
            mob_entity,
            variant: AtomicI32::new(variant.id() as i32),
            is_tame: AtomicBool::new(false),
            is_saddled: AtomicBool::new(false),
            is_dashing: AtomicBool::new(false),
            dash_cooldown: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(zombie_nautilus);
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
            goal_selector.add_goal(2, Box::new(MeleeAttackGoal::new(0.5, false)));
            goal_selector.add_goal(4, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                5,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(6, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            target_selector.add_goal(
                1,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn get_variant(&self) -> ZombieNautilusVariant {
        ZombieNautilusVariant::from_id(self.variant.load(Ordering::Relaxed) as u8)
            .unwrap_or_default()
    }

    pub fn set_variant(&self, variant: ZombieNautilusVariant) {
        self.variant.store(variant.id() as i32, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.set_synced_data(
            pumpkin_data::tracked_data::zombie_nautilus::DATA_VARIANT_ID,
            VarInt(variant.id() as i32),
        );
    }

    pub fn is_dashing(&self) -> bool {
        self.is_dashing.load(Ordering::Relaxed)
    }

    pub fn set_dashing(&self, dashing: bool) {
        self.is_dashing.store(dashing, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::zombie_nautilus::DASH, dashing);
    }

    pub fn is_tame(&self) -> bool {
        self.is_tame.load(Ordering::Relaxed)
    }

    pub fn get_ambient_sound(&self) -> Sound {
        self.get_underwater_sound(
            Sound::EntityZombieNautilusAmbient,
            Sound::EntityZombieNautilusAmbientLand,
        )
    }

    pub fn get_dash_sound(&self) -> Sound {
        self.get_underwater_sound(
            Sound::EntityZombieNautilusDash,
            Sound::EntityZombieNautilusDashLand,
        )
    }

    pub fn get_dash_ready_sound(&self) -> Sound {
        self.get_underwater_sound(
            Sound::EntityZombieNautilusDashReady,
            Sound::EntityZombieNautilusDashReadyLand,
        )
    }

    #[must_use]
    pub const fn get_eat_sound(&self) -> Sound {
        Sound::EntityZombieNautilusEat
    }

    #[must_use]
    pub const fn get_swim_sound(&self) -> Sound {
        Sound::EntityZombieNautilusSwim
    }

    fn get_underwater_sound(&self, underwater: Sound, on_land: Sound) -> Sound {
        if self
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed)
        {
            underwater
        } else {
            on_land
        }
    }
}

impl Animal for ZombieNautilusEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack
            .item
            .has_tag(&pumpkin_data::tag::Item::MINECRAFT_NAUTILUS_TAMING_ITEMS)
    }
}

impl CustomSound for ZombieNautilusEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(self.get_underwater_sound(
            Sound::EntityZombieNautilusHurt,
            Sound::EntityZombieNautilusHurtLand,
        ))
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(self.get_underwater_sound(
            Sound::EntityZombieNautilusDeath,
            Sound::EntityZombieNautilusDeathLand,
        ))
    }
}

impl Mob for ZombieNautilusEntity {
    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_string(
            "variant",
            format!("minecraft:{}", self.get_variant().to_name()),
        );
        nbt.put_bool("IsTame", self.is_tame.load(Ordering::Relaxed));
        nbt.put_bool("Saddled", self.is_saddled.load(Ordering::Relaxed));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(variant) = nbt
            .get_string("variant")
            .and_then(ZombieNautilusVariant::from_name)
        {
            self.set_variant(variant);
        }
        if let Some(is_tame) = nbt.get_bool("IsTame") {
            self.is_tame.store(is_tame, Ordering::Relaxed);
        }
        if let Some(saddled) = nbt.get_bool("Saddled") {
            self.is_saddled.store(saddled, Ordering::Relaxed);
        }
    }

    fn mob_set_variant_name(&self, name: &str) {
        if let Some(variant) = ZombieNautilusVariant::from_name(name) {
            self.set_variant(variant);
        }
    }

    fn mob_init_data_tracker(&self) {
        self.mob_entity.living_entity.entity.set_synced_data(
            pumpkin_data::tracked_data::zombie_nautilus::DATA_VARIANT_ID,
            VarInt(self.get_variant().id() as i32),
        );
        self.mob_entity.living_entity.entity.set_synced_data(
            pumpkin_data::tracked_data::zombie_nautilus::DASH,
            self.is_dashing(),
        );
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;

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
                    SoundCategory::Hostile,
                    &entity.pos.load(),
                );
            }
        }
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if !self.is_tame() && self.is_food(item_stack) {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            if rand::random::<u32>().is_multiple_of(3) {
                self.is_tame.store(true, Ordering::Relaxed);
                let world = self.mob_entity.living_entity.entity.world.load();
                world.send_entity_status(
                    &self.mob_entity.living_entity.entity,
                    pumpkin_data::entity::EntityStatus::TamingSucceeded,
                    None,
                );
            } else {
                let world = self.mob_entity.living_entity.entity.world.load();
                world.send_entity_status(
                    &self.mob_entity.living_entity.entity,
                    pumpkin_data::entity::EntityStatus::TamingFailed,
                    None,
                );
            }
            let entity = &self.mob_entity.living_entity.entity;
            let world = entity.world.load();
            world.play_sound(
                self.get_eat_sound(),
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
            return true;
        }

        self.animal_interact(player, item_stack, self.get_ambient_sound())
    }

    fn can_be_saddled(&self) -> bool {
        self.mob_entity.living_entity.entity.is_alive()
    }

    fn is_saddled(&self) -> bool {
        self.is_saddled.load(Ordering::Relaxed)
    }

    fn set_saddled(&self, saddled: bool) {
        self.is_saddled.store(saddled, Ordering::Relaxed);
    }

    /// Zombie nautiluses never spawn as babies, matching vanilla `canBeABaby`.
    fn spawn_as_baby(&self) -> bool {
        false
    }
}

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::entity::mob::zombie::ZombieEntityBase;
use crate::entity::mob::zombie::zombie::ZombieEntity;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
};
use pumpkin_data::entity::EntityType;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;

/// Vanilla `ConversionTracker` total water time before conversion starts.
const TOTAL_AFFLICTION_TIME: i32 = 600;
/// Vanilla `ConversionTracker` water conversion duration once triggered.
const TOTAL_CONVERSION_TIME: i32 = 300;

pub struct HuskEntity {
    entity: Arc<ZombieEntityBase>,
    /// Vanilla `ConversionTracker.afflictionTime` (`InWaterTime`). `0` while
    /// unafflicted, `-1` once the husk has left the water.
    affliction_time: AtomicI32,
    /// Vanilla `ConversionTracker.conversionTime` (`DrownedConversionTime`).
    /// `-1` means the husk is not converting.
    conversion_time: AtomicI32,
}

impl HuskEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        Self::with_can_break_doors(entity, false)
    }

    #[must_use]
    pub fn with_can_break_doors(entity: Entity, can_break_doors: bool) -> Arc<Self> {
        let entity = ZombieEntityBase::with_can_break_doors(entity, can_break_doors);
        let husk = Self {
            entity,
            affliction_time: AtomicI32::new(0),
            conversion_time: AtomicI32::new(-1),
        };
        Arc::new(husk)
    }

    /// Vanilla `Zombie.isUnderWaterConverting`.
    #[must_use]
    pub fn is_under_water_converting(&self) -> bool {
        self.conversion_time.load(Ordering::Relaxed) != -1
    }

    /// Vanilla `ConversionTracker.setConverting`: only the synced flag, the
    /// remaining time is kept separately so the two never overwrite each other.
    fn set_under_water_converting(&self, converting: bool) {
        self.entity.mob_entity.living_entity.entity.set_synced_data(
            pumpkin_data::tracked_data::zombie::DATA_DROWNED_CONVERSION_ID,
            converting,
        );
    }

    /// Vanilla `ConversionTracker.startConversion`.
    fn start_conversion(&self, time: i32) {
        self.conversion_time.store(time, Ordering::Relaxed);
        self.set_under_water_converting(true);
    }

    /// Vanilla `ConversionTracker.setConverting(false)` plus the time reset.
    fn stop_conversion(&self) {
        self.conversion_time.store(-1, Ordering::Relaxed);
        self.set_under_water_converting(false);
    }

    /// Vanilla `Zombie.tick` -> `ConversionTracker.tick`: `Husk.convertsInWater`
    /// returns `true`, so a husk that stays submerged converts into a zombie.
    fn tick_drowning_conversion(&self) {
        let entity = self.get_entity();
        if !entity.is_alive() || self.entity.mob_entity.is_no_ai() {
            return;
        }

        if entity.is_submerged_in_water() {
            if self.is_under_water_converting() {
                let remaining = self.conversion_time.fetch_sub(1, Ordering::Relaxed) - 1;
                if remaining < 0 {
                    self.stop_conversion();
                    self.finish_conversion();
                }
            } else {
                let affliction = self.affliction_time.fetch_add(1, Ordering::Relaxed) + 1;
                if affliction >= TOTAL_AFFLICTION_TIME {
                    self.start_conversion(TOTAL_CONVERSION_TIME);
                }
            }
        } else {
            self.affliction_time.store(-1, Ordering::Relaxed);
            self.stop_conversion();
        }
    }

    /// Vanilla `ConversionTracker.doConversion` with `ConversionType.SINGLE`
    /// (`keepEquipment = true`, `preserveCanPickUpLoot = true`). `Husk`
    /// converts into a plain `Zombie` and plays `SoundHuskToZombie`.
    fn finish_conversion(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();

        let converted = ZombieEntity::new(Entity::new(world.clone(), pos, &EntityType::ZOMBIE));
        let converted_base = &converted;
        let converted_entity = converted.get_entity();

        converted_entity.set_rotation(entity.yaw.load(), entity.pitch.load());
        converted_entity.head_yaw.store(entity.head_yaw.load());
        converted_entity.velocity.store(entity.velocity.load());
        converted_entity.set_silent(entity.is_silent());
        converted_entity.custom_name_visible.store(
            entity.custom_name_visible.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        if let Some(custom_name) = &**entity.custom_name.load() {
            converted_entity.set_custom_name(custom_name.clone());
        }
        if let Some(health) = converted.get_living_entity() {
            health.set_health(self.entity.mob_entity.living_entity.health.load());
        }

        // `ConversionType.convertCommon` mob flags/state.
        let converted_mob = converted_base.get_mob_entity();
        converted_mob.set_no_ai(self.entity.mob_entity.is_no_ai());
        converted_mob.persistence_required.store(
            self.entity
                .mob_entity
                .persistence_required
                .load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        converted_mob.set_can_pick_up_loot(self.entity.mob_entity.can_pick_up_loot());
        converted.set_baby(self.entity.is_baby());
        converted.set_can_break_doors(self.entity.can_break_doors());

        // `ConversionType.SINGLE` copies the equipment when `keepEquipment` is set.
        {
            let src_equip = self
                .entity
                .mob_entity
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(living) = converted.get_living_entity() {
                let mut dst_equip = living
                    .entity_equipment
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for (slot, item) in &src_equip.equipment {
                    if !item.is_empty() {
                        dst_equip.put(slot, item.clone());
                    }
                }
            }
        }

        world.spawn_entity(converted);
        entity.remove();

        if !entity.is_silent() {
            world.sync_world_event(WorldEvent::SoundHuskToZombie, entity.block_pos.load(), 0);
        }
    }
}

impl Mob for HuskEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn spawn_as_baby(&self) -> bool {
        self.entity.spawn_as_baby()
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.tick_drowning_conversion();
    }

    fn mob_java_spawn_metadata(
        &self,
        version: pumpkin_util::version::JavaMinecraftVersion,
    ) -> Option<Box<[u8]>> {
        if version < pumpkin_util::version::JavaMinecraftVersion::V_1_9 {
            return None;
        }
        let mut metadata = Vec::new();
        pumpkin_protocol::java::client::play::Metadata::new(
            pumpkin_data::tracked_data::zombie::DATA_DROWNED_CONVERSION_ID,
            self.is_under_water_converting(),
        )
        .write(&mut metadata, &version)
        .ok()?;
        metadata.push(255);
        Some(metadata.into_boxed_slice())
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.mob_write_nbt(nbt);
        // Vanilla `Zombie.addAdditionalSaveData` -> `ConversionTracker`.
        nbt.put_int(
            "DrownedConversionTime",
            if self.is_under_water_converting() {
                self.conversion_time.load(Ordering::Relaxed)
            } else {
                -1
            },
        );
        nbt.put_int(
            "InWaterTime",
            if self.get_entity().is_submerged_in_water() {
                self.affliction_time.load(Ordering::Relaxed)
            } else {
                -1
            },
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.entity.mob_read_nbt(nbt);
        // Vanilla `ConversionTracker.readAdditionalSaveData`.
        self.affliction_time
            .store(nbt.get_int("InWaterTime").unwrap_or(0), Ordering::Relaxed);
        match nbt.get_int("DrownedConversionTime") {
            Some(conversion_time) if conversion_time != -1 => {
                self.start_conversion(conversion_time);
            }
            _ => self.stop_conversion(),
        }
    }
}

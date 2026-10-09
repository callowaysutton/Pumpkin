//! Vanilla `SkeletonTrapGoal`: a thunder-spawned skeleton horse that, when a player comes
//! close, spawns a skeleton jockey and three more skeleton horses with riders.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::{Controls, Goal};
use crate::entity::EntityBase;
use crate::entity::ageable::AgeableMob;
use crate::entity::mob::Mob;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::spawn::finalize_spawn;
use crate::entity::passive::skeleton_horse::SkeletonHorseEntity;
use crate::entity::r#type::from_type;

/// Vanilla `SkeletonTrapGoal`: the trap triggers once, on the next tick after a player is near.
pub struct SkeletonTrapGoal {
    horse: Weak<SkeletonHorseEntity>,
    /// Distinguishes "already triggered" from `horse.is_trap() == false`, so the one-shot trap
    /// cannot fire a second time if the horse is re-armed.
    triggered: bool,
}

impl SkeletonTrapGoal {
    #[must_use]
    pub fn new(horse: &Arc<SkeletonHorseEntity>) -> Self {
        Self {
            horse: Arc::downgrade(horse),
            triggered: false,
        }
    }
}

impl Goal for SkeletonTrapGoal {
    /// Vanilla `Goal.canUse` default: trigger while an alive player is within 10 blocks.
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.triggered {
            return false;
        }
        let entity = mob.get_entity();
        entity
            .world
            .load()
            .get_closest_player(entity.pos.load(), 10.0)
            .is_some()
    }

    /// Vanilla removes the goal while ticking; we stop it instead and keep it dormant, since the
    /// goal selector's mutex must not be re-locked from inside `tick`.
    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let Some(horse) = self.horse.upgrade() else {
            return;
        };
        self.triggered = true;

        let entity = horse.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        let difficulty = RegionalDifficulty::at(&world, pos);

        horse.disarm_trap();
        horse.set_tame(true);
        horse.set_age(0);

        // Vanilla spawns a visual-only lightning bolt at the horse and triggers the trap.
        world.strike_lightning(pos, true);

        let Some(skeleton) = create_skeleton(&world, pos) else {
            return;
        };

        // The trap horse is already part of the world, so mount and spawn the rider directly.
        if !world.spawn_entity(skeleton.clone()) {
            return;
        }
        equip_trap_skeleton(&skeleton, &difficulty);
        let horse_arc: Arc<dyn EntityBase> = horse.clone();
        horse_arc
            .get_entity()
            .add_passenger(horse_arc.clone(), skeleton.clone());

        for _ in 0..3 {
            let Some(other_horse) = create_horse(&world, pos) else {
                continue;
            };
            let Some(other_skeleton) = create_skeleton(&world, other_horse.get_entity().pos.load())
            else {
                continue;
            };

            // Vanilla `otherHorse.push(...)` before adding it, so the spread is part of its
            // initial velocity.
            let mut rng = rand::rng();
            other_horse.get_entity().add_velocity(Vector3::new(
                triangle(&mut rng, 0.0, 1.1485),
                0.0,
                triangle(&mut rng, 0.0, 1.1485),
            ));

            if !world.spawn_entity(other_horse.clone()) {
                continue;
            }
            equip_trap_skeleton(&other_skeleton, &difficulty);
            other_horse
                .get_entity()
                .add_passenger(other_horse.clone(), other_skeleton.clone());
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

/// Vanilla `RandomSource.triangle(mean, deviation)`.
fn triangle(rng: &mut impl rand::Rng, mean: f64, deviation: f64) -> f64 {
    (rng.random::<f64>() - rng.random::<f64>()).mul_add(deviation, mean)
}

fn create_horse(
    world: &Arc<crate::world::World>,
    pos: Vector3<f64>,
) -> Option<Arc<dyn EntityBase>> {
    let horse = from_type(
        &EntityType::SKELETON_HORSE,
        pos,
        world,
        uuid::Uuid::new_v4(),
    );
    let horse_entity = horse.cast_any().downcast_ref::<SkeletonHorseEntity>()?;

    finalize_spawn(&horse, world, None);
    // Vanilla `setInvulnerableTime(60)` spawn protection is not modelled by Pumpkin's entities.
    horse_entity
        .get_mob_entity()
        .persistence_required
        .store(true, Ordering::Relaxed);
    horse_entity.set_tame(true);
    horse_entity.set_age(0);
    Some(horse)
}

fn create_skeleton(
    world: &Arc<crate::world::World>,
    pos: Vector3<f64>,
) -> Option<Arc<dyn EntityBase>> {
    let skeleton = from_type(&EntityType::SKELETON, pos, world, uuid::Uuid::new_v4());
    finalize_spawn(&skeleton, world, None);
    // Vanilla `setInvulnerableTime(60)` spawn protection is not modelled by Pumpkin's entities.
    skeleton
        .get_mob()?
        .get_mob_entity()
        .persistence_required
        .store(true, Ordering::Relaxed);
    Some(skeleton)
}

/// Vanilla force-equips an iron helmet if the rider's head slot is empty, then enchants both the
/// main hand and head. Run after the spawn equipment pass, which would otherwise overwrite it.
fn equip_trap_skeleton(skeleton: &Arc<dyn EntityBase>, difficulty: &RegionalDifficulty) {
    let Some(mob) = skeleton.get_mob() else {
        return;
    };
    let living = &mob.get_mob_entity().living_entity;
    {
        let mut equipment = living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if equipment.get(&EquipmentSlot::HEAD).is_empty() {
            equipment.put(&EquipmentSlot::HEAD, ItemStack::new(1, &Item::IRON_HELMET));
        }
    }
    mob.populate_default_equipment_enchantments(difficulty);
}

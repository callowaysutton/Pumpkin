use super::{Controls, Goal};
use crate::entity::mob::Mob;
use crate::entity::passive::tamable::TELEPORT_WHEN_DISTANCE_IS_SQ;
use std::sync::atomic::Ordering;

#[derive(Default)]
pub struct SitWhenOrderedToGoal {
    goal_control: Controls,
}

impl SitWhenOrderedToGoal {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            goal_control: Controls::MOVE.union(Controls::JUMP),
        }
    }
}

impl Goal for SitWhenOrderedToGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !mob.is_sitting() && !mob.is_tamed() {
            return false;
        }

        let entity = &mob.get_mob_entity().living_entity.entity;
        if entity.touching_water.load(Ordering::Relaxed) {
            return false;
        }

        if !entity.on_ground.load(Ordering::Relaxed) {
            return false;
        }

        // Vanilla: a nearby owner that has just been hurt calls the pet off the sit.
        if let Some(owner_uuid) = mob.get_owner_uuid()
            && let Some(owner) = entity.world.load().get_player_by_uuid(owner_uuid)
        {
            let owner_entity = &owner.living_entity.entity;
            let close = owner_entity
                .pos
                .load()
                .squared_distance_to_vec(&entity.pos.load())
                < TELEPORT_WHEN_DISTANCE_IS_SQ;
            if close && owner.living_entity.last_attacker_id.load(Ordering::Relaxed) != 0 {
                return false;
            }
        }

        mob.is_sitting()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.is_sitting()
    }

    fn start(&mut self, mob: &dyn Mob) {
        let mut navigator = mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.stop();
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

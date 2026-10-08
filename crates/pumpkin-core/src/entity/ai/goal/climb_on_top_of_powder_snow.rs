use std::sync::atomic::Ordering;

use pumpkin_data::Block;
use pumpkin_data::tag::Taggable;

use super::{Controls, Goal};
use crate::entity::mob::Mob;

/// Makes powder-snow-walkable mobs jump when they are inside powder snow so they
/// climb on top of it instead of sinking. Vanilla: `ClimbOnTopOfPowderSnowGoal`.
pub struct ClimbOnTopOfPowderSnowGoal {
    goal_control: Controls,
}

impl Default for ClimbOnTopOfPowderSnowGoal {
    fn default() -> Self {
        Self {
            goal_control: Controls::JUMP,
        }
    }
}

impl Goal for ClimbOnTopOfPowderSnowGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let in_powder_snow = entity.was_in_powder_snow.load(Ordering::Relaxed)
            || entity.is_in_powder_snow.load(Ordering::Relaxed);
        if !in_powder_snow
            || !entity
                .entity_type
                .has_tag(&pumpkin_data::tag::EntityType::MINECRAFT_POWDER_SNOW_WALKABLE_MOBS)
        {
            return false;
        }

        let above = entity.block_pos.load().up();
        let (block, state) = entity.world.load().get_block_and_state(&above);
        block == &Block::POWDER_SNOW || state.collision_shapes.is_empty()
    }

    fn tick(&mut self, mob: &dyn Mob) {
        // Pumpkin has no `JumpControl` wired into the mob tick, so set the flag it would
        // have written directly (same as `SwimGoal`).
        mob.get_mob_entity()
            .living_entity
            .jumping
            .store(true, Ordering::SeqCst);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

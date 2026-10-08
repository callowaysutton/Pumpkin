use pumpkin_data::BlockDirection;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use rand::RngExt;

use super::{Controls, Goal};
use crate::entity::mob::Mob;

/// Vanilla `Ghast.RandomFloatAroundGoal`.
///
/// Picks a random point in the air and asks the move control to fly there. `distance_to_blocks`
/// makes the goal require open space around the target (the happy ghast uses 16 so it does not
/// wedge itself next to terrain).
pub struct RandomFloatAroundGoal {
    goal_control: Controls,
    distance_to_blocks: i32,
}

impl RandomFloatAroundGoal {
    /// Maximum placement attempts before settling for any position.
    const MAX_ATTEMPTS: i32 = 64;
    /// Vanilla's random offset box around the mob.
    const RANDOM_OFFSET: f64 = 16.0;

    #[must_use]
    pub const fn new(distance_to_blocks: i32) -> Self {
        Self {
            goal_control: Controls::MOVE,
            distance_to_blocks,
        }
    }

    fn choose_random_position(mob: &dyn Mob, center: Vector3<f64>) -> Vector3<f64> {
        let mut rng = mob.get_random();
        Vector3::new(
            center.x + (rng.random::<f32>() * 2.0 - 1.0) as f64 * Self::RANDOM_OFFSET,
            center.y + (rng.random::<f32>() * 2.0 - 1.0) as f64 * Self::RANDOM_OFFSET,
            center.z + (rng.random::<f32>() * 2.0 - 1.0) as f64 * Self::RANDOM_OFFSET,
        )
    }

    fn choose_random_position_with_restriction(
        mob: &dyn Mob,
        center: Vector3<f64>,
    ) -> Option<Vector3<f64>> {
        let target = Self::choose_random_position(mob, center);
        let mob_entity = mob.get_mob_entity();
        if mob_entity.has_position_target()
            && !mob_entity.is_in_position_target_range_pos(&BlockPos::containing_vec(target))
        {
            return None;
        }
        Some(target)
    }

    fn is_good_target(mob: &dyn Mob, target: Vector3<f64>, distance_to_blocks: i32) -> bool {
        if distance_to_blocks <= 0 {
            return true;
        }
        let world = mob.get_entity().world.load();
        let pos = BlockPos::containing_vec(target);
        if !world.get_block_state(&pos).is_air() {
            return false;
        }
        for dir in BlockDirection::all() {
            let offset = dir.to_offset();
            for i in 1..distance_to_blocks {
                let candidate = BlockPos::new(
                    pos.0.x + offset.x * i,
                    pos.0.y + offset.y * i,
                    pos.0.z + offset.z * i,
                );
                if !world.get_block_state(&candidate).is_air() {
                    return true;
                }
            }
        }
        false
    }

    /// Vanilla `getSuitableFlyToPosition`: try to find a suitably open spot, otherwise fall back
    /// to any random point, then clamp below the motion-blocking height.
    fn get_suitable_fly_to_position(mob: &dyn Mob, distance_to_blocks: i32) -> Vector3<f64> {
        let center = mob.get_entity().pos.load();
        let mut result = None;

        for _ in 0..Self::MAX_ATTEMPTS {
            result = Self::choose_random_position_with_restriction(mob, center);
            if let Some(candidate) = result
                && Self::is_good_target(mob, candidate, distance_to_blocks)
            {
                return candidate;
            }
        }

        let mut result = result.unwrap_or_else(|| Self::choose_random_position(mob, center));
        let world = mob.get_entity().world.load();
        let pos = BlockPos::containing_vec(result);
        let height_y =
            world.get_heightmap_height(ChunkHeightmapType::MotionBlocking, pos.0.x, pos.0.z);
        if height_y < pos.0.y && height_y > world.dimension.min_y {
            result = Vector3::new(result.x, center.y - (center.y - result.y).abs(), result.z);
        }
        result
    }
}

impl Goal for RandomFloatAroundGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let move_control = mob
            .get_mob_entity()
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(wanted) = move_control.wanted_position() else {
            return true;
        };
        drop(move_control);

        let pos = mob.get_entity().pos.load();
        let dd = pos.squared_distance_to_vec(&wanted);
        dd < 1.0 || dd > 3600.0
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        let target = Self::get_suitable_fly_to_position(mob, self.distance_to_blocks);
        mob.get_mob_entity()
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_wanted_position(target.x, target.y, target.z, 1.0);
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

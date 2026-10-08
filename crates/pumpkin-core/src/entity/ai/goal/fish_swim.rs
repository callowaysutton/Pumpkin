use super::{Controls, Goal, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::pathfinder::node::PathComputationType;
use crate::entity::ai::pathfinder::pathfinding_context::PathfindingContext;
use crate::entity::ai::util::default_random_pos;
use crate::entity::mob::Mob;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use std::sync::atomic::Ordering;

/// Vanilla `RandomSwimmingGoal`: a `RandomStrollGoal` whose destination is a
/// random spot that is pathfindable through water (`BehaviorUtils.getRandomSwimmablePos`).
pub struct FishSwimGoal {
    goal_control: Controls,
    speed: f64,
    target: Option<Vector3<f64>>,
    interval: i32,
}

impl FishSwimGoal {
    #[must_use]
    pub const fn new(speed: f64, interval: i32) -> Self {
        Self {
            goal_control: Controls::MOVE,
            speed,
            target: None,
            interval,
        }
    }

    /// `BehaviorUtils.getRandomSwimmablePos`: retry the random draw while the
    /// chosen block is not pathfindable as water, at most ten times.
    fn get_position(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let world = mob.get_entity().world.load_full();
        let block_pos = mob.get_entity().block_pos.load();
        let context = PathfindingContext::new(
            Vector3::new(block_pos.0.x, block_pos.0.y, block_pos.0.z),
            world,
        );

        let mut target = default_random_pos::get_pos(mob, 10, 7);
        let mut count = 0;
        while let Some(pos) = target {
            if context.is_pathfindable(&BlockPos::containing_vec(pos), PathComputationType::Water)
                || count >= 10
            {
                break;
            }
            count += 1;
            target = default_random_pos::get_pos(mob, 10, 7);
        }
        target
    }
}

impl Goal for FishSwimGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !mob.can_random_swim() || mob.get_entity().has_passengers() {
            return false;
        }

        if mob
            .get_random()
            .random_range(0..to_goal_ticks(self.interval))
            != 0
        {
            return false;
        }

        self.target = Self::get_position(mob);
        self.target.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.is_navigator_idle() && !mob.get_entity().has_passengers()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(target) = self.target {
            let pos = mob.get_mob_entity().living_entity.entity.pos.load();
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_progress(NavigatorGoal::new(pos, target, self.speed));
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.target = None;
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        mob.get_mob_entity()
            .living_entity
            .jumping
            .store(false, Ordering::Relaxed);
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

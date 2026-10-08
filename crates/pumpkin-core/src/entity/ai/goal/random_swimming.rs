use super::{Controls, Goal, to_goal_ticks};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::util::{default_random_pos, goal_utils};
use crate::entity::mob::Mob;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

/// Port of vanilla `RandomSwimmingGoal`: a `RandomStrollGoal` that picks a position whose block
/// holds water (vanilla `BehaviorUtils.getRandomSwimmablePos`).
pub struct RandomSwimmingGoal {
    goal_control: Controls,
    speed: f64,
    target: Option<Vector3<f64>>,
    interval: i32,
    force_trigger: bool,
}

impl RandomSwimmingGoal {
    #[must_use]
    pub const fn new(speed: f64, interval: i32) -> Self {
        Self {
            goal_control: Controls::MOVE,
            speed,
            target: None,
            interval,
            force_trigger: false,
        }
    }

    fn get_position(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let world = mob.get_entity().world.load();
        let mut position = default_random_pos::get_pos(mob, 10, 7);
        let mut retries = 0;

        // Vanilla re-rolls while the position is not pathfindable as water, at most 10 times.
        while let Some(pos) = position
            && !goal_utils::is_water(&world, &BlockPos::floored_v(pos))
            && retries < 10
        {
            position = default_random_pos::get_pos(mob, 10, 7);
            retries += 1;
        }

        position
    }
}

impl Goal for RandomSwimmingGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if mob.get_entity().has_passengers() {
            return false;
        }

        if !self.force_trigger
            && mob
                .get_random()
                .random_range(0..to_goal_ticks(self.interval))
                != 0
        {
            return false;
        }

        self.target = Self::get_position(mob);
        if self.target.is_none() {
            return false;
        }
        self.force_trigger = false;
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let idle = mob.is_navigator_idle();
        !idle && !mob.get_entity().has_passengers()
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
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

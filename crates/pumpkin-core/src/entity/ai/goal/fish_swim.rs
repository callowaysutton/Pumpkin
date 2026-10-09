use super::{Controls, Goal, random_swimming::RandomSwimmingGoal};
use crate::entity::mob::Mob;
use crate::entity::passive::abstract_schooling_fish::SchoolingFish;

/// Wander speed of a fish (vanilla `AbstractFish.FishSwimGoal` uses 1.0).
const SWIM_SPEED: f64 = 1.0;
/// Vanilla `RandomSwimmingGoal` interval: try to pick a new target every 40 ticks.
const SWIM_INTERVAL: i32 = 40;

/// Vanilla `AbstractFish.FishSwimGoal`: fish wander, but only towards positions in water, and
/// only while they are free to swim about on their own (vanilla `canRandomSwim`).
pub struct FishSwimGoal {
    random_swimming: RandomSwimmingGoal,
}

impl FishSwimGoal {
    #[must_use]
    pub const fn new() -> Self {
        Self::with_speed(SWIM_SPEED)
    }

    #[must_use]
    pub const fn with_speed(speed: f64) -> Self {
        Self {
            random_swimming: RandomSwimmingGoal::new(speed, SWIM_INTERVAL),
        }
    }
}

impl Default for FishSwimGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for FishSwimGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        // Vanilla wraps `RandomSwimmingGoal.canUse` with `canRandomSwim`.
        if mob
            .as_schooling_fish()
            .is_some_and(|fish| !SchoolingFish::can_random_swim(fish))
        {
            return false;
        }
        self.random_swimming.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.random_swimming.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.random_swimming.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.random_swimming.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.random_swimming.controls()
    }
}

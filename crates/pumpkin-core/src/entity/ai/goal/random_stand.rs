use super::{Controls, Goal};
use crate::entity::mob::Mob;
use rand::RngExt;

/// Vanilla `RandomStandGoal`: occasionally makes a horse rear up while idle so it plays
/// its ambient stand sound.
pub struct RandomStandGoal {
    goal_control: Controls,
    next_stand: i32,
}

impl RandomStandGoal {
    #[must_use]
    pub fn new(mob: &dyn Mob) -> Self {
        Self {
            goal_control: Controls::empty(),
            next_stand: -mob.get_ambient_stand_interval(),
        }
    }

    fn reset_stand_interval(&mut self, mob: &dyn Mob) {
        self.next_stand = -mob.get_ambient_stand_interval();
    }
}

impl Goal for RandomStandGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.next_stand += 1;
        if self.next_stand > 0 && mob.get_random().random_range(0..1000) < self.next_stand {
            self.reset_stand_interval(mob);
            return !mob.is_immobile() && mob.get_random().random_range(0..10) == 0;
        }

        false
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.stand_if_possible();
        if let Some(sound) = mob.get_ambient_stand_sound() {
            mob.get_entity().play_sound(sound);
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

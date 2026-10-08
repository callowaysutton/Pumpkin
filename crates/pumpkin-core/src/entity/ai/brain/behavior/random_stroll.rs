use crate::entity::ai::brain::behavior::one_shot::OneShot;
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::types;
use crate::entity::ai::brain::memory::walk_target::WalkTarget;
use crate::entity::ai::random_pos::land_random_pos;

/// Vanilla `RandomStroll`: static constructors for stroll-style walk targets.
pub struct RandomStroll;

impl RandomStroll {
    /// Vanilla `RandomStroll.stroll(speed)`: pick a random land position 10/7 blocks out and walk
    /// there while no walk target is set; `mayStrollFromWater` is true, so any spot qualifies.
    #[must_use]
    pub fn stroll(name: &'static str, speed_modifier: f32) -> OneShot {
        OneShot::new(
            name,
            vec![(types::WALK_TARGET.id(), MemoryStatus::ValueAbsent)],
            move |tick| {
                let walk_target = land_random_pos(tick.mob, 10, 7)
                    .map(|pos| WalkTarget::from_vec(pos, speed_modifier, 0));
                tick.brain.set_optional(types::WALK_TARGET, walk_target);
                true
            },
        )
    }
}

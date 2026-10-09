use crate::entity::ai::brain::behavior::one_shot::OneShot;
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::types;
use crate::entity::ai::brain::memory::walk_target::WalkTarget;

/// Vanilla `SetWalkTargetFromLookTarget`: static constructors only, no state.
pub struct SetWalkTargetFromLookTarget;

impl SetWalkTargetFromLookTarget {
    /// Vanilla `SetWalkTargetFromLookTarget.create(speed, closeEnough)`: turn the remembered look
    /// target into a walk target while none is set. Vanilla's predicate always accepts the body.
    #[must_use]
    pub fn create(name: &'static str, speed_modifier: f32, close_enough_distance: i32) -> OneShot {
        OneShot::new(
            name,
            vec![
                (types::WALK_TARGET.id(), MemoryStatus::ValueAbsent),
                (types::LOOK_TARGET.id(), MemoryStatus::ValuePresent),
            ],
            move |tick| {
                let Some(target) = tick.brain.get(types::LOOK_TARGET).cloned() else {
                    return false;
                };
                tick.brain.set(
                    types::WALK_TARGET,
                    WalkTarget::new(target, speed_modifier, close_enough_distance),
                );
                true
            },
        )
    }
}

use crate::entity::ai::brain::BrainTick;
use crate::entity::ai::brain::behavior::timed::{Behavior, Timed};
use crate::entity::ai::brain::memory::types;
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus};

/// Vanilla `LookAtTargetSink`: look at the remembered target until it stops being visible.
pub struct LookAtTargetSink {
    min_duration: i32,
    max_duration: i32,
}

impl LookAtTargetSink {
    #[must_use]
    pub const fn new(min_duration: i32, max_duration: i32) -> Self {
        Self {
            min_duration,
            max_duration,
        }
    }

    /// Vanilla constructor: `LookAtTargetSink(45, 90)`.
    #[must_use]
    pub fn core() -> Timed<Self> {
        Timed::new(Self::new(45, 90))
    }
}

const ENTRY_CONDITIONS: [(MemoryModuleId, MemoryStatus); 1] =
    [(types::LOOK_TARGET.id(), MemoryStatus::ValuePresent)];

impl Behavior for LookAtTargetSink {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        &ENTRY_CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        self.min_duration
    }

    fn max_duration(&self) -> i32 {
        self.max_duration
    }

    /// Vanilla `canStillUse`: keep looking while the target is still visible by the mob.
    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        tick.brain.get(types::LOOK_TARGET).is_some_and(|target| {
            let ctx = tick.visibility();
            target.is_visible_by(&ctx)
        })
    }

    /// Vanilla `tick`: point the look control at the target's current position.
    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let Some(target) = tick.brain.get(types::LOOK_TARGET).cloned() else {
            return;
        };
        let position = target.current_position();
        let mut look_control = tick
            .mob
            .get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        look_control.look_at_position(tick.mob, position);
    }

    /// Vanilla `stop`: forget the look target.
    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        tick.brain.erase(types::LOOK_TARGET.id());
    }

    fn debug_name(&self) -> &'static str {
        "LookAtTargetSink"
    }
}

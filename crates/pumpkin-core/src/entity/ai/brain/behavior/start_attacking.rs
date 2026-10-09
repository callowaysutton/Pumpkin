use std::sync::Arc;

use crate::entity::EntityBase;
use crate::entity::ai::brain::BrainTick;
use crate::entity::ai::brain::behavior::one_shot::OneShot;
use crate::entity::ai::brain::memory::{MemoryStatus, types};

/// Vanilla `StartAttacking.TargetFinder`.
pub type TargetFinder =
    Box<dyn FnMut(&mut BrainTick<'_>) -> Option<Arc<dyn EntityBase>> + Send + Sync + 'static>;

/// Vanilla `StartAttacking`: fills `MEMORY_ATTACK_TARGET` from a finder callback.
pub struct StartAttacking;

impl StartAttacking {
    /// Vanilla `StartAttacking.create`: fills `MEMORY_ATTACK_TARGET` with the first target the
    /// finder yields that the mob may attack, resetting the unreachability timer on the way.
    #[must_use]
    pub fn create(name: &'static str, mut target_finder: TargetFinder) -> OneShot {
        OneShot::new(
            name,
            vec![
                (types::ATTACK_TARGET.id(), MemoryStatus::ValueAbsent),
                (
                    types::CANT_REACH_WALK_TARGET_SINCE.id(),
                    MemoryStatus::Registered,
                ),
            ],
            move |tick| {
                let Some(target) = target_finder(tick) else {
                    return false;
                };
                if !tick.mob.can_attack(target.as_ref()) {
                    return false;
                }
                tick.brain.set(types::ATTACK_TARGET, target);
                tick.brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
                true
            },
        )
    }
}

use crate::entity::ai::brain::BrainTick;
use crate::entity::ai::brain::behavior::one_shot::OneShot;
use crate::entity::ai::brain::memory::{MemoryStatus, types};

/// Vanilla `StopAttackingIfTargetInvalid.TIMEOUT_TO_GET_WITHIN_ATTACK_RANGE`.
const TIMEOUT_TO_GET_WITHIN_ATTACK_RANGE: i64 = 200;

/// Vanilla `StopAttackingIfTargetInvalid`: erases the attack target memory when it turns
/// invalid. Static constructors only, no state.
pub struct StopAttackingIfTargetInvalid;

impl StopAttackingIfTargetInvalid {
    /// Vanilla `StopAttackingIfTargetInvalid.create`: erases the attack target once it is dead,
    /// despawned, unattackable, or unreachable for too long.
    #[must_use]
    pub fn create(name: &'static str) -> OneShot {
        OneShot::new(
            name,
            vec![
                (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
                (
                    types::CANT_REACH_WALK_TARGET_SINCE.id(),
                    MemoryStatus::Registered,
                ),
            ],
            move |tick| {
                let Some(target) = tick.brain.get(types::ATTACK_TARGET).cloned() else {
                    return true;
                };
                let keeps_attacking = tick.mob.can_attack(target.as_ref())
                    && !is_tired_of_trying_to_reach_target(tick)
                    && target.get_entity().is_alive();
                // Vanilla also requires `target.level() == body.level()`; entities always compare
                // within their own world server here.
                if !keeps_attacking {
                    tick.brain.erase(types::ATTACK_TARGET.id());
                }
                true
            },
        )
    }
}

/// Vanilla `StopAttackingIfTargetInvalid.isTiredOfTryingToReachTarget`.
fn is_tired_of_trying_to_reach_target(tick: &BrainTick<'_>) -> bool {
    tick.brain
        .get(types::CANT_REACH_WALK_TARGET_SINCE)
        .is_some_and(|since| tick.time - *since > TIMEOUT_TO_GET_WITHIN_ATTACK_RANGE)
}

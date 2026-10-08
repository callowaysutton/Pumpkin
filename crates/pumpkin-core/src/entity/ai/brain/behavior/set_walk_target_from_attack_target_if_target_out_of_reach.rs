use std::sync::Arc;

use crate::entity::ai::brain::VisibilityContext;
use crate::entity::ai::brain::behavior::one_shot::OneShot;
use crate::entity::ai::brain::behavior::utils::entity_is_visible;
use crate::entity::ai::brain::behavior::utils::is_within_attack_range;
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::position_tracker::{EntityTracker, PositionTracker};
use crate::entity::ai::brain::memory::types;
use crate::entity::ai::brain::memory::walk_target::WalkTarget;

/// Vanilla `SetWalkTargetFromAttackTargetIfTargetOutOfReach`: static constructors only.
pub struct SetWalkTargetFromAttackTargetIfTargetOutOfReach;

impl SetWalkTargetFromAttackTargetIfTargetOutOfReach {
    /// Vanilla `SetWalkTargetFromAttackTargetIfTargetOutOfReach.create(speed)`: stand still while
    /// the attack target stays in melee reach, otherwise look at it and walk up to it.
    #[must_use]
    pub fn create(name: &'static str, speed_modifier: f32) -> OneShot {
        OneShot::new(
            name,
            vec![
                (types::WALK_TARGET.id(), MemoryStatus::Registered),
                (types::LOOK_TARGET.id(), MemoryStatus::Registered),
                (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
                (
                    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                    MemoryStatus::Registered,
                ),
            ],
            move |tick| {
                let Some(target) = tick.brain.get(types::ATTACK_TARGET).cloned() else {
                    return false;
                };
                let in_reach = {
                    let ctx: VisibilityContext<'_> = tick.visibility();
                    entity_is_visible(&ctx, target.as_ref())
                        && is_within_attack_range(tick.mob.get_mob_entity(), target.as_ref())
                };
                if in_reach {
                    tick.brain.erase(types::WALK_TARGET.id());
                } else {
                    tick.brain.set(
                        types::LOOK_TARGET,
                        Arc::new(EntityTracker::new(target.clone(), true))
                            as Arc<dyn PositionTracker>,
                    );
                    tick.brain.set(
                        types::WALK_TARGET,
                        WalkTarget::new(
                            Arc::new(EntityTracker::new(target, false)) as Arc<dyn PositionTracker>,
                            speed_modifier,
                            0,
                        ),
                    );
                }
                true
            },
        )
    }
}

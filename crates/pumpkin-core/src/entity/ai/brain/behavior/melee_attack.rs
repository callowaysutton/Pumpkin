use crate::entity::ai::brain::behavior::one_shot::OneShot;
use crate::entity::ai::brain::behavior::utils::{entity_is_visible, look_at_entity};
use crate::entity::ai::brain::memory::{MemoryStatus, types};
use crate::entity::ai::brain::{BrainTick, VisibilityContext};

/// Vanilla `MeleeAttack`: melee-range attack with its own cooldown memory write.
pub struct MeleeAttack;

impl MeleeAttack {
    /// Vanilla `MeleeAttack.create`: within melee reach and cooldown-free, swing, hit, and set a
    /// fresh attack cooldown. Vanilla additionally skips mobs holding a usable non-melee weapon;
    /// every vanilla brain that reaches melee attack can never hold one, so the check stays out.
    #[must_use]
    pub fn create(
        name: &'static str,
        can_attack_predicate: impl Fn(&BrainTick<'_>) -> bool + Send + Sync + 'static,
        cooldown_between_attacks: i32,
    ) -> OneShot {
        OneShot::new(
            name,
            vec![
                (types::LOOK_TARGET.id(), MemoryStatus::Registered),
                (types::ATTACK_TARGET.id(), MemoryStatus::ValuePresent),
                (types::ATTACK_COOLING_DOWN.id(), MemoryStatus::ValueAbsent),
                (
                    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                    MemoryStatus::ValuePresent,
                ),
            ],
            move |tick| {
                let Some(target) = tick.brain.get(types::ATTACK_TARGET).cloned() else {
                    return false;
                };
                if !can_attack_predicate(tick)
                    || !tick
                        .mob
                        .get_mob_entity()
                        .is_in_attack_range(target.as_ref())
                {
                    return false;
                }
                {
                    let ctx: VisibilityContext<'_> = tick.visibility();
                    if !entity_is_visible(&ctx, target.as_ref()) {
                        return false;
                    }
                }

                look_at_entity(tick.brain, target.clone());
                tick.mob.get_mob_entity().living_entity.swing_hand();
                tick.mob.do_hurt_target(target.as_ref());
                tick.brain.set_with_expiry(
                    types::ATTACK_COOLING_DOWN,
                    true,
                    i64::from(cooldown_between_attacks),
                );
                true
            },
        )
    }
}

use rand::RngExt;
use std::sync::Arc;

use pumpkin_util::math::int_provider::UniformIntProvider;

use crate::entity::ai::brain::VisibilityContext;
use crate::entity::ai::brain::behavior::one_shot::OneShot;
use crate::entity::ai::brain::memory::position_tracker::{EntityTracker, PositionTracker};
use crate::entity::ai::brain::memory::{MemoryStatus, types};

/// Vanilla `SetEntityLookTargetSometimes`: every interval ticks, look at one of the entities
/// visible within `max_dist`. The interval lives with the behavior, like vanilla's ticker.
pub struct SetEntityLookTargetSometimes {
    ticker: Ticker,
    max_dist_squared: f64,
}

impl SetEntityLookTargetSometimes {
    #[must_use]
    pub fn new(max_dist: f32, interval: UniformIntProvider) -> Self {
        Self {
            ticker: Ticker {
                interval,
                ticks_until_next_start: 0,
            },
            max_dist_squared: f64::from(max_dist) * f64::from(max_dist),
        }
    }

    /// Vanilla `SetEntityLookTargetSometimes.create(maxDist, interval)`.
    #[must_use]
    pub fn create(name: &'static str, max_dist: f32, interval: UniformIntProvider) -> OneShot {
        // Vanilla throws for intervals that can start every tick; keep it a debug check.
        debug_assert!(
            interval.get_min() > 1,
            "the interval must stay above one tick"
        );
        let mut behavior = Self::new(max_dist, interval);
        OneShot::new(
            name,
            vec![
                (types::LOOK_TARGET.id(), MemoryStatus::ValueAbsent),
                (
                    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
                    MemoryStatus::ValuePresent,
                ),
            ],
            move |tick| {
                let target = {
                    let ctx: VisibilityContext<'_> = tick.visibility();
                    let mob_pos = tick.mob.get_entity().pos.load();
                    let Some(visible) = tick.brain.get(types::NEAREST_VISIBLE_LIVING_ENTITIES)
                    else {
                        return false;
                    };
                    visible.find_closest(&ctx, |entity| {
                        entity
                            .get_entity()
                            .pos
                            .load()
                            .squared_distance_to_vec(&mob_pos)
                            <= behavior.max_dist_squared
                    })
                };
                let Some(target) = target else {
                    return false;
                };

                let mut rng = tick.mob.get_random();
                if !behavior.ticker.tick_down_and_check(&mut rng) {
                    return false;
                }

                tick.brain.set(
                    types::LOOK_TARGET,
                    Arc::new(EntityTracker::new(target, true)) as Arc<dyn PositionTracker>,
                );
                true
            },
        )
    }
}

/// Vanilla `SetEntityLookTargetSometimes.Ticker`: counts down the interval between looks.
struct Ticker {
    interval: UniformIntProvider,
    ticks_until_next_start: i32,
}

impl Ticker {
    /// Vanilla `Ticker.tickDownAndCheck`.
    fn tick_down_and_check(&mut self, rng: &mut rand::rngs::ThreadRng) -> bool {
        if self.ticks_until_next_start == 0 {
            self.ticks_until_next_start =
                rng.random_range(self.interval.get_min()..=self.interval.get_max()) - 1;
            false
        } else {
            self.ticks_until_next_start -= 1;
            self.ticks_until_next_start == 0
        }
    }
}

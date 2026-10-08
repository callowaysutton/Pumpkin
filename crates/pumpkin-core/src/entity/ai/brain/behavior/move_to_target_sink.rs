use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::ai::brain::BrainTick;
use crate::entity::ai::brain::behavior::timed::{Behavior, Timed};
use crate::entity::ai::brain::memory::position_tracker::PositionTracker;
use crate::entity::ai::brain::memory::types;
use crate::entity::ai::brain::memory::{MemoryModuleId, MemoryStatus};
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::ai::random_pos::land_random_pos_towards;

/// Vanilla `MoveToTargetSink`: walk to the remembered walk target, re-pathing when it moves,
/// and record reachability in `CANT_REACH_WALK_TARGET_SINCE`.
pub struct MoveToTargetSink {
    remaining_cooldown: i32,
    path: Option<Path>,
    last_target_pos: Option<BlockPos>,
    speed_modifier: f32,
    min_duration: i32,
    max_duration: i32,
}

/// Vanilla `MoveToTargetSink.MAX_COOLDOWN_BEFORE_RETRYING`.
const MAX_COOLDOWN_BEFORE_RETRYING: i32 = 40;

impl MoveToTargetSink {
    #[must_use]
    pub const fn new(min_duration: i32, max_duration: i32) -> Self {
        Self {
            remaining_cooldown: 0,
            path: None,
            last_target_pos: None,
            speed_modifier: 0.0,
            min_duration,
            max_duration,
        }
    }

    /// Vanilla constructor: `MoveToTargetSink()` runs 150–250 ticks.
    #[must_use]
    pub fn core() -> Timed<Self> {
        Timed::new(Self::new(150, 250))
    }

    /// Vanilla `MoveToTargetSink.reachedTarget`, on the walk target's current block position.
    fn reached_target(
        tick: &BrainTick<'_>,
        target: &dyn PositionTracker,
        close_enough: i32,
    ) -> bool {
        let mob_pos = tick.mob.get_entity().block_pos.load();
        target.current_block_position().manhattan_distance(mob_pos) <= close_enough
    }

    /// Vanilla `MoveToTargetSink.tryComputePath`.
    fn try_compute_path(
        &mut self,
        tick: &mut BrainTick<'_>,
        target: &dyn PositionTracker,
        speed: f32,
        close_enough: i32,
    ) -> bool {
        let target_pos = target.current_block_position();
        let destination = Vector3::new(
            f64::from(target_pos.0.x) + 0.5,
            f64::from(target_pos.0.y),
            f64::from(target_pos.0.z) + 0.5,
        );
        let mut navigator = tick
            .mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.path = navigator.create_path(&tick.mob.get_mob_entity().living_entity, destination, 0);
        drop(navigator);
        self.speed_modifier = speed;

        if Self::reached_target(tick, target, close_enough) {
            tick.brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
        } else {
            let can_reach = self.path.as_ref().is_some_and(Path::can_reach);
            if can_reach {
                tick.brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
            } else if !tick
                .brain
                .has_memory_value(types::CANT_REACH_WALK_TARGET_SINCE.id())
            {
                tick.brain
                    .set(types::CANT_REACH_WALK_TARGET_SINCE, tick.time);
            }

            if self.path.is_some() {
                return true;
            }

            // Vanilla fallback: a partial step towards the unreachable target.
            let partial_step = land_random_pos_towards(tick.mob, 10, 7, destination);
            if let Some(partial_step) = partial_step {
                let mut navigator = tick
                    .mob
                    .get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                self.path = navigator.create_path(
                    &tick.mob.get_mob_entity().living_entity,
                    partial_step,
                    0,
                );
                return self.path.is_some();
            }
        }

        false
    }
}

impl Behavior for MoveToTargetSink {
    fn entry_conditions(&self) -> &[(MemoryModuleId, MemoryStatus)] {
        const CONDITIONS: [(MemoryModuleId, MemoryStatus); 3] = [
            (
                types::CANT_REACH_WALK_TARGET_SINCE.id(),
                MemoryStatus::Registered,
            ),
            (types::PATH.id(), MemoryStatus::ValueAbsent),
            (types::WALK_TARGET.id(), MemoryStatus::ValuePresent),
        ];
        &CONDITIONS
    }

    fn min_duration(&self) -> i32 {
        self.min_duration
    }

    fn max_duration(&self) -> i32 {
        self.max_duration
    }

    /// Vanilla `checkExtraStartConditions`, including the retry cooldown after getting stuck.
    fn check_extra_start_conditions(&mut self, tick: &mut BrainTick<'_>) -> bool {
        if self.remaining_cooldown > 0 {
            self.remaining_cooldown -= 1;
            return false;
        }

        let Some(walk_target) = tick.brain.get(types::WALK_TARGET) else {
            return false;
        };
        let target = walk_target.target().clone();
        let speed = walk_target.speed_modifier();
        let close_enough = walk_target.close_enough_dist();

        let reached = Self::reached_target(tick, target.as_ref(), close_enough);
        if !reached && self.try_compute_path(tick, target.as_ref(), speed, close_enough) {
            self.last_target_pos = Some(target.current_block_position());
            return true;
        }

        tick.brain.erase(types::WALK_TARGET.id());
        if reached {
            tick.brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
        }
        false
    }

    /// Vanilla `canStillUse`.
    fn can_still_use(&mut self, tick: &BrainTick<'_>) -> bool {
        if self.path.is_none() || self.last_target_pos.is_none() {
            return false;
        }

        let Some(walk_target) = tick.brain.get(types::WALK_TARGET) else {
            return false;
        };
        let walked = walk_target.target().clone();
        let close_enough = walk_target.close_enough_dist();

        let not_done = !tick
            .mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_done();
        not_done
            && !walked.is_spectator()
            && !Self::reached_target(tick, walked.as_ref(), close_enough)
    }

    /// Vanilla `start`: remember the path and start the navigation with the walk speed.
    fn start(&mut self, tick: &mut BrainTick<'_>) {
        if let Some(path) = self.path.clone() {
            tick.brain.set(types::PATH, path);
        }
        let entity = &tick.mob.get_mob_entity().living_entity;
        let mut navigator = tick
            .mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.move_to_path(self.path.clone(), f64::from(self.speed_modifier), entity);
    }

    /// Vanilla `tick`: sync the remembered path and re-path when the walk target moved >2 blocks.
    fn tick(&mut self, tick: &mut BrainTick<'_>) {
        let navigation_path = tick
            .mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_path()
            .cloned();
        let changed = !match (&self.path, &navigation_path) {
            (Some(stored), Some(navigation)) => navigation.same_as_path(stored),
            (None, None) => true,
            _ => false,
        };
        if changed {
            self.path.clone_from(&navigation_path);
            if let Some(path) = navigation_path {
                tick.brain.set(types::PATH, path);
            }
        }

        let Some(walk_target) = tick.brain.get(types::WALK_TARGET) else {
            return;
        };
        let target = walk_target.target().clone();
        let speed = walk_target.speed_modifier();
        let close_enough = walk_target.close_enough_dist();
        let moved = self.last_target_pos.is_some_and(|last_target_pos| {
            target
                .current_block_position()
                .squared_distance(&last_target_pos)
                > 4
        });
        if moved && self.try_compute_path(tick, target.as_ref(), speed, close_enough) {
            self.last_target_pos = Some(target.current_block_position());
            self.start(tick);
        }
    }

    /// Vanilla `stop`: reset the navigation, keep a short retry cooldown when stuck.
    fn stop(&mut self, tick: &mut BrainTick<'_>) {
        let walk_target = tick.brain.get(types::WALK_TARGET).map(|walk_target| {
            (
                walk_target.target().clone(),
                walk_target.close_enough_dist(),
            )
        });
        let mut navigator = tick
            .mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stuck = walk_target.is_some_and(|(target, close_enough)| {
            !Self::reached_target(tick, target.as_ref(), close_enough) && navigator.is_stuck()
        });
        if stuck {
            self.remaining_cooldown = tick
                .mob
                .get_random()
                .random_range(0..MAX_COOLDOWN_BEFORE_RETRYING);
        }

        navigator.stop();
        drop(navigator);
        tick.brain.erase(types::WALK_TARGET.id());
        tick.brain.erase(types::PATH.id());
        self.path = None;
        self.last_target_pos = None;
    }

    fn debug_name(&self) -> &'static str {
        "MoveToTargetSink"
    }
}

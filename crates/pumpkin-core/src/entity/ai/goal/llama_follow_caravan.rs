use std::sync::Arc;

use pumpkin_data::entity::EntityType;

use super::{Controls, Goal, to_goal_ticks};
use crate::entity::Entity;
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::passive::llama::llama_caravan;

/// Vanilla `LlamaFollowCaravanGoal`.
///
/// A llama that is neither leashed nor already in a caravan joins the caravan of the closest
/// nearby llama with a free tail, or starts a caravan behind the closest leashed llama with a
/// free tail.
pub struct LlamaFollowCaravanGoal {
    speed_modifier: f64,
    dist_check_counter: i32,
}

impl LlamaFollowCaravanGoal {
    /// Vanilla `CARAVAN_LIMIT`: a caravan chain longer than this has no leashed root.
    pub const CARAVAN_LIMIT: i32 = 8;
    /// Vanilla registers the goal with 2.1 and resets to it in `stop`.
    const DEFAULT_SPEED_MODIFIER: f64 = 2.1;
    /// Vanilla: `this.llama.getBoundingBox().inflate(9.0, 4.0, 9.0)`.
    const SEARCH_XZ_RANGE: f64 = 9.0;
    const SEARCH_Y_RANGE: f64 = 4.0;
    /// Vanilla skips joining while closer than 4 square distance (2 blocks).
    const MIN_JOIN_DISTANCE_SQ: f64 = 4.0;
    /// Vanilla speeds up while further than 676 square distance (26 blocks).
    const FOLLOW_DISTANCE_SQ: f64 = 676.0;
    const MAX_SPEED_MODIFIER: f64 = 3.0;
    const SPEED_UP_FACTOR: f64 = 1.2;
    /// Vanilla recomputes the speed-up cooldown with `reducedTickDelay(40)`.
    const DIST_CHECK_TICKS: i32 = 40;
    /// Vanilla walks this close behind the caravan head.
    const FOLLOW_STOP_DISTANCE: f64 = 2.0;

    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            dist_check_counter: 0,
        }
    }

    /// Vanilla `firstIsLeashed`: the root of the caravan chain `current` is part of must be
    /// leashed to something.
    fn first_is_leashed(current: &dyn EntityBase, counter: i32) -> bool {
        if counter > Self::CARAVAN_LIMIT {
            return false;
        }
        let Some(caravan) = llama_caravan(current) else {
            return false;
        };
        if !caravan.in_caravan() {
            return false;
        }
        let Some(head) = caravan.get_caravan_head() else {
            return false;
        };
        head.get_entity().is_leashed() || Self::first_is_leashed(head.as_ref(), counter + 1)
    }

    /// Vanilla `LivingEntity.isAlive`: not removed and still has health.
    fn is_alive(entity: &dyn EntityBase) -> bool {
        entity.get_entity().is_alive()
            && entity
                .get_living_entity()
                .is_some_and(|living| living.health.load() > 0.0)
    }

    /// Vanilla checks `leashHolder instanceof LeashFenceKnotEntity` before moving, because a
    /// llama tied to a fence is moved by the leash follower logic instead.
    fn leashed_to_leash_knot(entity: &Entity) -> bool {
        let holder = entity
            .leashed_to
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        holder.is_some_and(|holder| holder.get_entity().entity_type == &EntityType::LEASH_KNOT)
    }
}

impl Goal for LlamaFollowCaravanGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(caravan) = llama_caravan(mob) else {
            return false;
        };
        // Vanilla: `!this.llama.isLeashed() && !this.llama.inCaravan()`.
        if mob.get_entity().is_leashed() || caravan.in_caravan() {
            return false;
        }

        let entity = mob.get_entity();
        let my_pos = entity.pos.load();
        let search_box = entity.bounding_box.load().expand(
            Self::SEARCH_XZ_RANGE,
            Self::SEARCH_Y_RANGE,
            Self::SEARCH_XZ_RANGE,
        );
        let world = entity.world.load();
        let candidates: Vec<Arc<dyn EntityBase>> = world
            .get_entities_at_box(&search_box)
            .into_iter()
            .filter(|candidate| {
                let candidate_type = candidate.get_entity().entity_type;
                candidate_type == &EntityType::LLAMA || candidate_type == &EntityType::TRADER_LLAMA
            })
            .collect();

        let mut closest: Option<(f64, Arc<dyn EntityBase>)> = None;

        // Vanilla scans the same candidate list twice: first in-caravan heads with a free
        // tail, then leashed llamas with a free tail.
        for candidate in &candidates {
            let Some(candidate_caravan) = llama_caravan(candidate.as_ref()) else {
                continue;
            };
            if candidate_caravan.in_caravan() && !candidate_caravan.has_caravan_tail() {
                let dist_sq = my_pos.squared_distance_to_vec(&candidate.get_entity().pos.load());
                if closest
                    .as_ref()
                    .is_none_or(|(current, _)| dist_sq <= *current)
                {
                    closest = Some((dist_sq, candidate.clone()));
                }
            }
        }

        if closest.is_none() {
            for candidate in &candidates {
                let Some(candidate_caravan) = llama_caravan(candidate.as_ref()) else {
                    continue;
                };
                if candidate.get_entity().is_leashed() && !candidate_caravan.has_caravan_tail() {
                    let dist_sq =
                        my_pos.squared_distance_to_vec(&candidate.get_entity().pos.load());
                    if closest
                        .as_ref()
                        .is_none_or(|(current, _)| dist_sq <= *current)
                    {
                        closest = Some((dist_sq, candidate.clone()));
                    }
                }
            }
        }

        let Some((dist_sq, head)) = closest else {
            return false;
        };

        if dist_sq < Self::MIN_JOIN_DISTANCE_SQ {
            return false;
        }

        let head_leashed = head.get_entity().is_leashed();
        if !head_leashed && !Self::first_is_leashed(head.as_ref(), 1) {
            return false;
        }

        caravan.join_caravan(entity.entity_uuid, head);
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(caravan) = llama_caravan(mob) else {
            return false;
        };
        let Some(head) = caravan.get_caravan_head() else {
            return false;
        };
        // Vanilla: `getCaravanHead().isAlive() && firstIsLeashed(this.llama, 0)`.
        if !Self::is_alive(head.as_ref()) || !Self::first_is_leashed(mob, 0) {
            return false;
        }

        let dist_sq = mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&head.get_entity().pos.load());

        if dist_sq > Self::FOLLOW_DISTANCE_SQ {
            if self.speed_modifier <= Self::MAX_SPEED_MODIFIER {
                self.speed_modifier *= Self::SPEED_UP_FACTOR;
                self.dist_check_counter = to_goal_ticks(Self::DIST_CHECK_TICKS);
                return true;
            }
            if self.dist_check_counter == 0 {
                return false;
            }
        }

        if self.dist_check_counter > 0 {
            self.dist_check_counter -= 1;
        }
        true
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(caravan) = llama_caravan(mob) {
            caravan.leave_caravan();
        }
        self.speed_modifier = Self::DEFAULT_SPEED_MODIFIER;
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(caravan) = llama_caravan(mob) else {
            return;
        };
        let Some(head) = caravan.get_caravan_head() else {
            return;
        };
        let entity = mob.get_entity();
        if Self::leashed_to_leash_knot(entity) {
            return;
        }

        let my_pos = entity.pos.load();
        let diff = head.get_entity().pos.load() - my_pos;
        let distance = diff.length();
        let delta = diff.normalize() * (distance - Self::FOLLOW_STOP_DISTANCE).max(0.0);

        let mut navigator = mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_progress(NavigatorGoal::new(
            my_pos,
            my_pos + delta,
            self.speed_modifier,
        ));
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

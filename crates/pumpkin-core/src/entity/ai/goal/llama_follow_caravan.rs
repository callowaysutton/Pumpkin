use std::sync::{Arc, Weak};

use super::{Controls, Goal, to_goal_ticks};
use crate::entity::Entity;
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::passive::llama::Llama;
use pumpkin_data::entity::EntityType;
use uuid::Uuid;

/// Vanilla `LlamaFollowCaravanGoal.CARAVAN_LIMIT`; caps how far a caravan chain is walked.
const CARAVAN_LIMIT: i32 = 8;

/// Mirrors vanilla `LlamaFollowCaravanGoal`. An unleashed, unattached llama searches for the
/// end of a leashed llama caravan nearby and follows its tail, keeping a distance of two blocks.
pub struct LlamaFollowCaravanGoal {
    /// The llama this instance runs for. Caravan links are [`Weak`] handles so joining a
    /// caravan never keeps an entity alive; this one supplies the tail side of the link.
    llama_weak: Weak<dyn EntityBase>,
    goal_control: Controls,
    speed_modifier: f64,
    dist_check_counter: i32,
}

impl LlamaFollowCaravanGoal {
    /// Speed the goal is registered with; vanilla resets to the same value in `stop()`.
    pub const CARAVAN_SPEED_MODIFIER: f64 = 2.1;

    #[must_use]
    pub fn new(llama_weak: Weak<dyn EntityBase>, speed_modifier: f64) -> Self {
        Self {
            llama_weak,
            goal_control: Controls::MOVE,
            speed_modifier,
            dist_check_counter: 0,
        }
    }

    /// Mirrors vanilla `LlamaFollowCaravanGoal#firstIsLeashed`: does the top of the caravan chain
    /// starting at `current` have a leash?
    fn first_is_leashed(current: &dyn Llama, counter: i32) -> bool {
        if counter > CARAVAN_LIMIT {
            return false;
        }
        if !current.in_caravan() {
            return false;
        }
        let Some(head) = current.get_caravan_head() else {
            return false;
        };
        if head.get_entity().is_leashed() {
            return true;
        }
        let Some(head_llama) = head.get_mob().and_then(Mob::as_llama) else {
            return false;
        };
        Self::first_is_leashed(head_llama, counter + 1)
    }

    /// The candidates vanilla scans with `getBoundingBox().inflate(9.0, 4.0, 9.0)` are all
    /// llamas; anything else, and this llama itself, is filtered out here.
    fn candidate_llama<'a>(
        candidate: &'a Arc<dyn EntityBase>,
        my_uuid: &Uuid,
    ) -> Option<(&'a Entity, &'a dyn Llama)> {
        let candidate_entity = candidate.get_entity();
        if candidate_entity.entity_uuid == *my_uuid {
            return None;
        }
        if candidate_entity.entity_type != &EntityType::LLAMA
            && candidate_entity.entity_type != &EntityType::TRADER_LLAMA
        {
            return None;
        }
        Some((candidate_entity, candidate.get_mob()?.as_llama()?))
    }
}

impl Goal for LlamaFollowCaravanGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(llama) = mob.as_llama() else {
            return false;
        };
        // Vanilla: !this.llama.isLeashed() && !this.llama.inCaravan()
        if mob.get_entity().is_leashed() || llama.in_caravan() {
            return false;
        }

        let entity = mob.get_entity();
        let world = entity.world.load();
        let my_uuid = entity.entity_uuid;

        // Vanilla: this.llama.getBoundingBox().inflate(9.0, 4.0, 9.0)
        let search_area = entity.bounding_box.load().expand(9.0, 4.0, 9.0);
        let candidates = world.get_entities_at_box(&search_area);

        let mut closest: Option<(f64, Arc<dyn EntityBase>)> = None;

        // First pass: join behind the tail of a caravan we can extend.
        for candidate in &candidates {
            let Some((_, candidate_llama)) = Self::candidate_llama(candidate, &my_uuid) else {
                continue;
            };
            if candidate_llama.in_caravan() && !candidate_llama.has_caravan_tail() {
                let dist_sq = entity
                    .pos
                    .load()
                    .squared_distance_to_vec(&candidate.get_entity().pos.load());
                if closest
                    .as_ref()
                    .is_none_or(|(best_dist, _)| dist_sq <= *best_dist)
                {
                    closest = Some((dist_sq, candidate.clone()));
                }
            }
        }

        // Second pass: a leashed llama without a tail can start a new caravan.
        if closest.is_none() {
            for candidate in &candidates {
                let Some((candidate_entity, candidate_llama)) =
                    Self::candidate_llama(candidate, &my_uuid)
                else {
                    continue;
                };
                if candidate_entity.is_leashed() && !candidate_llama.has_caravan_tail() {
                    let dist_sq = entity
                        .pos
                        .load()
                        .squared_distance_to_vec(&candidate_entity.pos.load());
                    if closest
                        .as_ref()
                        .is_none_or(|(best_dist, _)| dist_sq <= *best_dist)
                    {
                        closest = Some((dist_sq, candidate.clone()));
                    }
                }
            }
        }

        let Some((closest_dist_sq, closest)) = closest else {
            return false;
        };
        // Too close to the caravan tail: walking to it would not help.
        if closest_dist_sq < 4.0 {
            return false;
        }
        let Some(closest_llama) = closest.get_mob().and_then(Mob::as_llama) else {
            return false;
        };
        // Only join caravans whose top is leashed.
        if !closest.get_entity().is_leashed() && !Self::first_is_leashed(closest_llama, 1) {
            return false;
        }

        // Vanilla: this.llama.joinCaravan(closest)
        llama.join_caravan(&self.llama_weak, &closest)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(llama) = mob.as_llama() else {
            return false;
        };
        let Some(head) = llama.get_caravan_head() else {
            return false;
        };
        if !head.get_entity().is_alive() {
            return false;
        }
        if !Self::first_is_leashed(llama, 0) {
            return false;
        }

        let dist_sq = mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&head.get_entity().pos.load());
        // Vanilla: 676.0 is 26 squared, the leash snap distance.
        if dist_sq > 676.0 {
            if self.speed_modifier <= 3.0 {
                // We are too slow to catch up: speed up for 40 ticks (20 in goal ticks).
                self.speed_modifier *= 1.2;
                self.dist_check_counter = to_goal_ticks(40);
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
        if let Some(llama) = mob.as_llama() {
            llama.leave_caravan(&self.llama_weak);
        }
        self.speed_modifier = Self::CARAVAN_SPEED_MODIFIER;
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(llama) = mob.as_llama() else {
            return;
        };
        if !llama.in_caravan() {
            return;
        }
        // A llama leashed to a leash knot is held in place by the leash itself.
        let entity = mob.get_entity();
        let leashed_to_knot = entity
            .get_leash_holder()
            .is_some_and(|holder| holder.get_entity().entity_type == &EntityType::LEASH_KNOT);
        if leashed_to_knot {
            return;
        }

        let Some(head) = llama.get_caravan_head() else {
            return;
        };

        // Vanilla: move to `llama position + normalize(head - llama) * max(distance - 2.0, 0.0)`.
        let head_pos = head.get_entity().pos.load();
        let mob_pos = entity.pos.load();
        let distance = mob_pos.squared_distance_to_vec(&head_pos).sqrt();
        let delta = (head_pos - mob_pos).normalize() * f64::max(distance - 2.0, 0.0);

        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal::new(
                mob_pos,
                mob_pos + delta,
                self.speed_modifier,
            ));
    }
}

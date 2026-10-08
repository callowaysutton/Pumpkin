use super::{Controls, Goal, to_goal_ticks};
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::passive::schooling_fish::SchoolingFish;
use rand::RngExt;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

/// Distance at which a follower stops chasing its leader (vanilla `121.0` squared).
const LEADER_RANGE_SQ: f64 = 121.0;
const SEARCH_RANGE: f64 = 8.0;
const INTERVAL_TICKS: i32 = 200;
const PATH_RECALC_TICKS: i32 = 10;

/// Vanilla `FollowFlockLeaderGoal`: a schooling fish without followers looks for
/// nearby fish to school with and follows the chosen leader.
pub struct FollowFlockLeaderGoal {
    goal_control: Controls,
    self_weak: Weak<dyn EntityBase>,
    time_to_recalc_path: i32,
    next_start_tick: i32,
}

impl FollowFlockLeaderGoal {
    #[must_use]
    pub fn new(self_weak: Weak<dyn EntityBase>) -> Self {
        Self {
            goal_control: Controls::MOVE,
            self_weak,
            time_to_recalc_path: 0,
            next_start_tick: -1,
        }
    }

    /// Vanilla `nextStartTick`, computed lazily on the first check so the random
    /// draw uses the mob's rng.
    fn next_start_tick(mob: &dyn Mob) -> i32 {
        to_goal_ticks(INTERVAL_TICKS + mob.get_random().random_range(0..200) % 20)
    }

    fn stop_following(schooling: &dyn SchoolingFish) {
        let leader = schooling
            .leader()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(leader) = leader.and_then(|leader| leader.upgrade())
            && let Some(leader_schooling) = leader
                .get_mob()
                .and_then(crate::entity::mob::Mob::as_schooling_fish)
        {
            leader_schooling
                .school_size()
                .fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn start_following(schooling: &dyn SchoolingFish, leader: Weak<dyn EntityBase>) {
        *schooling
            .leader()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(leader);
    }

    /// Nearby fish of the same type, matching vanilla `getEntitiesOfClass` on the
    /// mob's class over an inflated bounding box. The mob itself is included.
    fn nearby_schoolmates(mob: &dyn Mob) -> Vec<Arc<dyn EntityBase>> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let search_box =
            entity
                .bounding_box
                .load()
                .expand(SEARCH_RANGE, SEARCH_RANGE, SEARCH_RANGE);
        let entity_type = entity.entity_type;

        world
            .get_all_at_box(&search_box)
            .into_iter()
            .filter(|other| other.get_entity().entity_type == entity_type)
            .collect()
    }

    /// Vanilla `AbstractSchoolingFish.addFollowers`: make nearby non-followers
    /// follow `leader`, respecting the leader's remaining school capacity.
    fn add_followers(
        leader_schooling: &dyn SchoolingFish,
        leader: &Weak<dyn EntityBase>,
        leader_id: i32,
        candidates: &[Arc<dyn EntityBase>],
    ) {
        let remaining = (leader_schooling.get_max_school_size()
            - leader_schooling.school_size().load(Ordering::Relaxed))
        .max(0) as usize;

        let joiners = candidates
            .iter()
            .filter(|candidate| {
                candidate
                    .get_mob()
                    .and_then(crate::entity::mob::Mob::as_schooling_fish)
                    .is_some_and(|fish| !fish.is_follower())
            })
            .take(remaining)
            .filter(|candidate| candidate.get_entity().entity_id != leader_id);

        for candidate in joiners {
            let Some(candidate_schooling) = candidate
                .get_mob()
                .and_then(crate::entity::mob::Mob::as_schooling_fish)
            else {
                continue;
            };
            Self::start_following(candidate_schooling, leader.clone());
            leader_schooling
                .school_size()
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    fn in_range_of_leader(mob: &dyn Mob, schooling: &dyn SchoolingFish) -> bool {
        let mob_pos = mob.get_entity().pos.load();
        schooling
            .leader()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|leader| {
                mob_pos.squared_distance_to_vec(&leader.get_entity().pos.load()) <= LEADER_RANGE_SQ
            })
    }

    fn path_to_leader(mob: &dyn Mob, schooling: &dyn SchoolingFish) {
        let Some(leader) = schooling
            .leader()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
        else {
            return;
        };

        let mob_pos = mob.get_entity().pos.load();
        let leader_pos = leader.get_entity().pos.load();
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal::new(mob_pos, leader_pos, 1.0));
    }
}

impl Goal for FollowFlockLeaderGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(schooling) = mob.as_schooling_fish() else {
            return false;
        };

        if schooling.has_followers() {
            return false;
        }
        if schooling.is_follower() {
            return true;
        }
        if self.next_start_tick < 0 {
            self.next_start_tick = Self::next_start_tick(mob);
        }
        if self.next_start_tick > 0 {
            self.next_start_tick -= 1;
            return false;
        }
        self.next_start_tick = Self::next_start_tick(mob);

        let candidates = Self::nearby_schoolmates(mob);
        // Prefer a leader that still has room, otherwise make this mob the leader.
        let chosen_leader = candidates
            .iter()
            .find(|candidate| {
                candidate
                    .get_mob()
                    .and_then(crate::entity::mob::Mob::as_schooling_fish)
                    .is_some_and(
                        crate::entity::passive::schooling_fish::SchoolingFish::can_be_followed,
                    )
            })
            .cloned();

        if let Some(leader) = chosen_leader {
            let Some(leader_schooling) = leader
                .get_mob()
                .and_then(crate::entity::mob::Mob::as_schooling_fish)
            else {
                return false;
            };
            Self::add_followers(
                leader_schooling,
                &Arc::downgrade(&leader),
                leader.get_entity().entity_id,
                &candidates,
            );
        } else {
            Self::add_followers(
                schooling,
                &self.self_weak,
                mob.get_entity().entity_id,
                &candidates,
            );
        }

        schooling.is_follower()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.as_schooling_fish().is_some_and(|schooling| {
            schooling.is_follower() && Self::in_range_of_leader(mob, schooling)
        })
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.time_to_recalc_path = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(schooling) = mob.as_schooling_fish() {
            Self::stop_following(schooling);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.time_to_recalc_path -= 1;
        if self.time_to_recalc_path <= 0 {
            self.time_to_recalc_path = to_goal_ticks(PATH_RECALC_TICKS);
            if let Some(schooling) = mob.as_schooling_fish() {
                Self::path_to_leader(mob, schooling);
            }
        }
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

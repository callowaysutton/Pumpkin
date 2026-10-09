use std::sync::Arc;

use rand::RngExt;

use super::{Controls, Goal, to_goal_ticks};
use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::passive::abstract_schooling_fish::SCHOOL_SEARCH_RANGE;

/// Port of vanilla `FollowFlockLeaderGoal`: a schooling fish joins a school with room or
/// becomes a leader the other fish follow.
///
/// The goal holds no flags, so `FishSwimGoal` and the navigation keep driving the leader while
/// the goal waits.
pub struct FollowFlockLeaderGoal {
    goal_control: Controls,
    next_start_tick: i32,
    time_to_recalc_path: i32,
}

impl FollowFlockLeaderGoal {
    #[must_use]
    pub fn new() -> Self {
        // Vanilla FollowFlockLeaderGoal#nextStartTick, seeded from the mob's random there.
        Self {
            goal_control: Controls::empty(),
            next_start_tick: to_goal_ticks(200 + rand::rng().random_range(0..200) % 20),
            time_to_recalc_path: 0,
        }
    }

    /// Vanilla `FollowFlockLeaderGoal#nextStartTick`.
    fn reset_next_start_tick(&mut self, mob: &dyn Mob) {
        self.next_start_tick = to_goal_ticks(200 + mob.get_random().random_range(0..200) % 20);
    }
}

impl Default for FollowFlockLeaderGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for FollowFlockLeaderGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(fish) = mob.as_schooling_fish() else {
            return false;
        };

        if fish.has_followers() {
            return false;
        }
        if fish.is_follower() {
            return true;
        }
        if self.next_start_tick > 0 {
            self.next_start_tick -= 1;
            return false;
        }
        self.reset_next_start_tick(mob);

        let entity = mob.get_entity();
        let world = entity.world.load();
        let search_box = entity.bounding_box.load().expand_all(SCHOOL_SEARCH_RANGE);

        // Vanilla: level().getEntitiesOfClass(..., fish -> fish.canBeFollowed() || !fish.isFollower())
        let candidates: Vec<Arc<dyn EntityBase>> = world
            .get_entities_at_box(&search_box)
            .into_iter()
            .filter(|other| other.get_entity().entity_type == entity.entity_type)
            .filter(|other| {
                other
                    .get_mob()
                    .and_then(|m| m.as_schooling_fish())
                    .is_some_and(|school| school.can_be_followed() || !school.is_follower())
            })
            .collect();

        // Vanilla: the first fish with school room leads, otherwise the mob itself volunteers
        // (which is among the candidates, inside its own search box).
        let leader = candidates
            .iter()
            .find_map(|other| {
                let school = other.get_mob()?.as_schooling_fish()?;
                school.can_be_followed().then(|| other.clone())
            })
            .or_else(|| {
                candidates
                    .iter()
                    .find(|other| other.get_entity().entity_uuid == entity.entity_uuid)
                    .cloned()
            });

        let Some(leader) = leader else {
            return false;
        };
        let Some(leader_school) = leader.get_mob().and_then(|m| m.as_schooling_fish()) else {
            return false;
        };

        // Vanilla AbstractSchoolingFish.addFollowers: non-followers join until the leader has no
        // room left; the follower limit is fixed when the stream is created.
        let free_slots = leader_school.max_school_size() - leader_school.schooling().school_size();
        let leader_uuid = leader.get_entity().entity_uuid;
        let mut joined = 0;
        for other in &candidates {
            if joined >= free_slots {
                break;
            }
            if other.get_entity().entity_uuid == leader_uuid {
                continue;
            }
            let Some(other_school) = other.get_mob().and_then(|m| m.as_schooling_fish()) else {
                continue;
            };
            if other_school.is_follower() {
                continue;
            }
            other_school.schooling().start_following(&leader);
            joined += 1;
        }

        fish.is_follower()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.as_schooling_fish()
            .is_some_and(|fish| fish.is_follower() && fish.in_range_of_leader())
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.time_to_recalc_path = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(fish) = mob.as_schooling_fish() {
            fish.stop_following();
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.time_to_recalc_path -= 1;
        if self.time_to_recalc_path > 0 {
            return;
        }
        self.time_to_recalc_path = self.get_tick_count(10);
        if let Some(fish) = mob.as_schooling_fish() {
            fish.path_to_leader();
        }
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

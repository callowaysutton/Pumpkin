use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI32, Ordering},
};

use crate::entity::{EntityBase, ai::pathfinder::NavigatorGoal, mob::Mob};
use rand::RngExt;

/// How far away a follower may drift from its leader (vanilla `inRangeOfLeader`).
const LEADER_RANGE_SQ: f64 = 121.0;
/// Vanilla `AbstractSchoolingFish.tick` checks the neighborhood every 200 ticks.
const SCHOOL_RESET_INTERVAL: i32 = 200;
/// Range at which school members look for each other: the vanilla 8.0 bounding box inflation
/// shared by the follow and the dissolve query.
pub const SCHOOL_SEARCH_RANGE: f64 = 8.0;

/// The shared state of vanilla `AbstractSchoolingFish`: the `leader` this fish follows and,
/// from the leader's perspective, the `schoolSize` follower count.
pub struct SchoolingData {
    leader: Mutex<Option<Arc<dyn EntityBase>>>,
    school_size: AtomicI32,
}

impl Default for SchoolingData {
    fn default() -> Self {
        // Vanilla starts every fish as a school of one.
        Self {
            leader: Mutex::new(None),
            school_size: AtomicI32::new(1),
        }
    }
}

impl SchoolingData {
    #[must_use]
    pub fn school_size(&self) -> i32 {
        self.school_size.load(Ordering::Relaxed)
    }

    /// Vanilla `AbstractSchoolingFish.hasFollowers`.
    #[must_use]
    pub fn has_followers(&self) -> bool {
        self.school_size() > 1
    }

    /// Vanilla `AbstractSchoolingFish.canBeFollowed`.
    #[must_use]
    pub fn can_be_followed(&self, max_school_size: i32) -> bool {
        self.has_followers() && self.school_size() < max_school_size
    }

    /// The leader pointer, `None` while this fish is independent.
    pub fn leader(&self) -> Option<Arc<dyn EntityBase>> {
        self.leader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn store_leader(&self, leader: Arc<dyn EntityBase>) {
        *self
            .leader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(leader);
    }

    fn take_leader(&self) -> Option<Arc<dyn EntityBase>> {
        self.leader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    /// Vanilla `AbstractSchoolingFish.startFollowing`: point at `leader` and count on it.
    pub fn start_following(&self, leader: &Arc<dyn EntityBase>) {
        self.store_leader(leader.clone());
        if let Some(leader_fish) = fish_schooling(leader) {
            leader_fish.schooling().add_follower();
        }
    }

    /// Vanilla `AbstractSchoolingFish.stopFollowing`.
    pub fn stop_following(&self) {
        if let Some(leader) = self.take_leader()
            && let Some(leader_fish) = fish_schooling(&leader)
        {
            leader_fish.schooling().remove_follower();
        }
    }

    // The counter lives on the leader, so mutations go through the leader's own state.
    fn add_follower(&self) {
        self.school_size.fetch_add(1, Ordering::Relaxed);
    }

    fn remove_follower(&self) {
        self.school_size
            .store(self.school_size().saturating_sub(1), Ordering::Relaxed);
    }

    /// Forget the school once it has scattered (called from [`SchoolingFish::tick_schooling`]).
    fn reset_school_size(&self) {
        self.school_size.store(1, Ordering::Relaxed);
    }
}

/// The `SchoolingFish` half of an entity, if it is a schooling fish like cod.
pub fn fish_schooling(entity: &Arc<dyn EntityBase>) -> Option<&dyn SchoolingFish> {
    entity.get_mob()?.as_schooling_fish()
}

/// Vanilla `AbstractSchoolingFish.SchoolSpawnGroupData`: the leader of one spawned school,
/// shared by the spawn-group data of the whole batch.
pub struct SchoolSpawnGroupData {
    pub leader: Arc<dyn EntityBase>,
}

/// Behaviour of vanilla `AbstractSchoolingFish`, on top of the usual mob behaviour.
pub trait SchoolingFish: Mob {
    /// The shared leader/school-size state.
    #[must_use]
    fn schooling(&self) -> &SchoolingData;

    /// Vanilla `getMaxSchoolSize`: how many fish may follow one leader.
    #[must_use]
    fn max_school_size(&self) -> i32 {
        self.get_entity().entity_type.limit_per_chunk
    }

    /// Vanilla `isFollower`: the leader must still be alive.
    #[must_use]
    fn is_follower(&self) -> bool {
        self.schooling()
            .leader()
            .is_some_and(|leader| fish_is_alive(leader.as_ref()))
    }

    /// Vanilla `hasFollowers`.
    #[must_use]
    fn has_followers(&self) -> bool {
        self.schooling().has_followers()
    }

    /// Vanilla `canBeFollowed`.
    #[must_use]
    fn can_be_followed(&self) -> bool {
        self.schooling().can_be_followed(self.max_school_size())
    }

    /// Vanilla `canRandomSwim`: schooling fish only wander about on their own while independent.
    #[must_use]
    fn can_random_swim(&self) -> bool {
        !self.is_follower()
    }

    /// Vanilla `inRangeOfLeader`.
    #[must_use]
    fn in_range_of_leader(&self) -> bool {
        self.schooling().leader().is_some_and(|leader| {
            self.get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&leader.get_entity().pos.load())
                <= LEADER_RANGE_SQ
        })
    }

    /// Vanilla `pathToLeader`.
    fn path_to_leader(&self) {
        if !self.is_follower() {
            return;
        }
        let Some(leader) = self.schooling().leader() else {
            return;
        };
        let mob_entity = self.get_mob_entity();
        let pos = mob_entity.living_entity.entity.pos.load();
        let leader_pos = leader.get_entity().pos.load();
        mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal::new(pos, leader_pos, 1.0));
    }

    /// Vanilla `stopFollowing`.
    fn stop_following(&self) {
        self.schooling().stop_following();
    }

    /// Vanilla `AbstractSchoolingFish.tick`: forget a school once every other fish of the same
    /// kind is gone, so a scattered school can reform somewhere else.
    fn tick_schooling(&self) {
        if !self.has_followers() {
            return;
        }
        if self.get_random().random_range(0..SCHOOL_RESET_INTERVAL) != 1 {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        let neighborhood = entity.bounding_box.load().expand(
            SCHOOL_SEARCH_RANGE,
            SCHOOL_SEARCH_RANGE,
            SCHOOL_SEARCH_RANGE,
        );
        let neighbors = world
            .get_entities_at_box(&neighborhood)
            .into_iter()
            .filter(|other| other.get_entity().entity_type == entity.entity_type)
            .count();
        if neighbors <= 1 {
            self.schooling().reset_school_size();
        }
    }
}

/// Vanilla `LivingEntity.isAlive` for a schooling fish: not removed and still has health.
fn fish_is_alive(fish: &dyn EntityBase) -> bool {
    !fish.get_entity().is_removed()
        && fish
            .get_living_entity()
            .is_none_or(|living| living.health.load() > 0.0)
}

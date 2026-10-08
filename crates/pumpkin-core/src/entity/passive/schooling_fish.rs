use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, Weak};

use crate::entity::EntityBase;

/// Vanilla `AbstractSchoolingFish` state: a follower keeps a reference to its
/// leader, while a leader only tracks how many fish follow it.
pub trait SchoolingFish: Send + Sync {
    fn school_size(&self) -> &AtomicI32;

    fn leader(&self) -> &Mutex<Option<Weak<dyn EntityBase>>>;

    /// Vanilla `AbstractSchoolingFish.getMaxSchoolSize`, which falls back to
    /// `AbstractFish.getMaxSpawnClusterSize`.
    fn get_max_school_size(&self) -> i32 {
        8
    }

    fn has_followers(&self) -> bool {
        self.school_size().load(Ordering::Relaxed) > 1
    }

    /// Vanilla `AbstractSchoolingFish.isFollower`.
    fn is_follower(&self) -> bool {
        self.leader()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|leader| leader.get_entity().is_alive())
    }

    /// Vanilla `AbstractSchoolingFish.canBeFollowed`.
    fn can_be_followed(&self) -> bool {
        self.has_followers()
            && self.school_size().load(Ordering::Relaxed) < self.get_max_school_size()
    }
}

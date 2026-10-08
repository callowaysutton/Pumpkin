use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, Weak};

use pumpkin_data::entity::EntityType;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        avoid_entity::AvoidEntityGoal, escape_danger::EscapeDangerGoal, fish_swim::FishSwimGoal,
        follow_flock_leader::FollowFlockLeaderGoal,
    },
    mob::{Mob, MobEntity},
    passive::schooling_fish::SchoolingFish,
};

/// Vanilla `AbstractFish.FishSwimGoal` parameters.
const SWIM_SPEED: f64 = 1.0;
const SWIM_INTERVAL: i32 = 40;
/// Vanilla `AbstractFish` panic/avoid goals.
const PANIC_SPEED: f64 = 1.25;
const AVOID_DISTANCE: f64 = 8.0;
const AVOID_SLOW_SPEED: f64 = 1.6;
const AVOID_FAST_SPEED: f64 = 1.4;
/// Vanilla `AbstractSchoolingFish.tick` neighbour search inflation.
const SCHOOL_NEIGHBOUR_RANGE: f64 = 8.0;

pub struct TropicalFishEntity {
    pub mob_entity: MobEntity,
    school_size: AtomicI32,
    leader: Mutex<Option<Weak<dyn EntityBase>>>,
}

impl TropicalFishEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let tropical_fish = Self {
            mob_entity,
            school_size: AtomicI32::new(1),
            leader: Mutex::new(None),
        };
        let mob_arc = Arc::new(tropical_fish);
        let self_weak: Weak<dyn EntityBase> = {
            let entity_base: Arc<dyn EntityBase> = mob_arc.clone();
            Arc::downgrade(&entity_base)
        };

        {
            let mut navigator = mob_arc
                .mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *navigator = crate::entity::ai::pathfinder::Navigator::water_bound(false);
            navigator.set_mob_dimensions(
                EntityType::TROPICAL_FISH.dimension[0],
                EntityType::TROPICAL_FISH.dimension[1],
            );
        };

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Vanilla `AbstractFish.registerGoals`.
            goal_selector.add_goal(0, EscapeDangerGoal::new(PANIC_SPEED));
            goal_selector.add_goal(
                2,
                Box::new(AvoidEntityGoal::new(
                    &EntityType::PLAYER,
                    AVOID_DISTANCE,
                    AVOID_SLOW_SPEED,
                    AVOID_FAST_SPEED,
                )),
            );
            goal_selector.add_goal(4, Box::new(FishSwimGoal::new(SWIM_SPEED, SWIM_INTERVAL)));
            // Vanilla `AbstractSchoolingFish.registerGoals`.
            goal_selector.add_goal(5, Box::new(FollowFlockLeaderGoal::new(self_weak)));
        };

        mob_arc
    }
}

impl Mob for TropicalFishEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `AbstractSchoolingFish.tick`: a stale school size is reset once its
    /// neighbours are gone.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        if !self.has_followers() {
            return;
        }

        let entity = &self.mob_entity.living_entity.entity;
        if self.get_random().random_range(0..200) != 1 {
            return;
        }

        let search_box = entity.bounding_box.load().expand(
            SCHOOL_NEIGHBOUR_RANGE,
            SCHOOL_NEIGHBOUR_RANGE,
            SCHOOL_NEIGHBOUR_RANGE,
        );
        let entity_type = entity.entity_type;
        let neighbours = entity
            .world
            .load()
            .get_all_at_box(&search_box)
            .into_iter()
            .filter(|other| other.get_entity().entity_type == entity_type)
            .count();

        if neighbours <= 1 {
            self.school_size.store(1, Ordering::Relaxed);
        }
    }

    /// Vanilla `AbstractSchoolingFish.canRandomSwim`.
    fn can_random_swim(&self) -> bool {
        !self.is_follower()
    }

    fn as_schooling_fish(&self) -> Option<&dyn SchoolingFish> {
        Some(self)
    }
}

impl SchoolingFish for TropicalFishEntity {
    fn school_size(&self) -> &AtomicI32 {
        &self.school_size
    }

    fn leader(&self) -> &Mutex<Option<Weak<dyn EntityBase>>> {
        &self.leader
    }
}

use std::sync::Arc;

use pumpkin_data::entity::EntityType;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        avoid_entity::AvoidEntityGoal, escape_danger::EscapeDangerGoal, fish_swim::FishSwimGoal,
        follow_flock_leader::FollowFlockLeaderGoal,
    },
    mob::{Mob, MobEntity},
    passive::abstract_schooling_fish::{SchoolingData, SchoolingFish},
};

/// Vanilla `AbstractFish` panic/avoid goals.
const PANIC_SPEED: f64 = 1.25;
const AVOID_DISTANCE: f64 = 8.0;
const AVOID_SLOW_SPEED: f64 = 1.6;
const AVOID_FAST_SPEED: f64 = 1.4;

pub struct TropicalFishEntity {
    pub mob_entity: MobEntity,
    schooling: SchoolingData,
}

impl TropicalFishEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let tropical_fish = Self {
            mob_entity,
            schooling: SchoolingData::default(),
        };
        let mob_arc = Arc::new(tropical_fish);

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
            goal_selector.add_goal(4, Box::new(FishSwimGoal::new()));
            // Vanilla `AbstractSchoolingFish.registerGoals`.
            goal_selector.add_goal(5, Box::new(FollowFlockLeaderGoal::new()));
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
        self.tick_schooling();
    }

    fn as_schooling_fish(&self) -> Option<&dyn SchoolingFish> {
        Some(self)
    }
}

impl SchoolingFish for TropicalFishEntity {
    fn schooling(&self) -> &SchoolingData {
        &self.schooling
    }
}

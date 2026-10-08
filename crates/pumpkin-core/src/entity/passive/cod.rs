use std::sync::{Arc, atomic::Ordering};

use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ai::{
        goal::{
            avoid_entity::AvoidEntityGoal, escape_danger::EscapeDangerGoal,
            fish_swim::FishSwimGoal, follow_flock_leader::FollowFlockLeaderGoal,
        },
        pathfinder::{Navigator, node::PathType},
        util::goal_utils,
    },
    mob::{Mob, MobEntity},
    passive::abstract_schooling_fish::{SchoolingData, SchoolingFish},
};

/// Represents a Cod, a common passive aquatic mob.
///
/// Wiki: <https://minecraft.wiki/w/Cod>
pub struct CodEntity {
    pub mob_entity: MobEntity,
    schooling: SchoolingData,
}

impl CodEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let cod = Self {
            mob_entity,
            schooling: SchoolingData::default(),
        };
        let mob_arc = Arc::new(cod);

        {
            // Vanilla AbstractFish.createNavigation: fish swim with WaterBoundPathNavigation.
            let mut navigator = mob_arc
                .mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *navigator = Navigator::water_bound(false);
            // Vanilla WaterAnimal constructor: water has no pathfinding malus.
            navigator.set_pathfinding_malus(PathType::Water, 0.0);
            // Vanilla WaterBoundPathNavigation.createPathFinder disables door passing.
            navigator.set_can_pass_doors(false);
            navigator
                .set_mob_dimensions(EntityType::COD.dimension[0], EntityType::COD.dimension[1]);
            drop(navigator);
        };

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Vanilla AbstractFish.registerGoals and AbstractSchoolingFish.registerGoals.
            goal_selector.add_goal(0, EscapeDangerGoal::new(1.25));
            goal_selector.add_goal(
                2,
                Box::new(AvoidEntityGoal::new(&EntityType::PLAYER, 8.0, 1.6, 1.4)),
            );
            goal_selector.add_goal(4, Box::new(FishSwimGoal::new()));
            goal_selector.add_goal(5, Box::new(FollowFlockLeaderGoal::new()));
        };

        mob_arc
    }
}

impl SchoolingFish for CodEntity {
    fn schooling(&self) -> &SchoolingData {
        &self.schooling
    }
}

impl Mob for CodEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_schooling_fish(&self) -> Option<&dyn SchoolingFish> {
        Some(self)
    }

    /// Vanilla `AbstractSchoolingFish.tick`: keep the school size in sync with real neighbors.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.tick_schooling();
    }

    /// Vanilla `AbstractFish.aiStep` flop and `AbstractFish.FishMoveControl.tick` buoyancy.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();

        if !entity.touching_water.load(Ordering::SeqCst)
            && entity.on_ground.load(Ordering::SeqCst)
            && entity.vertical_collision.load(Ordering::SeqCst)
        {
            // Vanilla AbstractFish.aiStep: flop off the ground with a random push and sound.
            let mut velocity = entity.velocity.load();
            let x = f64::from((rand::rng().random::<f32>() * 2.0 - 1.0) * 0.05);
            let z = f64::from((rand::rng().random::<f32>() * 2.0 - 1.0) * 0.05);
            velocity = velocity.add_raw(x, 0.4, z);
            entity.velocity.store(velocity);
            entity.on_ground.store(false, Ordering::SeqCst);

            let world = entity.world.load();
            world.play_sound(
                Sound::EntityCodFlop,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
        } else if goal_utils::is_water(
            &entity.world.load(),
            &BlockPos::floored_v(entity.get_eye_pos()),
        ) {
            // Vanilla AbstractFish.FishMoveControl.tick: rise slightly while the eyes are in water.
            let mut velocity = entity.velocity.load();
            velocity.y += 0.005;
            entity.velocity.store(velocity);
        }
    }
}

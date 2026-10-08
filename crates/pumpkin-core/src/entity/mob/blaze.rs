use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        active_target::ActiveTargetGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, move_towards_restriction::MoveTowardsRestrictionGoal,
        swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    ai::util::RandomExt,
    mob::{Mob, MobEntity},
};

pub struct BlazeEntity {
    pub entity: Arc<MobEntity>,
    pub is_charged: AtomicBool,
    /// Vanilla `Blaze.allowedHeightOffset`.
    allowed_height_offset: AtomicCell<f32>,
    /// Vanilla `Blaze.nextHeightOffsetChangeTick`.
    next_height_offset_change_tick: AtomicCell<i32>,
}

impl BlazeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = Arc::new(MobEntity::new(entity));
        let blaze = Self {
            entity,
            is_charged: AtomicBool::new(false),
            allowed_height_offset: AtomicCell::new(0.5),
            next_height_offset_change_tick: AtomicCell::new(0),
        };
        let mob_arc = Arc::new(blaze);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        {
            let mut goal_selector = mob_arc
                .entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut target_selector = mob_arc
                .entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));

            goal_selector.add_goal(
                4,
                Box::new(
                    crate::entity::ai::goal::blaze_attack::BlazeShootFireballGoal::new(
                        Arc::downgrade(&mob_arc),
                    ),
                ),
            );
            goal_selector.add_goal(5, Box::new(MoveTowardsRestrictionGoal::new(1.0)));
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.entity, &EntityType::PLAYER, true),
            );
        };

        mob_arc
    }

    pub fn is_charged(&self) -> bool {
        self.is_charged.load(Ordering::Relaxed)
    }

    pub fn set_charged(&self, charged: bool) {
        self.is_charged.store(charged, Ordering::Relaxed);
        let flags = i8::from(charged);
        self.entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::blaze::DATA_FLAGS_ID, flags);
    }
}

impl Mob for BlazeEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity
    }

    fn mob_tick(&self, caller: &dyn EntityBase) {
        let base_entity = &self.entity.living_entity.entity;
        if !base_entity.is_alive() {
            return;
        }

        // Vanilla `Blaze.aiStep`: slowly falls while airborne.
        let on_ground = base_entity.on_ground.load(Ordering::Relaxed);
        let vel = base_entity.velocity.load();
        if !on_ground && vel.y < 0.0 {
            base_entity
                .velocity
                .store(Vector3::new(vel.x, vel.y * 0.6, vel.z));
        }

        // Vanilla `Blaze.customServerAiStep`: rises towards a target above it, which
        // makes the blaze bob above the player. The offset is re-rolled every 100 ticks.
        let next_height_offset_change_tick = self.next_height_offset_change_tick.load() - 1;
        if next_height_offset_change_tick <= 0 {
            self.next_height_offset_change_tick.store(100);
            self.allowed_height_offset
                .store(rand::rng().triangle(0.5, 6.891) as f32);
        } else {
            self.next_height_offset_change_tick
                .store(next_height_offset_change_tick);
        }

        if let Some(target) = self.entity.get_target()
            && target.get_eye_pos().y
                > base_entity.get_eye_pos().y + f64::from(self.allowed_height_offset.load())
            && self.can_attack(target.as_ref())
        {
            let vel = base_entity.velocity.load();
            base_entity
                .velocity
                .store(Vector3::new(vel.x, vel.y + (0.3 - vel.y) * 0.3, vel.z));
            // The tracker broadcasts the velocity like vanilla's `hasImpulse` sync.
            base_entity.velocity_dirty.store(true, Ordering::SeqCst);
        }

        // Vanilla `Blaze.isSensitiveToWater` through `LivingEntity`: takes damage in
        // the water or while it rains.
        let world = base_entity.world.load();
        let raining_at_feet = world.is_raining_at(&base_entity.block_pos.load());
        let raining_at_head = world.is_raining_at(&base_entity.bounding_box.load().max_block_pos());
        if base_entity.touching_water.load(Ordering::Relaxed) || raining_at_feet || raining_at_head
        {
            caller.damage(caller, 1.0, DamageType::DROWN);
        }
    }
}

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam::atomic::AtomicCell;

use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::server::Server;

use pumpkin_data::Block;
use pumpkin_data::damage::DamageType;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::vehicle::vehicle::VehicleEntity;

/// Vanilla `AbstractBoat.Status`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BoatStatus {
    InWater,
    UnderWater,
    UnderFlowingWater,
    OnLand,
    InAir,
}

const PADDLE_SPEED: f32 = std::f32::consts::PI / 8.0;
const PADDLE_SOUND_TIME: f32 = std::f32::consts::PI / 4.0;
const TIME_TO_EJECT: f32 = 60.0;
const DEFAULT_GRAVITY: f64 = 0.04;

pub struct BoatEntity {
    pub vehicle: VehicleEntity,
    left_paddle_moving: AtomicBool,
    right_paddle_moving: AtomicBool,
    // Vanilla `AbstractBoat` physics state.
    status: AtomicCell<BoatStatus>,
    old_status: AtomicCell<BoatStatus>,
    out_of_control_ticks: AtomicCell<f32>,
    water_level: AtomicCell<f64>,
    land_friction: AtomicCell<f32>,
    last_yd: AtomicCell<f64>,
    left_paddle_pos: AtomicCell<f32>,
    right_paddle_pos: AtomicCell<f32>,
}

impl BoatEntity {
    pub const fn new(entity: Entity) -> Self {
        Self {
            vehicle: VehicleEntity::new(entity),
            left_paddle_moving: AtomicBool::new(false),
            right_paddle_moving: AtomicBool::new(false),
            status: AtomicCell::new(BoatStatus::InAir),
            old_status: AtomicCell::new(BoatStatus::InAir),
            out_of_control_ticks: AtomicCell::new(0.0),
            water_level: AtomicCell::new(0.0),
            land_friction: AtomicCell::new(0.6),
            last_yd: AtomicCell::new(0.0),
            left_paddle_pos: AtomicCell::new(0.0),
            right_paddle_pos: AtomicCell::new(0.0),
        }
    }

    fn get_paddle_state(&self, side: usize) -> bool {
        let state = if side == 0 {
            self.left_paddle_moving.load(Ordering::Relaxed)
        } else {
            self.right_paddle_moving.load(Ordering::Relaxed)
        };
        state && self.has_controlling_passenger()
    }

    pub fn set_paddles(&self, left: bool, right: bool) {
        if self.left_paddle_moving.load(Ordering::Relaxed) == left
            && self.right_paddle_moving.load(Ordering::Relaxed) == right
        {
            return;
        }
        self.left_paddle_moving.store(left, Ordering::Relaxed);
        self.right_paddle_moving.store(right, Ordering::Relaxed);

        self.vehicle.entity.send_meta_data(
            &[
                Metadata::new(pumpkin_data::tracked_data::boat::ID_PADDLE_LEFT, left),
                Metadata::new(pumpkin_data::tracked_data::boat::ID_PADDLE_RIGHT, right),
            ],
            None,
        );
    }

    fn send_wobble_metadata(&self) {
        self.vehicle.send_wobble_metadata();
    }

    fn has_controlling_passenger(&self) -> bool {
        self.vehicle
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .is_some_and(|passenger| passenger.get_player().is_some())
    }

    /// Vanilla `AbstractBoat.isUnderwater`.
    fn is_underwater(&self) -> Option<BoatStatus> {
        let world = self.vehicle.entity.world.load();
        let aabb = self.vehicle.entity.bounding_box.load();
        let max_y = aabb.max.y + 0.001;
        let x0 = aabb.min.x.floor() as i32;
        let x1 = aabb.max.x.ceil() as i32;
        let y0 = aabb.max.y.floor() as i32;
        let y1 = max_y.ceil() as i32;
        let z0 = aabb.min.z.floor() as i32;
        let z1 = aabb.max.z.ceil() as i32;
        let mut under_water = false;

        for x in x0..x1 {
            for y in y0..y1 {
                for z in z0..z1 {
                    let pos = BlockPos(Vector3::new(x, y, z));
                    let (fluid, fluid_state) = world.get_fluid_and_fluid_state(&pos);
                    if fluid.matches_type(&Fluid::WATER)
                        && max_y
                            < f64::from(y)
                                + f64::from(world.get_fluid_height(&pos, fluid, &fluid_state))
                    {
                        if !fluid_state.is_source {
                            return Some(BoatStatus::UnderFlowingWater);
                        }
                        under_water = true;
                    }
                }
            }
        }

        under_water.then_some(BoatStatus::UnderWater)
    }

    /// Vanilla `AbstractBoat.checkInWater`.
    fn check_in_water(&self) -> bool {
        let world = self.vehicle.entity.world.load();
        let bb = self.vehicle.entity.bounding_box.load();
        let min_x = bb.min.x.floor() as i32;
        let max_x = bb.max.x.ceil() as i32;
        let min_y = bb.min.y.floor() as i32;
        let max_y = (bb.min.y + 0.001).ceil() as i32;
        let min_z = bb.min.z.floor() as i32;
        let max_z = bb.max.z.ceil() as i32;
        let mut in_water = false;
        let mut water_level = f64::MIN;

        for x in min_x..max_x {
            for y in min_y..max_y {
                for z in min_z..max_z {
                    let pos = BlockPos(Vector3::new(x, y, z));
                    let (fluid, fluid_state) = world.get_fluid_and_fluid_state(&pos);
                    if fluid.matches_type(&Fluid::WATER) {
                        let height = f64::from(y)
                            + f64::from(world.get_fluid_height(&pos, fluid, &fluid_state));
                        water_level = water_level.max(height);
                        in_water |= bb.min.y < height;
                    }
                }
            }
        }

        self.water_level.store(water_level);
        in_water
    }

    /// Vanilla `AbstractBoat.getWaterLevelAbove`.
    fn get_water_level_above(&self) -> f32 {
        let world = self.vehicle.entity.world.load();
        let aabb = self.vehicle.entity.bounding_box.load();
        let min_x = aabb.min.x.floor() as i32;
        let max_x = aabb.max.x.ceil() as i32;
        let min_y = aabb.max.y.floor() as i32;
        let max_y = (aabb.max.y - self.last_yd.load()).ceil() as i32;
        let min_z = aabb.min.z.floor() as i32;
        let max_z = aabb.max.z.ceil() as i32;

        'outer: for y in min_y..max_y {
            let mut block_height = 0.0f32;

            for x in min_x..max_x {
                for z in min_z..max_z {
                    let pos = BlockPos(Vector3::new(x, y, z));
                    let (fluid, fluid_state) = world.get_fluid_and_fluid_state(&pos);
                    if fluid.matches_type(&Fluid::WATER) {
                        block_height =
                            block_height.max(world.get_fluid_height(&pos, fluid, &fluid_state));
                    }

                    if block_height >= 1.0 {
                        continue 'outer;
                    }
                }
            }

            if block_height < 1.0 {
                return y as f32 + block_height;
            }
        }

        (max_y + 1) as f32
    }

    /// Vanilla `AbstractBoat.getGroundFriction`.
    fn get_ground_friction(&self) -> f32 {
        let world = self.vehicle.entity.world.load();
        let bb = self.vehicle.entity.bounding_box.load();
        let box_min_y = bb.min.y - 0.001;
        let x0 = bb.min.x.floor() as i32 - 1;
        let x1 = bb.max.x.ceil() as i32 + 1;
        let y0 = box_min_y.floor() as i32 - 1;
        let y1 = bb.min.y.ceil() as i32 + 1;
        let z0 = bb.min.z.floor() as i32 - 1;
        let z1 = bb.max.z.ceil() as i32 + 1;
        let boat_box = pumpkin_util::math::boundingbox::BoundingBox::new(
            Vector3::new(bb.min.x, box_min_y, bb.min.z),
            Vector3::new(bb.max.x, bb.min.y, bb.max.z),
        );
        let mut friction = 0.0f32;
        let mut count = 0;

        for x in x0..x1 {
            for z in z0..z1 {
                let edges = i32::from(x == x0 || x == x1 - 1) + i32::from(z == z0 || z == z1 - 1);
                if edges != 2 {
                    for y in y0..y1 {
                        if edges <= 0 || (y != y0 && y != y1 - 1) {
                            let pos = BlockPos(Vector3::new(x, y, z));
                            let state = world.get_block_state(&pos);
                            let block = Block::from_state_id(state.id);
                            if block == &Block::LILY_PAD {
                                continue;
                            }
                            if state
                                .get_block_collision_shapes_at(&pos)
                                .any(|shape| shapes_join_is_not_empty(shape, boat_box))
                            {
                                friction += block.slipperiness;
                                count += 1;
                            }
                        }
                    }
                }
            }
        }

        if count == 0 {
            0.0
        } else {
            friction / count as f32
        }
    }

    /// Vanilla `AbstractBoat.getStatus`.
    fn get_status(&self) -> BoatStatus {
        if let Some(water_status) = self.is_underwater() {
            let bb = self.vehicle.entity.bounding_box.load();
            self.water_level.store(bb.max.y);
            return water_status;
        }

        if self.check_in_water() {
            return BoatStatus::InWater;
        }

        let friction = self.get_ground_friction();
        if friction > 0.0 {
            self.land_friction.store(friction);
            BoatStatus::OnLand
        } else {
            BoatStatus::InAir
        }
    }

    /// Vanilla `AbstractBoat.floatBoat`.
    fn float_boat(&self) {
        let entity = &self.vehicle.entity;
        let mut vspeed = -DEFAULT_GRAVITY;
        let mut buoyancy = 0.0f64;

        let status = self.status.load();
        let old_status = self.old_status.load();

        if old_status == BoatStatus::InAir
            && status != BoatStatus::InAir
            && status != BoatStatus::OnLand
        {
            let bb = entity.bounding_box.load();
            self.water_level.store(bb.max.y);
            let target_y = f64::from(self.get_water_level_above())
                - f64::from(entity.entity_dimension.load().height)
                + 0.101;
            let delta = Vector3::new(0.0, target_y - bb.min.y, 0.0);
            let test_box = bb.offset(pumpkin_util::math::boundingbox::BoundingBox::new(
                delta, delta,
            ));
            let world = entity.world.load();
            if world.is_space_empty(test_box) {
                entity.set_pos(Vector3::new(
                    entity.pos.load().x,
                    target_y,
                    entity.pos.load().z,
                ));
                let movement = entity.velocity.load();
                entity
                    .velocity
                    .store(Vector3::new(movement.x, 0.0, movement.z));
                self.last_yd.store(0.0);
            }

            self.status.store(BoatStatus::InWater);
        } else {
            let inv_friction = match status {
                BoatStatus::InWater => {
                    let bb = entity.bounding_box.load();
                    buoyancy = (self.water_level.load() - bb.min.y)
                        / f64::from(entity.entity_dimension.load().height);
                    0.9
                }
                BoatStatus::UnderFlowingWater => {
                    vspeed = -7.0E-4;
                    0.9
                }
                BoatStatus::UnderWater => {
                    buoyancy = 0.01;
                    0.45
                }
                BoatStatus::InAir => 0.9,
                BoatStatus::OnLand => {
                    let friction = self.land_friction.load();
                    if self.has_controlling_passenger() {
                        self.land_friction.store(friction / 2.0);
                    }
                    friction
                }
            };

            let movement = entity.velocity.load();
            entity.velocity.store(Vector3::new(
                movement.x * f64::from(inv_friction),
                movement.y + vspeed,
                movement.z * f64::from(inv_friction),
            ));

            if buoyancy > 0.0 {
                let movement = entity.velocity.load();
                entity.velocity.store(Vector3::new(
                    movement.x,
                    (movement.y + buoyancy * (DEFAULT_GRAVITY / 0.65)) * 0.75,
                    movement.z,
                ));
            }
        }
    }

    fn eject_passengers(&self) {
        let ids: Vec<i32> = self
            .vehicle
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|passenger| passenger.get_entity().entity_id)
            .collect();
        for id in ids {
            self.vehicle.entity.remove_passenger(id);
        }
    }

    /// Vanilla `AbstractBoat.getPaddleSound`.
    fn get_paddle_sound(&self) -> Option<Sound> {
        match self.status.load() {
            BoatStatus::InWater | BoatStatus::UnderWater | BoatStatus::UnderFlowingWater => {
                Some(Sound::EntityBoatPaddleWater)
            }
            BoatStatus::OnLand => Some(Sound::EntityBoatPaddleLand),
            BoatStatus::InAir => None,
        }
    }
}

/// Vanilla `Shapes.joinIsNotEmpty(blockShape, boatShape, BooleanOp.AND)` — true when
/// the two axis-aligned boxes overlap.
fn shapes_join_is_not_empty(
    block_shape: pumpkin_util::math::boundingbox::BoundingBox,
    boat_shape: pumpkin_util::math::boundingbox::BoundingBox,
) -> bool {
    block_shape.intersects(&boat_shape)
}

impl EntityBase for BoatEntity {
    fn get_entity(&self) -> &Entity {
        &self.vehicle.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn get_gravity(&self) -> f64 {
        DEFAULT_GRAVITY
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.vehicle.tick();
        // Vanilla `Entity.tick` base bookkeeping (position history, portals, fire).
        self.vehicle.entity.tick(caller, server);

        self.old_status.store(self.status.load());
        self.status.store(self.get_status());

        let status = self.status.load();
        if status != BoatStatus::UnderWater && status != BoatStatus::UnderFlowingWater {
            self.out_of_control_ticks.store(0.0);
        } else {
            self.out_of_control_ticks
                .store(self.out_of_control_ticks.load() + 1.0);
        }

        if self.out_of_control_ticks.load() >= TIME_TO_EJECT {
            self.eject_passengers();
        }

        if !self.has_controlling_passenger() {
            self.set_paddles(false, false);
        }

        self.float_boat();
        self.vehicle
            .entity
            .move_entity(caller, self.vehicle.entity.velocity.load());
        // Vanilla `AbstractBoat.checkFallDamage` stores the vertical delta here; Pumpkin
        // has no per-move fall hook, so approximate it with the post-move Y velocity.
        self.last_yd.store(self.vehicle.entity.velocity.load().y);

        // Vanilla ticks paddle sounds/rotations after movement.
        for i in 0..=1 {
            if self.get_paddle_state(i) {
                let paddle_pos = if i == 0 {
                    self.left_paddle_pos.load()
                } else {
                    self.right_paddle_pos.load()
                };
                if paddle_pos % (std::f32::consts::PI * 2.0) <= PADDLE_SOUND_TIME
                    && (paddle_pos + PADDLE_SPEED) % (std::f32::consts::PI * 2.0)
                        >= PADDLE_SOUND_TIME
                    && let Some(sound) = self.get_paddle_sound()
                {
                    let entity = &self.vehicle.entity;
                    let view_vector =
                        Vector3::from_yaw_pitch(entity.yaw.load(), entity.pitch.load());
                    let dx = if i == 1 {
                        -view_vector.z
                    } else {
                        view_vector.z
                    };
                    let dz = if i == 1 {
                        view_vector.x
                    } else {
                        -view_vector.x
                    };
                    let pos = entity.pos.load();
                    entity.world.load().play_sound(
                        sound,
                        SoundCategory::Neutral,
                        &Vector3::new(pos.x + dx, pos.y, pos.z + dz),
                    );
                }

                let new_pos = paddle_pos + PADDLE_SPEED;
                if i == 0 {
                    self.left_paddle_pos.store(new_pos);
                } else {
                    self.right_paddle_pos.store(new_pos);
                }
            } else if i == 0 {
                self.left_paddle_pos.store(0.0);
            } else {
                self.right_paddle_pos.store(0.0);
            }
        }
    }

    fn init_data_tracker(&self) {
        self.send_wobble_metadata();
    }

    fn can_hit(&self) -> bool {
        self.vehicle.entity.is_alive()
    }

    fn is_collidable(&self, _entity: Option<Box<dyn EntityBase>>) -> bool {
        true
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        self.vehicle.damage_with_context(amount, source)
    }

    fn interact(&self, player: &Arc<Player>, _item_stack: &mut ItemStack) -> bool {
        if player.get_entity().is_sneaking() {
            return false;
        }

        if self.out_of_control_ticks.load() >= TIME_TO_EJECT {
            return false;
        }

        if self
            .vehicle
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
            >= 2
        {
            return false;
        }

        if player.get_entity().has_vehicle() {
            return false;
        }

        let world = self.vehicle.entity.world.load();
        let Some(vehicle) = world.get_entity_by_id(self.vehicle.entity.entity_id) else {
            return false;
        };

        let Some(passenger) = world.get_player_by_id(player.entity_id()) else {
            return false;
        };

        self.vehicle
            .entity
            .add_passenger(vehicle, passenger as Arc<dyn EntityBase>);

        true
    }

    fn set_paddle_state(&self, left: bool, right: bool) {
        self.set_paddles(left, right);
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushable(&self) -> bool {
        true
    }
}

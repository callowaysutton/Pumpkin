use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam::atomic::AtomicCell;

use pumpkin_data::Block;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::translation;

use crate::entity::player::Player;
use crate::entity::vehicle::container::{self, VehicleInventory};
use crate::entity::vehicle::vehicle::VehicleEntity;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::item::items::boat::BoatItem;
use crate::server::Server;
use crate::world::World;

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::GameMode;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;

/// Vanilla `AbstractBoat.TIME_TO_EJECT`: underwater ticks before the passengers are
/// ejected.
const TIME_TO_EJECT: f32 = 60.0;

/// Vanilla `AbstractBoat.PADDLE_SPEED`, the per-stroke paddle spot advance.
const PADDLE_SPEED: f32 = std::f32::consts::PI / 8.0;
/// The paddle sound plays when the stroke crosses `PADDLE_SOUND_TIME` in vanilla.
const PADDLE_CHECK: f32 = std::f32::consts::PI / 4.0;

/// Vanilla `AbstractBoat.getDefaultGravity`.
const GRAVITY: f64 = 0.04;

/// Vanilla `AbstractBoat.Status`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Status {
    InWater,
    UnderWater,
    UnderFlowingWater,
    OnLand,
    InAir,
}

pub struct BoatEntity {
    pub vehicle: VehicleEntity,
    /// Vanilla `AbstractChestBoat.itemStacks`: only the chest boat types carry one.
    chest_inventory: Option<Arc<VehicleInventory>>,
    /// Vanilla `AbstractBoat.dropItem`: the boat item that drops when destroyed.
    drop_item: &'static Item,
    left_paddle_moving: AtomicBool,
    right_paddle_moving: AtomicBool,
    old_status: AtomicCell<Status>,
    status: AtomicCell<Status>,
    out_of_control_ticks: AtomicCell<f32>,
    delta_rotation: AtomicCell<f32>,
    water_level: AtomicCell<f64>,
    land_friction: AtomicCell<f32>,
    /// Vanilla `AbstractBoat.lastYd`, written inside `checkFallDamage` during `move`.
    last_yd: AtomicCell<f64>,
    paddle_positions: [AtomicCell<f32>; 2],
}

/// Vanilla picks the container variants structurally (`instanceof AbstractChestBoat`;
/// there is no entity type tag for them).
const fn is_chest_boat(id: u16) -> bool {
    id == EntityType::OAK_CHEST_BOAT.id
        || id == EntityType::SPRUCE_CHEST_BOAT.id
        || id == EntityType::BIRCH_CHEST_BOAT.id
        || id == EntityType::JUNGLE_CHEST_BOAT.id
        || id == EntityType::ACACIA_CHEST_BOAT.id
        || id == EntityType::DARK_OAK_CHEST_BOAT.id
        || id == EntityType::MANGROVE_CHEST_BOAT.id
        || id == EntityType::CHERRY_CHEST_BOAT.id
        || id == EntityType::PALE_OAK_CHEST_BOAT.id
        || id == EntityType::POPLAR_CHEST_BOAT.id
        || id == EntityType::BAMBOO_CHEST_RAFT.id
}

const fn is_boat_type(id: u16) -> bool {
    id == EntityType::OAK_BOAT.id
        || id == EntityType::OAK_CHEST_BOAT.id
        || id == EntityType::SPRUCE_BOAT.id
        || id == EntityType::SPRUCE_CHEST_BOAT.id
        || id == EntityType::BIRCH_BOAT.id
        || id == EntityType::BIRCH_CHEST_BOAT.id
        || id == EntityType::JUNGLE_BOAT.id
        || id == EntityType::JUNGLE_CHEST_BOAT.id
        || id == EntityType::ACACIA_BOAT.id
        || id == EntityType::ACACIA_CHEST_BOAT.id
        || id == EntityType::DARK_OAK_BOAT.id
        || id == EntityType::DARK_OAK_CHEST_BOAT.id
        || id == EntityType::MANGROVE_BOAT.id
        || id == EntityType::MANGROVE_CHEST_BOAT.id
        || id == EntityType::CHERRY_BOAT.id
        || id == EntityType::CHERRY_CHEST_BOAT.id
        || id == EntityType::PALE_OAK_BOAT.id
        || id == EntityType::PALE_OAK_CHEST_BOAT.id
        || id == EntityType::POPLAR_BOAT.id
        || id == EntityType::POPLAR_CHEST_BOAT.id
        || id == EntityType::BAMBOO_RAFT.id
        || id == EntityType::BAMBOO_CHEST_RAFT.id
}

impl BoatEntity {
    pub fn new(entity: Entity) -> Self {
        let chest_inventory =
            is_chest_boat(entity.entity_type.id).then(|| Arc::new(VehicleInventory::new(27)));
        let drop_item = BoatItem::entity_to_item(entity.entity_type);
        Self {
            vehicle: VehicleEntity::new(entity),
            chest_inventory,
            drop_item,
            left_paddle_moving: AtomicBool::new(false),
            right_paddle_moving: AtomicBool::new(false),
            old_status: AtomicCell::new(Status::InAir),
            status: AtomicCell::new(Status::InAir),
            out_of_control_ticks: AtomicCell::new(0.0),
            delta_rotation: AtomicCell::new(0.0),
            water_level: AtomicCell::new(0.0),
            land_friction: AtomicCell::new(0.0),
            last_yd: AtomicCell::new(0.0),
            paddle_positions: [AtomicCell::new(0.0), AtomicCell::new(0.0)],
        }
    }

    pub fn set_paddles(&self, left: bool, right: bool) {
        let left_changed = self.left_paddle_moving.swap(left, Ordering::Relaxed) != left;
        let right_changed = self.right_paddle_moving.swap(right, Ordering::Relaxed) != right;

        if left_changed || right_changed {
            self.vehicle.entity.send_meta_data(
                &[
                    Metadata::new(pumpkin_data::tracked_data::boat::ID_PADDLE_LEFT, left),
                    Metadata::new(pumpkin_data::tracked_data::boat::ID_PADDLE_RIGHT, right),
                ],
                None,
            );
        }
    }

    fn send_wobble_metadata(&self) {
        self.vehicle.send_wobble_metadata();
    }

    /// Vanilla `AbstractBoat.getControllingPassenger`: the first passenger when it is a
    /// `Player`, otherwise the (empty) `Entity` default. `Entity.isClientAuthoritative`
    /// is true for every player, so the boat is server-driven exactly when this is
    /// `None`.
    fn get_controlling_player(&self) -> Option<Arc<dyn EntityBase>> {
        let passengers = self
            .vehicle
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        passengers
            .first()
            .filter(|passenger| passenger.get_player().is_some())
            .cloned()
    }

    /// Vanilla `AbstractBoat.getMaxPassengers`: two for boats, one for chest boats.
    const fn max_passengers(&self) -> usize {
        if self.chest_inventory.is_some() { 1 } else { 2 }
    }

    /// Vanilla `AbstractBoat.canAddPassenger`.
    fn can_add_passenger(&self) -> bool {
        let passenger_count = self
            .vehicle
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        passenger_count < self.max_passengers() && !self.vehicle.entity.is_submerged_in_water()
    }

    fn get_status(&self) -> Status {
        let entity = &self.vehicle.entity;
        let bbox = entity.bounding_box.load();
        let world = entity.world.load();

        if let Some(status) = Self::is_underwater(&world, &bbox) {
            self.water_level.store(bbox.max.y);
            return status;
        }

        if self.check_in_water(&world, &bbox) {
            return Status::InWater;
        }

        let friction = Self::get_ground_friction(&world, &bbox);
        if friction > 0.0 {
            self.land_friction.store(friction);
            return Status::OnLand;
        }

        Status::InAir
    }

    /// Vanilla `AbstractBoat.isUnderwater`, returning None where the Java method returns
    /// null.
    fn is_underwater(world: &Arc<World>, bbox: &BoundingBox) -> Option<Status> {
        let max_y = bbox.max.y + 0.001;
        let min_x = bbox.min.x.floor() as i32;
        let max_x = bbox.max.x.ceil() as i32;
        let min_y = bbox.max.y.floor() as i32;
        let max_y_bound = max_y.ceil() as i32;
        let min_z = bbox.min.z.floor() as i32;
        let max_z = bbox.max.z.ceil() as i32;

        let mut under_water = false;
        for x in min_x..max_x {
            for y in min_y..max_y_bound {
                for z in min_z..max_z {
                    let block_pos = BlockPos(Vector3::new(x, y, z));
                    let (fluid, state) = world.get_fluid_and_fluid_state(&block_pos);
                    if fluid.has_tag(&tag::Fluid::MINECRAFT_WATER)
                        && max_y < f64::from(block_pos.0.y) + f64::from(state.height)
                    {
                        if !state.is_source {
                            return Some(Status::UnderFlowingWater);
                        }
                        under_water = true;
                    }
                }
            }
        }
        under_water.then_some(Status::UnderWater)
    }

    /// Vanilla `AbstractBoat.checkInWater`, including its `waterLevel` side effect.
    fn check_in_water(&self, world: &Arc<World>, bbox: &BoundingBox) -> bool {
        let min_x = bbox.min.x.floor() as i32;
        let max_x = bbox.max.x.ceil() as i32;
        let min_y = bbox.min.y.floor() as i32;
        let max_y_bound = (bbox.min.y + 0.001).ceil() as i32;
        let min_z = bbox.min.z.floor() as i32;
        let max_z = bbox.max.z.ceil() as i32;

        let mut in_water = false;
        let mut water_level = f64::NEG_INFINITY;
        for x in min_x..max_x {
            for y in min_y..max_y_bound {
                for z in min_z..max_z {
                    let block_pos = BlockPos(Vector3::new(x, y, z));
                    let (fluid, state) = world.get_fluid_and_fluid_state(&block_pos);
                    if fluid.has_tag(&tag::Fluid::MINECRAFT_WATER) {
                        let height = f64::from(block_pos.0.y) + f64::from(state.height);
                        water_level = water_level.max(height);
                        in_water |= bbox.min.y < height;
                    }
                }
            }
        }
        self.water_level.store(water_level);
        in_water
    }

    /// Vanilla `AbstractBoat.getGroundFriction`: average friction of the blocks whose
    /// collision shape intersects the slab under the boat, ignoring lily pads.
    fn get_ground_friction(world: &Arc<World>, bbox: &BoundingBox) -> f32 {
        let friction_box = BoundingBox::new(
            Vector3::new(bbox.min.x, bbox.min.y - 0.001, bbox.min.z),
            Vector3::new(bbox.max.x, bbox.min.y, bbox.max.z),
        );

        let x0 = friction_box.min.x.floor() as i32 - 1;
        let x1 = friction_box.max.x.ceil() as i32 + 1;
        let y0 = friction_box.min.y.floor() as i32 - 1;
        let y1 = friction_box.max.y.ceil() as i32 + 1;
        let z0 = friction_box.min.z.floor() as i32 - 1;
        let z1 = friction_box.max.z.ceil() as i32 + 1;

        let mut friction = 0.0f32;
        let mut count = 0;

        for x in x0..x1 {
            let x_edge = usize::from(x != x0 && x != x1 - 1);
            for z in z0..z1 {
                let edges = x_edge + usize::from(z != z0 && z != z1 - 1);
                if edges == 2 {
                    continue;
                }

                for y in y0..y1 {
                    if edges == 0 || (y != y0 && y != y1 - 1) {
                        let block_pos = BlockPos(Vector3::new(x, y, z));
                        let state = world.get_block_state(&block_pos);
                        if state.is_air() {
                            continue;
                        }

                        let block = Block::from_state_id(state.id);
                        if block == &Block::LILY_PAD {
                            continue;
                        }

                        for shape in state.get_block_collision_shapes_at(&block_pos) {
                            if shape.at_pos(block_pos).intersects(&friction_box) {
                                friction += block.slipperiness;
                                count += 1;
                                break;
                            }
                        }
                    }
                }
            }
        }

        friction / count as f32
    }

    /// Vanilla `AbstractBoat.getWaterLevelAbove`.
    fn get_water_level_above(&self, world: &Arc<World>) -> f32 {
        let bbox = self.vehicle.entity.bounding_box.load();
        let min_x = bbox.min.x.floor() as i32;
        let max_x = bbox.max.x.ceil() as i32;
        let min_y = bbox.max.y.floor() as i32;
        let max_y = (bbox.max.y - self.last_yd.load()).ceil() as i32;
        let min_z = bbox.min.z.floor() as i32;
        let max_z = bbox.max.z.ceil() as i32;

        for y in min_y..max_y {
            let mut block_height = 0.0f32;
            for x in min_x..max_x {
                for z in min_z..max_z {
                    let block_pos = BlockPos(Vector3::new(x, y, z));
                    let (fluid, state) = world.get_fluid_and_fluid_state(&block_pos);
                    if fluid.has_tag(&tag::Fluid::MINECRAFT_WATER) {
                        block_height = block_height.max(state.height);
                    }
                    if block_height >= 1.0 {
                        break;
                    }
                }
            }

            if block_height < 1.0 {
                return y as f32 + block_height;
            }
        }

        max_y as f32 + 1.0
    }

    /// Vanilla `AbstractBoat.floatBoat`.
    fn float_boat(&self) {
        let mut vspeed = -self.get_gravity();
        let mut buoyancy = 0.0;
        let inv_friction;

        let entity = &self.vehicle.entity;
        let old_status = self.old_status.load();
        let status = self.status.load();
        let pos = entity.pos.load();
        let bbox = entity.bounding_box.load();
        let bb_height = bbox.max.y - bbox.min.y;

        if old_status == Status::InAir && status != Status::InAir && status != Status::OnLand {
            // Falling into water: catch the boat on the water surface.
            self.water_level.store(pos.y + bb_height);
            let target_y =
                f64::from(self.get_water_level_above(&entity.world.load())) - bb_height + 0.101;
            let moved_box = bbox.shift(Vector3::new(0.0, target_y - pos.y, 0.0));
            if entity.world.load().is_space_empty(moved_box) {
                entity.set_pos(Vector3::new(pos.x, target_y, pos.z));
                let velocity = entity.velocity.load();
                entity
                    .velocity
                    .store(Vector3::new(velocity.x, 0.0, velocity.z));
                self.last_yd.store(0.0);
            }

            self.status.store(Status::InWater);
        } else {
            match status {
                Status::InWater => {
                    buoyancy = (self.water_level.load() - pos.y) / bb_height;
                    inv_friction = 0.9;
                }
                Status::UnderFlowingWater => {
                    vspeed = -7.0e-4;
                    inv_friction = 0.9;
                }
                Status::UnderWater => {
                    buoyancy = 0.01;
                    inv_friction = 0.45;
                }
                Status::InAir => {
                    inv_friction = 0.9;
                }
                Status::OnLand => {
                    inv_friction = self.land_friction.load();
                    if self.get_controlling_player().is_some() {
                        self.land_friction.store(inv_friction / 2.0);
                    }
                }
            }

            let velocity = entity.velocity.load();
            entity.velocity.store(Vector3::new(
                velocity.x * f64::from(inv_friction),
                velocity.y + vspeed,
                velocity.z * f64::from(inv_friction),
            ));
            self.delta_rotation
                .store(self.delta_rotation.load() * inv_friction);

            if buoyancy > 0.0 {
                let velocity = entity.velocity.load();
                entity.velocity.store(Vector3::new(
                    velocity.x,
                    (velocity.y + buoyancy * (self.get_gravity() / 0.65)) * 0.75,
                    velocity.z,
                ));
            }
        }
    }

    /// Vanilla `AbstractBoat.getPaddleSound`.
    fn get_paddle_sound(&self) -> Option<Sound> {
        match self.status.load() {
            Status::InWater | Status::UnderWater | Status::UnderFlowingWater => {
                Some(Sound::EntityBoatPaddleWater)
            }
            Status::OnLand => Some(Sound::EntityBoatPaddleLand),
            Status::InAir => None,
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

    /// Vanilla `AbstractBoat.hasEnoughSpaceFor`.
    fn has_enough_space_for(&self, other: &dyn EntityBase) -> bool {
        let self_bb = self.vehicle.entity.bounding_box.load();
        let other_bb = other.get_entity().bounding_box.load();
        other_bb.max.x - other_bb.min.x < self_bb.max.x - self_bb.min.x
    }

    /// Vanilla `AbstractBoat.interactWithContainerVehicle` through
    /// `AbstractChestBoat.interact`.
    fn interact_with_container_vehicle(&self, player: &Arc<Player>) -> bool {
        let Some(inventory) = &self.chest_inventory else {
            return false;
        };

        let custom_name = self.vehicle.entity.custom_name.load().as_ref().clone();
        let opened = container::open(
            custom_name,
            player,
            inventory,
            TextComponent::translate_cross(
                translation::java::ENTITY_MINECRAFT_CHEST_BOAT,
                translation::bedrock::ENTITY_CHEST_BOAT_NAME,
                [],
            ),
            false,
        );
        if opened {
            let world = self.vehicle.entity.world.load();
            world.emit_game_event(
                GameEvent::ContainerOpen.name(),
                self.vehicle.entity.pos.load(),
            );
        }
        opened
    }
}

impl EntityBase for BoatEntity {
    fn get_entity(&self) -> &Entity {
        &self.vehicle.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    /// Vanilla `AbstractBoat.getDefaultGravity` through `Entity.getGravity`.
    fn get_gravity(&self) -> f64 {
        if self.vehicle.entity.has_no_gravity() {
            0.0
        } else {
            GRAVITY
        }
    }

    /// Vanilla `AbstractBoat.tick`. It is longer than 100 lines to keep the port
    /// readable next to the Java method.
    #[allow(clippy::too_many_lines)]
    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.vehicle.tick();

        let world = self.vehicle.entity.world.load();

        // Vanilla `AbstractBoat.tick`: status update, out-of-control passenger ejection.
        self.old_status.store(self.status.load());
        let status = self.get_status();
        self.status.store(status);

        let out_of_control = if status == Status::UnderWater || status == Status::UnderFlowingWater
        {
            self.out_of_control_ticks.load() + 1.0
        } else {
            0.0
        };
        self.out_of_control_ticks.store(out_of_control);
        if out_of_control >= TIME_TO_EJECT {
            self.eject_passengers();
        }

        if self.get_controlling_player().is_none() {
            // Vanilla `AbstractBoat.tick` server-authoritative path: paddles off,
            // float, move. The player-driven path is client authoritative and syncs
            // through the MoveVehicle packet.
            self.set_paddles(false, false);
            self.float_boat();
            // Vanilla `checkFallDamage`: `lastYd` takes the pre-move vertical motion.
            self.last_yd.store(self.vehicle.entity.velocity.load().y);
            let motion = self.vehicle.entity.velocity.load();
            self.move_entity(caller, motion);

            // Keep passenger positions in step with the moved boat while the boat is
            // server-driven (same pattern as the minecart).
            let new_pos = self.vehicle.entity.pos.load();
            if let Ok(passengers) = self.vehicle.entity.passengers.try_lock() {
                for passenger in passengers.iter() {
                    passenger.get_entity().set_pos(new_pos);
                }
            }
        } else {
            self.vehicle
                .entity
                .velocity
                .store(Vector3::new(0.0, 0.0, 0.0));
            self.delta_rotation.store(0.0);
        }

        // Vanilla paddle positions and sounds. The server plays the sounds; paddle
        // acceleration (`controlBoat`) only runs on the controlling player's client.
        for i in 0..2 {
            let paddle_moving = if i == 0 {
                self.left_paddle_moving.load(Ordering::Relaxed)
            } else {
                self.right_paddle_moving.load(Ordering::Relaxed)
            };

            if paddle_moving && self.get_controlling_player().is_some() {
                if !self.vehicle.entity.is_silent() {
                    let paddle_pos = self.paddle_positions[i].load();
                    if paddle_pos % (std::f32::consts::PI * 2.0) <= PADDLE_CHECK
                        && (paddle_pos + PADDLE_SPEED) % (std::f32::consts::PI * 2.0)
                            >= PADDLE_CHECK
                        && let Some(sound) = self.get_paddle_sound()
                    {
                        let entity = &self.vehicle.entity;
                        let view = Vector3::rotation_vector(
                            f64::from(entity.pitch.load()),
                            f64::from(entity.yaw.load()),
                        );
                        let (dx, dz) = if i == 1 {
                            (-view.z, view.x)
                        } else {
                            (view.z, -view.x)
                        };
                        let pos = entity.pos.load();
                        world.play_sound_fine(
                            sound,
                            SoundCategory::Neutral,
                            &Vector3::new(pos.x + dx, pos.y, pos.z + dz),
                            1.0,
                            0.8 + 0.4 * rand::random::<f32>(),
                        );
                    }
                }
                self.paddle_positions[i].store(self.paddle_positions[i].load() + PADDLE_SPEED);
            } else {
                self.paddle_positions[i].store(0.0);
            }
        }

        // Vanilla entity scan: entities in the inflated box are pushed, or picked up
        // as passengers by unattended boats.
        let inflated = self
            .vehicle
            .entity
            .bounding_box
            .load()
            .expand(0.2, -0.01, 0.2);
        let entities = world.get_entities_at_box(&inflated);
        if !entities.is_empty() {
            // Vanilla `EntitySelector.pushableBy(this)`.
            let add_new_passengers = self.get_controlling_player().is_none();
            let self_id = self.vehicle.entity.entity_id;
            for entity in entities {
                if !entity.is_pushable()
                    || entity.is_spectator()
                    || entity.get_entity().entity_id == self_id
                    || entity.has_passenger(self)
                    || self
                        .vehicle
                        .entity
                        .has_passenger(entity.get_entity().entity_id)
                {
                    continue;
                }

                if add_new_passengers
                    && self.can_add_passenger()
                    && !entity.is_passenger()
                    && self.has_enough_space_for(&*entity)
                    && entity.get_living_entity().is_some()
                    && !entity
                        .get_entity()
                        .entity_type
                        .has_tag(&tag::EntityType::MINECRAFT_CANNOT_BE_PUSHED_ONTO_BOATS)
                {
                    let Some(vehicle) = world.get_entity_by_id(self_id) else {
                        continue;
                    };
                    entity.get_entity().add_passenger(vehicle, entity.clone());
                } else {
                    self.push(&*entity);
                }
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

    /// Vanilla `AbstractBoat.push`: boats only displace each other when their boxes
    /// overlap vertically; everything else only when it sits at or below the boat.
    fn push(&self, entity: &dyn EntityBase) {
        let self_entity = self.get_entity();
        let other_entity = entity.get_entity();

        if self_entity.no_physics.load(Ordering::Relaxed)
            || other_entity.no_physics.load(Ordering::Relaxed)
        {
            return;
        }

        let self_bb = self_entity.bounding_box.load();
        let other_bb = other_entity.bounding_box.load();
        let can_push = if is_boat_type(other_entity.entity_type.id) {
            other_bb.min.y < self_bb.max.y
        } else {
            other_bb.min.y <= self_bb.min.y
        };
        if !can_push
            || self_entity.has_passenger(other_entity.entity_id)
            || other_entity.has_passenger(self_entity.entity_id)
        {
            return;
        }

        let mut dx = other_entity.pos.load().x - self_entity.pos.load().x;
        let mut dz = other_entity.pos.load().z - self_entity.pos.load().z;
        let mut d = dx.abs().max(dz.abs());
        if d < 0.01 {
            return;
        }
        d = d.sqrt();
        dx /= d;
        dz /= d;
        let mut pow = 1.0 / d;
        if pow > 1.0 {
            pow = 1.0;
        }
        dx *= pow * 0.05;
        dz *= pow * 0.05;

        if !self_entity.has_passengers() && self.is_pushable() {
            let mut vel = self_entity.velocity.load();
            vel.x -= dx;
            vel.z -= dz;
            self_entity.velocity.store(vel);
            self_entity.velocity_dirty.store(true, Ordering::SeqCst);
        }
        if !other_entity.has_passengers() && entity.is_pushable() {
            let mut vel = other_entity.velocity.load();
            vel.x += dx;
            vel.z += dz;
            other_entity.velocity.store(vel);
            other_entity.velocity_dirty.store(true, Ordering::SeqCst);
        }
    }

    fn is_pushable(&self) -> bool {
        true
    }

    /// Vanilla `AbstractBoat.interact` with `AbstractChestBoat.interact` layered on:
    /// a normal click mounts, a sneak click opens a chest boat's container.
    fn interact(&self, player: &Arc<Player>, _item_stack: &mut ItemStack) -> bool {
        let secondary_use_active = player.get_entity().is_sneaking();
        let out_of_control_expired = self.out_of_control_ticks.load() >= TIME_TO_EJECT;

        if !secondary_use_active && !out_of_control_expired {
            if self.can_add_passenger() && !player.get_entity().has_vehicle() {
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
                return true;
            }

            // Vanilla: a failed ride attempt returns PASS even for chest boats
            // unless the player was sneaking.
            return false;
        }

        if self.can_add_passenger() && !secondary_use_active {
            return false;
        }

        self.interact_with_container_vehicle(player)
    }

    fn set_paddle_state(&self, left: bool, right: bool) {
        self.set_paddles(left, right);
    }
    fn cast_any(&self) -> &dyn std::any::Any {
        self
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
        let creative = source
            .and_then(EntityBase::get_player)
            .is_some_and(|player| player.gamemode.load() == GameMode::Creative);
        let will_break = self.vehicle.entity.is_alive()
            && (creative || self.vehicle.get_damage() + amount * 10.0 > 40.0);

        let damaged = self.vehicle.damage_with_context(amount, source);

        // Vanilla `AbstractChestBoat.destroy`: the boat item keeps its custom name and
        // the container contents scatter. Creative players `discard` everything.
        if will_break && !creative && self.vehicle.entity.is_removed() {
            let world = self.vehicle.entity.world.load();
            if world.level_info.load().game_rules.entity_drops {
                let position = self.vehicle.entity.block_pos.load();
                if let Some(inventory) = &self.chest_inventory
                    && inventory.claim_drops()
                {
                    inventory.unpack_loot();
                    let inventory: Arc<dyn pumpkin_inventory::Inventory> = inventory.clone();
                    world.scatter_inventory(&position, &inventory);
                }

                let mut stack = ItemStack::new(1, self.drop_item);
                if let Some(custom_name) = self.vehicle.entity.custom_name.load().as_ref().clone() {
                    // Vanilla `VehicleEntity.destroy`: the dropped boat item keeps the
                    // custom name.
                    stack.set_custom_name(custom_name.get_text());
                }
                world.drop_stack(&position, stack);
            }
        }

        damaged
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(inventory) = &self.chest_inventory {
            inventory.write_nbt(nbt);
        }
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let Some(inventory) = &self.chest_inventory {
            inventory.read_nbt(nbt);
        }
    }
}

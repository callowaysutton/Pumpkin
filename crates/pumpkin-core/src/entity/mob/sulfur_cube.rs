use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ageable::{AgeableData, AgeableMob};
use crate::entity::ai::control::{Control, MoveControlTrait};
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::custom_sound::CustomSound;
use crate::entity::{
    Entity, EntityBase,
    mob::{Mob, MobEntity},
};
use crate::world::World;

/// Vanilla `SulfurCube.SPLIT_COUNT`.
pub const SPLIT_COUNT: i32 = 2;
/// Vanilla `SulfurCube.MAX_SIZE`.
pub const MAX_SIZE: i32 = 2;
/// Vanilla `SulfurCube.MIN_SIZE`.
pub const MIN_SIZE: i32 = 1;
/// Vanilla `SulfurCube.PICKUP_TIMER_DURATION`.
pub const PICKUP_TIMER_DURATION: i32 = 100;

/// Represents a Sulfur Cube, a cube-shaped mob that swallows items and can be tempted
/// with its food while still a baby.
///
/// Wiki: <https://minecraft.wiki/w/Sulfur_Cube>
pub struct SulfurCubeEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    /// Vanilla `SulfurCube.pickupTimer`: blocks item pickup for a while after shearing.
    pub pickup_timer: AtomicI32,
    size: AtomicI32,
    /// Vanilla `CubeMobMoveControl.direction`.
    move_direction: AtomicCell<f32>,
    /// Vanilla `CubeMobMoveControl.wantedMovement`.
    wanted_movement: AtomicCell<f32>,
    /// Vanilla `CubeMobMoveControl.isAggressive`.
    is_aggressive: std::sync::atomic::AtomicBool,
}

impl SulfurCubeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let sulfur_cube = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            pickup_timer: AtomicI32::new(0),
            size: AtomicI32::new(MAX_SIZE),
            move_direction: AtomicCell::new(0.0),
            wanted_movement: AtomicCell::new(0.0),
            is_aggressive: std::sync::atomic::AtomicBool::new(false),
        };
        let mob_arc = Arc::new(sulfur_cube);

        {
            let mut move_control = mob_arc
                .mob_entity
                .move_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *move_control = Box::new(SulfurCubeMobMoveControl::new(Arc::downgrade(&mob_arc)));

            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // `AbstractCubeMob.registerGoals` order, with `SulfurCube.addBehaviourGoals` in between.
            goal_selector.add_goal(1, Box::new(SulfurCubeFloatGoal::new(mob_arc.clone())));
            goal_selector.add_goal(2, Box::new(SulfurCubeTemptGoal::new(mob_arc.clone())));
            goal_selector.add_goal(
                3,
                Box::new(SulfurCubeSearchForItemsGoal::new(mob_arc.clone())),
            );
            goal_selector.add_goal(
                4,
                Box::new(SulfurCubeRandomDirectionGoal::new(mob_arc.clone())),
            );
            goal_selector.add_goal(
                5,
                Box::new(SulfurCubeKeepOnJumpingGoal::new(mob_arc.clone())),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn get_size(&self) -> i32 {
        self.size.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_tiny(&self) -> bool {
        self.get_size() <= MIN_SIZE
    }

    /// Vanilla `CubeMobMoveControl.setDirection`.
    pub fn set_direction(&self, y_rot: f32, is_aggressive: bool) {
        self.move_direction.store(y_rot);
        self.is_aggressive.store(is_aggressive, Ordering::Relaxed);
    }

    /// Vanilla `CubeMobMoveControl.setWantedMovement`.
    pub fn set_wanted_movement(&self, speed: f32) {
        self.wanted_movement.store(speed);
    }

    /// Vanilla `AbstractCubeMob.getJumpDelay`.
    #[must_use]
    pub fn get_jump_delay(&self) -> i32 {
        rand::random_range(10..30)
    }

    /// Vanilla `AbstractCubeMob.getSoundPitch`.
    #[must_use]
    pub fn get_sound_pitch(&self) -> f32 {
        let pitch_adjuster = if self.is_tiny() { 1.4 } else { 0.8 };
        ((rand::random_range(0.0..1.0) - rand::random_range(0.0..1.0)) * 0.2 + 1.0) * pitch_adjuster
    }

    /// Vanilla `AbstractCubeMob.setSize`, with `SulfurCube.setSize`'s baby coercion.
    pub fn set_size(&self, size: i32, update_health: bool) {
        let actual_size = size.clamp(MIN_SIZE, 127);
        self.size.store(actual_size, Ordering::Relaxed);

        {
            let mut attributes = self
                .mob_entity
                .living_entity
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(health) = attributes.get_mut(&Attributes::MAX_HEALTH.id) {
                health.base_value = (4 * actual_size) as f64;
                health.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(speed) = attributes.get_mut(&Attributes::MOVEMENT_SPEED.id) {
                speed.base_value = f64::from(0.2 + 0.1 * actual_size as f32);
                speed.dirty.store(true, Ordering::Relaxed);
            }
        }

        if update_health {
            let max_health = self
                .mob_entity
                .living_entity
                .get_attribute_value(&Attributes::MAX_HEALTH) as f32;
            self.mob_entity.living_entity.health.store(max_health);
        }

        let entity = &self.mob_entity.living_entity.entity;
        let base = entity.entity_type.dimension;
        let dimensions = EntityDimensions {
            width: base[0] * actual_size as f32,
            height: base[1] * actual_size as f32,
            eye_height: entity.entity_type.eye_height * actual_size as f32,
        };
        entity.entity_dimension.store(dimensions);
        let pos = entity.pos.load();
        entity
            .bounding_box
            .store(BoundingBox::new_from_pos(pos.x, pos.y, pos.z, &dimensions));

        if update_health && actual_size == MIN_SIZE && !self.is_baby() {
            self.set_baby(true);
        }
    }
}

impl AgeableMob for SulfurCubeEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// Vanilla `SulfurCube.setSize` coerces a size of one into the baby state, and
    /// `ageBoundaryReached` restores the adult size. Mirror both here.
    fn set_baby(&self, baby: bool) {
        self.set_age(if baby {
            crate::entity::ageable::BABY_START_AGE
        } else {
            0
        });
        let size = if baby { MIN_SIZE } else { MAX_SIZE };
        if self.get_size() != size {
            self.set_size(size, true);
        }
    }
}

impl CustomSound for SulfurCubeEntity {
    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntitySulfurCubeDeath)
    }

    fn hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntitySulfurCubeHurt)
    }
}

impl Mob for SulfurCubeEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        // Vanilla `SulfurCube.customServerAiStep` timer.
        let pickup_timer = self.pickup_timer.load(Ordering::Relaxed);
        if pickup_timer > 0 {
            self.pickup_timer.store(pickup_timer - 1, Ordering::Relaxed);
        }

        // Vanilla `SulfurCube.ageBoundaryReached`: growing up restores the adult size.
        if !self.is_baby() && self.get_size() != MAX_SIZE {
            self.set_size(MAX_SIZE, true);
        }
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("pickup_timer", self.pickup_timer.load(Ordering::Relaxed));
        // Vanilla `AbstractCubeMob.addAdditionalSaveData`.
        nbt.put_int("Size", self.get_size() - 1);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.pickup_timer
            .store(nbt.get_int("pickup_timer").unwrap_or(0), Ordering::Relaxed);
        // Vanilla `AbstractCubeMob.readAdditionalSaveData`; the age (and thus baby state)
        // has already been restored by `AgeableMob::read_ageable_nbt`.
        self.set_size(nbt.get_int("Size").unwrap_or(1) + 1, false);
        if self.is_baby() && !self.is_tiny() {
            self.set_size(MIN_SIZE, false);
        }
    }
}

// ---------------------------------------------------------------------------
// Movement
// ---------------------------------------------------------------------------

/// Vanilla `AbstractCubeMob.CubeMobMoveControl`: the cube mobs only steer by a desired
/// yaw plus a wanted movement speed instead of pathfinding.
pub struct SulfurCubeMobMoveControl {
    mob: Weak<SulfurCubeEntity>,
    jump_delay: i32,
}

impl SulfurCubeMobMoveControl {
    #[must_use]
    pub const fn new(mob: Weak<SulfurCubeEntity>) -> Self {
        Self { mob, jump_delay: 0 }
    }
}

impl Control for SulfurCubeMobMoveControl {}

impl MoveControlTrait for SulfurCubeMobMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(sulfur_cube) = self.mob.upgrade() else {
            return;
        };
        let mob_entity = mob.get_mob_entity();
        let living_entity = &mob_entity.living_entity;
        let entity = &living_entity.entity;

        let target = sulfur_cube.move_direction.load();
        let new_yaw = self.change_angle(entity.yaw.load(), target, 90.0);
        entity.yaw.store(new_yaw);
        entity.head_yaw.store(new_yaw);
        entity.body_yaw.store(new_yaw);

        let speed_modifier = f64::from(sulfur_cube.wanted_movement.load());
        if speed_modifier <= 0.0 {
            living_entity
                .movement_input
                .store(Vector3::new(0.0, 0.0, 0.0));
            return;
        }

        let speed = speed_modifier * living_entity.get_attribute_value(&Attributes::MOVEMENT_SPEED);
        living_entity
            .movement_input
            .store(Vector3::new(0.0, 0.0, speed));

        if !entity.on_ground.load(Ordering::Relaxed) {
            return;
        }

        // Vanilla jump cadence: jump, then wait `getJumpDelay()` ticks, halved while aggressive.
        if self.jump_delay <= 0 {
            self.jump_delay = sulfur_cube.get_jump_delay();
            if sulfur_cube.is_aggressive.load(Ordering::Relaxed) {
                self.jump_delay /= 3;
            }
            living_entity.jumping.store(true, Ordering::SeqCst);
            let world = entity.world.load();
            world.play_sound_fine(
                Sound::EntitySulfurCubeJump,
                SoundCategory::Hostile,
                &entity.pos.load(),
                0.4 * sulfur_cube.get_size() as f32,
                sulfur_cube.get_sound_pitch(),
            );
        } else {
            self.jump_delay -= 1;
            living_entity.jumping.store(false, Ordering::SeqCst);
        }
    }

    fn has_wanted(&self) -> bool {
        self.mob
            .upgrade()
            .is_some_and(|cube| cube.wanted_movement.load() > 0.0)
    }
}

// ---------------------------------------------------------------------------
// Goals
// ---------------------------------------------------------------------------

/// Vanilla `AbstractCubeMob.CubeMobFloatGoal`.
pub struct SulfurCubeFloatGoal {
    sulfur_cube: Arc<SulfurCubeEntity>,
}

impl SulfurCubeFloatGoal {
    #[must_use]
    pub const fn new(sulfur_cube: Arc<SulfurCubeEntity>) -> Self {
        Self { sulfur_cube }
    }
}

impl Goal for SulfurCubeFloatGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let entity = &self.sulfur_cube.mob_entity.living_entity.entity;
        entity.touching_water.load(Ordering::Relaxed)
            || entity.touching_lava.load(Ordering::Relaxed)
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        if rand::random_range(0.0..1.0) < 0.8 {
            self.sulfur_cube
                .mob_entity
                .living_entity
                .jumping
                .store(true, Ordering::SeqCst);
        }
        self.sulfur_cube.set_wanted_movement(1.2);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::JUMP | Controls::MOVE
    }
}

/// Vanilla `AbstractCubeMob.CubeMobRandomDirectionGoal`.
pub struct SulfurCubeRandomDirectionGoal {
    sulfur_cube: Arc<SulfurCubeEntity>,
    chosen_degrees: f32,
    next_randomize_time: i32,
}

impl SulfurCubeRandomDirectionGoal {
    #[must_use]
    pub const fn new(sulfur_cube: Arc<SulfurCubeEntity>) -> Self {
        Self {
            sulfur_cube,
            chosen_degrees: 0.0,
            next_randomize_time: 0,
        }
    }
}

impl Goal for SulfurCubeRandomDirectionGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let entity = &self.sulfur_cube.mob_entity.living_entity.entity;
        self.sulfur_cube.mob_entity.get_target().is_none()
            && (entity.on_ground.load(Ordering::Relaxed)
                || entity.touching_water.load(Ordering::Relaxed)
                || entity.touching_lava.load(Ordering::Relaxed))
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        if self.next_randomize_time > 0 {
            self.next_randomize_time -= 1;
        }
        if self.next_randomize_time <= 0 {
            self.next_randomize_time = 40 + rand::random_range(0..60);
            self.chosen_degrees = rand::random_range(0.0..360.0);
        }
        self.sulfur_cube.set_direction(self.chosen_degrees, false);
    }

    fn controls(&self) -> Controls {
        Controls::LOOK
    }
}

/// Vanilla `AbstractCubeMob.CubeMobKeepOnJumpingGoal`.
pub struct SulfurCubeKeepOnJumpingGoal {
    sulfur_cube: Arc<SulfurCubeEntity>,
}

impl SulfurCubeKeepOnJumpingGoal {
    #[must_use]
    pub const fn new(sulfur_cube: Arc<SulfurCubeEntity>) -> Self {
        Self { sulfur_cube }
    }
}

impl Goal for SulfurCubeKeepOnJumpingGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        !self
            .sulfur_cube
            .mob_entity
            .living_entity
            .entity
            .has_vehicle()
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        self.sulfur_cube.set_wanted_movement(1.0);
    }

    fn controls(&self) -> Controls {
        Controls::JUMP | Controls::MOVE
    }
}

/// Vanilla `SulfurCube.SulfurCubeTemptGoal`.
pub struct SulfurCubeTemptGoal {
    sulfur_cube: Arc<SulfurCubeEntity>,
    target_player: Option<Arc<crate::entity::player::Player>>,
    cooldown: i32,
}

impl SulfurCubeTemptGoal {
    #[must_use]
    pub const fn new(sulfur_cube: Arc<SulfurCubeEntity>) -> Self {
        Self {
            sulfur_cube,
            target_player: None,
            cooldown: 0,
        }
    }

    /// Vanilla predicate: babies follow `minecraft:sulfur_cube_food`, adults follow
    /// any swallowable item.
    fn is_tempt_item(&self, stack: &ItemStack) -> bool {
        if stack.is_empty() {
            return false;
        }
        if self.sulfur_cube.is_baby() {
            stack
                .item
                .has_tag(&pumpkin_data::tag::Item::MINECRAFT_SULFUR_CUBE_FOOD)
        } else {
            stack
                .item
                .has_tag(&pumpkin_data::tag::Item::MINECRAFT_SULFUR_CUBE_SWALLOWABLE)
        }
    }

    fn is_holding_tempt_item(&self, player: &crate::entity::player::Player) -> bool {
        self.is_tempt_item(&player.inventory().held_item())
            || self.is_tempt_item(&player.inventory().off_hand_item())
    }
}

impl Goal for SulfurCubeTemptGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return false;
        }
        let mob_entity = mob.get_mob_entity();
        let range = mob_entity
            .living_entity
            .get_attribute_value(&Attributes::TEMPT_RANGE);
        let world = mob_entity.living_entity.entity.world.load();
        self.target_player = world.get_nearest_player(
            mob_entity.living_entity.entity.pos.load(),
            range,
            |player| player.living_entity.is_part_of_game() && self.is_holding_tempt_item(player),
        );
        self.target_player.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let Some(player) = self.target_player.clone() else {
            return;
        };
        // Vanilla `SulfurCubeTemptGoal.navigateTowards`: look at the player and steer
        // the cube move control along the mob's current yaw.
        let mob_entity = &self.sulfur_cube.mob_entity;
        let player_pos = player.get_entity().pos.load();
        mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_with_range(
                player_pos.x,
                player.get_entity().get_eye_y(),
                player_pos.z,
                10.0,
                10.0,
            );

        let yaw = entity_yaw_towards(&mob_entity.living_entity.entity.pos.load(), &player_pos);
        // `navigateTowards` runs every tick; re-issue it here so the direction keeps up
        // even while the goal stays active.
        self.sulfur_cube.set_direction(yaw, true);
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        self.target_player = None;
        self.sulfur_cube.set_wanted_movement(0.0);
        self.cooldown = 100;
    }

    fn controls(&self) -> Controls {
        // Vanilla only declares `Goal.Flag.LOOK` for this goal.
        Controls::LOOK
    }
}

/// Vanilla `SulfurCube.SulfurCubeSearchForItemsGoal`: an adult Sulfur Cube looks at the
/// nearest item it can swallow.
pub struct SulfurCubeSearchForItemsGoal {
    sulfur_cube: Arc<SulfurCubeEntity>,
    target_item: Option<Arc<dyn EntityBase>>,
}

impl SulfurCubeSearchForItemsGoal {
    #[must_use]
    pub const fn new(sulfur_cube: Arc<SulfurCubeEntity>) -> Self {
        Self {
            sulfur_cube,
            target_item: None,
        }
    }

    /// Vanilla `SulfurCube.ALLOWED_ITEMS`.
    fn is_allowed_item(entity: &Arc<dyn EntityBase>) -> bool {
        let Some(item_entity) = entity.get_item_entity() else {
            return false;
        };
        if item_entity.get_pickup_delay() > 0 || !entity.get_entity().is_alive() {
            return false;
        }
        item_entity
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .item
            .has_tag(&pumpkin_data::tag::Item::MINECRAFT_SULFUR_CUBE_SWALLOWABLE)
    }
}

impl Goal for SulfurCubeSearchForItemsGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        if self.sulfur_cube.is_baby() || self.sulfur_cube.pickup_timer.load(Ordering::Relaxed) > 0 {
            return false;
        }

        let entity = &self.sulfur_cube.mob_entity.living_entity.entity;
        let world: Arc<World> = entity.world.load_full();
        self.target_item =
            world.get_nearest_entity(entity.pos.load(), 8.0, None, Self::is_allowed_item);
        self.target_item.is_some()
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let Some(target) = self.target_item.clone() else {
            return;
        };
        let mob_entity = &self.sulfur_cube.mob_entity;
        let item_pos = target.get_entity().pos.load();
        mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_with_range(
                item_pos.x,
                target.get_entity().get_eye_y(),
                item_pos.z,
                10.0,
                10.0,
            );

        let yaw = entity_yaw_towards(&mob_entity.living_entity.entity.pos.load(), &item_pos);
        self.sulfur_cube.set_direction(yaw, true);
    }

    fn controls(&self) -> Controls {
        Controls::LOOK
    }
}

/// Vanilla `SulfurCube.checkSulfurCubeSpawnRules`.
#[must_use]
pub const fn check_sulfur_cube_spawn_rules(
    _world: &World,
    _pos: &pumpkin_util::math::position::BlockPos,
) -> bool {
    true
}

/// Vanilla `Mob.lookAt` horizontal yaw toward `target` (the value `getYRot` would hold
/// after a clamped look).
fn entity_yaw_towards(from: &Vector3<f64>, target: &Vector3<f64>) -> f32 {
    let dx = target.x - from.x;
    let dz = target.z - from.z;
    pumpkin_util::math::wrap_degrees((dz.atan2(dx) as f32).to_degrees() - 90.0)
}

use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::Sound;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase, EntityPose,
    ai::{
        goal::{Controls, Goal},
        util::default_random_pos,
    },
    mob::{Mob, breeze::BreezeEntity},
    projectile::{ThrownItemEntity, wind_charge::WindChargeEntity},
};

/// Vanilla `BreezeUtil.MAX_LINE_OF_SIGHT_TEST_RANGE`.
const MAX_LINE_OF_SIGHT_TEST_RANGE: f64 = 50.0;

/// Vanilla `Shoot`: how long the breeze charges before releasing the wind charge.
const SHOOT_INITIAL_DELAY_TICKS: i32 = 15;
/// Vanilla `Shoot`: recovery ticks after the charge is fired.
const SHOOT_RECOVER_DELAY_TICKS: i32 = 4;
/// Vanilla `Shoot`: cooldown before another charge can be started.
const SHOOT_COOLDOWN_TICKS: i32 = 10;
/// Vanilla `Shoot.ATTACK_RANGE_MAX_SQRT`.
const ATTACK_RANGE_MAX_SQRT: f64 = 256.0;
/// Vanilla `Shoot.PROJECTILE_MOVEMENT_SCALE`.
const PROJECTILE_MOVEMENT_SCALE: f64 = 0.7;

/// Vanilla `Slide`: movement speed while sliding.
const SLIDE_SPEED: f64 = 0.6;
/// Vanilla `BreezeAi.JUMP_CIRCLE_INNER_RADIUS`.
const JUMP_CIRCLE_INNER_RADIUS: f64 = 4.0;
/// Vanilla `BreezeAi.JUMP_CIRCLE_MIDDLE_RADIUS`.
const JUMP_CIRCLE_MIDDLE_RADIUS: f64 = 8.0;

/// `BreezeUtil.randomPointBehindTarget`.
fn random_point_behind_target(target: &Entity, rng: &mut impl RngExt) -> Vector3<f64> {
    let view_angle = target.yaw.load() + 180.0 + rng.random::<f32>() * 90.0 / 2.0;
    let r = 4.0 + rng.random::<f64>() * (8.0 - 4.0);
    Vector3::from_yaw_pitch(view_angle, 0.0).multiply(r, r, r)
}

/// `BreezeUtil.hasLineOfSight`.
fn has_line_of_sight(breeze: &dyn EntityBase, target: Vector3<f64>) -> bool {
    let from = breeze.get_entity().pos.load();
    if target.sub(&from).length() > MAX_LINE_OF_SIGHT_TEST_RANGE {
        return false;
    }
    let world = breeze.get_entity().world.load_full();
    world
        .raycast(from, target, |block_pos, world| {
            world.get_block_state(block_pos).is_solid()
        })
        .is_none()
}

fn random_point_in_middle_circle(
    breeze_pos: Vector3<f64>,
    enemy_pos: Vector3<f64>,
    rng: &mut impl RngExt,
) -> Vector3<f64> {
    let direction = enemy_pos.sub(&breeze_pos);
    let distance = direction.length()
        - (JUMP_CIRCLE_MIDDLE_RADIUS
            - rng.random::<f64>() * (JUMP_CIRCLE_MIDDLE_RADIUS - JUMP_CIRCLE_INNER_RADIUS));
    breeze_pos.add(&direction.normalize().multiply(distance, distance, distance))
}

/// Picks where the breeze should slide to, matching the `Slide`/`LongJump`
/// `start` logic: away from the target if it is too close, otherwise behind or
/// at middle-circle range from it.
fn slide_target(breeze: &dyn Mob, enemy: &dyn EntityBase) -> Vector3<f64> {
    let breeze_pos = breeze.get_entity().pos.load();
    let enemy_pos = enemy.get_entity().pos.load();
    let mut rng = breeze.get_random();

    if within_inner_circle_range(breeze_pos, enemy_pos)
        && let Some(pos) = default_random_pos::get_pos_away(breeze, 5, 5, enemy_pos)
        && has_line_of_sight(breeze, pos)
        && enemy_pos.squared_distance_to_vec(&pos) > breeze_pos.squared_distance_to_vec(&enemy_pos)
    {
        return pos;
    }

    if rng.random_range(0..2) == 0 {
        random_point_behind_target(enemy.get_entity(), &mut rng)
    } else {
        random_point_in_middle_circle(breeze_pos, enemy_pos, &mut rng)
    }
}

/// `Breeze.withinInnerCircleRange`: closer than 4 blocks in the x/z plane and 10 in y.
fn within_inner_circle_range(breeze_pos: Vector3<f64>, target: Vector3<f64>) -> bool {
    let dx = target.x - breeze_pos.x;
    let dz = target.z - breeze_pos.z;
    let horizontal = dx.mul_add(dx, dz * dz).sqrt();
    horizontal < JUMP_CIRCLE_INNER_RADIUS && (target.y - breeze_pos.y).abs() < 10.0
}

/// Vanilla `Breeze.getFiringYPosition`.
fn firing_y_position(entity: &Entity) -> f64 {
    entity.pos.load().y + f64::from(entity.entity_dimension.load().height) / 2.0 + 0.3
}

/// Port of vanilla `Shoot`: the breeze charges, then fires a `BREEZE_WIND_CHARGE`.
pub struct BreezeShootGoal {
    breeze: Weak<BreezeEntity>,
    charging_ticks: i32,
    recovering_ticks: i32,
    cooldown_ticks: i32,
    fired: bool,
}

impl BreezeShootGoal {
    #[must_use]
    pub const fn new(breeze: Weak<BreezeEntity>) -> Self {
        Self {
            breeze,
            charging_ticks: 0,
            recovering_ticks: 0,
            cooldown_ticks: 0,
            fired: false,
        }
    }

    fn shoot(breeze: &BreezeEntity, target: &dyn EntityBase) {
        let shooter = &breeze.mob_entity.living_entity.entity;
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        let target_y = target_pos.y
            + if target_entity.has_passengers() {
                0.8
            } else {
                0.3
            };
        let shooter_pos = shooter.pos.load();
        let dx = target_pos.x - shooter_pos.x;
        let dy = target_y - firing_y_position(shooter);
        let dz = target_pos.z - shooter_pos.z;

        let world = shooter.world.load_full();
        let difficulty = world.level_info.load().difficulty as i32;
        let uncertainty = 5.0 - f64::from(difficulty) * 4.0;

        let spawn_pos = Vector3::new(shooter_pos.x, firing_y_position(shooter), shooter_pos.z);
        let base_entity = Entity::from_uuid(
            uuid::Uuid::new_v4(),
            world.clone(),
            spawn_pos,
            &EntityType::BREEZE_WIND_CHARGE,
        );

        let thrown = ThrownItemEntity::new(
            base_entity,
            shooter,
            crate::entity::projectile::wind_charge::WIND_CHARGE_GRAVITY,
        );
        // `ThrownItemEntity::new` anchors the projectile at the shooter's eye; vanilla fires from
        // `getFiringYPosition`.
        thrown.entity.pos.store(spawn_pos);
        thrown.set_velocity(dx, dy, dz, PROJECTILE_MOVEMENT_SCALE, uncertainty);

        let wind_charge = WindChargeEntity::new_breeze(thrown);
        world.spawn_entity(Arc::new(wind_charge));

        shooter.play_sound(Sound::EntityBreezeShoot);
    }
}

impl Goal for BreezeShootGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        self.cooldown_ticks = (self.cooldown_ticks - 1).max(0);
        if self.cooldown_ticks > 0 {
            return false;
        }
        let Some(breeze) = self.breeze.upgrade() else {
            return false;
        };
        if breeze.get_entity().pose.load() != EntityPose::Standing {
            return false;
        }
        let Some(target) = breeze.mob_entity.get_target() else {
            return false;
        };
        if !breeze.can_attack(target.as_ref()) {
            return false;
        }
        let breeze_pos = breeze.get_entity().pos.load();
        let target_pos = target.get_entity().pos.load();
        breeze_pos.squared_distance_to_vec(&target_pos) < ATTACK_RANGE_MAX_SQRT
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        // Vanilla `Shoot` fires a single charge per activation; `stop` re-arms the cooldown.
        if self.fired {
            return false;
        }
        let Some(breeze) = self.breeze.upgrade() else {
            return false;
        };
        breeze.mob_entity.get_target().is_some()
    }

    fn start(&mut self, _mob: &dyn Mob) {
        let Some(breeze) = self.breeze.upgrade() else {
            return;
        };
        breeze.get_entity().set_pose(EntityPose::Shooting);
        breeze.get_entity().play_sound(Sound::EntityBreezeInhale);
        self.charging_ticks = SHOOT_INITIAL_DELAY_TICKS;
        self.recovering_ticks = 0;
        self.fired = false;
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        if let Some(breeze) = self.breeze.upgrade()
            && breeze.get_entity().pose.load() == EntityPose::Shooting
        {
            breeze.get_entity().set_pose(EntityPose::Standing);
        }
        self.cooldown_ticks = SHOOT_COOLDOWN_TICKS;
        self.charging_ticks = 0;
        self.recovering_ticks = 0;
        self.fired = false;
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let Some(breeze) = self.breeze.upgrade() else {
            return;
        };
        let Some(target) = breeze.mob_entity.get_target() else {
            return;
        };

        let target_pos = target.get_entity().pos.load();
        breeze.get_entity().look_at(target_pos);

        if self.charging_ticks > 0 {
            self.charging_ticks -= 1;
            return;
        }
        if self.recovering_ticks > 0 {
            self.recovering_ticks -= 1;
            return;
        }
        self.recovering_ticks = SHOOT_RECOVER_DELAY_TICKS;
        self.fired = true;
        Self::shoot(&breeze, target.as_ref());
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// Port of vanilla `Slide`: the breeze slides toward a point around its target.
pub struct BreezeSlideGoal {
    breeze: Weak<BreezeEntity>,
    target: Option<Vector3<f64>>,
}

impl BreezeSlideGoal {
    #[must_use]
    pub const fn new(breeze: Weak<BreezeEntity>) -> Self {
        Self {
            breeze,
            target: None,
        }
    }
}

impl Goal for BreezeSlideGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let Some(breeze) = self.breeze.upgrade() else {
            return false;
        };
        let entity = breeze.get_entity();
        if !entity.on_ground.load(Ordering::Relaxed)
            || entity.touching_water.load(Ordering::Relaxed)
            || entity.pose.load() != EntityPose::Standing
        {
            return false;
        }
        let Some(target) = breeze.mob_entity.get_target() else {
            return false;
        };
        if !breeze.can_attack(target.as_ref()) {
            return false;
        }
        self.target = Some(slide_target(breeze.as_ref(), target.as_ref()));
        true
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        let Some(breeze) = self.breeze.upgrade() else {
            return false;
        };
        breeze.mob_entity.get_target().is_some()
    }

    fn start(&mut self, _mob: &dyn Mob) {
        let Some(breeze) = self.breeze.upgrade() else {
            return;
        };
        let Some(target) = self.target else {
            return;
        };
        let pos = breeze.get_entity().pos.load();
        breeze
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(crate::entity::ai::pathfinder::NavigatorGoal::new(
                pos,
                target,
                SLIDE_SPEED,
            ));
        breeze.get_entity().play_sound(Sound::EntityBreezeSlide);
        breeze.get_entity().set_pose(EntityPose::Sliding);
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        if let Some(breeze) = self.breeze.upgrade() {
            breeze.get_entity().set_pose(EntityPose::Standing);
            breeze
                .mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
        }
        self.target = None;
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

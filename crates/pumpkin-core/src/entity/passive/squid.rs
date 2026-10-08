use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityStatus;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{Goal, to_goal_ticks},
    mob::{Mob, MobEntity},
};

/// Vanilla `AgeableWaterCreature.getAmbientSoundInterval()`.
const AMBIENT_SOUND_INTERVAL: i32 = 120;

pub struct SquidEntity {
    pub mob_entity: MobEntity,
    /// The squid's current target movement vector (`Squid.movementVector`).
    movement_vector: AtomicCell<Vector3<f64>>,
    /// `Squid.noActionTime` equivalent; reset on hurt so idle squids stop moving.
    no_action_time: AtomicI32,
    /// Vanilla `Squid.tentacleMovement` (server-driven pendulum phase).
    tentacle_movement: AtomicCell<f32>,
    /// Vanilla `Squid.tentacleSpeed`.
    tentacle_speed: AtomicCell<f32>,
    /// Vanilla `Squid.rotateSpeed`.
    rotate_speed: AtomicCell<f32>,
    /// Vertical body rotation (`Squid.xBodyRot`), used to orient the ink cloud.
    x_body_rot: AtomicCell<f32>,
    ambient_sound_chance: AtomicI32,
}

impl SquidEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let squid = Self {
            mob_entity,
            movement_vector: AtomicCell::new(Vector3::new(0.0, 0.0, 0.0)),
            no_action_time: AtomicI32::new(0),
            tentacle_movement: AtomicCell::new(0.0),
            tentacle_speed: AtomicCell::new(1.0 / (rand::random::<f32>() + 1.0) * 0.2),
            rotate_speed: AtomicCell::new(0.0),
            x_body_rot: AtomicCell::new(0.0),
            ambient_sound_chance: AtomicI32::new(AMBIENT_SOUND_INTERVAL),
        };
        let mob_arc = Arc::new(squid);

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Vanilla `Squid.registerGoals` registers these two goals; squids navigate
            // with their own movement vector instead of a path navigator.
            goal_selector.add_goal(0, Box::new(SquidRandomMovementGoal::new(mob_arc.clone())));
            goal_selector.add_goal(1, Box::new(SquidFleeGoal::new(mob_arc.clone())));
        };

        mob_arc
    }

    #[must_use]
    pub fn has_movement_vector(&self) -> bool {
        self.movement_vector.load().length_squared() > 1.0e-5
    }

    fn set_movement_vector(&self, vector: Vector3<f64>) {
        self.movement_vector.store(vector);
    }

    /// Vanilla `Squid.rotateVector`: applies the previous frame body rotations.
    fn rotate_vector(&self, vec: Vector3<f64>) -> Vector3<f64> {
        let pitch_rad = (self.x_body_rot.load() * (std::f32::consts::PI / 180.0)) as f64;
        let x_rot = rotate_x(vec, pitch_rad);
        let yaw_rad = -(self.mob_entity.living_entity.entity.body_yaw.load() as f64).to_radians();
        rotate_y(x_rot, yaw_rad)
    }

    fn is_in_water(&self) -> bool {
        self.mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed)
    }

    /// Vanilla `Squid.spawnInk`: plays the squirt sound and sprays ink particles.
    fn spawn_ink(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();
        world.play_sound_fine(
            Self::get_squirt_sound(),
            SoundCategory::Neutral,
            &pos,
            1.0,
            1.0,
        );

        let ink_pos = self.rotate_vector(Vector3::new(0.0, -1.0, 0.0)) + pos;

        let ink_pos_offset_scale = if self.is_baby() { 0.1 } else { 0.3 };
        let mut rng = rand::rng();
        for _ in 0..30 {
            let dir = self.rotate_vector(Vector3::new(
                f64::from(rng.random::<f32>() * 0.6 - 0.3),
                -1.0,
                f64::from(rng.random::<f32>() * 0.6 - 0.3),
            ));
            let dir_offset = dir * (ink_pos_offset_scale + f64::from(rng.random::<f32>()) * 2.0);
            world.spawn_particles(
                Self::get_ink_particle(),
                Vector3::new(ink_pos.x, ink_pos.y + 0.5, ink_pos.z),
                0,
                Vector3::new(
                    dir_offset.x as f32,
                    dir_offset.y as f32,
                    dir_offset.z as f32,
                ),
                0.1,
            );
        }
    }

    const fn get_squirt_sound() -> Sound {
        Sound::EntitySquidSquirt
    }

    const fn get_ink_particle() -> Particle {
        Particle::SquidInk
    }

    fn is_baby(&self) -> bool {
        self.mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0
    }

    fn tick_ambient_sound(&self, pos: &Vector3<f64>) {
        let chance = self.ambient_sound_chance.fetch_sub(1, Ordering::Relaxed);
        if chance <= 0 {
            self.ambient_sound_chance
                .store(AMBIENT_SOUND_INTERVAL, Ordering::Relaxed);
            // Vanilla `getVoicePitch()`: babies pitch higher.
            let base = if self.is_baby() { 1.5 } else { 1.0 };
            let pitch = (rand::random::<f32>() - rand::random::<f32>()) * 0.2 + base;
            self.mob_entity
                .living_entity
                .entity
                .world
                .load()
                .play_sound_fine(
                    Sound::EntitySquidAmbient,
                    SoundCategory::Ambient,
                    pos,
                    0.4,
                    pitch,
                );
        }
    }

    /// Vanilla `Squid.aiStep`. Runs every tick even without AI; the squid moves by
    /// its own `movementVector` and never uses the generic water travel.
    fn ai_step(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let mut tentacle_movement = self.tentacle_movement.load() + self.tentacle_speed.load();
        let pi = std::f32::consts::PI;

        if tentacle_movement > pi * 2.0 {
            tentacle_movement -= pi * 2.0;
            if rand::random_range(0..10) == 0 {
                self.tentacle_speed
                    .store(1.0 / (rand::random::<f32>() + 1.0) * 0.2);
            }
            let world = entity.world.load();
            world.send_entity_status(
                entity,
                EntityStatus::SquidAnimSynch,
                Some(pumpkin_protocol::bedrock::server::actor_event::ActorEventID::SquidFleeing),
            );
        }
        self.tentacle_movement.store(tentacle_movement);

        let in_water = self.is_in_water();
        if in_water {
            if tentacle_movement < pi {
                // tentacleAngle only affects rendering; rotateSpeed drives body roll.
                if tentacle_movement / pi > 0.75 {
                    entity.set_velocity(self.movement_vector.load());
                    self.rotate_speed.store(1.0);
                } else {
                    self.rotate_speed.store(self.rotate_speed.load() * 0.8);
                }
            } else {
                entity.set_velocity(entity.velocity.load() * 0.9);
                self.rotate_speed.store(self.rotate_speed.load() * 0.99);
            }

            let movement = entity.velocity.load();
            let horizontal_movement = movement.horizontal_length();
            let current_body_yaw = entity.body_yaw.load();
            let yaw = current_body_yaw
                + (-((movement.x.atan2(movement.z) as f32).to_degrees()) - current_body_yaw) * 0.1;
            entity.body_yaw.store(yaw);
            entity.yaw.store(yaw);

            let current_x_body_rot = self.x_body_rot.load();
            self.x_body_rot.store(
                current_x_body_rot
                    + (-((horizontal_movement.atan2(movement.y) as f32).to_degrees())
                        - current_x_body_rot)
                        * 0.1,
            );
        } else {
            self.x_body_rot
                .store(self.x_body_rot.load() + (-90.0 - self.x_body_rot.load()) * 0.02);

            // Pumpkin entities are always server-side, so vanilla's
            // `!level().isClientSide()` guard is unnecessary here.
            let yd = if self
                .mob_entity
                .living_entity
                .has_effect(&StatusEffect::LEVITATION)
            {
                let amplifier = self
                    .mob_entity
                    .living_entity
                    .get_effect(&StatusEffect::LEVITATION)
                    .map_or(0, |effect| effect.amplifier);
                0.05 * f64::from(amplifier + 1)
            } else {
                entity.velocity.load().y - self.get_gravity()
            };

            entity.set_velocity(Vector3::new(0.0, yd * Self::get_air_drag(), 0.0));
        }
    }

    fn get_gravity(&self) -> f64 {
        // Vanilla `Entity.getGravity()`: `isNoGravity() ? 0.0 : getDefaultGravity()`.
        if self.mob_entity.living_entity.entity.has_no_gravity() {
            0.0
        } else {
            Mob::get_mob_gravity(self)
        }
    }

    /// Vanilla `LivingEntity.getAirDrag()` default (0.98).
    const fn get_air_drag() -> f64 {
        0.98
    }
}

fn rotate_x(vec: Vector3<f64>, radians: f64) -> Vector3<f64> {
    let (sin, cos) = radians.sin_cos();
    Vector3::new(vec.x, vec.y * cos + vec.z * sin, vec.z * cos - vec.y * sin)
}

fn rotate_y(vec: Vector3<f64>, radians: f64) -> Vector3<f64> {
    let (sin, cos) = radians.sin_cos();
    Vector3::new(vec.x * cos + vec.z * sin, vec.y, vec.z * cos - vec.x * sin)
}

impl Mob for SquidEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        if !entity.is_alive() {
            return;
        }

        self.no_action_time.fetch_add(1, Ordering::Relaxed);

        self.ai_step();

        let pos = entity.pos.load();
        self.tick_ambient_sound(&pos);
    }

    /// Vanilla `Squid.travel` moves the squid by its own delta movement and skips
    /// the generic water/air travel in `LivingEntity.tick_movement`.
    fn travel(&self, caller: &dyn EntityBase) -> bool {
        let velocity = self.mob_entity.living_entity.entity.velocity.load();
        self.mob_entity
            .living_entity
            .entity
            .move_entity(caller, velocity);
        true
    }

    fn on_damage(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        _source: Option<&dyn EntityBase>,
    ) {
        // Vanilla resets `noActionTime` on hurt.
        self.no_action_time.store(0, Ordering::Relaxed);

        // Vanilla `Squid.hurtServer` spawns ink when hurt by a living entity.
        if self.mob_entity.living_entity.get_kill_credit().is_some() {
            self.spawn_ink();
        }
    }

    fn as_custom_sound(&self) -> Option<&dyn crate::entity::custom_sound::CustomSound> {
        Some(self)
    }

    fn get_mob_gravity(&self) -> f64 {
        // Vanilla `Squid.getDefaultGravity()`.
        0.08
    }
}

impl crate::entity::custom_sound::CustomSound for SquidEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntitySquidHurt)
    }

    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntitySquidDeath)
    }
}

/// Vanilla `Squid.SquidRandomMovementGoal`.
struct SquidRandomMovementGoal {
    squid: Arc<SquidEntity>,
}

impl SquidRandomMovementGoal {
    const fn new(squid: Arc<SquidEntity>) -> Self {
        Self { squid }
    }
}

impl Goal for SquidRandomMovementGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        true
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let no_action_time = self.squid.no_action_time.load(Ordering::Relaxed);
        if no_action_time > 100 {
            self.squid.set_movement_vector(Vector3::new(0.0, 0.0, 0.0));
        } else {
            let mut rng = rand::rng();
            let was_touching_water = self.squid.is_in_water();
            if rng.random_range(0..to_goal_ticks(50)) == 0
                || !was_touching_water
                || !self.squid.has_movement_vector()
            {
                let angle = rng.random::<f32>() * (std::f32::consts::PI * 2.0);
                self.squid.set_movement_vector(Vector3::new(
                    f64::from(angle.cos()) * 0.2,
                    f64::from(-0.1 + rng.random::<f32>() * 0.2),
                    f64::from(angle.sin()) * 0.2,
                ));
            }
        }
    }
}

/// Vanilla `Squid.SquidFleeGoal`.
struct SquidFleeGoal {
    squid: Arc<SquidEntity>,
    flee_ticks: i32,
}

impl SquidFleeGoal {
    const fn new(squid: Arc<SquidEntity>) -> Self {
        Self {
            squid,
            flee_ticks: 0,
        }
    }
}

impl Goal for SquidFleeGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !self.squid.is_in_water() {
            return false;
        }
        let Some(attacker) = mob.get_mob_entity().living_entity.get_kill_credit() else {
            return false;
        };
        let own_pos = mob.get_entity().pos.load();
        let attacker_pos = attacker.get_entity().pos.load();
        own_pos.squared_distance_to_vec(&attacker_pos) < 100.0
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.flee_ticks = 0;
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.flee_ticks += 1;
        let Some(attacker) = mob.get_mob_entity().living_entity.get_kill_credit() else {
            return;
        };

        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let attacker_pos = attacker.get_entity().pos.load();
        let mut flee_to = Vector3::new(
            pos.x - attacker_pos.x,
            pos.y - attacker_pos.y,
            pos.z - attacker_pos.z,
        );

        let world = entity.world.load();
        let target_pos = BlockPos::floored(pos.x + flee_to.x, pos.y + flee_to.y, pos.z + flee_to.z);
        let block_state = world.get_block_state(&target_pos);
        let (fluid, _) = world.get_fluid_and_fluid_state(&target_pos);
        let is_water = fluid.has_tag(&pumpkin_data::tag::Fluid::MINECRAFT_WATER);
        let is_air = block_state.is_air();

        if is_water || is_air {
            let length = flee_to.length();
            if length > 0.0 {
                flee_to = flee_to.normalize();
                let mut avoid_speed = 3.0;
                if length > 5.0 {
                    avoid_speed -= (length - 5.0) / 5.0;
                }
                if avoid_speed > 0.0 {
                    flee_to = flee_to * avoid_speed;
                }
            }

            if is_air {
                flee_to = flee_to.add_raw(0.0, -flee_to.y, 0.0);
            }

            self.squid.set_movement_vector(flee_to * (1.0 / 20.0));
        }

        if self.flee_ticks % 10 == 5 {
            world.spawn_particles(Particle::Bubble, pos, 1, Vector3::new(0.0, 0.0, 0.0), 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{rotate_x, rotate_y};
    use pumpkin_util::math::vector3::Vector3;

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1.0e-9, "{actual} != {expected}");
    }

    /// Vanilla `Vec3.xRot` for a 90 degree rotation about X.
    #[test]
    fn rotate_x_matches_vanilla_vec3() {
        let rotated = rotate_x(Vector3::new(0.0, 1.0, 0.0), std::f64::consts::FRAC_PI_2);
        assert_close(rotated.x, 0.0);
        assert_close(rotated.y, 0.0);
        assert_close(rotated.z, -1.0);
    }

    /// Vanilla `Vec3.yRot` for a 90 degree rotation about Y.
    #[test]
    fn rotate_y_matches_vanilla_vec3() {
        let rotated = rotate_y(Vector3::new(1.0, 0.0, 0.0), std::f64::consts::FRAC_PI_2);
        assert_close(rotated.x, 0.0);
        assert_close(rotated.y, 0.0);
        assert_close(rotated.z, -1.0);
    }
}

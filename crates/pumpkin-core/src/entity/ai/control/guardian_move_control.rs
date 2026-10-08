use std::sync::atomic::Ordering;

use pumpkin_data::attributes::Attributes;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::ai::control::move_control::Operation;
use crate::entity::ai::control::{Control, MoveControlTrait};
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::mob::Mob;

/// Vanilla `Guardian.GuardianMoveControl`: the guardian's fish-like swim wobble.
///
/// It steers the body towards the navigation waypoint and adds the lateral push that
/// makes a guardian swim with a tail swish, while flagging the mob as moving so clients
/// animate the tail and spikes.
///
/// Pumpkin's navigator writes `movement_input` itself rather than going through
/// `MoveControl.setWantedPosition`, so this control reads the waypoint straight from
/// the navigator and only contributes the extra velocity and rotation vanilla adds.
pub struct GuardianMoveControl {
    operation: Operation,
    /// Last value pushed to `DATA_ID_MOVING`, so the synced-data set (which allocates
    /// when serializing) only runs on a transition.
    moving: bool,
}

impl Default for GuardianMoveControl {
    fn default() -> Self {
        Self {
            operation: Operation::Wait,
            moving: false,
        }
    }
}

impl Control for GuardianMoveControl {}

impl MoveControlTrait for GuardianMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let living_entity = &mob_entity.living_entity;
        let entity = &living_entity.entity;

        // Pull the current waypoint out of the navigator. `server_ai_step` already
        // released the navigator lock before ticking the controls.
        let waypoint = {
            let navigator = mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if navigator.is_done() {
                None
            } else {
                navigator
                    .get_path()
                    .and_then(Path::get_next_node_pos)
                    .map(|pos| {
                        Vector3::new(
                            f64::from(pos.0.x) + 0.5,
                            f64::from(pos.0.y) + 0.5,
                            f64::from(pos.0.z) + 0.5,
                        )
                    })
            }
        };

        let Some(wanted) = waypoint else {
            self.operation = Operation::Wait;
            if self.moving {
                self.moving = false;
                set_moving(entity, false);
            }
            return;
        };
        self.operation = Operation::MoveTo;

        let pos = entity.pos.load();
        let delta = wanted - pos;
        let length = delta.length();
        if length < 1.0E-5 {
            return;
        }
        let yd = delta.y / length;

        // `rotlerp` towards the waypoint, then snap the body to the new yaw.
        let y_rot_d = (delta.z.atan2(delta.x).to_degrees() as f32) - 90.0;
        let new_yaw = self.change_angle(entity.yaw.load(), y_rot_d, 90.0);
        entity.yaw.store(new_yaw);
        entity.body_yaw.store(new_yaw);

        let target_speed = living_entity.get_attribute_value(&Attributes::MOVEMENT_SPEED);
        let current_speed = living_entity.movement_input.load().z;
        let new_speed = 0.125f64.mul_add(target_speed - current_speed, current_speed);

        // The sinusoidal push keyed off the entity id keeps a shoal of guardians out
        // of sync; vanilla uses tickCount + id which is `age + entity_id` here.
        let phase = f64::from(entity.age.load(Ordering::Relaxed)) + f64::from(entity.entity_id);
        let push = (phase * 0.5).sin() * 0.05;
        let cos = (f64::from(new_yaw) * (std::f64::consts::PI / 180.0)).cos();
        let sin = (f64::from(new_yaw) * (std::f64::consts::PI / 180.0)).sin();
        let y_push = (phase * 0.75).sin() * 0.05;

        let velocity = entity.velocity.load();
        entity.set_velocity(Vector3::new(
            velocity.x + push * cos,
            velocity.y + y_push * (sin + cos) * 0.25 + new_speed * yd * 0.1,
            velocity.z + push * sin,
        ));

        if !self.moving {
            self.moving = true;
            set_moving(entity, true);
        }
    }
}

fn set_moving(entity: &crate::entity::Entity, value: bool) {
    let key = if entity.entity_type == &pumpkin_data::entity::EntityType::ELDER_GUARDIAN {
        pumpkin_data::tracked_data::elder_guardian::DATA_ID_MOVING
    } else {
        pumpkin_data::tracked_data::guardian::DATA_ID_MOVING
    };
    entity.set_synced_data(key, value);
}

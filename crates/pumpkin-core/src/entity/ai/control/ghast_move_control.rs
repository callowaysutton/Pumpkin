use std::sync::atomic::Ordering;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::ai::control::move_control::Operation;
use crate::entity::ai::control::{Control, MoveControlTrait};
use crate::entity::mob::Mob;
use crate::world::World;

/// Vanilla `Ghast.GhastMoveControl`: floats a flying mob toward a wanted position by adding a
/// small step to its delta movement every few ticks, refusing steps that would clip a block.
///
/// `should_be_stopped` mirrors vanilla's `BooleanSupplier`; when it reports true the mob waits
/// and stops in place (used by the happy ghast for its "stays still" timeout).
pub struct GhastMoveControl {
    pub wanted_x: f64,
    pub wanted_y: f64,
    pub wanted_z: f64,
    pub speed_modifier: f64,
    pub operation: Operation,
    float_duration: i32,
    careful: bool,
    should_be_stopped: fn(&dyn Mob) -> bool,
}

impl GhastMoveControl {
    #[must_use]
    pub const fn new(careful: bool, should_be_stopped: fn(&dyn Mob) -> bool) -> Self {
        Self {
            wanted_x: 0.0,
            wanted_y: 0.0,
            wanted_z: 0.0,
            speed_modifier: 0.0,
            operation: Operation::Wait,
            float_duration: 0,
            careful,
            should_be_stopped,
        }
    }

    /// Vanilla `canReach`: reject a step whose destination clips a block the mob cannot pass.
    ///
    /// Deviation: vanilla also sweeps every block intersected along the travelled segment. The
    /// move steps here are tiny (one step every few ticks), so the inflated destination box is
    /// the dominant test and the swept walk is skipped.
    fn can_reach(mob: &dyn Mob, travel: Vector3<f64>, careful: bool) -> bool {
        let entity = mob.get_entity();
        let aabb = entity.bounding_box.load();
        let aabb_at_destination = aabb.shift(travel);
        let world = entity.world.load();

        let in_water = entity.touching_water.load(Ordering::SeqCst);
        let in_lava = entity.touching_lava.load(Ordering::SeqCst);

        if careful {
            for pos in Self::blocks_in(&aabb_at_destination.expand_all(1.0)) {
                if !Self::block_traversal_possible(&world, &pos, careful, in_water, in_lava) {
                    return false;
                }
            }
        }

        // Blocks the destination box itself overlaps.
        for pos in Self::blocks_in(&aabb_at_destination) {
            if !aabb.intersects(&pumpkin_util::math::boundingbox::BoundingBox::from_block(
                &pos,
            )) && !Self::block_traversal_possible(&world, &pos, careful, in_water, in_lava)
            {
                return false;
            }
        }

        true
    }

    fn blocks_in(
        aabb: &pumpkin_util::math::boundingbox::BoundingBox,
    ) -> pumpkin_util::math::position::BlockPosIterator {
        BlockPos::between_closed(aabb.min_block_pos(), aabb.max_block_pos())
    }

    fn block_traversal_possible(
        world: &World,
        pos: &BlockPos,
        careful: bool,
        in_water: bool,
        in_lava: bool,
    ) -> bool {
        let state = world.get_block_state(pos);
        if state.is_air() {
            return true;
        }

        let path_no_collisions = state.get_block_collision_shapes_at(pos).next().is_none();
        if !careful {
            return path_no_collisions;
        }

        if state
            .id
            .to_block()
            .has_tag(&tag::Block::MINECRAFT_HAPPY_GHAST_AVOIDS)
        {
            return false;
        }

        let (fluid, fluid_state) = world.get_fluid_and_fluid_state(pos);
        if !fluid_state.is_empty {
            if fluid.id == Fluid::WATER.id {
                return in_water;
            }
            if fluid.id == Fluid::LAVA.id {
                return in_lava;
            }
        }

        path_no_collisions
    }
}

impl Control for GhastMoveControl {}

impl MoveControlTrait for GhastMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if (self.should_be_stopped)(mob) {
            self.operation = Operation::Wait;
            mob.get_mob_entity()
                .living_entity
                .movement_input
                .store(Vector3::new(0.0, 0.0, 0.0));
            mob.get_entity().velocity.store(Vector3::new(0.0, 0.0, 0.0));
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
        }

        if self.operation != Operation::MoveTo {
            return;
        }

        self.float_duration -= 1;
        if self.float_duration > 0 {
            return;
        }
        self.float_duration += mob.get_random().random_range(0..5) + 2;

        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let travel = Vector3::new(
            self.wanted_x - pos.x,
            self.wanted_y - pos.y,
            self.wanted_z - pos.z,
        );

        if Self::can_reach(mob, travel, self.careful) {
            let flying_speed = mob
                .get_mob_entity()
                .living_entity
                .get_attribute_value(&Attributes::FLYING_SPEED);
            let scale = flying_speed * 5.0 / 3.0;
            let len = travel.length();
            if len > 1.0E-4 {
                let step = Vector3::new(
                    travel.x / len * scale,
                    travel.y / len * scale,
                    travel.z / len * scale,
                );
                let current = entity.velocity.load();
                entity.velocity.store(Vector3::new(
                    current.x + step.x,
                    current.y + step.y,
                    current.z + step.z,
                ));
            }
        } else {
            self.operation = Operation::Wait;
        }
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.wanted_x = x;
        self.wanted_y = y;
        self.wanted_z = z;
        self.speed_modifier = speed_modifier;
        self.operation = Operation::MoveTo;
    }

    fn has_wanted(&self) -> bool {
        self.operation == Operation::MoveTo
    }

    fn wanted_position(&self) -> Option<Vector3<f64>> {
        (self.operation == Operation::MoveTo).then_some(Vector3::new(
            self.wanted_x,
            self.wanted_y,
            self.wanted_z,
        ))
    }
}

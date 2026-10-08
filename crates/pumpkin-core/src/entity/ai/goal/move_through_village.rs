use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::{Controls, Goal};
use crate::block::blocks::doors::DoorBlock;
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::ai::util::{default_random_pos, land_random_pos};
use crate::entity::mob::Mob;

/// Vanilla `MoveThroughVillageGoal`: stroll between unvisited village POIs.
pub struct MoveThroughVillageGoal {
    speed_modifier: f64,
    path: Option<Path>,
    poi_pos: BlockPos,
    only_at_night: bool,
    visited: Vec<BlockPos>,
    distance_to_poi: i32,
}

impl MoveThroughVillageGoal {
    #[must_use]
    pub const fn new(speed_modifier: f64, only_at_night: bool, distance_to_poi: i32) -> Self {
        Self {
            speed_modifier,
            path: None,
            poi_pos: BlockPos::ZERO,
            only_at_night,
            visited: Vec::new(),
            distance_to_poi,
        }
    }

    fn has_not_visited(&self, poi: &BlockPos) -> bool {
        !self.visited.contains(poi)
    }

    /// Vanilla `updateVisited`.
    fn update_visited(&mut self) {
        if self.visited.len() > 15 {
            self.visited.remove(0);
        }
    }

    fn poi_center(&self) -> Vector3<f64> {
        Vector3::new(
            f64::from(self.poi_pos.0.x) + 0.5,
            f64::from(self.poi_pos.0.y),
            f64::from(self.poi_pos.0.z) + 0.5,
        )
    }

    fn closer_to_poi_than(&self, mob: &dyn Mob, distance: f64) -> bool {
        let pos = mob.get_entity().pos.load();
        self.poi_pos.dist_to_center_sqr(pos.x, pos.y, pos.z) < distance * distance
    }
}

impl Goal for MoveThroughVillageGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .can_navigate_ground()
        {
            return false;
        }

        self.update_visited();
        let entity = mob.get_entity();
        let world = entity.world.load();
        if self.only_at_night && world.is_bright_outside() {
            return false;
        }

        let pos = entity.block_pos.load();
        if !world.is_close_to_village(&pos, 6) {
            return false;
        }

        // Vanilla `LandRandomPos.getPos(mob, 15, 7, positionWeight)`: a candidate is only kept if
        // it sits in a village, and is scored by the negated squared distance to the nearest
        // unvisited village POI.
        let Some(land_pos) = land_random_pos::get_pos_with_weight(mob, 15, 7, |candidate| {
            if !world.is_village(candidate) {
                return f64::NEG_INFINITY;
            }
            world
                .find_village_poi(candidate, 10, |poi| self.has_not_visited(poi))
                .map_or(f64::NEG_INFINITY, |poi_pos| {
                    -f64::from(poi_pos.squared_distance(&pos))
                })
        }) else {
            return false;
        };

        let center = BlockPos::containing_vec(land_pos);
        let Some(poi_pos) = world.find_village_poi(&center, 10, |poi| self.has_not_visited(poi))
        else {
            return false;
        };
        self.poi_pos = poi_pos;

        let entity_living = &mob.get_mob_entity().living_entity;
        let mut navigator = mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_can_open_doors(mob.can_break_doors());
        self.path = navigator.create_path(entity_living, self.poi_center(), 0);
        navigator.set_can_open_doors(true);
        if self.path.is_none() {
            let Some(partial_step) = default_random_pos::get_pos_towards(
                mob,
                10,
                7,
                self.poi_center(),
                std::f64::consts::FRAC_PI_2,
            ) else {
                return false;
            };
            navigator.set_can_open_doors(mob.can_break_doors());
            self.path = navigator.create_path(entity_living, partial_step, 0);
            navigator.set_can_open_doors(true);
            if self.path.is_none() {
                return false;
            }
        }

        // Vanilla walks the fresh path looking for wooden doors and, if any node sits above one,
        // replaces the path with one that ends at that node.
        if let Some(path) = &self.path {
            let mut door_node = None;
            for i in 0..path.get_node_count() {
                let Some(node) = path.get_node(i) else {
                    continue;
                };
                let door_pos = BlockPos::new(node.pos.0.x, node.pos.0.y + 1, node.pos.0.z);
                if DoorBlock::is_wooden_door(&world, &door_pos) {
                    door_node = Some(node.pos);
                    break;
                }
            }
            if let Some(node_pos) = door_node {
                self.path = navigator.create_path(
                    entity_living,
                    Vector3::new(
                        f64::from(node_pos.0.x),
                        f64::from(node_pos.0.y),
                        f64::from(node_pos.0.z),
                    ),
                    0,
                );
            }
        }

        self.path.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let done = mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_done();
        if done {
            return false;
        }
        let width = f64::from(mob.get_entity().width());
        !self.closer_to_poi_than(mob, width + f64::from(self.distance_to_poi))
    }

    fn start(&mut self, mob: &dyn Mob) {
        let entity = &mob.get_mob_entity().living_entity;
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .move_to_path(self.path.take(), self.speed_modifier, entity);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let done = mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_done();
        if done || self.closer_to_poi_than(mob, f64::from(self.distance_to_poi)) {
            self.visited.push(self.poi_pos);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

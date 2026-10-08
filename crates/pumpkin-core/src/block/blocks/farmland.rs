use std::sync::Arc;

use crate::block::{
    BlockBehaviour, CanPlaceAtArgs, GetStateForNeighborUpdateArgs, OnLandedUponArgs, OnPlaceArgs,
    OnScheduledTickArgs, PathComputationType, RandomTickArgs,
};
use crate::world::World;
use pumpkin_data::block_properties::FarmlandLikeProperties;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, BlockState, BlockStateId};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockAccessor;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

type FarmlandProperties = FarmlandLikeProperties;

#[pumpkin_block("minecraft:farmland")]
pub struct FarmlandBlock;

impl BlockBehaviour for FarmlandBlock {
    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        if !can_survive(args.world.as_ref(), args.position) {
            Self::turn_to_base_block(args.world, args.position);
        }
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        if !can_survive(args.world, args.position) {
            return Block::DIRT.default_state.id;
        }
        args.block.default_state.id
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        if args.direction == BlockDirection::Up && !can_survive(args.world, args.position) {
            args.world
                .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
        }
        args.state_id
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_survive(args.block_accessor, args.position)
    }

    fn random_tick(&self, args: RandomTickArgs<'_>) {
        let state_id = args.world.get_block_state_id(args.position);
        let mut props = FarmlandProperties::from_state_id(state_id);
        if !is_near_water(args.world, args.position)
            && !args.world.is_raining_at(&args.position.up())
        {
            if props.moisture > 0 {
                let mut new_moisture = (props.moisture as i32 - 1).clamp(0, 7);
                if let Some(server) = args.world.server.upgrade() {
                    let mut event =
                        crate::plugin::api::events::block::moisture_change::MoistureChangeEvent::new(
                            *args.position,
                            args.world.clone(),
                            new_moisture,
                        );
                    server.plugin_manager.fire_blocking(&server, &mut event);
                    if event.cancelled {
                        return;
                    }
                    new_moisture = event.new_moisture;
                }
                props.moisture = new_moisture.clamp(0, 7) as u8;
                args.world.set_block_state(
                    args.position,
                    props.to_state_id(args.block),
                    BlockFlags::NOTIFY_NEIGHBORS,
                );
            } else if !should_maintain_farmland(args.world.as_ref(), args.position) {
                Self::turn_to_base_block(args.world, args.position);
            }
        } else if props.moisture < 7 {
            let mut new_moisture = 7;
            if let Some(server) = args.world.server.upgrade() {
                let mut event =
                    crate::plugin::api::events::block::moisture_change::MoistureChangeEvent::new(
                        *args.position,
                        args.world.clone(),
                        new_moisture,
                    );
                server.plugin_manager.fire_blocking(&server, &mut event);
                if event.cancelled {
                    return;
                }
                new_moisture = event.new_moisture;
            }
            props.moisture = new_moisture.clamp(0, 7) as u8;
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_NEIGHBORS,
            );
        }
    }

    /// Vanilla `FarmlandBlock#fallOn`: heavy enough living entities that land hard
    /// trample the farmland back to its base block.
    fn on_landed_upon(&self, args: OnLandedUponArgs<'_>) {
        if let Some(living) = args.entity.get_living_entity() {
            let entity = args.entity.get_entity();
            if rand::rng().random::<f32>() < args.fall_distance - 0.5 {
                let can_trample = args.entity.get_player().map_or_else(
                    || args.world.level_info.load().game_rules.mob_griefing,
                    |player| {
                        !args
                            .world
                            .is_in_spawn_protection(player, &entity.block_pos.load())
                    },
                );
                let width = entity.width();
                let height = entity.height();
                if can_trample && width * width * height > 0.512 {
                    let pos = entity.block_pos.load();
                    Self::turn_to_base_block(args.world, &pos);
                }
            }
            living.handle_fall_damage(args.entity, args.fall_distance, 1.0);
        }
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }
}

impl FarmlandBlock {
    /// Vanilla `FarmlandBlock#turnToBaseBlock`.
    fn turn_to_base_block(world: &Arc<World>, pos: &BlockPos) {
        let new_state = push_entities_up(world, pos);
        world.set_block_state(pos, new_state.id, BlockFlags::NOTIFY_ALL);
        world.emit_game_event(GameEvent::BlockChange.name(), pos.to_centered_f64());
    }
}

/// Vanilla `Block#pushEntitiesUp` for the farmland → base block transition.
///
/// Farmland's collision column is one sixteenth shorter than its base block's full
/// cube, so turning it back only makes the top slice newly solid. Entities standing
/// in that slice are lifted so their feet rest on the new block surface.
fn push_entities_up(world: &Arc<World>, pos: &BlockPos) -> &'static BlockState {
    let top = Vector3::new(pos.0.x as f64, pos.0.y as f64 + 15.0 / 16.0, pos.0.z as f64);
    let new_solid = BoundingBox::new(
        top,
        Vector3::new(
            pos.0.x as f64 + 1.0,
            pos.0.y as f64 + 1.0,
            pos.0.z as f64 + 1.0,
        ),
    );

    for entity in world.get_all_at_box(&new_solid) {
        let entity = entity.get_entity();
        let bb = entity.bounding_box.load();
        let new_y = new_solid.max.y;
        if bb.min.y < new_y {
            // `teleportRelative(0.0, 1.0 + offset, 0.0)`: the entity's feet end up
            // resting on the newly solid block top, keeping X/Z untouched.
            let pos = entity.pos.load();
            entity.set_pos(Vector3::new(pos.x, pos.y + (new_y - bb.min.y), pos.z));
        }
    }

    Block::DIRT.default_state
}

fn can_survive(world: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
    let state = world.get_block_state(&block_pos.up());
    !state.is_solid() || should_maintain_farmland(world, block_pos)
}

/// Vanilla `FarmlandBlock#shouldMaintainFarmland`.
fn should_maintain_farmland(world: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
    world
        .get_block_state(&block_pos.up())
        .id
        .to_block()
        .has_tag(&tag::Block::MINECRAFT_MAINTAINS_FARMLAND)
}

/// Vanilla `FarmlandBlock#isNearWater`: any water fluid within a 9x2x9 box around
/// the farmland (including the farmland's own y and the layer above).
fn is_near_water(world: &Arc<World>, block_pos: &BlockPos) -> bool {
    for dx in -4..=4 {
        for dy in 0..=1 {
            for dz in -4..=4 {
                let check_pos = block_pos.offset(Vector3 {
                    x: dx,
                    y: dy,
                    z: dz,
                });
                if world
                    .get_fluid(&check_pos)
                    .has_tag(&tag::Fluid::MINECRAFT_WATER)
                {
                    return true;
                }
            }
        }
    }
    false
}

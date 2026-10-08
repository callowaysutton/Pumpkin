use crate::block::{
    BlockBehaviour, CanPlaceAtArgs, GetStateForNeighborUpdateArgs, OnPlaceArgs,
    OnScheduledTickArgs, PathComputationType,
};
use crate::world::World;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, BlockState, BlockStateId, tag};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use std::sync::Arc;

#[pumpkin_block("minecraft:dirt_path")]
pub struct DirtPathBlock;

impl BlockBehaviour for DirtPathBlock {
    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        turn_to_base_block(args.world, args.position);
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        if !can_place_at(args.world, args.position) {
            push_entities_up(
                args.block.default_state,
                Block::DIRT.default_state,
                args.world,
                args.position,
            );
            return Block::DIRT.default_state.id;
        }

        args.block.default_state.id
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        if args.direction == BlockDirection::Up && !can_place_at(args.world, args.position) {
            args.world
                .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
        }
        args.state_id
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_place_at(args.block_accessor, args.position)
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }
}

/// Vanilla: `PathBlock.turnToBaseBlock`.
fn turn_to_base_block(world: &Arc<World>, position: &BlockPos) {
    let state = world.get_block_state(position);
    let new_state = Block::DIRT.default_state;
    push_entities_up(state, new_state, world, position);
    world.set_block_state(position, new_state.id, BlockFlags::NOTIFY_ALL);
    world.emit_game_event(GameEvent::BlockChange.name(), position.to_centered_f64());
}

/// Vanilla: `Block.pushEntitiesUp`, reduced to a block with full-width collision
/// shapes turning into a full block: joining the two collision shapes with
/// `BooleanOp.ONLY_SECOND` leaves the slab between their tops, and
/// `Shapes.collide` lifts every entity intersecting it exactly onto the new top
/// surface.
fn push_entities_up(
    state: &BlockState,
    new_state: &BlockState,
    world: &World,
    position: &BlockPos,
) {
    let old_top = state
        .get_block_collision_shapes()
        .fold(f64::NEG_INFINITY, |top, shape| top.max(shape.max.y));
    let new_top = new_state
        .get_block_collision_shapes()
        .fold(f64::NEG_INFINITY, |top, shape| top.max(shape.max.y));
    if old_top >= new_top {
        return; // vanilla: the joined shape is empty
    }

    let base_y = f64::from(position.0.y);
    let slab = BoundingBox {
        min: Vector3::new(
            f64::from(position.0.x),
            base_y + old_top,
            f64::from(position.0.z),
        ),
        max: Vector3::new(
            f64::from(position.0.x) + 1.0,
            base_y + new_top,
            f64::from(position.0.z) + 1.0,
        ),
    };
    for entity in world.get_all_at_box(&slab) {
        let entity_base = entity.get_entity();
        let bounding_box = entity_base.bounding_box.load();
        // Entities with their feet below the block base are missed by vanilla's
        // `Shapes.collide` grid sweep and are not lifted.
        if bounding_box.min.y < base_y - 1.0E-7 {
            continue;
        }
        let entity_position = entity_base.pos.load();
        let position = Vector3::new(entity_position.x, base_y + new_top, entity_position.z);
        // Dispatch through `EntityBase` so players get client-authoritative
        // movement (`Player::teleport`), matching vanilla's `teleportRelative`.
        entity.teleport(position, None, None, entity_base.world.load_full());
    }
}

/// Vanilla: `PathBlock.canSurvive`.
fn can_place_at(world: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
    let state = world.get_block_state(&block_pos.up());
    !state.is_solid() || Block::from_state_id(state.id).has_tag(&tag::Block::C_FENCE_GATES)
}

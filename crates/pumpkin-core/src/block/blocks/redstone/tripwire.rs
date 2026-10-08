use std::sync::Arc;

use pumpkin_data::item::Item;
use pumpkin_data::{
    Block, BlockDirection, BlockState, BlockStateId, block_properties::HorizontalFacing,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::{boundingbox::BoundingBox, position::BlockPos};
use pumpkin_world::{tick::TickPriority, world::BlockFlags};

use crate::{
    block::{
        BlockBehaviour, BrokenArgs, GetStateForNeighborUpdateArgs, OnEntityCollisionArgs,
        OnPlaceArgs, OnScheduledTickArgs, OnStateReplacedArgs, PlacedArgs,
    },
    entity::EntityBase,
    world::World,
};

use super::tripwire_hook::TripwireHookBlock;

type TripwireProperties = pumpkin_data::block_properties::TripwireLikeProperties;
type TripwireHookProperties = pumpkin_data::block_properties::TripwireHookLikeProperties;

#[pumpkin_block("minecraft:tripwire")]
pub struct TripwireBlock;

impl BlockBehaviour for TripwireBlock {
    fn on_entity_collision(&self, args: OnEntityCollisionArgs<'_>) {
        // Vanilla `TripWireBlock#entityInside`: only re-check when the wire is not already
        // powered and no re-check is already pending.
        let props = TripwireProperties::from_state_id(args.state.id);
        if props.powered
            || args
                .world
                .is_block_tick_scheduled(args.position, args.block)
        {
            return;
        }
        Self::check_pressed(
            args.world,
            args.position,
            args.state,
            args.block,
            std::iter::once(args.entity),
        );
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let [connect_north, connect_east, connect_south, connect_west] = [
            BlockDirection::North,
            BlockDirection::East,
            BlockDirection::South,
            BlockDirection::West,
        ]
        .map(|dir| {
            let current_pos = args.position.offset(dir.to_offset());
            let state_id = args.world.get_block_state_id(&current_pos);
            Self::should_connect_to(state_id, dir)
        });

        let mut props = TripwireProperties::from_state_id(args.block.default_state.id);

        props.north = connect_north;
        props.south = connect_south;
        props.west = connect_west;
        props.east = connect_east;

        props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        if Block::from_state_id(args.old_state_id) == Block::from_state_id(args.state_id) {
            return;
        }

        Self::update(args.world, args.position, args.state_id);
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        // Vanilla `TripWireBlock#playerWillDestroy`: shears disarm the wire.
        let has_shears = args.player.inventory().held_item().get_item() == &Item::SHEARS;
        if !has_shears {
            return;
        }

        let mut props = TripwireProperties::from_state_id(args.state.id);
        props.disarmed = true;
        let state_id = props.to_state_id(args.block);
        // The block has already been removed at this point (Pumpkin calls `broken` after
        // `break_block`), so the disarmed state is only used as the wire source state for the
        // hook re-calculation instead of being written back to the world.
        Self::update(args.world, args.position, state_id);

        args.world.emit_game_event(
            pumpkin_data::game_event::GameEvent::Shear.name(),
            args.position.to_centered_f64(),
        );
        // Vanilla `ShearsItem#mineBlock` damages the tool by one regardless of the block's
        // destroy speed (which is zero for tripwire).
        args.player.damage_held_item(1);
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        args.direction
            .to_horizontal_facing()
            .map_or(args.state_id, |facing| {
                let mut props = TripwireProperties::from_state_id(args.state_id);
                *match facing {
                    HorizontalFacing::North => &mut props.north,
                    HorizontalFacing::South => &mut props.south,
                    HorizontalFacing::West => &mut props.west,
                    HorizontalFacing::East => &mut props.east,
                } = Self::should_connect_to(args.neighbor_state_id, args.direction);
                props.to_state_id(args.block)
            })
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let state_id = args.world.get_block_state_id(args.position);
        let state = state_id.to_state();
        let props = TripwireProperties::from_state_id(state_id);
        if !props.powered {
            return;
        }

        // Vanilla `TripWireBlock#tick` -> `checkPressed(level, pos)`.
        let aabb = Self::entity_detection_box(state, args.position);
        let entities = args.world.get_entities_at_box(&aabb);
        let players = args.world.get_players_at_box(&aabb);
        Self::check_pressed(
            args.world,
            args.position,
            state,
            args.block,
            entities
                .iter()
                .map(|e| e.as_ref() as &dyn EntityBase)
                .chain(players.iter().map(|p| p.as_ref() as &dyn EntityBase)),
        );
    }

    fn on_state_replaced(&self, args: OnStateReplacedArgs<'_>) {
        if args.moved || Block::from_state_id(args.old_state_id) == args.block {
            return;
        }
        // Vanilla `affectNeighborsAfterRemoval` forces `POWERED` true so the hook is
        // re-evaluated as if the removed wire were still pressing it.
        let mut props = TripwireProperties::from_state_id(args.old_state_id);
        props.powered = true;
        Self::update(args.world, args.position, props.to_state_id(args.block));
    }
}

impl TripwireBlock {
    /// Vanilla `TripWireBlock#checkPressed`: recompute the pressed state from the entities
    /// overlapping the wire, notify the attached hooks, and reschedule the re-check.
    fn check_pressed<'a>(
        world: &Arc<World>,
        pos: &BlockPos,
        state: &BlockState,
        block: &Block,
        entities: impl Iterator<Item = &'a dyn EntityBase>,
    ) {
        let mut props = TripwireProperties::from_state_id(state.id);
        let was_pressed = props.powered;
        let should_be_pressed = entities
            .into_iter()
            .any(|entity| !entity.is_ignoring_block_triggers());

        if should_be_pressed != was_pressed {
            props.powered = should_be_pressed;
            let state_id = props.to_state_id(block);
            world.set_block_state(pos, state_id, BlockFlags::NOTIFY_ALL);
            Self::update(world, pos, state_id);
        }

        if should_be_pressed {
            world.schedule_block_tick(block, *pos, 10, TickPriority::Normal);
        } else if was_pressed {
            world.schedule_block_tick(block, *pos, 1, TickPriority::Normal);
        }
    }

    /// Vanilla `Block#getShape(...).bounds().move(pos)` for the wire's outline shape.
    fn entity_detection_box(state: &BlockState, pos: &BlockPos) -> BoundingBox {
        let mut shapes = state.get_block_outline_shapes_at(pos);
        let Some(first) = shapes.next() else {
            return BoundingBox::from_block(pos);
        };
        shapes.fold(first, |acc, shape| BoundingBox {
            min: pumpkin_util::math::vector3::Vector3::new(
                acc.min.x.min(shape.min.x),
                acc.min.y.min(shape.min.y),
                acc.min.z.min(shape.min.z),
            ),
            max: pumpkin_util::math::vector3::Vector3::new(
                acc.max.x.max(shape.max.x),
                acc.max.y.max(shape.max.y),
                acc.max.z.max(shape.max.z),
            ),
        })
    }

    fn update(world: &Arc<World>, pos: &BlockPos, state_id: BlockStateId) {
        for dir in [BlockDirection::South, BlockDirection::West] {
            for i in 1..42 {
                let current_pos = pos.offset_dir(dir.to_offset(), i);
                let (current_block, current_state) = world.get_block_and_state_id(&current_pos);
                if current_block == &Block::TRIPWIRE_HOOK {
                    let current_props = TripwireHookProperties::from_state_id(current_state);
                    if dir
                        .opposite()
                        .to_horizontal_facing()
                        .is_some_and(|f| current_props.facing == f)
                    {
                        TripwireHookBlock::update(
                            world,
                            current_pos,
                            current_state,
                            false,
                            true,
                            i,
                            Some(state_id),
                        );
                    }
                    break;
                }
                if current_block != &Block::TRIPWIRE {
                    break;
                }
            }
        }
    }

    #[must_use]
    pub fn should_connect_to(state_id: BlockStateId, facing: BlockDirection) -> bool {
        let block = Block::from_state_id(state_id);
        if block == &Block::TRIPWIRE_HOOK {
            let props = TripwireHookProperties::from_state_id(state_id);
            Some(props.facing) == facing.opposite().to_horizontal_facing()
        } else {
            block == &Block::TRIPWIRE
        }
    }
}

use pumpkin_data::block_properties::{Facing, PistonType};
use pumpkin_data::{Block, BlockState, BlockStateId, FacingExt};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use crate::block::{
    BlockBehaviour, OnNeighborUpdateArgs, OnStateReplacedArgs, PathComputationType,
};
use crate::block::{BrokenArgs, GetStateForNeighborUpdateArgs};
use crate::world::World;

use super::piston::PistonProps;

pub(crate) type PistonHeadProperties = pumpkin_data::block_properties::PistonHeadLikeProperties;
pub(crate) type MovingPistonProps = pumpkin_data::block_properties::MovingPistonLikeProperties;

#[pumpkin_block("minecraft:piston_head")]
pub struct PistonHeadBlock;

impl PistonHeadBlock {
    /// Vanilla `PistonHeadBlock.isFittingBase`: the given base block state is the
    /// matching (same type, same facing) extended piston for this head.
    fn is_fitting_base(
        world: &World,
        base_pos: &BlockPos,
        head_facing: Facing,
        sticky: bool,
    ) -> bool {
        let (base_block, base_state_id) = world.get_block_and_state_id(base_pos);
        let matching_block = if sticky {
            &Block::STICKY_PISTON
        } else {
            &Block::PISTON
        };
        if base_block != matching_block {
            return false;
        }
        let base_props = PistonProps::from_state_id(base_state_id);
        base_props.extended && base_props.facing == head_facing
    }

    /// Vanilla `PistonHeadBlock.canSurvive`.
    fn can_survive(world: &World, base_pos: &BlockPos, head_facing: Facing, sticky: bool) -> bool {
        let base_block = world.get_block(base_pos);
        base_block == &Block::MOVING_PISTON
            && MovingPistonProps::from_state_id(world.get_block_state_id(base_pos)).facing
                == head_facing
            || Self::is_fitting_base(world, base_pos, head_facing, sticky)
    }

    /// The position of the piston this head is attached to.
    fn base_pos(position: &BlockPos, head_facing: Facing) -> BlockPos {
        position.offset(head_facing.opposite().to_block_direction().to_offset())
    }
}

impl BlockBehaviour for PistonHeadBlock {
    fn broken(&self, args: BrokenArgs<'_>) {
        // Vanilla `playerWillDestroy`: a player that prevents block drops (creative
        // insta-mining) also removes the piston the head is attached to, without
        // drops. Other removals go through `on_state_replaced`.
        if args.player.gamemode.load() == pumpkin_util::GameMode::Creative {
            let props = PistonHeadProperties::from_state_id(args.state.id);
            let base_pos = Self::base_pos(args.position, props.facing);
            let sticky = props.r#type == PistonType::Sticky;
            if Self::is_fitting_base(args.world, &base_pos, props.facing, sticky) {
                args.world
                    .break_block(&base_pos, None, BlockFlags::SKIP_DROPS);
            }
        }
    }

    fn on_state_replaced(&self, args: OnStateReplacedArgs<'_>) {
        // Vanilla `affectNeighborsAfterRemoval`: a removed head also removes the
        // fitting base piston (with drops).
        let props = PistonHeadProperties::from_state_id(args.old_state_id);
        let base_pos = Self::base_pos(args.position, props.facing);
        let sticky = props.r#type == PistonType::Sticky;
        if Self::is_fitting_base(args.world, &base_pos, props.facing, sticky) {
            args.world
                .break_block(&base_pos, None, BlockFlags::NOTIFY_ALL);
        }
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        // Vanilla `updateShape`: the head pops off when the block it is mounted on
        // goes away.
        let props = PistonHeadProperties::from_state_id(args.state_id);
        let sticky = props.r#type == PistonType::Sticky;
        let base_pos = Self::base_pos(args.position, props.facing);
        if args.direction == props.facing.to_block_direction().opposite()
            && !Self::can_survive(args.world, &base_pos, props.facing, sticky)
        {
            Block::AIR.default_state.id
        } else {
            args.state_id
        }
    }

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        // Vanilla `neighborChanged`: relay the update through the head to the
        // piston base so it can check whether it should retract.
        let head_state_id = args.world.get_block_state_id(args.position);
        let props = PistonHeadProperties::from_state_id(head_state_id);
        let sticky = props.r#type == PistonType::Sticky;
        let base_pos = Self::base_pos(args.position, props.facing);
        if Self::can_survive(args.world, &base_pos, props.facing, sticky) {
            args.world.update_neighbor(&base_pos, args.source_block);
        }
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }
}

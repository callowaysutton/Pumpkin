use pumpkin_data::fluid::Fluid;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use crate::block::{
    BlockBehaviour, CanPlaceAtArgs, GetStateForNeighborUpdateArgs, OnEntityCollisionArgs,
};
use crate::world::World;

#[pumpkin_block("minecraft:lily_pad")]
pub struct LilyPadBlock;

impl BlockBehaviour for LilyPadBlock {
    fn on_entity_collision(&self, args: OnEntityCollisionArgs<'_>) {
        // Vanilla checks `entity instanceof AbstractBoat`, which covers every boat and raft.
        let resource_name = args.entity.get_entity().entity_type.resource_name;
        if resource_name.ends_with("_boat") || resource_name.ends_with("_raft") {
            args.world
                .break_block(args.position, None, BlockFlags::NOTIFY_ALL);
        }
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        let Some(world) = args.world else {
            return false; // Fluid access is required to check the placement.
        };
        Self::may_place_on(world, &args.position.down())
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        if !Self::may_place_on(args.world, &args.position.down()) {
            return Block::AIR.default_state.id;
        }
        args.state_id
    }
}

impl LilyPadBlock {
    /// Vanilla `LilyPadBlock#mayPlaceOn` with `pos` being the block below the lily pad: the
    /// fluid there must be in `FluidTags#SUPPORTS_LILY_PAD` (a source water fluid state or a
    /// waterlogged block; Pumpkin normalizes all water blocks to the flowing family, so
    /// `matches_type` plus the source state) or the block must be in
    /// `BlockTags#SUPPORTS_LILY_PAD` (ice), and the fluid at the lily pad itself must be
    /// empty. Also vanilla `VegetationBlock#canSurvive`, which runs this check on the block
    /// below.
    fn may_place_on(world: &World, pos: &BlockPos) -> bool {
        let (support_fluid, support_fluid_state) = world.get_fluid_and_fluid_state(pos);
        ((support_fluid_state.is_source && support_fluid.matches_type(&Fluid::WATER))
            || world
                .get_block(pos)
                .has_tag(&tag::Block::MINECRAFT_SUPPORTS_LILY_PAD))
            && world.get_fluid_and_fluid_state(&pos.up()).1.is_empty
    }
}

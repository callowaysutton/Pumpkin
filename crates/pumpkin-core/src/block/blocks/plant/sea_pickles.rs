use crate::block::BlockIsReplacing;
use crate::block::blocks::plant::PlantBlockBase;
use crate::block::{
    BlockBehaviour, BonemealArgs, CanPlaceAtArgs, CanUpdateAtArgs, GetStateForNeighborUpdateArgs,
    OnPlaceArgs, PathComputationType,
};
use crate::entity::EntityBase;
use pumpkin_data::entity::EntityPose;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, BlockState, BlockStateId, tag};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::{RandomGenerator, RandomImpl, xoroshiro128::Xoroshiro};
use pumpkin_world::world::BlockFlags;

type SeaPickleProperties = pumpkin_data::block_properties::SeaPickleLikeProperties;

#[pumpkin_block("minecraft:sea_pickle")]
pub struct SeaPickleBlock;

impl SeaPickleBlock {
    /// A sea pickle is "dead" once it is no longer waterlogged.
    #[must_use]
    pub fn is_dead(state_id: BlockStateId) -> bool {
        !SeaPickleProperties::from_state_id(state_id).waterlogged
    }
}

impl BlockBehaviour for SeaPickleBlock {
    fn is_valid_bonemeal_target(&self, args: BonemealArgs<'_>) -> bool {
        !Self::is_dead(args.state_id)
            && args
                .world
                .get_block(&args.position.down())
                .has_tag(&tag::Block::MINECRAFT_CORAL_BLOCKS)
    }

    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    fn perform_bonemeal(&self, args: BonemealArgs<'_>) {
        // 1:1 vanilla algorithm (SeaPickleBlock#performBonemeal).
        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::random::<u64>()));

        let base_x = args.position.0.x - 2;
        let mut z_off_set = 0;
        let mut z_span = 1;
        let mut count = 0;
        for added_x in 0..5 {
            for added_z in 0..z_span {
                let end_y = 2 + args.position.0.y - 1;
                for y in (end_y - 2)..end_y {
                    let lv =
                        BlockPos::new(base_x + added_x, y, args.position.0.z - z_off_set + added_z);
                    if &lv == args.position
                        || random.next_bounded_i32(6) != 0
                        || !args.world.get_block(&lv).eq(&Block::WATER)
                        || !args
                            .world
                            .get_block(&lv.down())
                            .has_tag(&tag::Block::MINECRAFT_CORAL_BLOCKS)
                    {
                        continue;
                    }
                    let mut sea_pickle_prop = SeaPickleProperties::default(args.block);
                    sea_pickle_prop.pickles = (random.next_bounded_i32(4) + 1) as u8;
                    args.world.set_block_state(
                        &lv,
                        sea_pickle_prop.to_state_id(args.block),
                        BlockFlags::NOTIFY_ALL,
                    );
                }
            }
            if count < 2 {
                z_span += 2;
                z_off_set += 1;
            } else {
                z_span -= 2;
                z_off_set -= 1;
            }
            count += 1;
        }

        let mut sea_pickle_prop = SeaPickleProperties::from_state_id(args.state_id);
        sea_pickle_prop.pickles = 4;
        args.world.set_block_state(
            args.position,
            sea_pickle_prop.to_state_id(args.block),
            BlockFlags::NOTIFY_LISTENERS,
        );
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        if args.player.get_entity().pose.load() != EntityPose::Crouching
            && let BlockIsReplacing::Itself(state_id) = args.replacing
        {
            let mut sea_pickle_prop = SeaPickleProperties::from_state_id(state_id);
            if sea_pickle_prop.pickles < 4 {
                sea_pickle_prop.pickles += 1;
            }
            return sea_pickle_prop.to_state_id(args.block);
        }

        let mut sea_pickle_prop = SeaPickleProperties::default(args.block);
        sea_pickle_prop.waterlogged = args.replacing.water_source();
        sea_pickle_prop.to_state_id(args.block)
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        let support_block = args.block_accessor.get_block_state(&args.position.down());
        support_block.is_center_solid(BlockDirection::Up)
    }

    fn can_update_at(&self, args: CanUpdateAtArgs<'_>) -> bool {
        args.player.get_entity().pose.load() != EntityPose::Crouching
            && SeaPickleProperties::from_state_id(args.state_id).pickles < 4
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        <Self as PlantBlockBase>::get_state_for_neighbor_update(
            self,
            args.world,
            args.position,
            args.state_id,
        )
    }

    fn is_pathfindable(&self, _state: &BlockState, _computation_type: PathComputationType) -> bool {
        false
    }
}

impl PlantBlockBase for SeaPickleBlock {}

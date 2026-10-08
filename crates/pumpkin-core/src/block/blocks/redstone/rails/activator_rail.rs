use pumpkin_data::BlockStateId;
use pumpkin_macros::pumpkin_block;
use pumpkin_world::world::BlockFlags;

use crate::block::BlockBehaviour;
use crate::block::CanPlaceAtArgs;
use crate::block::OnNeighborUpdateArgs;
use crate::block::OnPlaceArgs;
use crate::block::OnStateReplacedArgs;
use crate::block::PlacedArgs;
use crate::entity::EntityBase;

use super::RailProperties;
use super::common::{
    can_place_rail_at, compute_placed_rail_shape, rail_placement_is_valid,
    update_flanking_rails_shape, update_powered_state,
};

// In vanilla `ActivatorRailBlock` is a `PoweredRailBlock`, so it shares the powered-rail power
// search and update behaviour; only the block it searches for differs.
#[pumpkin_block("minecraft:activator_rail")]
pub struct ActivatorRailBlock;

impl BlockBehaviour for ActivatorRailBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut rail_props = RailProperties::default(args.block);
        let player_facing = args.player.get_entity().get_horizontal_facing();

        rail_props.set_waterlogged(args.replacing.water_source());
        rail_props.set_straight_shape(compute_placed_rail_shape(
            args.world,
            args.position,
            player_facing,
        ));

        rail_props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        update_flanking_rails_shape(args.world, args.block, args.state_id, args.position);

        update_powered_state(args.world, args.block, args.position);
    }

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        if !rail_placement_is_valid(args.world, args.block, args.position) {
            args.world
                .break_block(args.position, None, BlockFlags::NOTIFY_ALL);
            return;
        }

        update_powered_state(args.world, args.block, args.position);
    }

    fn on_state_replaced(&self, args: OnStateReplacedArgs<'_>) {
        if args.moved {
            return;
        }

        let rail_props = RailProperties::new(args.old_state_id, args.block);

        if rail_props.shape().is_ascending() {
            args.world.update_neighbors(&args.position.up(), None);
        }

        args.world.update_neighbors(args.position, None);
        args.world.update_neighbors(&args.position.down(), None);
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_place_rail_at(args.block_accessor, args.position)
    }
}

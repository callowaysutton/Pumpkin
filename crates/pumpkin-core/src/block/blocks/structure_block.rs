use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, NormalUseArgs, OnNeighborUpdateArgs, OnPlaceArgs, PlacedArgs, PlayerPlacedArgs,
};

use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{StructureBlockLikeProperties, StructureblockMode};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::{GameMode, PermissionLvl};

#[pumpkin_block("minecraft:structure_block")]
pub struct StructureBlock;

impl BlockBehaviour for StructureBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        // Vanilla registers LOAD as the default state of the block; the block
        // entity mirrors it through its construction.
        StructureBlockLikeProperties {
            r#mode: StructureblockMode::Load,
        }
        .to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        // Vanilla `newBlockEntity` takes the mode from the placed block state.
        let mode = StructureBlockLikeProperties::from_state_id(args.state_id).r#mode;
        let block_entity =
            crate::block::entities::structure_block::StructureBlockBlockEntity::new_with_mode(
                *args.position,
                mode,
            );
        args.world
            .add_block_entity(std::sync::Arc::new(block_entity));
    }

    fn player_placed(&self, args: PlayerPlacedArgs<'_>) {
        // Vanilla `setPlacedBy` signs the placing entity in as the author.
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(structure_block) = block_entity
                .as_any()
                .downcast_ref::<crate::block::entities::structure_block::StructureBlockBlockEntity>(
                )
        {
            structure_block.created_by(&args.player.gameprofile.name);
        }
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        {
            // Vanilla `StructureBlockEntity#usedBy` -> `canUseGameMasterBlocks`:
            // instabuild plus gamemaster permissions.
            if args.player.permission_lvl.load() < PermissionLvl::Two
                || args.player.gamemode.load() != GameMode::Creative
            {
                return BlockActionResult::Pass;
            }
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return BlockActionResult::Pass;
            };
            args.world.update_block_entity(&block_entity);

            BlockActionResult::Success
        }
    }

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        // Vanilla `StructureBlock#neighborChanged`: rising power edges trigger
        // the block for its mode, falling power edges clear the powered flag.
        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            return;
        };
        let Some(structure_block) =
            block_entity
                .as_any()
                .downcast_ref::<crate::block::entities::structure_block::StructureBlockBlockEntity>(
                )
        else {
            return;
        };

        let should_trigger = crate::block::blocks::redstone::block_receives_redstone_power(
            args.world,
            args.position,
        );
        let is_powered = structure_block.is_powered();
        if should_trigger && !is_powered {
            structure_block.set_powered(true);
            trigger(args.world, structure_block);
        } else if !should_trigger && is_powered {
            structure_block.set_powered(false);
        } else {
            // Neither edge changed state, so there is nothing to sync.
            return;
        }
        // Vanilla's `setChanged` calls inside the triggered actions land here:
        // the powered flag and any refreshed size info go to clients and to the
        // chunk save in one sync.
        args.world.update_block_entity(&block_entity);
    }
}

/// Vanilla `StructureBlock#trigger`: per mode, either save, place, unload or
/// nothing.
fn trigger(
    world: &std::sync::Arc<crate::world::World>,
    structure_block: &crate::block::entities::structure_block::StructureBlockBlockEntity,
) {
    match structure_block.get_mode_value() {
        StructureblockMode::Save => {
            let _ = structure_block.save_structure(world, false);
        }
        StructureblockMode::Load => structure_block.place_structure(world),
        StructureblockMode::Corner => structure_block.unload_structure(),
        StructureblockMode::Data => {}
    }
}

use std::sync::Arc;
use std::sync::Mutex;

use crate::block::{
    GetComparatorOutputArgs, GetScreenHandlerFactoryArgs, OnPlaceArgs, OnSyncedBlockEventArgs,
    PlacedArgs,
};
use crate::block::{
    registry::BlockActionResult,
    {BlockBehaviour, NormalUseArgs},
};

use crate::block::entities::shulker_box::ShulkerBoxBlockEntity;
use crate::entity::mob::shulker::ShulkerEntity;
use pumpkin_data::BlockState;
use pumpkin_data::BlockStateId;
use pumpkin_data::FacingExt;
use pumpkin_data::translation;
use pumpkin_inventory::Inventory;
use pumpkin_inventory::generic_container_screen_handler::create_generic_9x3;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;

use crate::world::World;

struct ShulkerBoxScreenFactory(Arc<dyn Inventory>);

impl ScreenHandlerFactory for ShulkerBoxScreenFactory {
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        let handler = create_generic_9x3(sync_id, player_inventory, self.0.clone(), player);
        let screen_handler_arc = Arc::new(Mutex::new(handler));

        Some(screen_handler_arc as SharedScreenHandler)
    }

    fn get_display_name(&self) -> TextComponent {
        pumpkin_macros::translate_cross!(
            translation::java::CONTAINER_SHULKERBOX,
            translation::bedrock::CONTAINER_SHULKERBOX
        )
    }
}

#[pumpkin_block_from_tag("minecraft:shulker_boxes")]
pub struct ShulkerBoxBlock;

type EndRodLikeProperties = pumpkin_data::block_properties::EndRodLikeProperties;

impl BlockBehaviour for ShulkerBoxBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = EndRodLikeProperties::default(args.block);
        props.facing = args.direction.to_facing().opposite();
        props.to_state_id(args.block)
    }

    fn on_synced_block_event(&self, args: OnSyncedBlockEventArgs<'_>) -> bool {
        // On the server, we don't need the Animation steps for now, because the client is responsible for that.
        args.r#type == Self::OPEN_ANIMATION_EVENT_TYPE
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        {
            let barrel_block_entity = ShulkerBoxBlockEntity::new(*args.position);
            args.world.add_block_entity(Arc::new(barrel_block_entity));
        }
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        // Vanilla `ShulkerBoxBlock.useWithoutItem` only opens the box when `canOpen` allows it.
        let can_open = args
            .world
            .get_block_entity(args.position)
            .as_ref()
            .and_then(|entity| entity.as_any().downcast_ref::<ShulkerBoxBlockEntity>())
            .is_some_and(|block_entity| {
                Self::can_open(
                    args.world.get_block_state(args.position),
                    args.world,
                    args.position,
                    block_entity,
                )
            });

        if can_open
            && let Some(factory) = self.get_screen_handler_factory(GetScreenHandlerFactoryArgs {
                server: args.server,
                world: args.world,
                block: args.block,
                position: args.position,
                player: args.player,
            })
        {
            args.player
                .open_handled_screen(factory.as_ref(), Some(*args.position));
            args.player.increment_stat(
                pumpkin_data::statistic::StatisticCategory::Custom,
                pumpkin_data::statistic::CustomStatistic::OpenShulkerBox as i32,
                1,
            );
        }

        BlockActionResult::Success
    }

    fn get_screen_handler_factory(
        &self,
        args: GetScreenHandlerFactoryArgs<'_>,
    ) -> Option<Box<dyn ScreenHandlerFactory>> {
        let block_entity = args.world.get_block_entity(args.position)?;
        let inventory = block_entity.get_inventory()?;
        Some(Box::new(ShulkerBoxScreenFactory(inventory)))
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        crate::block::container_comparator_output(&args)
    }
}

impl ShulkerBoxBlock {
    pub const OPEN_ANIMATION_EVENT_TYPE: u8 = 1;

    /// Vanilla: `ShulkerBoxBlock.canOpen`.
    fn can_open(
        state: &BlockState,
        world: &Arc<World>,
        position: &BlockPos,
        block_entity: &ShulkerBoxBlockEntity,
    ) -> bool {
        // Vanilla allows re-opening while the box is animating (OPENING/OPENED/CLOSING);
        // without a server-side animation, any viewer counts as "not closed".
        if block_entity.viewers.get_viewer_count() != 0 {
            return true;
        }

        let props = EndRodLikeProperties::from_state_id(state.id);
        // Vanilla: Shulker.getProgressDeltaAabb(1.0F, FACING, 0.0F, 0.5F, Vec3.atBottomCenterOf(pos)).deflate(1.0E-6)
        let lid_box = ShulkerEntity::get_progress_delta_aabb(
            1.0,
            props.facing.to_block_direction(),
            0.0,
            0.5,
            position.to_f64(),
        )
        .contract_all(1.0E-6);
        world.is_space_empty(lid_box)
            // Vanilla `noCollision` also fails when a collidable entity is in the lid's way.
            && !world
                .get_entities_at_box(&lid_box.expand_all(1.0E-7))
                .iter()
                .any(|entity| !entity.is_spectator() && entity.is_collidable(None))
    }
}

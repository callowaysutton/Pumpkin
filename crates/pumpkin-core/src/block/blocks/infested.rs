use std::sync::Arc;

use pumpkin_data::Enchantment;
use pumpkin_data::entity::EntityType;
use pumpkin_data::entity_status::EntityStatus;
use pumpkin_data::game_rules::{GameRule, GameRuleValue};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;

use crate::block::BlockBehaviour;
use crate::block::BrokenArgs;
use crate::block::ExplodeArgs;
use crate::entity::r#type::from_type;
use crate::world::World;

#[pumpkin_block_from_tag("c:cobblestones/infested")]
pub struct InfestedBlock;

impl InfestedBlock {
    /// Spawns the silverfish hidden inside an infested block.
    ///
    /// Mirrors vanilla `InfestedBlock.spawnInfestation`.
    fn spawn_infestation(world: &Arc<World>, position: &BlockPos) {
        let pos = position.to_f64();
        let silverfish = from_type(&EntityType::SILVERFISH, pos, world, uuid::Uuid::new_v4());
        if world.spawn_entity(silverfish.clone()) {
            // Vanilla `Mob.spawnAnim()`: broadcast entity event 20 (silverfish poof).
            world.broadcast_entity_event(
                silverfish.get_entity(),
                EntityStatus::SilverfishMergeAnim,
                None,
            );
        }
    }
}

impl BlockBehaviour for InfestedBlock {
    fn broken(&self, args: BrokenArgs<'_>) {
        // Vanilla only spawns the silverfish from `playerDestroy` -> `dropResources` ->
        // `spawnAfterBreak`, which is skipped when the player prevents block drops
        // (creative / spectator) or when the `block_drops` gamerule is off.
        if args.player.gamemode.load() == GameMode::Creative {
            return;
        }
        let block_drops = matches!(
            args.world.get_game_rule(&GameRule::BlockDrops),
            GameRuleValue::Bool(true)
        );
        if !block_drops {
            return;
        }

        // Vanilla `EnchantmentHelper.hasTag(tool, EnchantmentTags.PREVENTS_INFESTED_SPAWNS)`.
        let held = args.player.inventory().held_item();
        let prevents_spawn = Enchantment::from_name("silk_touch")
            .is_some_and(|silk_touch| held.get_enchantment_level(silk_touch) > 0);
        if prevents_spawn {
            return;
        }

        Self::spawn_infestation(args.world, args.position);
    }

    fn explode(&self, args: ExplodeArgs<'_>) {
        // Vanilla `BlockBehaviour.onExplosionHit` calls `spawnAfterBreak` with an empty
        // tool, so an explosion also releases the silverfish (gamerule permitting).
        if matches!(
            args.world.get_game_rule(&GameRule::BlockDrops),
            GameRuleValue::Bool(true)
        ) {
            Self::spawn_infestation(args.world, args.position);
        }
    }
}

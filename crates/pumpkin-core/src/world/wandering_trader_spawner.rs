//! Vanilla `WanderingTraderSpawner`: periodically spawns a Wandering Trader
//! near a random player, escorted by two leashed trader llamas.
//!
//! See `net.minecraft.world.entity.npc.wanderingtrader.WanderingTraderSpawner`.

use std::path::Path;
use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{Taggable, WorldgenBiome};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use pumpkin_world::world_info::data_files::{
    WanderingTraderData, read_wandering_trader, write_wandering_trader,
};
use rand::RngExt;
use rand::seq::IndexedRandom;
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::mob::Mob;
use crate::entity::r#type::from_type;
use crate::world::World;
use crate::world::natural_spawner::is_spawn_position_ok;

/// Ticks between each attempt to advance the countdown (vanilla `DEFAULT_TICK_DELAY`).
const DEFAULT_TICK_DELAY: i32 = 1200;
/// Delay restored after a failed spawn attempt (vanilla `DEFAULT_SPAWN_DELAY`).
const DEFAULT_SPAWN_DELAY: i32 = 24_000;
const MIN_SPAWN_CHANCE: i32 = 25;
const MAX_SPAWN_CHANCE: i32 = 75;
const SPAWN_CHANCE_INCREASE: i32 = 25;
/// The trader only actually spawns one out of this many successful chance rolls.
const SPAWN_ONE_IN_X_CHANCE: i32 = 10;
const NUMBER_OF_SPAWN_ATTEMPTS: i32 = 10;

/// Initial despawn delay applied to a freshly spawned trader.
const TRADER_SPAWN_DESPAWN_DELAY: i32 = 48_000;
/// Horizontal spawn radius around the reference position.
const SPAWN_RADIUS: i32 = 48;
/// Distance the trader is restricted to relative to its wander target.
const HOME_RADIUS: i32 = 16;
/// Radius in which escorted llamas are placed around the trader.
const LLAMA_SPAWN_RADIUS: i32 = 4;

/// Custom spawner running once per Overworld tick, mirroring vanilla's
/// `WanderingTraderSpawner` and persisting its countdown in
/// `<world>/data/minecraft/wandering_trader.dat`.
pub struct WanderingTraderSpawner {
    tick_delay: i32,
    spawn_delay: i32,
    spawn_chance: i32,
    data_version: i32,
    /// Set once the persisted state has been loaded from disk, so a brand new
    /// world is not read from disk before the first tick.
    loaded: bool,
}

impl Default for WanderingTraderSpawner {
    fn default() -> Self {
        Self::new()
    }
}

impl WanderingTraderSpawner {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tick_delay: DEFAULT_TICK_DELAY,
            spawn_delay: DEFAULT_SPAWN_DELAY,
            spawn_chance: MIN_SPAWN_CHANCE,
            data_version: 0,
            loaded: false,
        }
    }

    /// Called once per world tick. `spawn_wandering_traders` mirrors the vanilla
    /// `spawnWanderingTraders` game rule.
    pub fn tick(&mut self, world: &Arc<World>, spawn_wandering_traders: bool) {
        if !spawn_wandering_traders {
            return;
        }
        self.ensure_data(world);

        self.tick_delay -= 1;
        if self.tick_delay > 0 {
            return;
        }
        self.tick_delay = DEFAULT_TICK_DELAY;

        self.spawn_delay -= DEFAULT_TICK_DELAY;
        if self.spawn_delay > 0 {
            return;
        }

        self.spawn_delay = DEFAULT_SPAWN_DELAY;
        let chance_to_spawn = self.spawn_chance;
        self.spawn_chance =
            (chance_to_spawn + SPAWN_CHANCE_INCREASE).clamp(MIN_SPAWN_CHANCE, MAX_SPAWN_CHANCE);

        if rand::rng().random_range(1..=100) <= chance_to_spawn && Self::spawn(world) {
            self.spawn_chance = MIN_SPAWN_CHANCE;
        }
    }

    /// Loads the persisted countdown on the first tick (never on server startup,
    /// so world loading is not blocked by disk I/O).
    fn ensure_data(&mut self, world: &Arc<World>) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let data = read_wandering_trader(&world.level.level_folder.root_folder);
        self.spawn_delay = data.spawn_delay;
        self.spawn_chance = data.spawn_chance;
        self.data_version = data.data_version;
    }

    fn spawn(world: &Arc<World>) -> bool {
        let players = world.players.load();
        let Some(player) = players.choose(&mut rand::rng()) else {
            // Vanilla treats "no player" as a successful spawn so the chance resets.
            return true;
        };

        if rand::rng().random_range(0..SPAWN_ONE_IN_X_CHANCE) != 0 {
            return false;
        }

        let player_pos = player.get_entity().block_pos.load();
        // Vanilla prefers a village meeting (bell) POI within 48 blocks; Pumpkin does
        // not track meeting POIs yet, so the player position is always used.
        let reference_pos = player_pos;

        let Some(spawn_pos) = Self::find_spawn_position_near(world, &reference_pos, SPAWN_RADIUS)
        else {
            return false;
        };
        if !Self::has_enough_space(world, &spawn_pos) {
            return false;
        }
        if world
            .get_biome(&spawn_pos)
            .has_tag(&WorldgenBiome::MINECRAFT_WITHOUT_WANDERING_TRADER_SPAWNS)
        {
            return false;
        }

        let spawn_vec = Vector3::new(
            f64::from(spawn_pos.0.x) + 0.5,
            f64::from(spawn_pos.0.y),
            f64::from(spawn_pos.0.z) + 0.5,
        );
        let trader_entity = from_type(
            &EntityType::WANDERING_TRADER,
            spawn_vec,
            world,
            Uuid::new_v4(),
        );
        let Some(trader) = trader_entity
            .get_mob()
            .and_then(|mob| mob.as_wandering_trader())
        else {
            return false;
        };

        trader.set_despawn_delay(TRADER_SPAWN_DESPAWN_DELAY);
        trader.set_wander_target(Some(reference_pos));
        let mob = trader.get_mob_entity();
        mob.position_target.store(reference_pos);
        mob.position_target_range
            .store(HOME_RADIUS, std::sync::atomic::Ordering::Relaxed);

        if !world.spawn_entity(trader_entity.clone()) {
            return false;
        }

        for _ in 0..2 {
            Self::try_to_spawn_llama_for(world, &trader_entity, LLAMA_SPAWN_RADIUS);
        }

        true
    }

    fn try_to_spawn_llama_for(
        world: &Arc<World>,
        trader: &Arc<dyn crate::entity::EntityBase>,
        radius: i32,
    ) {
        let trader_pos = trader.get_entity().block_pos.load();
        let Some(spawn_pos) = Self::find_spawn_position_near(world, &trader_pos, radius) else {
            return;
        };
        let spawn_vec = Vector3::new(
            f64::from(spawn_pos.0.x) + 0.5,
            f64::from(spawn_pos.0.y),
            f64::from(spawn_pos.0.z) + 0.5,
        );
        let llama_entity = from_type(&EntityType::TRADER_LLAMA, spawn_vec, world, Uuid::new_v4());
        if world.spawn_entity(llama_entity.clone()) {
            llama_entity.get_entity().leash_to(trader.clone());
        }
    }

    fn find_spawn_position_near(
        world: &Arc<World>,
        reference: &BlockPos,
        radius: i32,
    ) -> Option<BlockPos> {
        let mut rng = rand::rng();
        for _ in 0..NUMBER_OF_SPAWN_ATTEMPTS {
            let x = reference.0.x + rng.random_range(-radius..radius);
            let z = reference.0.z + rng.random_range(-radius..radius);
            let y = world.get_heightmap_height(ChunkHeightmapType::MotionBlockingNoLeaves, x, z);
            let pos = BlockPos::new(x, y, z);
            if is_spawn_position_ok(world, &pos, &EntityType::WANDERING_TRADER) {
                return Some(pos);
            }
        }
        None
    }

    /// Vanilla `hasEnoughSpace`: the 2x3x2 box at and above the spawn block is empty.
    fn has_enough_space(world: &Arc<World>, spawn_pos: &BlockPos) -> bool {
        let min = Vector3::new(
            f64::from(spawn_pos.0.x),
            f64::from(spawn_pos.0.y),
            f64::from(spawn_pos.0.z),
        );
        let max = Vector3::new(min.x + 1.0, min.y + 2.0, min.z + 1.0);
        world.is_space_empty(BoundingBox::new(min, max))
    }

    /// Persists the countdown state. Called on world save.
    pub fn save(&self, level_folder: &Path) {
        if !self.loaded {
            return;
        }
        let data = WanderingTraderData {
            spawn_delay: self.spawn_delay,
            spawn_chance: self.spawn_chance,
            data_version: self.data_version,
        };
        if let Err(e) = write_wandering_trader(level_folder, &data) {
            tracing::error!("Failed to write wandering_trader.dat: {e}");
        }
    }
}

impl std::fmt::Debug for WanderingTraderSpawner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WanderingTraderSpawner")
            .field("tick_delay", &self.tick_delay)
            .field("spawn_delay", &self.spawn_delay)
            .field("spawn_chance", &self.spawn_chance)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_vanilla() {
        let spawner = WanderingTraderSpawner::new();
        assert_eq!(spawner.tick_delay, 1200);
        assert_eq!(spawner.spawn_delay, 24_000);
        assert_eq!(spawner.spawn_chance, 25);
        assert!(!spawner.loaded);
    }

    #[test]
    fn spawn_chance_is_clamped() {
        // Mirrors vanilla `Mth.clamp(chance + 25, 25, 75)`.
        assert_eq!(
            (25 + SPAWN_CHANCE_INCREASE).clamp(MIN_SPAWN_CHANCE, MAX_SPAWN_CHANCE),
            50
        );
        assert_eq!(
            (75 + SPAWN_CHANCE_INCREASE).clamp(MIN_SPAWN_CHANCE, MAX_SPAWN_CHANCE),
            75
        );
        assert_eq!(
            SPAWN_CHANCE_INCREASE.clamp(MIN_SPAWN_CHANCE, MAX_SPAWN_CHANCE),
            25
        );
    }
}

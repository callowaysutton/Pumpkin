use super::BlockEntity;
use crate::entity::EntityBase;
use crate::world::World;
use pumpkin_data::{Block, BlockDirection, BlockState, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::RandomGenerator;
use pumpkin_util::random::xoroshiro128::Xoroshiro;
use pumpkin_world::generation::feature::features::sculk::SculkLevel;
use pumpkin_world::generation::feature::features::sculk::spreader::{ChargeCursor, SculkSpreader};
use rand::{Rng, RngExt};
use std::sync::{Arc, Mutex};

/// The listener radius of a sculk catalyst, in blocks.
pub const LISTENER_RADIUS: i32 = 8;

/// Runtime [`SculkLevel`] view over a live [`World`].
///
/// Positions in unloaded chunks report `None`, so the spreader treats them
/// as solid obstacles. Vanilla loads the chunk on the read; doing that here
/// would run inside the block-entity tick on a Rayon worker, so unloaded
/// positions are left untouched instead.
struct RuntimeSculkLevel<'a> {
    world: &'a Arc<World>,
}

impl SculkLevel for RuntimeSculkLevel<'_> {
    fn sculk_get(&self, pos: BlockPos) -> Option<BlockStateId> {
        self.world.get_block_state_id_if_loaded(&pos)
    }

    fn sculk_set(&mut self, pos: BlockPos, state: &'static BlockState) {
        self.world
            .set_block_state(&pos, state.id, pumpkin_world::world::BlockFlags::NOTIFY_ALL);
    }

    fn sculk_is_air(&self, pos: BlockPos) -> bool {
        // Unloaded positions report `None` from `sculk_get` (treated as a
        // solid obstacle), so they must not report as air here either.
        self.world
            .get_block_state_id_if_loaded(&pos)
            .is_some_and(|id| BlockState::from_id(id).is_air())
    }

    fn sculk_is_water_source(&self, pos: BlockPos) -> bool {
        let (fluid, state) = self.world.get_fluid_and_fluid_state(&pos);
        fluid == &pumpkin_data::fluid::Fluid::WATER && state.is_source
    }

    fn sculk_is_water(&self, pos: BlockPos) -> bool {
        let (fluid, _) = self.world.get_fluid_and_fluid_state(&pos);
        fluid == &pumpkin_data::fluid::Fluid::WATER
    }

    fn sculk_is_face_sturdy(&self, pos: BlockPos, face: BlockDirection) -> bool {
        self.world
            .get_block_state_if_loaded(&pos)
            .is_some_and(|state| state.is_side_solid(face))
    }

    fn sculk_is_full_cube(&self, pos: BlockPos) -> bool {
        self.world
            .get_block_state_if_loaded(&pos)
            .is_some_and(BlockState::is_full_cube)
    }
}

pub struct SculkCatalystBlockEntity {
    pub position: BlockPos,
    pub sculk_spreader: Mutex<SculkSpreader>,
}

impl BlockEntity for SculkCatalystBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let mut spreader = SculkSpreader::new_level_spreader();
        spreader.load_cursors(
            nbt.get_list("cursors")
                .unwrap_or_default()
                .iter()
                .filter_map(charge_cursor_from_nbt),
        );
        Self {
            position,
            sculk_spreader: Mutex::new(spreader),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        if let Ok(spreader) = self.sculk_spreader.lock() {
            nbt.put_list(
                "cursors",
                spreader
                    .cursors()
                    .iter()
                    .map(charge_cursor_to_nbt)
                    .collect(),
            );
        }
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let spreader = self.sculk_spreader.try_lock().ok()?;
        let mut nbt = NbtCompound::new();
        nbt.put_list(
            "cursors",
            spreader
                .cursors()
                .iter()
                .map(charge_cursor_to_nbt)
                .collect(),
        );
        Some(nbt)
    }

    fn tick(&self, world: &Arc<World>) {
        let Ok(mut spreader) = self.sculk_spreader.try_lock() else {
            return;
        };
        if spreader.cursors().is_empty() {
            return;
        }
        let mut level = RuntimeSculkLevel { world };
        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::rng().next_u64()));
        spreader.update_cursors(&mut level, self.position, &mut random, true);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl SculkCatalystBlockEntity {
    pub const ID: &'static str = "minecraft:sculk_catalyst";

    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            sculk_spreader: Mutex::new(SculkSpreader::new_level_spreader()),
        }
    }

    /// Handles a nearby entity death, mirroring vanilla's catalyst
    /// `GameEventListener`.
    ///
    /// `charge` is the experience the mob would drop, and `source_pos` is the
    /// death position already lifted by 0.5 blocks (as vanilla does before
    /// flooring). The catalyst always claims the death (so the caller marks
    /// the mob as consumed and no other catalyst blooms), but only seeds the
    /// spreader when there is experience to absorb.
    ///
    /// Returns `true` when the death was consumed.
    pub fn handle_entity_death(
        &self,
        world: &Arc<World>,
        source_pos: BlockPos,
        charge: i32,
        mob: &Arc<dyn EntityBase>,
    ) -> bool {
        if charge > 0
            && let Ok(mut spreader) = self.sculk_spreader.lock()
        {
            spreader.add_cursors(source_pos, charge);
            drop(spreader);
            try_award_it_spreads_advancement(world, mob);
        }

        if let Some(server) = world.server.upgrade() {
            let mut event = crate::plugin::api::events::block::sculk_bloom::SculkBloomEvent::new(
                self.position,
                world.clone(),
                charge,
            );
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return true;
            }
        }

        self.bloom(world);
        true
    }

    /// Sets the `bloom` block state, schedules the reset tick, and plays the
    /// bloom sound and particles.
    fn bloom(&self, world: &Arc<World>) {
        let state = world.get_block_state(&self.position);
        let mut props =
            pumpkin_data::block_properties::SculkCatalystLikeProperties::from_state_id(state.id);
        props.bloom = true;
        world.set_block_state(
            &self.position,
            props.to_state_id(&Block::SCULK_CATALYST),
            pumpkin_world::world::BlockFlags::NOTIFY_ALL,
        );
        world.schedule_block_tick(
            &Block::SCULK_CATALYST,
            self.position,
            8,
            pumpkin_world::tick::TickPriority::Normal,
        );

        let pos = &self.position;
        world.spawn_particles(
            pumpkin_data::particle::Particle::SculkSoul,
            pumpkin_util::math::vector3::Vector3::new(
                f64::from(pos.0.x) + 0.5,
                f64::from(pos.0.y) + 1.15,
                f64::from(pos.0.z) + 0.5,
            ),
            2,
            pumpkin_util::math::vector3::Vector3::new(0.2, 0.0, 0.2),
            0.0,
        );
        let mut random = rand::rng();
        let pitch = 0.6 + random.random::<f32>() * 0.4;
        world.play_sound_fine(
            pumpkin_data::sound::Sound::BlockSculkCatalystBloom,
            pumpkin_data::sound::SoundCategory::Blocks,
            &pos.to_f64(),
            2.0,
            pitch,
        );
    }
}

/// Fires the `kill_mob_near_sculk_catalyst` advancement trigger for the
/// player that last hurt the mob.
fn try_award_it_spreads_advancement(world: &Arc<World>, mob: &Arc<dyn EntityBase>) {
    let Some(living) = mob.get_living_entity() else {
        return;
    };
    let attacker_id = living
        .last_hurt_by_player_id
        .load(std::sync::atomic::Ordering::Relaxed);
    let Some(player) = world.get_player_by_id(attacker_id) else {
        return;
    };
    player.trigger_advancement_criterion(
        pumpkin_data::advancement::Advancement::ADVENTURE_KILL_MOB_NEAR_SCULK_CATALYST,
        "kill_mob_near_sculk_catalyst",
    );
}

fn charge_cursor_to_nbt(cursor: &ChargeCursor) -> NbtTag {
    let mut compound = NbtCompound::new();
    compound.put(
        "pos",
        NbtTag::IntArray(vec![cursor.pos.0.x, cursor.pos.0.y, cursor.pos.0.z]),
    );
    compound.put_int("charge", i32::from(cursor.charge));
    compound.put_int("decay_delay", i32::from(cursor.decay_delay));
    compound.put_int("update_delay", i32::from(cursor.update_delay));
    if cursor.faces.is_some() {
        compound.put(
            "facings",
            NbtTag::List(
                cursor
                    .facing_directions()
                    .map(|dir| NbtTag::String(direction_name(dir).into()))
                    .collect(),
            ),
        );
    }
    NbtTag::Compound(compound)
}

fn charge_cursor_from_nbt(tag: &NbtTag) -> Option<ChargeCursor> {
    let compound = tag.extract_compound()?;
    let pos = compound.get_int_array("pos")?;
    let pos = BlockPos::new(*pos.first()?, *pos.get(1)?, *pos.get(2)?);
    let faces = compound.get_list("facings").map(|list| {
        list.iter()
            .filter_map(NbtTag::extract_string)
            .fold(0u8, |bits, name| {
                direction_from_name(name).map_or(bits, |dir| bits | (1 << dir.to_index()))
            })
    });
    Some(ChargeCursor {
        pos,
        charge: compound.get_int("charge").unwrap_or(0).clamp(0, 1000) as u16,
        decay_delay: compound.get_int("decay_delay").unwrap_or(1).clamp(0, 1) as u8,
        update_delay: compound.get_int("update_delay").unwrap_or(0).max(0) as u8,
        faces,
    })
}

const fn direction_name(dir: BlockDirection) -> &'static str {
    match dir {
        BlockDirection::Down => "down",
        BlockDirection::Up => "up",
        BlockDirection::North => "north",
        BlockDirection::South => "south",
        BlockDirection::West => "west",
        BlockDirection::East => "east",
    }
}

fn direction_from_name(name: &str) -> Option<BlockDirection> {
    match name {
        "down" => Some(BlockDirection::Down),
        "up" => Some(BlockDirection::Up),
        "north" => Some(BlockDirection::North),
        "south" => Some(BlockDirection::South),
        "west" => Some(BlockDirection::West),
        "east" => Some(BlockDirection::East),
        _ => None,
    }
}

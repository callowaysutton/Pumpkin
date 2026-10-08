use std::sync::{Arc, LazyLock};

use dashmap::DashMap;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::{RandomGenerator, RandomImpl, xoroshiro128::Xoroshiro};
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;
use uuid::Uuid;

use crate::block::{
    BlockBehaviour, CanPlaceAtArgs, GetStateForNeighborUpdateArgs, OnEntityCollisionArgs,
    OnScheduledTickArgs, PlacedArgs,
};
use crate::entity::r#type::from_type;
use crate::world::World;

/// Vanilla `FrogspawnBlock` hatch delay bounds, in ticks.
const MIN_HATCH_TICK_DELAY: i32 = 3600;
const MAX_HATCH_TICK_DELAY: i32 = 12000;

/// A scheduled tick only carries a `u8` delay, which is far shorter than the
/// vanilla multi-minute frogspawn hatch delay. The hatch is therefore driven by
/// a chain of wake-ups and the number of remaining wake-ups is tracked here,
/// keyed by world UUID and block position. Entries are removed once the spawn
/// hatches or is destroyed.
static HATCH_STEPS: LazyLock<DashMap<(Uuid, BlockPos), u32>> = LazyLock::new(DashMap::new);

#[pumpkin_block("minecraft:frogspawn")]
pub struct FrogspawnBlock;

impl FrogspawnBlock {
    /// Vanilla `FrogspawnBlock.mayPlaceOn`.
    fn may_place_on(world: &World, pos: &BlockPos) -> bool {
        let fluid = world.get_fluid(pos);
        let fluid_above = world.get_fluid(&pos.up());
        (fluid.has_tag(&tag::Fluid::MINECRAFT_SUPPORTS_FROGSPAWN)
            || world
                .get_block(pos)
                .has_tag(&tag::Block::MINECRAFT_SUPPORTS_FROGSPAWN))
            && *fluid_above == pumpkin_data::fluid::Fluid::EMPTY
    }

    fn can_survive(world: &World, pos: &BlockPos) -> bool {
        Self::may_place_on(world, &pos.down())
    }

    /// Vanilla `FrogspawnBlock.getFrogspawnHatchDelay`.
    fn get_frogspawn_hatch_delay(random: &mut RandomGenerator) -> i32 {
        random.next_inbetween_i32_exclusive(MIN_HATCH_TICK_DELAY, MAX_HATCH_TICK_DELAY)
    }

    /// Starts the wake-up chain for a block that needs to hatch `delay` ticks
    /// from now. Each wake-up waits [`u8::MAX`] ticks.
    fn schedule_hatch(world: &Arc<World>, pos: &BlockPos, delay: i32) {
        let steps = ((delay + u8::MAX as i32 - 1) / u8::MAX as i32).max(1) as u32;
        HATCH_STEPS.insert((world.uuid, *pos), steps);
        world.schedule_block_tick(&Block::FROGSPAWN, *pos, u8::MAX, TickPriority::Normal);
    }

    /// Vanilla `FrogspawnBlock.hatchFrogspawn`.
    fn hatch_frogspawn(world: &Arc<World>, pos: &BlockPos, random: &mut RandomGenerator) {
        Self::destroy_block(world, pos);
        world.play_sound(
            Sound::BlockFrogspawnHatch,
            SoundCategory::Blocks,
            &pos.to_f64(),
        );
        Self::spawn_tadpoles(world, pos, random);
    }

    /// Vanilla `FrogspawnBlock.destroyBlock`.
    fn destroy_block(world: &Arc<World>, pos: &BlockPos) {
        HATCH_STEPS.remove(&(world.uuid, *pos));
        world.break_block(pos, None, BlockFlags::SKIP_DROPS);
    }

    /// Vanilla `FrogspawnBlock.spawnTadpoles`.
    fn spawn_tadpoles(world: &Arc<World>, pos: &BlockPos, random: &mut RandomGenerator) {
        let tadpole_amount = random.next_inbetween_i32_exclusive(2, 6);

        for _ in 1..=tadpole_amount {
            let x = f64::from(pos.0.x) + Self::get_random_tadpole_position_offset(random);
            let z = f64::from(pos.0.z) + Self::get_random_tadpole_position_offset(random);
            let y_rot = random.next_inbetween_i32_exclusive(1, 361);
            let tadpole = from_type(
                &EntityType::TADPOLE,
                Vector3::new(x, f64::from(pos.0.y) - 0.5, z),
                world,
                Uuid::new_v4(),
            );
            tadpole.get_entity().set_rotation(y_rot as f32, 0.0);

            if let Some(mob) = tadpole.get_mob() {
                mob.get_mob_entity()
                    .persistence_required
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }

            world.spawn_entity(tadpole);
        }
    }

    /// Vanilla `FrogspawnBlock.getRandomTadpolePositionOffset`.
    fn get_random_tadpole_position_offset(random: &mut RandomGenerator) -> f64 {
        random.next_f64().clamp(0.2, 0.799_999_997_019_767_8)
    }
}

impl BlockBehaviour for FrogspawnBlock {
    fn placed(&self, args: PlacedArgs<'_>) {
        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::random::<u64>()));
        let delay = Self::get_frogspawn_hatch_delay(&mut random);
        Self::schedule_hatch(args.world, args.position, delay);
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        args.world
            .is_none_or(|world| Self::can_survive(world, args.position))
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        if Self::can_survive(args.world, args.position) {
            args.state_id
        } else {
            Block::AIR.default_state.id
        }
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        if !Self::can_survive(args.world, args.position) {
            Self::destroy_block(args.world, args.position);
            return;
        }

        let steps = HATCH_STEPS
            .get_mut(&(args.world.uuid, *args.position))
            .map_or(0, |mut entry| {
                *entry = entry.saturating_sub(1);
                *entry
            });

        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::random::<u64>()));
        if steps == 0 {
            Self::hatch_frogspawn(args.world, args.position, &mut random);
        } else {
            args.world.schedule_block_tick(
                &Block::FROGSPAWN,
                *args.position,
                u8::MAX,
                TickPriority::Normal,
            );
        }
    }

    fn on_entity_collision(&self, args: OnEntityCollisionArgs<'_>) {
        if args.entity.get_entity().entity_type.id == EntityType::FALLING_BLOCK.id {
            Self::destroy_block(args.world, args.position);
        }
    }
}

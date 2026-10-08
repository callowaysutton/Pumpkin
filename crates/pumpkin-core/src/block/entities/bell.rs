use crate::block::entities::BlockEntity;
use crate::entity::EntityBase;
use crate::entity::ai::brain::memory::types::HEARD_BELL_TIME;
use crate::world::World;
use crossbeam::atomic::AtomicCell;
use pumpkin_data::block_properties::HorizontalFacing;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use std::any::Any;
use std::sync::Arc;
use std::sync::Mutex;

/// Vanilla `SEARCH_RADIUS`, the radius cached by `updateEntities`.
const SEARCH_RADIUS: f64 = 48.0;
/// Vanilla `HEAR_BELL_RADIUS`.
const HEAR_BELL_RADIUS: f64 = 32.0;
/// Vanilla `HIGHLIGHT_RAIDERS_RADIUS`.
const HIGHLIGHT_RAIDERS_RADIUS: f64 = 48.0;
/// Vanilla `MIN_TICKS_BETWEEN_SEARCHES`.
const MIN_TICKS_BETWEEN_SEARCHES: i64 = 60;
/// Vanilla `MAX_RESONATION_TICKS`.
const MAX_RESONATION_TICKS: i32 = 40;

pub struct BellBlockEntity {
    pub position: BlockPos,
    pub last_side_hit: AtomicCell<Option<HorizontalFacing>>,
    pub ring_ticks: AtomicCell<i32>,
    pub ringing: AtomicCell<bool>,
    resonating: AtomicCell<bool>,
    resonate_time: AtomicCell<i32>,
    /// Game time of the last nearby-entity search, mirroring vanilla `lastRingTimestamp`.
    last_ring_timestamp: AtomicCell<i64>,
    /// Entities cached by the most recent ring, mirroring vanilla `nearbyEntities`.
    nearby_entities: Mutex<Option<Vec<Arc<dyn EntityBase>>>>,
}

impl BellBlockEntity {
    pub const ID: &'static str = "minecraft:bell";
    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            last_side_hit: AtomicCell::new(None),
            ring_ticks: AtomicCell::new(0),
            resonate_time: AtomicCell::new(0),
            resonating: AtomicCell::new(false),
            ringing: AtomicCell::new(false),
            last_ring_timestamp: AtomicCell::new(i64::MIN),
            nearby_entities: Mutex::new(None),
        }
    }

    /// Mirrors vanilla `BellBlockEntity.onHit`: resets ring/resonation state and
    /// refreshes the cached nearby entities.
    pub fn activate(&self, world: &Arc<World>, direction: HorizontalFacing) {
        self.last_side_hit.store(Some(direction));
        self.resonate_time.store(0);
        if self.ringing.load() {
            self.ring_ticks.store(0);
        } else {
            self.ringing.store(true);
        }
        self.update_entities(world);
    }

    /// Mirrors vanilla `BellBlockEntity.updateEntities`. Only refreshes the cached
    /// entity list every [`MIN_TICKS_BETWEEN_SEARCHES`] game ticks.
    fn update_entities(&self, world: &Arc<World>) {
        let game_time = world.get_world_age();
        if game_time > self.last_ring_timestamp.load() + MIN_TICKS_BETWEEN_SEARCHES {
            self.last_ring_timestamp.store(game_time);
            let aabb = BoundingBox::from_block(&self.position).expand(
                SEARCH_RADIUS,
                SEARCH_RADIUS,
                SEARCH_RADIUS,
            );
            *self
                .nearby_entities
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(world.get_entities_at_box(&aabb));
        }

        let center = self.position.to_centered_f64();
        // Snapshot the cache so no lock is held while mutating mob brains.
        let entities = self.nearby_entities_snapshot();
        for entity in &entities {
            let entity_base = entity.get_entity();
            if entity_base.is_alive()
                && entity_base.pos.load().squared_distance_to_vec(&center)
                    < HEAR_BELL_RADIUS * HEAR_BELL_RADIUS
                && let Some(mob) = entity.get_mob()
            {
                mob.get_mob_entity()
                    .brain
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set(HEARD_BELL_TIME, game_time);
            }
        }
    }

    /// Cheap snapshot of the cached nearby entities, avoids holding the cache lock
    /// while touching entity state.
    fn nearby_entities_snapshot(&self) -> Vec<Arc<dyn EntityBase>> {
        self.nearby_entities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map_or_else(Vec::new, Clone::clone)
    }

    /// Mirrors vanilla `BellBlockEntity.areRaidersNearby`.
    fn are_raiders_nearby(&self) -> bool {
        let center = self.position.to_centered_f64();
        self.nearby_entities_snapshot().iter().any(|entity| {
            let entity_base = entity.get_entity();
            entity_base.is_alive()
                && entity_base.pos.load().squared_distance_to_vec(&center)
                    < HEAR_BELL_RADIUS * HEAR_BELL_RADIUS
                && entity_base
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
        })
    }

    fn raiders_hear_bell(&self) -> bool {
        self.are_raiders_nearby()
    }

    /// Mirrors vanilla `BellBlockEntity.makeRaidersGlow`, the server resonation end action.
    fn make_raiders_glow(&self) {
        let center = self.position.to_centered_f64();
        let radius_squared = HIGHLIGHT_RAIDERS_RADIUS * HIGHLIGHT_RAIDERS_RADIUS;
        for entity in self.nearby_entities_snapshot() {
            let entity_base = entity.get_entity();
            if entity_base.is_alive()
                && entity_base.pos.load().squared_distance_to_vec(&center) < radius_squared
                && entity_base
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
                && let Some(living) = entity.get_living_entity()
            {
                living.add_effect(Effect {
                    effect_type: &StatusEffect::GLOWING,
                    duration: 60,
                    amplifier: 0,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                });
            }
        }
    }
}

impl BlockEntity for BellBlockEntity {
    fn write_nbt(&self, _nbt: &mut NbtCompound) {}

    fn from_nbt(_nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        Self::new(position)
    }

    fn tick(&self, world: &Arc<World>) {
        if self.ringing.load() {
            self.ring_ticks.fetch_add(1);
        }
        if self.ring_ticks.load() >= 50 {
            self.ringing.store(false);
            self.ring_ticks.store(0);
        }
        if self.ring_ticks.load() >= 5 && self.resonate_time.load() == 0 && self.raiders_hear_bell()
        {
            self.resonating.store(true);
            world.play_sound_fine(
                Sound::BlockBellResonate,
                SoundCategory::Blocks,
                &self.position.to_f64(),
                1.0,
                1.0,
            );
        }

        if self.resonating.load() {
            if self.resonate_time.load() < MAX_RESONATION_TICKS {
                self.resonate_time.fetch_add(1);
            } else {
                self.make_raiders_glow();
                self.resonating.store(false);
            }
        }
    }

    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

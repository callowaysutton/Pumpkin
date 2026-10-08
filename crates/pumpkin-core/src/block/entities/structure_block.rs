use super::BlockEntity;
use pumpkin_data::block_properties::{StructureBlockLikeProperties, StructureblockMode};
use pumpkin_data::{Block, BlockState, BlockStateId, Mirror, Rotation};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::identifier::Identifier;
use pumpkin_util::math::block_box::BlockBox;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::{RandomImpl, legacy_rand::LegacyRand};
use pumpkin_world::generation::structure::template::StructureTemplate;
use pumpkin_world::generation::structure::template::{
    BlockPlacer, PaletteEntry, get_template, global_cache,
};
use pumpkin_world::generation::structure::template::{
    BlockStateResolver, Palette, StructureBlockInfo, StructureEntityInfo, TemplateBlock,
};
use pumpkin_world::world::{BlockFlags, WorldPortalExt};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::world::World;

/// Vanilla `StructureMode#valueOf`, the name written under the `mode` NBT key
/// through the legacy enum codec.
#[must_use]
const fn mode_raw_value(mode: StructureblockMode) -> &'static str {
    match mode {
        StructureblockMode::Save => "SAVE",
        StructureblockMode::Load => "LOAD",
        StructureblockMode::Corner => "CORNER",
        StructureblockMode::Data => "DATA",
    }
}

/// Vanilla `StructureMode.LEGACY_CODEC`: unknown values fall back to `DATA`,
/// like vanilla's `loadAdditional` `orElse(StructureMode.DATA)`.
#[must_use]
fn mode_from_raw_value(value: &str) -> StructureblockMode {
    match value.to_uppercase().as_str() {
        "SAVE" => StructureblockMode::Save,
        "LOAD" => StructureblockMode::Load,
        "CORNER" => StructureblockMode::Corner,
        _ => StructureblockMode::Data,
    }
}

/// How far a SAVE structure block scans for CORNER blocks, vanilla
/// `StructureBlockEntity.SCAN_CORNER_BLOCKS_RANGE`.
const SCAN_CORNER_BLOCKS_RANGE: i32 = 80;
/// Vanilla `StructureBlockEntity.MAX_OFFSET_PER_AXIS`.
const MAX_OFFSET_PER_AXIS: i32 = 48;
/// Vanilla `StructureBlockEntity.MAX_SIZE_PER_AXIS`.
pub const MAX_SIZE_PER_AXIS: i32 = 48;
/// Vanilla `StructureBlockEntity.DEFAULT_POS`.
const DEFAULT_POS: Vector3<i32> = Vector3::new(0, 1, 0);

pub struct StructureBlockBlockEntity {
    pub position: BlockPos,
    pub name: Mutex<String>,
    pub author: Mutex<String>,
    pub metadata: Mutex<String>,
    pub pos_x: Mutex<i32>,
    pub pos_y: Mutex<i32>,
    pub pos_z: Mutex<i32>,
    pub size_x: Mutex<i32>,
    pub size_y: Mutex<i32>,
    pub size_z: Mutex<i32>,
    pub rotation: Mutex<String>,
    pub mirror: Mutex<String>,
    pub mode: Mutex<String>,
    pub ignore_entities: Mutex<bool>,
    pub strict: Mutex<bool>,
    pub powered: Mutex<bool>,
    pub show_air: Mutex<bool>,
    pub show_bounding_box: Mutex<bool>,
    pub integrity: Mutex<f32>,
    pub seed: Mutex<i64>,
}

impl BlockEntity for StructureBlockBlockEntity {
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
        let mode = nbt
            .get_string("mode")
            .map_or(StructureblockMode::Data, mode_from_raw_value);
        // Vanilla `loadAdditional` clamps both fields to their UI bounds, and
        // the relative position defaults to one block above the structure
        // block.
        let pos_x = nbt
            .get_int("posX")
            .unwrap_or(DEFAULT_POS.x)
            .clamp(-MAX_OFFSET_PER_AXIS, MAX_OFFSET_PER_AXIS);
        let pos_y = nbt
            .get_int("posY")
            .unwrap_or(DEFAULT_POS.y)
            .clamp(-MAX_OFFSET_PER_AXIS, MAX_OFFSET_PER_AXIS);
        let pos_z = nbt
            .get_int("posZ")
            .unwrap_or(DEFAULT_POS.z)
            .clamp(-MAX_OFFSET_PER_AXIS, MAX_OFFSET_PER_AXIS);
        let size_x = nbt
            .get_int("sizeX")
            .unwrap_or(0)
            .clamp(0, MAX_SIZE_PER_AXIS);
        let size_y = nbt
            .get_int("sizeY")
            .unwrap_or(0)
            .clamp(0, MAX_SIZE_PER_AXIS);
        let size_z = nbt
            .get_int("sizeZ")
            .unwrap_or(0)
            .clamp(0, MAX_SIZE_PER_AXIS);
        Self {
            position,
            name: Mutex::new(nbt.get_string("name").unwrap_or("").to_string()),
            author: Mutex::new(nbt.get_string("author").unwrap_or("").to_string()),
            metadata: Mutex::new(nbt.get_string("metadata").unwrap_or("").to_string()),
            pos_x: Mutex::new(pos_x),
            pos_y: Mutex::new(pos_y),
            pos_z: Mutex::new(pos_z),
            size_x: Mutex::new(size_x),
            size_y: Mutex::new(size_y),
            size_z: Mutex::new(size_z),
            rotation: Mutex::new(nbt.get_string("rotation").unwrap_or("NONE").to_string()),
            mirror: Mutex::new(nbt.get_string("mirror").unwrap_or("NONE").to_string()),
            mode: Mutex::new(mode_raw_value(mode).to_string()),
            ignore_entities: Mutex::new(nbt.get_bool("ignoreEntities").unwrap_or(true)),
            strict: Mutex::new(nbt.get_bool("strict").unwrap_or(false)),
            powered: Mutex::new(nbt.get_bool("powered").unwrap_or(false)),
            show_air: Mutex::new(
                nbt.get_bool("showair")
                    .or_else(|| nbt.get_bool("showAir"))
                    .unwrap_or(false),
            ),
            show_bounding_box: Mutex::new(
                nbt.get_bool("showboundingbox")
                    .or_else(|| nbt.get_bool("showBoundingBox"))
                    .unwrap_or(true),
            ),
            integrity: Mutex::new(nbt.get_float("integrity").unwrap_or(1.0)),
            seed: Mutex::new(nbt.get_long("seed").unwrap_or(0)),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        if let Ok(name) = self.name.lock() {
            nbt.put_string("name", name.clone());
        }
        if let Ok(author) = self.author.lock() {
            nbt.put_string("author", author.clone());
        }
        if let Ok(metadata) = self.metadata.lock() {
            nbt.put_string("metadata", metadata.clone());
        }
        if let Ok(pos_x) = self.pos_x.lock() {
            nbt.put_int("posX", *pos_x);
        }
        if let Ok(pos_y) = self.pos_y.lock() {
            nbt.put_int("posY", *pos_y);
        }
        if let Ok(pos_z) = self.pos_z.lock() {
            nbt.put_int("posZ", *pos_z);
        }
        if let Ok(size_x) = self.size_x.lock() {
            nbt.put_int("sizeX", *size_x);
        }
        if let Ok(size_y) = self.size_y.lock() {
            nbt.put_int("sizeY", *size_y);
        }
        if let Ok(size_z) = self.size_z.lock() {
            nbt.put_int("sizeZ", *size_z);
        }
        if let Ok(rotation) = self.rotation.lock() {
            nbt.put_string("rotation", rotation.clone());
        }
        if let Ok(mirror) = self.mirror.lock() {
            nbt.put_string("mirror", mirror.clone());
        }
        if let Ok(mode) = self.mode.lock() {
            nbt.put_string("mode", mode.clone());
        }
        if let Ok(ignore_entities) = self.ignore_entities.lock() {
            nbt.put_bool("ignoreEntities", *ignore_entities);
        }
        if let Ok(strict) = self.strict.lock() {
            nbt.put_bool("strict", *strict);
        }
        if let Ok(powered) = self.powered.lock() {
            nbt.put_bool("powered", *powered);
        }
        if let Ok(show_air) = self.show_air.lock() {
            nbt.put_bool("showair", *show_air);
        }
        if let Ok(show_bounding_box) = self.show_bounding_box.lock() {
            nbt.put_bool("showboundingbox", *show_bounding_box);
        }
        if let Ok(integrity) = self.integrity.lock() {
            nbt.put_float("integrity", *integrity);
        }
        if let Ok(seed) = self.seed.lock() {
            nbt.put_long("seed", *seed);
        }
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        nbt.put_string("name", self.name.try_lock().ok()?.clone());
        nbt.put_string("author", self.author.try_lock().ok()?.clone());
        nbt.put_string("metadata", self.metadata.try_lock().ok()?.clone());
        nbt.put_int("posX", *self.pos_x.try_lock().ok()?);
        nbt.put_int("posY", *self.pos_y.try_lock().ok()?);
        nbt.put_int("posZ", *self.pos_z.try_lock().ok()?);
        nbt.put_int("sizeX", *self.size_x.try_lock().ok()?);
        nbt.put_int("sizeY", *self.size_y.try_lock().ok()?);
        nbt.put_int("sizeZ", *self.size_z.try_lock().ok()?);
        nbt.put_string("rotation", self.rotation.try_lock().ok()?.clone());
        nbt.put_string("mirror", self.mirror.try_lock().ok()?.clone());
        nbt.put_string("mode", self.mode.try_lock().ok()?.clone());
        nbt.put_bool("ignoreEntities", *self.ignore_entities.try_lock().ok()?);
        nbt.put_bool("strict", *self.strict.try_lock().ok()?);
        nbt.put_bool("powered", *self.powered.try_lock().ok()?);
        nbt.put_bool("showair", *self.show_air.try_lock().ok()?);
        nbt.put_bool("showboundingbox", *self.show_bounding_box.try_lock().ok()?);
        nbt.put_float("integrity", *self.integrity.try_lock().ok()?);
        nbt.put_long("seed", *self.seed.try_lock().ok()?);
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl StructureBlockBlockEntity {
    pub const ID: &'static str = "minecraft:structure_block";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        // The vanilla entity takes its mode from the block state; a placed
        // structure block defaults to LOAD mode.
        Self::new_with_mode(position, StructureblockMode::Load)
    }

    #[must_use]
    pub fn new_with_mode(position: BlockPos, mode: StructureblockMode) -> Self {
        Self {
            position,
            name: Mutex::new(String::new()),
            author: Mutex::new(String::new()),
            metadata: Mutex::new(String::new()),
            pos_x: Mutex::new(DEFAULT_POS.x),
            pos_y: Mutex::new(DEFAULT_POS.y),
            pos_z: Mutex::new(DEFAULT_POS.z),
            size_x: Mutex::new(0),
            size_y: Mutex::new(0),
            size_z: Mutex::new(0),
            rotation: Mutex::new("NONE".to_string()),
            mirror: Mutex::new("NONE".to_string()),
            mode: Mutex::new(mode_raw_value(mode).to_string()),
            ignore_entities: Mutex::new(true),
            strict: Mutex::new(false),
            powered: Mutex::new(false),
            show_air: Mutex::new(false),
            show_bounding_box: Mutex::new(true),
            integrity: Mutex::new(1.0),
            seed: Mutex::new(0),
        }
    }

    // --- Field accessors, mirroring the vanilla getter/setter pairs ---

    #[must_use]
    pub fn get_structure_name(&self) -> String {
        self.name
            .try_lock()
            .map_or(String::new(), |guard| guard.clone())
    }

    #[must_use]
    pub fn has_structure_name(&self) -> bool {
        !self.get_structure_name().is_empty()
    }

    /// `StructureBlockEntity#setStructureName(String)`: an unparsable or empty
    /// name leaves the entity without one.
    pub fn set_structure_name(&self, structure_name: &str) {
        let valid = if structure_name.is_empty() {
            None
        } else {
            Identifier::parse(structure_name).ok()
        };
        *self
            .name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            valid.map_or_else(String::new, |id| id.to_string());
    }

    /// The parsed, namespaced structure name; `None` when blank or invalid.
    #[must_use]
    pub fn parse_structure_name(&self) -> Option<String> {
        let name = self.get_structure_name();
        if name.is_empty() {
            return None;
        }
        Identifier::parse(&name).ok().map(|id| id.to_string())
    }

    /// Sets the mode, mirroring `StructureBlockEntity#setMode`, which also
    /// writes the matching block state like its clients-facing `setBlock`.
    pub fn set_mode(&self, world: &Arc<World>, mode: StructureblockMode) {
        *self
            .mode
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = mode_raw_value(mode).to_string();

        let old_state_id = world.get_block_state_id(&self.position);
        let state_id = StructureBlockLikeProperties { r#mode }.to_state_id(&Block::STRUCTURE_BLOCK);
        if state_id != old_state_id {
            world.set_block_state(
                &self.position,
                state_id,
                BlockFlags::NOTIFY_ALL | BlockFlags::SKIP_BLOCK_ADDED_CALLBACK,
            );
        }
    }

    pub fn set_structure_pos(&self, structure_pos: Vector3<i32>) {
        let mut pos_x = self
            .pos_x
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut pos_y = self
            .pos_y
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut pos_z = self
            .pos_z
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *pos_x = structure_pos.x;
        *pos_y = structure_pos.y;
        *pos_z = structure_pos.z;
    }

    #[must_use]
    pub fn get_structure_pos(&self) -> Vector3<i32> {
        Vector3::new(
            self.pos_x.try_lock().map_or(0, |guard| *guard),
            self.pos_y.try_lock().map_or(0, |guard| *guard),
            self.pos_z.try_lock().map_or(0, |guard| *guard),
        )
    }

    pub fn set_structure_size(&self, structure_size: Vector3<i32>) {
        let mut size_x = self
            .size_x
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut size_y = self
            .size_y
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut size_z = self
            .size_z
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *size_x = structure_size.x;
        *size_y = structure_size.y;
        *size_z = structure_size.z;
    }

    #[must_use]
    pub fn get_structure_size(&self) -> Vector3<i32> {
        Vector3::new(
            self.size_x.try_lock().map_or(0, |guard| *guard),
            self.size_y.try_lock().map_or(0, |guard| *guard),
            self.size_z.try_lock().map_or(0, |guard| *guard),
        )
    }

    pub fn set_ignore_entities(&self, ignore_entities: bool) {
        *self
            .ignore_entities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ignore_entities;
    }

    #[must_use]
    pub fn is_ignore_entities(&self) -> bool {
        self.ignore_entities.try_lock().is_ok_and(|guard| *guard)
    }

    pub fn set_strict(&self, strict: bool) {
        *self
            .strict
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = strict;
    }

    #[must_use]
    pub fn is_strict(&self) -> bool {
        self.strict.try_lock().is_ok_and(|guard| *guard)
    }

    pub fn set_integrity(&self, integrity: f32) {
        *self
            .integrity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = integrity;
    }

    #[must_use]
    pub fn get_integrity(&self) -> f32 {
        self.integrity.try_lock().map_or(1.0, |guard| *guard)
    }

    pub fn set_seed(&self, seed: i64) {
        *self
            .seed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = seed;
    }

    #[must_use]
    pub fn get_seed(&self) -> i64 {
        self.seed.try_lock().map_or(0, |guard| *guard)
    }

    pub fn set_rotation(&self, rotation: Rotation) {
        let value = match rotation {
            Rotation::Clockwise90 => "CLOCKWISE_90",
            Rotation::Rotate180 => "CLOCKWISE_180",
            Rotation::CounterClockwise90 => "COUNTERCLOCKWISE_90",
            Rotation::None => "NONE",
        };
        *self
            .rotation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = value.to_string();
    }

    #[must_use]
    pub fn get_rotation(&self) -> Rotation {
        self.rotation
            .try_lock()
            .map_or(Rotation::None, |guard| match guard.as_str() {
                "CLOCKWISE_90" => Rotation::Clockwise90,
                "CLOCKWISE_180" => Rotation::Rotate180,
                "COUNTERCLOCKWISE_90" => Rotation::CounterClockwise90,
                _ => Rotation::None,
            })
    }

    pub fn set_mirror(&self, mirror: Mirror) {
        let value = match mirror {
            Mirror::LeftRight => "LEFT_RIGHT",
            Mirror::FrontBack => "FRONT_BACK",
            Mirror::None => "NONE",
        };
        *self
            .mirror
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = value.to_string();
    }

    #[must_use]
    pub fn get_mirror(&self) -> Mirror {
        self.mirror
            .try_lock()
            .map_or(Mirror::None, |guard| match guard.as_str() {
                "LEFT_RIGHT" => Mirror::LeftRight,
                "FRONT_BACK" => Mirror::FrontBack,
                _ => Mirror::None,
            })
    }

    #[must_use]
    pub fn get_mode_value(&self) -> StructureblockMode {
        self.mode
            .try_lock()
            .map_or(StructureblockMode::Data, |guard| {
                mode_from_raw_value(&guard)
            })
    }

    pub fn set_powered(&self, powered: bool) {
        *self
            .powered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = powered;
    }

    #[must_use]
    pub fn is_powered(&self) -> bool {
        self.powered.try_lock().is_ok_and(|guard| *guard)
    }

    pub fn set_show_air(&self, show_air: bool) {
        *self
            .show_air
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = show_air;
    }

    #[must_use]
    pub fn get_show_air(&self) -> bool {
        self.show_air.try_lock().is_ok_and(|guard| *guard)
    }

    pub fn set_show_bounding_box(&self, show_bounding_box: bool) {
        *self
            .show_bounding_box
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = show_bounding_box;
    }

    #[must_use]
    pub fn get_show_bounding_box(&self) -> bool {
        self.show_bounding_box
            .try_lock()
            .map_or(true, |guard| *guard)
    }

    pub fn set_metadata(&self, metadata: &str) {
        *self
            .metadata
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = metadata.to_string();
    }

    #[must_use]
    pub fn get_metadata(&self) -> String {
        self.metadata
            .try_lock()
            .map_or(String::new(), |guard| guard.clone())
    }

    /// `StructureBlockEntity#createdBy`: the placing entity signs its name in.
    pub fn created_by(&self, author: &str) {
        *self
            .author
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = author.to_string();
    }

    #[must_use]
    pub fn get_author(&self) -> String {
        self.author
            .try_lock()
            .map_or(String::new(), |guard| guard.clone())
    }

    // --- Structure actions, mirroring the vanilla StructureBlockEntity ---

    /// `StructureBlockEntity#isStructureLoadable`.
    #[must_use]
    pub fn is_structure_loadable(&self) -> bool {
        if self.get_mode_value() != StructureblockMode::Load {
            return false;
        }
        self.parse_structure_name()
            .and_then(|name| get_template(&name))
            .is_some()
    }

    /// `StructureBlockEntity#placeStructureIfSameSize`: places only while the
    /// template still matches the size the UI shows, otherwise refreshes the
    /// size info and reports "prepared".
    #[must_use]
    pub fn place_structure_if_same_size(&self, world: &Arc<World>) -> bool {
        if self.get_mode_value() != StructureblockMode::Load || !self.has_structure_name() {
            return false;
        }
        let Some(name) = self.parse_structure_name() else {
            return false;
        };
        let Some(template) = get_template(&name) else {
            return false;
        };
        if template.get_size() == self.get_structure_size() {
            self.place_structure_with(world, &template);
            true
        } else {
            self.load_structure_info(&template);
            false
        }
    }

    /// `StructureBlockEntity#placeStructure`, used by the redstone trigger:
    /// places without the size match check.
    pub fn place_structure(&self, world: &Arc<World>) {
        if let Some(name) = self.parse_structure_name()
            && let Some(template) = get_template(&name)
        {
            self.place_structure_with(world, &template);
        }
    }

    /// `StructureBlockEntity#loadStructureInfo`: refreshes the author and size
    /// from the template. The caller syncs the block entity, mirroring
    /// vanilla's `setChanged`.
    pub fn load_structure_info(&self, template: &StructureTemplate) {
        let author = template.get_author();
        *self
            .author
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = author.to_string();
        self.set_structure_size(template.get_size());
    }

    /// Vanilla `StructureBlockEntity#saveStructure(boolean)`.
    #[must_use]
    pub fn save_structure(&self, world: &Arc<World>, save_to_disk: bool) -> bool {
        let Some(name) = self.parse_structure_name() else {
            return false;
        };
        let pos = self.position.offset(self.get_structure_pos());
        save_structure(
            world,
            &name,
            &pos,
            self.get_structure_size(),
            self.is_ignore_entities(),
            &self.get_author(),
            save_to_disk,
        )
    }

    /// Vanilla `StructureBlockEntity#unloadStructure`: drops the template
    /// cache entry; the saved file stays on disk.
    pub fn unload_structure(&self) {
        if self.has_structure_name() {
            global_cache().remove(&self.get_structure_name());
        }
    }

    /// Vanilla `StructureBlockEntity#detectSize`: derives the saved region from
    /// CORNER blocks sharing this structure's name.
    #[must_use]
    pub fn detect_size(&self, world: &Arc<World>) -> bool {
        if self.get_mode_value() != StructureblockMode::Save {
            return false;
        }
        let Some(name) = self.parse_structure_name() else {
            return false;
        };
        let Some(bounding_box) = get_enclosing_bounding_box(world, &self.position, &name) else {
            return false;
        };
        let delta_x = bounding_box.max.x - bounding_box.min.x;
        let delta_y = bounding_box.max.y - bounding_box.min.y;
        let delta_z = bounding_box.max.z - bounding_box.min.z;
        if delta_x > 1 && delta_y > 1 && delta_z > 1 {
            self.set_structure_pos(Vector3::new(
                bounding_box.min.x - self.position.0.x + 1,
                bounding_box.min.y - self.position.0.y + 1,
                bounding_box.min.z - self.position.0.z + 1,
            ));
            self.set_structure_size(Vector3::new(delta_x - 1, delta_y - 1, delta_z - 1));
            true
        } else {
            false
        }
    }

    // --- Template placement, mirroring `StructureTemplate#placeInWorld` ---

    /// `StructureBlockEntity#placeStructure(ServerLevel, StructureTemplate)`:
    /// places the template with this entity's mirror, rotation, integrity and
    /// seed. Neighbor updates run like vanilla's post-placement pass; the
    /// `UPDATE_KNOWN_SHAPE` edge pass of vanilla's strict flag is left out.
    pub fn place_structure_with(&self, world: &Arc<World>, template: &StructureTemplate) {
        self.load_structure_info(template);

        let mirror = self.get_mirror();
        let rotation = self.get_rotation();
        let non_strict = !self.is_strict();
        let integrity = self.get_integrity();
        let seed = self.get_seed();

        // Vanilla builds two independent `createRandom(seed)` sources per
        // placement: the rot processor's settings random and the placement
        // random used for the loot table seeds.
        let mut integrity_random = (integrity < 1.0).then(|| create_random(seed));
        let mut seed_random = create_random(seed);

        let origin = self.position.offset(self.get_structure_pos());
        let Some(placed) = place_structure_template(
            world,
            template,
            origin,
            mirror,
            rotation,
            integrity_random.as_mut(),
            integrity,
            &mut seed_random,
        ) else {
            // Vanilla loads unloaded chunks inside `placeInWorld`, which
            // Pumpkin cannot do synchronously, so the placement is refused
            // instead of partially writing the template.
            tracing::warn!("Structure block load skipped: the target area has unloaded chunks");
            return;
        };

        // Vanilla runs `updateFromNeighbourShapes` and one `updateNeighborsAt`
        // per placed block only when the placement is not strict; strict
        // placements only touch the placed block entities.
        if non_strict {
            for position in &placed {
                let state_id = world.get_block_state_id(position);
                let new_state_id = world.update_from_neighbor_shapes(state_id, position);
                if new_state_id != state_id {
                    world.set_block_state(
                        position,
                        new_state_id,
                        BlockFlags::UPDATE_KNOWN_SHAPE | BlockFlags::SKIP_DROPS,
                    );
                }
                world.update_neighbors_at(position, Block::from_state_id(new_state_id), None);
            }
        }

        // Vanilla `placeEntities`: fresh entity positions from the template's
        // entity list when the placement is not ignoring entities.
        if self.is_ignore_entities() {
            return;
        }
        let nbts: Vec<NbtCompound> = template
            .entity_info_list
            .iter()
            .map(|entity| transform_entity_nbt(entity, &origin, mirror, rotation))
            .collect();
        if !nbts.is_empty() {
            crate::world::WorldPortal(world.clone()).spawn_structure_entities(nbts);
        }
    }
}

/// Vanilla `StructureBlockEntity.createRandom`: a fresh random per placement,
/// time-seeded when the configured seed is zero.
#[must_use]
fn create_random(seed: i64) -> LegacyRand {
    let seed = if seed == 0 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64)
    } else {
        u64::from_ne_bytes(seed.to_le_bytes())
    };
    LegacyRand::from_seed(seed)
}

/// Vanilla `StructureBlockEntity.saveStructure(ServerLevel, Identifier, ...)`:
/// captures the region into a template, registers it in the template cache
/// and, when `save_to_disk`, writes
/// `<world root>/generated/<namespace>/structure/<path>.nbt`.
#[must_use]
fn save_structure(
    world: &Arc<World>,
    name: &str,
    pos: &BlockPos,
    size: Vector3<i32>,
    ignore_entities: bool,
    author: &str,
    save_to_disk: bool,
) -> bool {
    let mut template = fill_from_world(world, pos, size, !ignore_entities);
    template.set_author(author.to_string());
    if !save_to_disk {
        // Vanilla `saveStructure(false)` only leaves the filled template in
        // the template repository.
        global_cache().store(name, template);
        return true;
    }
    let generated_dir = world.level.level_folder.root_folder.join("generated");
    match global_cache().save(name, &template, &generated_dir) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!("Failed to save structure template '{name}': {error}");
            false
        }
    }
}

/// Vanilla `StructureTemplate.fillFromWorld`: reads the region into a single
/// palette. Vanilla reads block states through `Level.findBlocksIn`, which
/// loads unloaded chunks; Pumpkin only reads loaded chunk columns and skips the
/// rest.
#[must_use]
#[allow(clippy::too_many_lines)]
fn fill_from_world(
    world: &Arc<World>,
    pos: &BlockPos,
    size: Vector3<i32>,
    include_entities: bool,
) -> StructureTemplate {
    let mut template = StructureTemplate::default();
    template.size = size;
    if size.x < 1 || size.y < 1 || size.z < 1 {
        return template;
    }

    let corner_2 = pos.0 + size - Vector3::new(1, 1, 1);
    let min = Vector3::new(
        pos.0.x.min(corner_2.x),
        pos.0.y.min(corner_2.y),
        pos.0.z.min(corner_2.z),
    );
    let max = Vector3::new(
        pos.0.x.max(corner_2.x),
        pos.0.y.max(corner_2.y),
        pos.0.z.max(corner_2.z),
    );

    let mut palette: Vec<PaletteEntry> = Vec::new();
    let mut palette_index: HashMap<BlockStateId, usize> = HashMap::new();
    let mut full_blocks: Vec<TemplateBlock> = Vec::new();
    let mut other_blocks: Vec<TemplateBlock> = Vec::new();
    let mut block_entities: Vec<TemplateBlock> = Vec::new();

    for x in min.x..=max.x {
        for z in min.z..=max.z {
            for y in min.y..=max.y {
                let position = BlockPos::new(x, y, z);
                let Some(state_id) = world.get_block_state_id_if_loaded(&position) else {
                    continue;
                };
                let block = Block::from_state_id(state_id);
                // Vanilla passes `Blocks.STRUCTURE_VOID` in the ignore list, so
                // structure void is never captured.
                if block.id == pumpkin_data::BlockId::STRUCTURE_VOID {
                    continue;
                }
                let nbt = world.get_block_entity(&position).map(|block_entity| {
                    let mut nbt = NbtCompound::new();
                    block_entity.write_internal(&mut nbt);
                    nbt
                });
                let entry = palette_index.entry(state_id).or_insert_with(|| {
                    let properties = block
                        .properties(state_id)
                        .map(|properties| {
                            properties
                                .to_props()
                                .iter()
                                .map(|(key, value)| (key.to_string(), value.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    palette.push(PaletteEntry::with_properties(
                        format!("minecraft:{}", block.name),
                        properties,
                    ));
                    palette.len() - 1
                });
                let info = TemplateBlock {
                    pos: position.0 - min,
                    state: *entry as u32,
                    nbt,
                };
                if info.nbt.is_some() {
                    block_entities.push(info);
                } else if BlockState::from_id(state_id).is_full_cube() {
                    full_blocks.push(info);
                } else {
                    other_blocks.push(info);
                }
            }
        }
    }

    // Vanilla sorts each bucket by y, x, z and concatenates full blocks,
    // non-full blocks and block entities in that order.
    for bucket in [&mut full_blocks, &mut other_blocks, &mut block_entities] {
        bucket.sort_by(|a, b| {
            a.pos
                .y
                .cmp(&b.pos.y)
                .then(a.pos.x.cmp(&b.pos.x))
                .then(a.pos.z.cmp(&b.pos.z))
        });
    }
    template.blocks = full_blocks
        .into_iter()
        .chain(other_blocks)
        .chain(block_entities)
        .collect();
    template.palette = palette;
    // The single palette mirrors the block list state by state, like vanilla
    // saves its one-palette templates.
    template.palettes = vec![Palette::new(
        template
            .blocks
            .iter()
            .map(|block| {
                let entry = template.palette[block.state as usize].clone();
                StructureBlockInfo::new(block.pos, entry, block.nbt.clone())
            })
            .collect(),
    )];

    // Vanilla `fillEntityList`: every non-player entity inside the box, saved
    // with positions relative to the min corner.
    if include_entities {
        let bounding_box = BoundingBox {
            min: min.to_f64(),
            max: max.to_f64() + Vector3::new(1.0, 1.0, 1.0),
        };
        for entity in world.get_entities_at_box(&bounding_box) {
            let base = entity.get_entity();
            if base.entity_type.id == pumpkin_data::entity::EntityType::PLAYER.id {
                continue;
            }
            let relative = base.pos.load()
                - Vector3::new(f64::from(min.x), f64::from(min.y), f64::from(min.z));
            let mut nbt = NbtCompound::new();
            entity.write_nbt(&mut nbt);
            template.entity_info_list.push(StructureEntityInfo::new(
                relative,
                Vector3::new(
                    relative.x.floor() as i32,
                    relative.y.floor() as i32,
                    relative.z.floor() as i32,
                ),
                nbt,
            ));
        }
    }
    template
}

/// Vanilla `StructureBlockEntity.getEnclosingBoundingBox` over the loaded block
/// entities: grows the box from CORNER blocks sharing the name and, with
/// exactly one corner, includes the SAVE block itself.
#[must_use]
fn get_enclosing_bounding_box(
    world: &Arc<World>,
    self_pos: &BlockPos,
    name: &str,
) -> Option<BlockBox> {
    let mut corner_count = 0;
    let mut min = BlockPos::new(0, 0, 0);
    let mut max = BlockPos::new(0, 0, 0);
    for chunk_entities in &world.block_entities {
        for (position, block_entity) in chunk_entities.value() {
            if (position.0.x - self_pos.0.x).abs() > SCAN_CORNER_BLOCKS_RANGE
                || (position.0.z - self_pos.0.z).abs() > SCAN_CORNER_BLOCKS_RANGE
            {
                continue;
            }
            let Some(structure_block) = block_entity
                .as_any()
                .downcast_ref::<StructureBlockBlockEntity>()
            else {
                continue;
            };
            if structure_block.get_mode_value() != StructureblockMode::Corner
                || structure_block.get_structure_name() != name
            {
                continue;
            }
            if corner_count == 0 {
                min = *position;
                max = *position;
            } else {
                min = BlockPos::min(min, *position);
                max = BlockPos::max(max, *position);
            }
            corner_count += 1;
        }
    }
    match corner_count {
        0 => None,
        1 => Some(BlockBox::new(
            min.0.x.min(self_pos.0.x),
            min.0.y.min(self_pos.0.y),
            min.0.z.min(self_pos.0.z),
            max.0.x.max(self_pos.0.x),
            max.0.y.max(self_pos.0.y),
            max.0.z.max(self_pos.0.z),
        )),
        _ => Some(BlockBox::new(
            min.0.x, min.0.y, min.0.z, max.0.x, max.0.y, max.0.z,
        )),
    }
}

/// Vanilla `StructureTemplate.transform` for block positions at the settings
/// pivot, which structure block placements leave at (0, 0): mirror, then
/// rotate.
#[must_use]
const fn transform_block_pos(
    pos: Vector3<i32>,
    mirror: Mirror,
    rotation: Rotation,
) -> Vector3<i32> {
    let (x, z) = match mirror {
        Mirror::LeftRight => (pos.x, -pos.z),
        Mirror::FrontBack => (-pos.x, pos.z),
        Mirror::None => (pos.x, pos.z),
    };
    match rotation {
        Rotation::Clockwise90 => Vector3::new(-z, pos.y, x),
        Rotation::Rotate180 => Vector3::new(-x, pos.y, -z),
        Rotation::CounterClockwise90 => Vector3::new(z, pos.y, -x),
        Rotation::None => Vector3::new(x, pos.y, z),
    }
}

/// Vanilla `StructureTemplate.transform` for floating point entity positions,
/// where the mirror lands at one minus the coordinate and the rotations keep
/// the center offset.
#[must_use]
const fn transform_entity_pos(
    pos: Vector3<f64>,
    mirror: Mirror,
    rotation: Rotation,
) -> Vector3<f64> {
    let (x, z) = match mirror {
        Mirror::LeftRight => (pos.x, 1.0 - pos.z),
        Mirror::FrontBack => (1.0 - pos.x, pos.z),
        Mirror::None => (pos.x, pos.z),
    };
    match rotation {
        Rotation::Clockwise90 => Vector3::new(1.0 - z, pos.y, x),
        Rotation::Rotate180 => Vector3::new(1.0 - x, pos.y, 1.0 - z),
        Rotation::CounterClockwise90 => Vector3::new(z, pos.y, 1.0 - x),
        Rotation::None => Vector3::new(x, pos.y, z),
    }
}

/// Vanilla `StructureTemplate#placeEntities` for one entity: transforms the
/// stored position and facing, drops the copied UUID so a fresh one is
/// generated on load, and refreshes `block_pos` for hanging entities.
#[must_use]
fn transform_entity_nbt(
    entity: &StructureEntityInfo,
    origin: &BlockPos,
    mirror: Mirror,
    rotation: Rotation,
) -> NbtCompound {
    let mut nbt = entity.nbt.clone();
    let transformed = transform_entity_pos(entity.pos, mirror, rotation);
    let world_pos = Vector3::new(
        transformed.x + f64::from(origin.0.x),
        transformed.y + f64::from(origin.0.y),
        transformed.z + f64::from(origin.0.z),
    );
    nbt.put(
        "Pos",
        pumpkin_nbt::tag::NbtTag::List(vec![
            pumpkin_nbt::tag::NbtTag::Double(world_pos.x),
            pumpkin_nbt::tag::NbtTag::Double(world_pos.y),
            pumpkin_nbt::tag::NbtTag::Double(world_pos.z),
        ]),
    );
    // Vanilla `yRot = entity.rotate(rotation) + entity.mirror(mirror) -
    // entity.getYRot()`: the saved facing turns with the placement.
    if let Some(rotation_nbt) = nbt.get_list("Rotation")
        && rotation_nbt.len() == 2
    {
        {
            let yaw = rotation_nbt[0].extract_float().unwrap_or_default();
            let pitch = rotation_nbt[1].extract_float().unwrap_or_default();
            let addend = match rotation {
                Rotation::None => 0.0,
                Rotation::Clockwise90 => 90.0,
                Rotation::Rotate180 => 180.0,
                Rotation::CounterClockwise90 => 270.0,
            };
            let mirrored = match mirror {
                Mirror::FrontBack => -yaw,
                Mirror::LeftRight => 180.0 - yaw,
                Mirror::None => yaw,
            };
            let mut y_rot = pumpkin_util::math::wrap_degrees(yaw + addend);
            y_rot += mirrored - yaw;
            nbt.put(
                "Rotation",
                pumpkin_nbt::tag::NbtTag::List(vec![y_rot.into(), pitch.into()]),
            );
        }
    }
    nbt.child_tags.remove("UUID");
    if let Some(block_pos) = nbt.child_tags.get_mut("block_pos") {
        let transformed_block_pos =
            transform_block_pos(entity.block_pos, mirror, rotation) + origin.0;
        *block_pos = pumpkin_nbt::tag::NbtTag::List(vec![
            pumpkin_nbt::tag::NbtTag::Int(transformed_block_pos.x),
            pumpkin_nbt::tag::NbtTag::Int(transformed_block_pos.y),
            pumpkin_nbt::tag::NbtTag::Int(transformed_block_pos.z),
        ]);
    }
    nbt
}

/// Vanilla `StructureTemplate#placeInWorld` block placement for the structure
/// block load: template block order, vanilla pivot geometry, the integrity rot
/// processor and the sequential loot table seeding.
///
/// Returns the placed positions for the caller's shape and neighbor update
/// passes, or `None` when a target chunk column is unloaded; vanilla loads
/// those chunks on demand, which Pumpkin must not do synchronously.
#[allow(clippy::too_many_arguments)]
fn place_structure_template(
    world: &Arc<World>,
    template: &StructureTemplate,
    origin: BlockPos,
    mirror: Mirror,
    rotation: Rotation,
    mut integrity_random: Option<&mut LegacyRand>,
    integrity: f32,
    seed_random: &mut LegacyRand,
) -> Option<Vec<BlockPos>> {
    // Resolve every transformed position up front so the loaded-chunk
    // preflight sees the full footprint before the first write.
    let mut transforms: Vec<(BlockPos, usize)> = Vec::with_capacity(template.blocks.len());
    let mut min = origin;
    let mut max = origin;
    for (index, block) in template.blocks.iter().enumerate() {
        if block.state as usize >= template.palette.len() {
            tracing::warn!("Template has a block with an out of range palette state");
            continue;
        }
        let transformed = transform_block_pos(block.pos, mirror, rotation);
        let position = origin.offset(transformed);
        min = BlockPos::min(min, position);
        max = BlockPos::max(max, position);
        transforms.push((position, index));
    }
    for chunk_x in block_pos_to_chunk(min.0.x)..=block_pos_to_chunk(max.0.x) {
        for chunk_z in block_pos_to_chunk(min.0.z)..=block_pos_to_chunk(max.0.z) {
            if !world
                .level
                .loaded_chunks
                .contains_key(&Vector2::new(chunk_x, chunk_z))
            {
                return None;
            }
        }
    }

    let mut placer = crate::world::block_placer::WorldBlockPlacer::new(world);
    let mut placed: Vec<BlockPos> = Vec::with_capacity(transforms.len());
    for (position, index) in transforms {
        let block = &template.blocks[index];
        // Vanilla `BlockRotProcessor`: blocks rot with probability
        // `1 - integrity`, decided on the processor's settings random.
        if let Some(integrity_random) = integrity_random.as_mut()
            && integrity_random.next_f32() > integrity
        {
            continue;
        }
        let entry = &template.palette[block.state as usize];
        let Some(state) = BlockStateResolver::resolve(entry, rotation, mirror) else {
            continue;
        };
        placer.set_block_state(&position.0, state);
        // Vanilla places a BARRIER first to detach any old block entity; the
        // block entity map insert replaces by position, which serves the same.
        if let Some(nbt) = &block.nbt {
            let mut placed_nbt = NbtCompound::new();
            for (key, value) in &nbt.child_tags {
                if key.as_ref() != "x" && key.as_ref() != "y" && key.as_ref() != "z" {
                    placed_nbt.child_tags.insert(key.clone(), value.clone());
                }
            }
            if placed_nbt.get_string("LootTable").is_some()
                && placed_nbt.get_long("LootTableSeed").is_none()
            {
                // Vanilla draws the seed from the placement random in block
                // order, so the same seed keeps the same container loot.
                placed_nbt.put_long("LootTableSeed", seed_random.next_i64());
            }
            placer.add_block_entity(placed_nbt);
        }
        placed.push(position);
    }
    placer.finalize();
    world.queue_block_updates(&placer.changed_positions);
    world.flush_block_updates();
    Some(placed)
}

/// `SectionPos.blockToSectionCoord`, the chunk coordinate holding a block.
#[must_use]
const fn block_pos_to_chunk(coordinate: i32) -> i32 {
    coordinate >> 4
}

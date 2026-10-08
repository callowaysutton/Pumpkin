use core::f32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::entity::item::ItemEntity;
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::server::Server;
use pumpkin_data::data_component_impl::CustomNameImpl;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::painting_variant::PaintingVariant;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockDirection, BlockState, damage::DamageType};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::version::JavaMinecraftVersion;

/// The world stores a painting facing as a horizontal value: 0 south, 1 west,
/// 2 north, 3 east (`Direction.get2DDataValue`). The entity data, and the spawn
/// packet built from it, use the 3D index instead: north 2, south 3, west 4,
/// east 5. Sending the raw horizontal value makes the client read 0 as down,
/// fail the horizontal check in `HangingEntity.setDirection` and disconnect.
const fn facing_from_horizontal(value: u8) -> BlockDirection {
    match value & 3 {
        1 => BlockDirection::West,
        2 => BlockDirection::North,
        3 => BlockDirection::East,
        _ => BlockDirection::South,
    }
}

/// Vanilla reads the field with `getByte`, which yields 0 (south) when it is
/// absent, so a partial entity file keeps a horizontal default.
fn facing_from_nbt(nbt: &NbtCompound) -> BlockDirection {
    facing_from_horizontal(nbt.get_byte("facing").unwrap_or(0) as u8)
}

const fn facing_to_horizontal(direction: BlockDirection) -> u8 {
    match direction {
        BlockDirection::West => 1,
        BlockDirection::North => 2,
        BlockDirection::East => 3,
        _ => 0,
    }
}

/// Vanilla `BlockAttachedEntity.CHECK_INTERVAL`: hanging entities check their
/// attachment only every hundred ticks; block changes never notify them directly.
const CHECK_INTERVAL: u32 = 100;

/// Vanilla tracks the facing in the 3D data value index and defaults to south.
/// Vertical indices would be rejected by `HangingEntity.setDirection`, so an
/// unset `/summon` painting (data 0, down) also reads as south.
const fn facing_from_data(index: i32) -> BlockDirection {
    match BlockDirection::from_index(index as u8) {
        Some(direction) if direction.is_horizontal() => direction,
        _ => BlockDirection::South,
    }
}

pub struct PaintingEntity {
    pub entity: Entity,
    variant_id: AtomicU32,
    /// Vanilla `BlockAttachedEntity.ticksSinceLastCheck`.
    ticks_since_last_check: AtomicU32,
}

impl PaintingEntity {
    /// Vanilla `Painting.DEPTH`: the thickness of the painting sliver.
    pub const DEPTH: f64 = 0.0625;

    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        Self::new_with_variant(entity, PaintingVariant::Alban)
    }

    #[must_use]
    pub const fn new_with_variant(entity: Entity, variant: PaintingVariant) -> Self {
        Self {
            entity,
            variant_id: AtomicU32::new(variant.id()),
            ticks_since_last_check: AtomicU32::new(0),
        }
    }

    #[must_use]
    pub fn variant(&self) -> PaintingVariant {
        let id = self.variant_id.load(Ordering::Relaxed);
        PaintingVariant::all()
            .get(id as usize)
            .copied()
            .unwrap_or(PaintingVariant::Alban)
    }

    pub fn set_variant(&self, variant: PaintingVariant) {
        self.variant_id.store(variant.id(), Ordering::Relaxed);
        self.sync_variant();
    }

    pub fn sync_variant(&self) {
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::painting::DATA_PAINTING_VARIANT_ID,
            VarInt(self.variant().id() as i32),
        );
    }

    /// Calculates the exact floating-point center position for a painting of given width and height (in blocks),
    /// placed on `face` of `location`.
    #[must_use]
    pub fn calculate_center_pos(
        location: BlockPos,
        face: BlockDirection,
        width: u32,
        height: u32,
    ) -> Vector3<f64> {
        let target_pos = location.offset(face.to_offset());
        let face_ccw = face.rotate_counter_clockwise();
        let step_x = f64::from(face.to_offset().x);
        let step_z = f64::from(face.to_offset().z);
        let ccw_step_x = f64::from(face_ccw.to_offset().x);
        let ccw_step_z = f64::from(face_ccw.to_offset().z);

        let mut x = f64::from(target_pos.0.x) + 0.5 - step_x * 0.46875;
        let mut y = f64::from(target_pos.0.y) + 0.5;
        let mut z = f64::from(target_pos.0.z) + 0.5 - step_z * 0.46875;

        let width_offset = if width.is_multiple_of(2) { 0.5 } else { 0.0 };
        let height_offset = if height.is_multiple_of(2) { 0.5 } else { 0.0 };

        x += width_offset * ccw_step_x;
        z += width_offset * ccw_step_z;
        y += height_offset;

        Vector3::new(x, y, z)
    }

    /// Checks if a painting of the specified variant fits on the wall at `location` facing `face`.
    /// The wall blocks behind the painting must be solid, and the blocks in front must not be solid.
    #[must_use]
    pub fn painting_fits(
        world: &crate::world::World,
        location: BlockPos,
        face: BlockDirection,
        variant: PaintingVariant,
    ) -> bool {
        let width = variant.width();
        let height = variant.height();
        let face_ccw = face.rotate_counter_clockwise();
        let ccw_offset = face_ccw.to_offset();
        let face_offset = face.to_offset();

        let k = -((width as i32 - 1) / 2);
        let l = -((height as i32 - 1) / 2);

        for x_idx in 0..width {
            for y_idx in 0..height {
                let wall_pos = BlockPos(Vector3::new(
                    location.0.x + ccw_offset.x * (x_idx as i32 + k),
                    location.0.y + (y_idx as i32 + l),
                    location.0.z + ccw_offset.z * (x_idx as i32 + k),
                ));
                let front_pos = wall_pos.offset(face_offset);

                let wall_state = world.get_block_state(&wall_pos);
                if !wall_state.is_solid() {
                    return false;
                }

                let front_state = world.get_block_state(&front_pos);
                if front_state.is_solid() {
                    return false;
                }
            }
        }
        true
    }

    /// Chooses a placeable painting variant that fits the wall at `location` facing `face`.
    /// Matches vanilla Minecraft: filters to variants that fit, selects the subset with
    /// the largest area (width * height), and randomly picks one from that subset.
    #[must_use]
    pub fn choose_variant(
        world: &crate::world::World,
        location: BlockPos,
        face: BlockDirection,
    ) -> Option<PaintingVariant> {
        use rand::seq::IndexedRandom;

        let mut fitting_variants = Vec::new();
        let mut max_area = 0;

        for &variant in PaintingVariant::all_placeable() {
            if Self::painting_fits(world, location, face, variant) {
                let area = variant.width() * variant.height();
                if area > max_area {
                    max_area = area;
                    fitting_variants.clear();
                    fitting_variants.push(variant);
                } else if area == max_area {
                    fitting_variants.push(variant);
                }
            }
        }

        let mut rng = rand::rng();
        fitting_variants.choose(&mut rng).copied()
    }

    /// Vanilla keeps the facing in the 3D data value index; see `facing_from_data`.
    fn facing(&self) -> BlockDirection {
        facing_from_data(self.entity.data.load(Ordering::Relaxed))
    }

    /// Vanilla `Painting.dropItem`: gated by the entity drops game rule, plays the
    /// break sound and, unless a creative player caused the break, spawns the
    /// painting item slightly in front of the painting.
    fn drop_item(&self, caused_by: Option<&dyn EntityBase>) {
        let entity = &self.entity;
        let world = entity.world.load();
        if !world.level_info.load().game_rules.entity_drops {
            return;
        }
        world.play_sound(
            Sound::EntityPaintingBreak,
            SoundCategory::Blocks,
            &entity.pos.load(),
        );

        let is_creative = caused_by.is_some_and(|c| {
            c.cast_any()
                .downcast_ref::<Player>()
                .is_some_and(Player::is_creative)
        });
        if is_creative {
            return;
        }

        // A custom-named painting drops a custom-named item.
        let mut stack = ItemStack::new(1, &Item::PAINTING);
        if let Some(name) = entity.custom_name.load().as_ref() {
            stack.set_data_component(CustomNameImpl { name: name.clone() });
        }

        // Vanilla `HangingEntity.spawnAtLocation`: the item spawns 0.15 blocks in
        // front of the painting along its facing.
        let pos = entity.pos.load();
        let step = self.facing().to_offset();
        let spawn_pos = Vector3::new(
            pos.x + f64::from(step.x) * 0.15,
            pos.y,
            pos.z + f64::from(step.z) * 0.15,
        );
        world.spawn_entity(Arc::new(ItemEntity::new(
            Entity::new(world.clone(), spawn_pos, &EntityType::ITEM),
            stack,
        )));
    }

    /// Vanilla `Painting.calculateBoundingBox`: the painting box is centred on the
    /// painting's position, thin as [`Self::DEPTH`] along the facing and as wide
    /// as the variant elsewhere.
    #[must_use]
    pub fn calculate_bounding_box(
        center: Vector3<f64>,
        facing: BlockDirection,
        variant: PaintingVariant,
    ) -> BoundingBox {
        let width = f64::from(variant.width());
        let height = f64::from(variant.height());
        let (x_size, z_size) = if facing.to_offset().x != 0 {
            (Self::DEPTH, width)
        } else {
            (width, Self::DEPTH)
        };
        let (x_half, y_half, z_half) = (x_size / 2.0, height / 2.0, z_size / 2.0);
        BoundingBox {
            min: Vector3::new(center.x - x_half, center.y - y_half, center.z - z_half),
            max: Vector3::new(center.x + x_half, center.y + y_half, center.z + z_half),
        }
    }

    /// Vanilla `HangingEntity.isSupportingBlock`: solid blocks and diodes support
    /// hanging entities.
    fn is_supporting_block(state: &BlockState) -> bool {
        let block = Block::from_state_id(state.id);
        state.is_solid() || block == &Block::REPEATER || block == &Block::COMPARATOR
    }

    /// Vanilla `HangingEntity.survives`: the pop box must be free of block
    /// collisions, every wall block in the support box must provide support, and no
    /// other hanging entity of the same type or facing may sit in the pop box.
    fn survives(&self) -> bool {
        let entity = &self.entity;
        let world = entity.world.load();
        let facing = self.facing();
        let pop_box = Self::calculate_bounding_box(entity.pos.load(), facing, self.variant());

        // Vanilla `HangingEntity.hasLevelCollision(getPopBox())`: a solid front
        // block collides with the painting box and pops it.
        if !world.is_space_empty(pop_box) {
            return false;
        }

        // Vanilla `HangingEntity.calculateSupportBox`: the pop box pushed half a
        // block behind the painting, deflated so face contact with neighbouring
        // blocks never counts as support.
        let step = facing.to_offset();
        let shifted = pop_box.shift(Vector3::new(
            f64::from(step.x) * -0.5,
            f64::from(step.y) * -0.5,
            f64::from(step.z) * -0.5,
        ));
        let deflate = 1.0E-7;
        let support_box = BoundingBox {
            min: Vector3::new(
                shifted.min.x + deflate,
                shifted.min.y + deflate,
                shifted.min.z + deflate,
            ),
            max: Vector3::new(
                shifted.max.x - deflate,
                shifted.max.y - deflate,
                shifted.max.z - deflate,
            ),
        };
        for pos in BlockPos::iterate(support_box.min_block_pos(), support_box.max_block_pos()) {
            if !Self::is_supporting_block(world.get_block_state(&pos)) {
                return false;
            }
        }

        // Vanilla `HangingEntity.canCoexist`: an overlapping hanging entity blocks
        // survival when it is of the same type or faces the same way.
        let others = world.get_entities_at_box(&pop_box);
        for other in &others {
            let other_entity = other.get_entity();
            if other_entity.entity_id == entity.entity_id {
                continue;
            }
            let id = other_entity.entity_type.id;
            let is_hanging = id == EntityType::PAINTING.id
                || id == EntityType::ITEM_FRAME.id
                || id == EntityType::GLOW_ITEM_FRAME.id;
            if !is_hanging {
                continue;
            }
            let same_type = id == entity.entity_type.id;
            let same_facing = facing_from_data(other_entity.data.load(Ordering::Relaxed)) == facing;
            if other_entity.bounding_box.load().intersects(&pop_box) && (same_type || same_facing) {
                return false;
            }
        }
        true
    }
}

impl EntityBase for PaintingEntity {
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_byte("facing", facing_to_horizontal(self.facing()) as i8);
        nbt.put_string("variant", self.variant().asset_id().to_string());
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        let facing = facing_from_nbt(nbt);
        self.entity
            .data
            .store(i32::from(facing.to_index()), Ordering::Relaxed);
        if let Some(variant_str) = nbt
            .get_string("variant")
            .or_else(|| nbt.get_string("Motive"))
            && let Some(variant) = PaintingVariant::from_name(variant_str)
        {
            self.set_variant(variant);
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn init_data_tracker(&self) {
        self.sync_variant();
    }

    /// Vanilla `BlockAttachedEntity.tick`: paintings discard and drop when they
    /// no longer survive, checked every `CHECK_INTERVAL` ticks. The base entity
    /// tick is skipped, as it is for block-attached entities in vanilla.
    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        // Vanilla `BlockAttachedEntity.checkBelowWorld`.
        self.entity.check_out_of_world(caller);
        if !self.entity.is_alive() {
            return;
        }
        if self.ticks_since_last_check.fetch_add(1, Ordering::Relaxed) + 1 < CHECK_INTERVAL {
            return;
        }
        self.ticks_since_last_check.store(0, Ordering::Relaxed);
        if !self.survives() {
            self.entity.remove();
            self.drop_item(None);
        }
    }

    /// Vanilla `BlockAttachedEntity.thunderHit`: block-attached entities ignore
    /// lightning.
    fn on_lightning_strike(
        &self,
        _caller: &dyn EntityBase,
        _lightning: &crate::entity::lightning::LightningBoltEntity,
    ) {
    }

    fn java_spawn_metadata(&self, version: JavaMinecraftVersion) -> Option<Box<[u8]>> {
        let mut metadata = Vec::new();
        Metadata::new(
            pumpkin_data::tracked_data::painting::DATA_PAINTING_VARIANT_ID,
            VarInt(self.variant().id() as i32),
        )
        .write(&mut metadata, &version)
        .ok()?;
        metadata.push(255);
        Some(metadata.into_boxed_slice())
    }

    fn set_variant_name(&self, name: &str) {
        if let Some(variant) = PaintingVariant::from_name(name) {
            self.set_variant(variant);
        }
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let entity = &self.entity;
        // Vanilla `BlockAttachedEntity.hurtServer`: invulnerable entities (or the
        // `bypasses_invulnerability` tag) resolve before anything else. The
        // attacker splits into `source` and `cause` depending on the damage path
        // (player attacks pass both, arrows only pass the owner as `source`).
        let attacker = cause.or(source);
        if entity.is_invulnerable_to(&damage_type, attacker) {
            return false;
        }
        // Vanilla: mobs can only pop block-attached entities when mob griefing is
        // enabled.
        if !entity
            .world
            .load()
            .level_info
            .load()
            .game_rules
            .mob_griefing
            && attacker.is_some_and(|cause| cause.get_mob().is_some())
        {
            return false;
        }
        if !entity.is_alive() {
            return true;
        }
        // Vanilla order: the entity dies first, then `dropItem` plays the sound
        // and spawns the item.
        entity.remove();
        self.drop_item(attacker);
        true
    }

    fn can_hit(&self) -> bool {
        self.entity.is_alive()
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_and_3d_facings_round_trip() {
        for horizontal in 0u8..4 {
            let direction = facing_from_horizontal(horizontal);
            assert!(direction.is_horizontal(), "{direction:?} is vertical");
            assert_eq!(facing_to_horizontal(direction), horizontal);
        }
    }

    #[test]
    fn south_two_is_not_down_zero() {
        // The old bug: south (0 in the world file) was sent as index 0, which the
        // client reads as down and rejects.
        assert_eq!(facing_from_horizontal(0), BlockDirection::South);
        assert_eq!(facing_from_horizontal(0).to_index(), 3);
    }

    #[test]
    fn missing_facing_defaults_to_south() {
        let nbt = NbtCompound::new();
        assert_eq!(facing_from_nbt(&nbt), BlockDirection::South);
        assert_eq!(facing_from_nbt(&nbt).to_index(), 3);

        let mut with_facing = NbtCompound::new();
        with_facing.put_byte("facing", 1);
        assert_eq!(facing_from_nbt(&with_facing), BlockDirection::West);
    }

    #[test]
    fn out_of_range_values_wrap_into_horizontal_directions() {
        assert!(facing_from_horizontal(4).is_horizontal());
        assert!(facing_from_horizontal(255).is_horizontal());
    }

    #[test]
    fn unset_or_vertical_data_defaults_to_south() {
        // An unset tracking field (`/summon`) reads 0, which would be down; vanilla
        // `DEFAULT_DIRECTION` is south and vertical facings are rejected.
        assert_eq!(facing_from_data(0), BlockDirection::South);
        assert_eq!(facing_from_data(1), BlockDirection::South);
        assert_eq!(facing_from_data(2), BlockDirection::North);
        assert_eq!(facing_from_data(3), BlockDirection::South);
        assert_eq!(facing_from_data(4), BlockDirection::West);
        assert_eq!(facing_from_data(5), BlockDirection::East);
    }

    #[test]
    fn calculate_bounding_box_sized_by_variant_and_facing() {
        // Vanilla `Painting.calculateBoundingBox` is `AABB.ofSize` centred on the
        // painting's position: `PAINTING_DEPTH` thin along the facing and matching
        // the variant elsewhere. A 1x1 painting facing north off the wall block at
        // (10, 64, 20) has its box inside the front block, touching the wall face
        // at z = 20.
        let center = Vector3::new(10.5, 64.5, 19.96875);
        let north = PaintingEntity::calculate_bounding_box(
            center,
            BlockDirection::North,
            PaintingVariant::Kebab,
        );
        assert_eq!(north.min.x, 10.0);
        assert_eq!(north.max.x, 11.0);
        assert_eq!(north.min.y, 64.0);
        assert_eq!(north.max.y, 65.0);
        assert_eq!(north.min.z, 19.9375);
        assert_eq!(north.max.z, 20.0);

        let west = PaintingEntity::calculate_bounding_box(
            center,
            BlockDirection::West,
            PaintingVariant::Kebab,
        );
        assert_eq!(west.min.x, 10.46875);
        assert_eq!(west.max.x, 10.53125);
        assert_eq!(west.min.z, 19.46875);
        assert_eq!(west.max.z, 20.46875);

        // Any 2x2 variant spans a whole extra block along both width and height.
        let square = PaintingVariant::all()
            .iter()
            .find(|variant| variant.width() == 2 && variant.height() == 2)
            .copied()
            .unwrap_or(PaintingVariant::Alban);
        let south = PaintingEntity::calculate_bounding_box(
            Vector3::new(10.5, 64.5, 21.03125),
            BlockDirection::South,
            square,
        );
        assert_eq!(south.min.x, 9.5);
        assert_eq!(south.max.x, 11.5);
        assert_eq!(south.min.y, 63.5);
        assert_eq!(south.max.y, 65.5);
        assert_eq!(south.min.z, 21.0);
        assert_eq!(south.max.z, 21.0625);
    }

    #[test]
    fn calculate_center_pos_works() {
        let location = BlockPos(Vector3::new(10, 64, 20));

        // 1x1 painting facing North: placed against block at (10, 64, 20)
        let pos_1x1 = PaintingEntity::calculate_center_pos(location, BlockDirection::North, 1, 1);
        assert_eq!(pos_1x1.x, 10.5);
        assert_eq!(pos_1x1.y, 64.5);
        assert!((pos_1x1.z - 19.96875).abs() < 1e-6);

        // 2x1 painting facing North: width is 2, shifts along counter-clockwise (West = -X)
        let pos_2x1 = PaintingEntity::calculate_center_pos(location, BlockDirection::North, 2, 1);
        assert_eq!(pos_2x1.x, 10.0);
        assert_eq!(pos_2x1.y, 64.5);
        assert!((pos_2x1.z - 19.96875).abs() < 1e-6);

        // 2x2 painting facing North: height is 2, shifts +0.5 along Y
        let pos_2x2 = PaintingEntity::calculate_center_pos(location, BlockDirection::North, 2, 2);
        assert_eq!(pos_2x2.x, 10.0);
        assert_eq!(pos_2x2.y, 65.0);
        assert!((pos_2x2.z - 19.96875).abs() < 1e-6);

        // 4x4 painting facing North
        let pos_4x4 = PaintingEntity::calculate_center_pos(location, BlockDirection::North, 4, 4);
        assert_eq!(pos_4x4.x, 10.0);
        assert_eq!(pos_4x4.y, 65.0);
        assert!((pos_4x4.z - 19.96875).abs() < 1e-6);

        // 1x1 painting facing South: placed on south face of (10, 64, 20) -> target is (10, 64, 21)
        let pos_south = PaintingEntity::calculate_center_pos(location, BlockDirection::South, 1, 1);
        assert_eq!(pos_south.x, 10.5);
        assert_eq!(pos_south.y, 64.5);
        assert!((pos_south.z - 21.03125).abs() < 1e-6);
    }

    #[test]
    fn variant_resolution() {
        assert_eq!(
            PaintingVariant::from_name("minecraft:kebab"),
            Some(PaintingVariant::Kebab)
        );
        assert_eq!(
            PaintingVariant::from_name("kebab"),
            Some(PaintingVariant::Kebab)
        );
        assert_eq!(PaintingVariant::from_name("invalid"), None);
        assert_eq!(PaintingVariant::Kebab.asset_id(), "minecraft:kebab");
    }
}

#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_macros::translate_cross;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;

use crate::block::entities::structure_block::StructureBlockBlockEntity;
use pumpkin_data::Mirror;
use pumpkin_data::block_properties::StructureblockMode;
use pumpkin_protocol::java::server::play::SSetStructureBlock;

impl JavaClient {
    /// Vanilla `ServerGamePacketListener#handleSetStructureBlock`: applies the
    /// GUI settings and runs the requested update action.
    #[allow(clippy::too_many_lines)]
    pub fn handle_set_structure_block(&self, player: &Player, packet: &SSetStructureBlock<'_>) {
        // Vanilla `canUseGameMasterBlocks`: instabuild plus gamemaster perms.
        if player.permission_lvl.load() < PermissionLvl::Two
            || player.gamemode.load() != GameMode::Creative
        {
            return;
        }
        if packet.action.0 > SSetStructureBlock::ACTION_SCAN_AREA {
            debug!(
                "Player used structure block with invalid action {}",
                packet.action.0
            );
            return;
        }

        let world = player.world();
        let Some(block_entity) = world.get_block_entity(&packet.location) else {
            return;
        };
        let Some(structure_block) = block_entity
            .as_any()
            .downcast_ref::<StructureBlockBlockEntity>()
        else {
            return;
        };

        // The packet enum order matches vanilla's modes.
        let mode = match packet.mode.0 {
            SSetStructureBlock::MODE_SAVE => Some(StructureblockMode::Save),
            SSetStructureBlock::MODE_LOAD => Some(StructureblockMode::Load),
            SSetStructureBlock::MODE_CORNER => Some(StructureblockMode::Corner),
            SSetStructureBlock::MODE_DATA => Some(StructureblockMode::Data),
            _ => None,
        };
        let Some(mode) = mode else {
            debug!(
                "Player used structure block with invalid mode {}",
                packet.mode.0
            );
            return;
        };
        let mirror = match packet.mirror.0 {
            0 => Mirror::None,
            1 => Mirror::LeftRight,
            2 => Mirror::FrontBack,
            _ => {
                debug!(
                    "Player used structure block with invalid mirror {}",
                    packet.mirror.0
                );
                return;
            }
        };
        let rotation = match packet.rotation.0 {
            0 => pumpkin_data::Rotation::None,
            1 => pumpkin_data::Rotation::Clockwise90,
            2 => pumpkin_data::Rotation::Rotate180,
            3 => pumpkin_data::Rotation::CounterClockwise90,
            _ => {
                debug!(
                    "Player used structure block with invalid rotation {}",
                    packet.rotation.0
                );
                return;
            }
        };

        // Vanilla clamps the offsets and sizes while decoding the packet.
        structure_block.set_mode(&world, mode);
        structure_block.set_structure_name(packet.name);
        structure_block.set_structure_pos(Vector3::new(
            (i32::from(packet.offset_x)).clamp(-48, 48),
            (i32::from(packet.offset_y)).clamp(-48, 48),
            (i32::from(packet.offset_z)).clamp(-48, 48),
        ));
        structure_block.set_structure_size(Vector3::new(
            i32::from(packet.size_x).clamp(0, 48),
            i32::from(packet.size_y).clamp(0, 48),
            i32::from(packet.size_z).clamp(0, 48),
        ));
        structure_block.set_mirror(mirror);
        structure_block.set_rotation(rotation);
        structure_block.set_metadata(packet.metadata);
        structure_block.set_ignore_entities(packet.ignore_entities());
        structure_block.set_strict(packet.strict());
        structure_block.set_show_air(packet.show_air());
        structure_block.set_show_bounding_box(packet.show_bounding_box());
        structure_block.set_integrity(packet.integrity.clamp(0.0, 1.0));
        structure_block.set_seed(packet.seed.0);

        if structure_block.has_structure_name() {
            let structure_name_text = TextComponent::text(structure_block.get_structure_name());
            match packet.action.0 {
                SSetStructureBlock::ACTION_SAVE_AREA => {
                    // Vanilla `saveStructure()` only saves from SAVE mode.
                    let saved = structure_block.get_mode_value() == StructureblockMode::Save
                        && structure_block.save_structure(&world, true);
                    if saved {
                        player.send_system_message(&translate_cross!(
                            translation::java::STRUCTURE_BLOCK_SAVE_SUCCESS,
                            translation::java::STRUCTURE_BLOCK_SAVE_SUCCESS,
                            structure_name_text
                        ));
                    } else {
                        player.send_system_message(&translate_cross!(
                            translation::java::STRUCTURE_BLOCK_SAVE_FAILURE,
                            translation::java::STRUCTURE_BLOCK_SAVE_FAILURE,
                            structure_name_text
                        ));
                    }
                }
                SSetStructureBlock::ACTION_LOAD_AREA => {
                    if !structure_block.is_structure_loadable() {
                        player.send_system_message(&translate_cross!(
                            translation::java::STRUCTURE_BLOCK_LOAD_NOT_FOUND,
                            translation::java::STRUCTURE_BLOCK_LOAD_NOT_FOUND,
                            structure_name_text
                        ));
                    } else if structure_block.place_structure_if_same_size(&world) {
                        player.send_system_message(&translate_cross!(
                            translation::java::STRUCTURE_BLOCK_LOAD_SUCCESS,
                            translation::java::STRUCTURE_BLOCK_LOAD_SUCCESS,
                            structure_name_text
                        ));
                    } else {
                        player.send_system_message(&translate_cross!(
                            translation::java::STRUCTURE_BLOCK_LOAD_PREPARE,
                            translation::java::STRUCTURE_BLOCK_LOAD_PREPARE,
                            structure_name_text
                        ));
                    }
                }
                SSetStructureBlock::ACTION_SCAN_AREA => {
                    if structure_block.detect_size(&world) {
                        player.send_system_message(&translate_cross!(
                            translation::java::STRUCTURE_BLOCK_SIZE_SUCCESS,
                            translation::java::STRUCTURE_BLOCK_SIZE_SUCCESS,
                            structure_name_text
                        ));
                    } else {
                        player.send_system_message(&translate_cross!(
                            translation::java::STRUCTURE_BLOCK_SIZE_FAILURE,
                            translation::java::STRUCTURE_BLOCK_SIZE_FAILURE,
                        ));
                    }
                }
                // Vanilla's UPDATE_DATA only stores and resends the block.
                _ => {}
            }
        } else {
            player.send_system_message(&translate_cross!(
                translation::java::STRUCTURE_BLOCK_INVALID_STRUCTURE_NAME,
                translation::java::STRUCTURE_BLOCK_INVALID_STRUCTURE_NAME,
                TextComponent::text(packet.name.to_string())
            ));
        }

        // Vanilla `structure.setChanged()` plus `sendBlockUpdated(pos, state,
        // state, 3)`: the settings go back to players and the chunk save.
        world.update_block_entity(&block_entity);
    }
}

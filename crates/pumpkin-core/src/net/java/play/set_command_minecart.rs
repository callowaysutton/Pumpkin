#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_protocol::java::server::play::SSetCommandMinecart;

use crate::entity::vehicle::minecart::MinecartEntity;

impl JavaClient {
    pub fn handle_set_command_minecart(&self, player: &Player, packet: &SSetCommandMinecart<'_>) {
        if player.permission_lvl.load() < PermissionLvl::Two {
            return;
        }

        let world = player.world();
        let Some(entity) = world.get_entity_by_id(packet.entity_id.0) else {
            return;
        };

        let Some(minecart) = entity.cast_any().downcast_ref::<MinecartEntity>() else {
            debug!(
                "Player {} tried to update command of non-minecart entity {}",
                player.gameprofile.name, packet.entity_id.0
            );
            return;
        };

        let command = packet.command.strip_prefix('/').unwrap_or(packet.command);
        if !minecart.set_command(command, packet.track_output) {
            debug!(
                "Player {} tried to update command of non command block minecart {}",
                player.gameprofile.name, packet.entity_id.0
            );
            return;
        }

        debug!(
            "Player {} updated command minecart {} command to: {}",
            player.gameprofile.name, packet.entity_id.0, command
        );
    }
}

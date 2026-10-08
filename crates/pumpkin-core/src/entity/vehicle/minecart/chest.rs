use std::sync::Arc;

use pumpkin_data::translation;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::text::TextComponent;

use crate::entity::player::Player;

use super::super::container::{self, ContainerVehicleInventory};

#[derive(Clone)]
pub(super) struct ChestMinecart {
    inventory: Arc<ContainerVehicleInventory>,
}

impl ChestMinecart {
    pub(super) fn new() -> Self {
        Self {
            inventory: Arc::new(ContainerVehicleInventory::new(27)),
        }
    }

    pub(super) const fn inventory(&self) -> &Arc<ContainerVehicleInventory> {
        &self.inventory
    }

    pub(super) fn interact(
        &self,
        custom_name: Option<TextComponent>,
        player: &Arc<Player>,
    ) -> bool {
        container::open(
            custom_name,
            player,
            &self.inventory,
            TextComponent::translate_cross(
                translation::java::ENTITY_MINECRAFT_CHEST_MINECART,
                translation::bedrock::ENTITY_CHEST_MINECART_NAME,
                [],
            ),
            false,
        )
    }

    pub(super) fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.inventory.write_nbt(nbt);
    }

    pub(super) fn read_nbt(&self, nbt: &NbtCompound) {
        self.inventory.read_nbt(nbt);
    }
}

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase,
    item::ItemEntity,
    mob::{Mob, MobEntity, skeleton::SkeletonEntityBase},
    player::Player,
    projectile::arrow::ArrowEntity,
};
use crate::plugin::api::events::player::player_shear_entity::PlayerShearEntityEvent;
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};

pub struct BoggedSkeletonEntity {
    entity: Arc<SkeletonEntityBase>,
    sheared: AtomicBool,
}

impl BoggedSkeletonEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = SkeletonEntityBase::new_with_bow_attack_intervals(
            entity,
            SkeletonEntityBase::INCREASED_HARD_ATTACK_INTERVAL,
            SkeletonEntityBase::INCREASED_NORMAL_ATTACK_INTERVAL,
        );
        let bogged = Self {
            entity,
            sheared: AtomicBool::new(false),
        };
        Arc::new(bogged)
    }

    /// Vanilla `Bogged::isSheared`; the state is also synced via tracked data.
    #[must_use]
    pub fn is_sheared(&self) -> bool {
        self.sheared.load(Ordering::Relaxed)
    }

    /// Vanilla `Bogged::setSheared`.
    pub fn set_sheared(&self, sheared: bool) {
        self.sheared.store(sheared, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(pumpkin_data::tracked_data::bogged::DATA_SHEARED, sheared);
    }

    /// Vanilla `Bogged::readyForShearing`: a bogged only shears once.
    #[must_use]
    pub fn ready_for_shearing(&self) -> bool {
        !self.is_sheared()
    }

    /// Vanilla `Bogged::shear`: play the shear sound, drop the mushrooms and mark
    /// the bogged sheared so it can only be sheared once.
    fn shear(&self, tool: &ItemStack) {
        let entity = &self.entity.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();

        world.play_sound(Sound::EntityBoggedShear, SoundCategory::Players, &pos);
        self.spawn_sheared_mushrooms(&world, &pos, tool);
        self.set_sheared(true);
    }

    /// Vanilla `Bogged::spawnShearedMushrooms`: rolls the `shearing/bogged` loot
    /// table with the shears as tool and drops the stacks at bounding box height.
    fn spawn_sheared_mushrooms(
        &self,
        world: &Arc<crate::world::World>,
        pos: &Vector3<f64>,
        tool: &ItemStack,
    ) {
        let entity = &self.entity.mob_entity.living_entity.entity;

        let Some(table) = world.get_loot_table("minecraft:shearing/bogged") else {
            return;
        };
        let params = LootContextParameters {
            this_entity: Some(entity.entity_type),
            position: Some(*pos),
            tool: Some(tool.clone()),
            ..Default::default()
        };
        let seed: i64 = rand::random();
        let drop_pos = Vector3::new(
            pos.x,
            pos.y + f64::from(entity.entity_dimension.load().height),
            pos.z,
        );
        let stacks = generate_loot_from_handle(&table, seed, &params);
        for stack in stacks {
            let item_entity = Arc::new(ItemEntity::new(
                Entity::new(world.clone(), drop_pos, &EntityType::ITEM),
                stack,
            ));
            world.spawn_entity(item_entity);
        }
    }
}

impl Mob for BoggedSkeletonEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn modify_shot_arrow(&self, arrow: &ArrowEntity) {
        // Vanilla `Bogged::getArrow` adds a poison effect on top of the base arrow.
        arrow.add_effect(&StatusEffect::POISON, 100, 0);
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        // Vanilla `SHEARED_TAG_NAME`.
        nbt.put_bool("sheared", self.is_sheared());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let sheared = nbt.get_bool("sheared").unwrap_or(false);
        self.set_sheared(sheared);
    }

    /// Vanilla `Bogged::mobInteract`: shears drop the mushrooms on the first use.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if item_stack.get_item() == &Item::SHEARS && self.ready_for_shearing() {
            let entity = &self.entity.mob_entity.living_entity.entity;
            let world = entity.world.load();
            let pos = entity.pos.load();

            if let Some(server) = world.server.upgrade() {
                let mut event = PlayerShearEntityEvent {
                    player: player.clone(),
                    entity_id: entity.entity_id,
                    hand: 0,
                    cancelled: false,
                };
                server.plugin_manager.fire_blocking(&server, &mut event);
                if event.cancelled {
                    return self.get_mob_entity().mob_interact(player, item_stack);
                }
            }

            self.shear(item_stack);
            world.emit_game_event(GameEvent::Shear.name(), pos);
            player.damage_held_item(1);
            return true;
        }
        self.get_mob_entity().mob_interact(player, item_stack)
    }
}

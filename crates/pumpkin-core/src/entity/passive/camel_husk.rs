use std::sync::{Arc, atomic::Ordering::Relaxed};

use pumpkin_data::{
    game_event::GameEvent,
    item_stack::ItemStack,
    sound::Sound,
    tag::{self, Taggable},
};
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    mob::{Mob, MobEntity},
    passive::{animal::Animal, camel::CamelEntity},
    player::Player,
};

/// Vanilla `CamelHusk` extends `Camel`, so all camel AI and tracked data carry over.
///
/// The overrides below only carry the parts vanilla overrides: husk sounds, rabbit
/// foot as food, natural despawning like a monster, no love and no baby form.
pub struct CamelHuskEntity {
    entity: Arc<CamelEntity>,
}

impl CamelHuskEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = CamelEntity::new(entity);
        let husk = Self { entity };
        Arc::new(husk)
    }
}

impl AgeableMob for CamelHuskEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.entity.ageable_data
    }

    fn can_be_a_baby(&self) -> bool {
        false
    }
}

impl Animal for CamelHuskEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_CAMEL_HUSK_FOOD)
    }
}

impl Mob for CamelHuskEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    /// Vanilla `CamelHusk.removeWhenFarAway`: camel husks despawn like monsters,
    /// unlike their `Animal` parent.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        true
    }

    /// Vanilla `AgeableMob.setBaby` is a no-op because `canBeABaby` is false.
    fn spawn_as_baby(&self) -> bool {
        false
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
    }

    fn mob_init_data_tracker(&self) {
        self.entity.mob_init_data_tracker();
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("Saddle", self.entity.is_saddled());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(saddle) = nbt.get_bool("Saddle") {
            self.entity.set_saddled(saddle);
        }
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        // Vanilla `CamelHusk.interact`: every interaction marks the husk persistent.
        self.get_mob_entity()
            .persistence_required
            .store(true, Relaxed);

        if self.entity.interact_saddle_or_ride(
            player,
            item_stack,
            self.is_food(item_stack),
            Sound::EntityCamelHuskSaddle,
        ) {
            return true;
        }

        // Vanilla `Camel.handleEating` with the husk overrides: no love and no growing
        // up (`canFallInLove` and `canBeABaby` are false), only the heal when hurt.
        if self.is_food(item_stack) {
            let living_entity = &self.get_mob_entity().living_entity;
            if living_entity.health.load() < living_entity.get_max_health() {
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                living_entity.heal(2.0);
                self.play_eating_sound(Sound::EntityCamelHuskEat);
                // Vanilla `handleEating` emits the `eat` game event for plugins.
                living_entity
                    .entity
                    .world
                    .load()
                    .emit_game_event(GameEvent::Eat.name(), living_entity.entity.pos.load());
                return true;
            }
        }

        self.get_mob_entity().mob_interact(player, item_stack)
    }
}

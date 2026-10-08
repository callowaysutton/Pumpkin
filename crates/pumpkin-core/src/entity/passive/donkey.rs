use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{
    damage::DamageType,
    tag::{self, Taggable},
};
use pumpkin_nbt::compound::NbtCompound;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, escape_danger::EscapeDangerGoal, follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        random_stand::RandomStandGoal, run_around_like_crazy::RunAroundLikeCrazyGoal,
        swim::SwimGoal, tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

const TEMPT_ITEMS: &[&Item] = &[
    &Item::GOLDEN_APPLE,
    &Item::ENCHANTED_GOLDEN_APPLE,
    &Item::GOLDEN_CARROT,
];

pub const FLAG_TAME: u8 = 2;
pub const FLAG_SADDLE: u8 = 4;
pub const FLAG_BRED: u8 = 8;
pub const FLAG_EATING: u8 = 16;
pub const FLAG_STANDING: u8 = 32;
pub const FLAG_OPEN_MOUTH: u8 = 64;

pub struct DonkeyEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub flags: AtomicU8,
    pub has_chest: AtomicBool,
    pub temper: AtomicI32,
    pub stand_counter: AtomicI32,
    pub mouth_counter: AtomicI32,
    pub owner: AtomicCell<Option<Uuid>>,
}

/// Vanilla `AbstractHorse#getMaxTemper`.
const MAX_TEMPER: i32 = 100;
/// Vanilla `AbstractHorse#getAmbientSoundInterval`, reused by `getAmbientStandInterval`.
const AMBIENT_SOUND_INTERVAL: i32 = 400;
/// Vanilla `AbstractHorse#standIfPossible` rears for 20 ticks.
const STAND_TICKS: i32 = 20;

impl DonkeyEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let donkey = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            flags: AtomicU8::new(0),
            has_chest: AtomicBool::new(false),
            temper: AtomicI32::new(0),
            stand_counter: AtomicI32::new(0),
            mouth_counter: AtomicI32::new(0),
            owner: AtomicCell::new(None),
        };
        let mob_arc = Arc::new(donkey);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(RunAroundLikeCrazyGoal::new(1.2)));
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.2));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.25, TEMPT_ITEMS, false)));
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.0)));
            goal_selector.add_goal(6, Box::new(WanderAroundGoal::new(0.7)));
            goal_selector.add_goal(
                7,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(9, Box::new(RandomStandGoal::new(&*mob_arc)));
        };

        mob_arc
    }

    #[must_use]
    pub fn has_flag(&self, flag: u8) -> bool {
        (self.flags.load(Ordering::Relaxed) & flag) != 0
    }

    pub fn set_flag(&self, flag: u8, val: bool) {
        let current = self.flags.load(Ordering::Relaxed);
        let new_flags = if val { current | flag } else { current & !flag };
        self.flags.store(new_flags, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::donkey::DATA_ID_FLAGS,
            new_flags as i8,
        );
    }

    #[must_use]
    pub fn is_tame(&self) -> bool {
        self.has_flag(FLAG_TAME)
    }

    pub fn set_tame(&self, val: bool) {
        self.set_flag(FLAG_TAME, val);
    }

    #[must_use]
    pub fn is_saddled(&self) -> bool {
        self.has_flag(FLAG_SADDLE)
    }

    pub fn set_saddled(&self, val: bool) {
        self.set_flag(FLAG_SADDLE, val);
    }

    #[must_use]
    pub fn has_chest(&self) -> bool {
        self.has_chest.load(Ordering::Relaxed)
    }

    pub fn set_has_chest(&self, val: bool) {
        self.has_chest.store(val, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::donkey::DATA_ID_CHEST, val);
    }

    #[must_use]
    pub fn get_temper(&self) -> i32 {
        self.temper.load(Ordering::Relaxed)
    }

    pub fn set_temper(&self, temper: i32) {
        self.temper.store(temper, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_eating(&self) -> bool {
        self.has_flag(FLAG_EATING)
    }

    pub fn set_eating(&self, eating: bool) {
        self.set_flag(FLAG_EATING, eating);
    }

    #[must_use]
    pub fn is_standing(&self) -> bool {
        self.has_flag(FLAG_STANDING)
    }

    /// Vanilla `AbstractHorse#setStanding`.
    pub fn set_standing(&self, ticks: i32) {
        self.set_eating(false);
        self.set_flag(FLAG_STANDING, true);
        self.stand_counter.store(ticks, Ordering::Relaxed);
    }

    /// Vanilla `AbstractHorse#clearStanding`.
    pub fn clear_standing(&self) {
        self.set_flag(FLAG_STANDING, false);
        self.stand_counter.store(0, Ordering::Relaxed);
    }

    /// Vanilla `AbstractHorse#handleEating`. Returns whether the item had any effect and
    /// should therefore be consumed.
    fn handle_eating(&self, player: &Player, item_stack: &ItemStack) -> bool {
        let item = item_stack.get_item();
        let (heal, age_up, temper) = if item == &Item::WHEAT {
            (2.0, 20, 3)
        } else if item == &Item::SUGAR {
            (1.0, 30, 3)
        } else if item == &Item::HAY_BLOCK {
            (20.0, 180, 0)
        } else if item == &Item::APPLE {
            (3.0, 60, 3)
        } else if item == &Item::RED_MUSHROOM {
            (3.0, 0, 3)
        } else if item == &Item::CARROT {
            (3.0, 60, 3)
        } else if item == &Item::GOLDEN_CARROT {
            (4.0, 60, 5)
        } else if item == &Item::GOLDEN_APPLE || item == &Item::ENCHANTED_GOLDEN_APPLE {
            (10.0, 240, 10)
        } else {
            (0.0, 0, 0)
        };

        let mut item_used = false;
        let living = &self.mob_entity.living_entity;

        if living.health.load() < living.get_max_health() && heal > 0.0 {
            living.heal(heal);
            item_used = true;
        }

        // Golden food breeds a tamed adult horse that is not already in love.
        if (item == &Item::GOLDEN_CARROT
            || item == &Item::GOLDEN_APPLE
            || item == &Item::ENCHANTED_GOLDEN_APPLE)
            && self.is_tame()
            && self.get_age() == 0
            && !self.mob_entity.is_in_love()
        {
            item_used = true;
            self.mob_entity
                .set_love_ticks(600, Some(player.gameprofile.id));
        }

        if self.is_baby() && age_up > 0 && !self.is_age_locked() {
            let entity = self.get_entity();
            let pos = entity.pos.load();
            entity.world.load().spawn_particle(
                pos + pumpkin_util::math::vector3::Vector3::new(
                    0.0,
                    f64::from(entity.height()) + 0.5,
                    0.0,
                ),
                pumpkin_util::math::vector3::Vector3::new(0.5, 0.5, 0.5),
                1.0,
                7,
                pumpkin_data::particle::Particle::HappyVillager,
            );
            self.age_up(age_up, false);
            item_used = true;
        }

        if temper > 0 && (item_used || !self.is_tame()) && self.get_temper() < MAX_TEMPER {
            let temper = (self.get_temper() + temper).clamp(0, MAX_TEMPER);
            self.set_temper(temper);
            item_used = true;
        }

        if item_used {
            self.eating();
        }

        item_used
    }

    /// Vanilla `AbstractHorse#eating`: open the mouth and play the eating sound.
    fn eating(&self) {
        self.open_mouth();
        // Vanilla randomises the pitch by +-0.2; the shared entity sound helper uses 1.0.
        self.get_entity().play_sound(Sound::EntityDonkeyEat);
    }

    /// Vanilla `AbstractHorse#openMouth`.
    fn open_mouth(&self) {
        self.mouth_counter.store(1, Ordering::Relaxed);
        self.set_flag(FLAG_OPEN_MOUTH, true);
    }

    /// Vanilla `AbstractHorse#fedFood`.
    fn fed_food(&self, player: &Player, item_stack: &mut ItemStack) -> bool {
        let ate_food = self.handle_eating(player, item_stack);
        if ate_food {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        }

        ate_food
    }
}

impl AgeableMob for DonkeyEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for DonkeyEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_HORSE_FOOD)
            || item_stack.item == &Item::WHEAT
            || item_stack.item == &Item::SUGAR
            || item_stack.item == &Item::HAY_BLOCK
            || item_stack.item == &Item::APPLE
            || item_stack.item == &Item::GOLDEN_CARROT
            || item_stack.item == &Item::GOLDEN_APPLE
            || item_stack.item == &Item::ENCHANTED_GOLDEN_APPLE
    }
}

impl Mob for DonkeyEntity {
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_bool("ChestedHorse", self.has_chest());
        nbt.put_bool("EatingHaystack", self.is_eating());
        nbt.put_bool("Bred", self.has_flag(FLAG_BRED));
        nbt.put_bool("Tame", self.is_tame());
        nbt.put_int("Temper", self.temper.load(Ordering::Relaxed));
        if let Some(owner) = self.owner.load() {
            nbt.put_uuid("Owner", owner);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(chested) = nbt.get_bool("ChestedHorse") {
            self.set_has_chest(chested);
        }
        if let Some(eating) = nbt.get_bool("EatingHaystack") {
            self.set_eating(eating);
        }
        if let Some(bred) = nbt.get_bool("Bred") {
            self.set_flag(FLAG_BRED, bred);
        }
        if let Some(tame) = nbt.get_bool("Tame") {
            self.set_tame(tame);
        }
        if let Some(temper) = nbt.get_int("Temper") {
            self.temper.store(temper, Ordering::Relaxed);
        }
        if let Some(owner) = nbt.get_uuid("Owner") {
            self.owner.store(Some(owner));
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn is_tamed(&self) -> bool {
        self.is_tame()
    }

    fn get_temper(&self) -> i32 {
        Self::get_temper(self)
    }

    fn get_max_temper(&self) -> i32 {
        MAX_TEMPER
    }

    /// Vanilla `AbstractHorse#modifyTemper`, clamped to `0..=MAX_TEMPER`.
    fn modify_temper(&self, amount: i32) -> i32 {
        let temper = (self.get_temper() + amount).clamp(0, MAX_TEMPER);
        self.set_temper(temper);
        temper
    }

    /// Vanilla `AbstractHorse#tameWithName`.
    fn tame_with_name(&self, player: &Player) -> bool {
        self.owner.store(Some(player.gameprofile.id));
        self.set_tame(true);

        let entity = self.get_entity();
        // Vanilla triggers CriteriaTriggers.TAME_ANIMAL here; Pumpkin has no criterion system yet.
        entity.world.load().broadcast_entity_event(
            entity,
            pumpkin_data::entity::EntityStatus::TamingSucceeded,
            Some(pumpkin_protocol::bedrock::server::actor_event::ActorEventID::TamingSucceeded),
        );
        true
    }

    /// Vanilla `Donkey`/`AbstractChestedHorse` do not override `canPerformRearing`.
    fn can_perform_rearing(&self) -> bool {
        true
    }

    fn is_standing(&self) -> bool {
        Self::is_standing(self)
    }

    /// Vanilla `AbstractHorse#standIfPossible`.
    fn stand_if_possible(&self) {
        if self.can_perform_rearing() {
            self.set_standing(STAND_TICKS);
        }
    }

    /// Vanilla `AbstractHorse#makeMad`.
    fn make_mad(&self) {
        if !self.is_standing() {
            self.stand_if_possible();
            self.get_entity().play_sound(Sound::EntityDonkeyAngry);
        }
    }

    /// Vanilla `AbstractHorse#isImmobile`.
    fn is_immobile(&self) -> bool {
        let entity = self.get_entity();
        let dead = self.mob_entity.living_entity.health.load() <= 0.0;
        (dead && entity.has_passengers() && self.is_saddled())
            || self.is_eating()
            || self.is_standing()
    }

    fn get_ambient_stand_interval(&self) -> i32 {
        AMBIENT_SOUND_INTERVAL
    }

    /// Vanilla `AbstractHorse#hurtServer`: a hurt horse rears up a third of the time.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        if self.get_random().random_range(0..3) == 0 {
            self.stand_if_possible();
        }
    }

    fn get_ambient_stand_sound(&self) -> Option<Sound> {
        Some(Sound::EntityDonkeyAmbient)
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
        // Vanilla `AbstractHorse#tick`: clear the rearing pose when the counter runs out.
        let stand_counter = self.stand_counter.load(Ordering::Relaxed);
        if stand_counter > 0 {
            let remaining = stand_counter - 1;
            self.stand_counter.store(remaining, Ordering::Relaxed);
            if remaining <= 0 {
                self.clear_standing();
            }
        }
        // Vanilla `AbstractHorse#tick`: close the mouth ~30 ticks after eating.
        let mouth_counter = self.mouth_counter.load(Ordering::Relaxed);
        if mouth_counter > 0 {
            let next = mouth_counter + 1;
            if next > 30 {
                self.mouth_counter.store(0, Ordering::Relaxed);
                self.set_flag(FLAG_OPEN_MOUTH, false);
            } else {
                self.mouth_counter.store(next, Ordering::Relaxed);
            }
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::donkey::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::donkey::DATA_ID_CHEST,
            self.has_chest(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::donkey::DATA_ID_FLAGS,
            self.flags.load(Ordering::Relaxed) as i8,
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let item = item_stack.get_item();
        let entity = self.get_entity();

        // Vanilla `AbstractChestedHorse#mobInteract`: food and equipment are only handled
        // while the donkey is not already being ridden.
        if !entity.has_passengers() && !item_stack.is_empty() {
            if self.is_food(item_stack) {
                return self.fed_food(player, item_stack);
            }

            // A wild donkey refuses anything but food; feeding it is the only way to raise
            // its temper before riding.
            if !self.is_tame() {
                self.make_mad();
                return true;
            }

            if !self.has_chest() && item == &Item::CHEST && !self.is_baby() {
                self.set_has_chest(true);
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                let world = entity.world.load();
                world.play_sound(
                    Sound::EntityDonkeyChest,
                    SoundCategory::Neutral,
                    &entity.pos.load(),
                );
                return true;
            }
        }

        // Pumpkin equips saddles in the interaction handler rather than via the equipment slot.
        if self.is_tame() && item == &Item::SADDLE && !self.is_saddled() && !self.is_baby() {
            self.set_saddled(true);
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            let world = entity.world.load();
            world.play_sound(
                Sound::EntityHorseSaddle,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
            return true;
        }

        if !self.is_baby() {
            let world = player.world();
            let ent = &self.mob_entity.living_entity.entity;
            if let Some(vehicle) = world.get_entity_by_id(ent.entity_id)
                && let Some(passenger) = world.get_player_by_id(player.entity_id())
            {
                ent.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
                return true;
            }
        }

        // Baby donkeys still use the generic animal feeding (age up / breeding).
        if self.is_baby() {
            return self.animal_interact(player, item_stack, Sound::EntityDonkeyAmbient);
        }

        false
    }
}

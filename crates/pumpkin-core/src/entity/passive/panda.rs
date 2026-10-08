use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::{EntityStatus, EntityType, MobCategory};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_inventory::screen_handler::InventoryPlayer;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use std::sync::atomic::Ordering::Relaxed;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::control::move_control::MoveControl,
    ai::control::{Control, MoveControlTrait},
    ai::goal::{
        avoid_entity::AvoidEntityGoal, breed::BreedGoal, escape_danger::EscapeDangerGoal,
        follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        melee_attack::MeleeAttackGoal, revenge::RevengeGoal, swim::SwimGoal, tempt::TemptGoal,
        to_goal_ticks, wander_around::WanderAroundGoal,
    },
    ai::pathfinder::NavigatorGoal,
    ai::target_predicate::TargetPredicate,
    item::ItemEntity,
    mob::Mob,
    mob::MobEntity,
    passive::animal::Animal,
    player::Player,
    predicate::EntityPredicate,
};

/// Vanilla `TemptGoal(this, 1.0, ItemTags.PANDA_FOOD, false)`.
const TEMPT_ITEMS: &[&Item] = &[&Item::BAMBOO];

/// Vanilla `LookAtPlayerGoal.DEFAULT_PROBABILITY`.
const LOOK_PROBABILITY: f32 = 0.02;
/// Vanilla `Panda.registerGoals`: `lookAtPlayerGoal = new PandaLookAtPlayerGoal(this, Player.class, 6.0F)`.
const LOOK_RANGE: f32 = 6.0;
/// Vanilla `Panda.BREED_TARGETING` range, used when showing the unbreedable "no bamboo" mood.
const BREED_PLAYER_RANGE: f64 = 8.0;
/// Vanilla `Mob.ITEM_PICKUP_REACH`.
const ITEM_PICKUP_REACH: (f64, f64, f64) = (1.0, 0.0, 1.0);
/// Vanilla `Panda.canFindBamboo`: scans ring `r < 8` in 3 blocks of height.
const CAN_FIND_BAMBOO_RADIUS: i32 = 8;
const CAN_FIND_BAMBOO_HEIGHT: i32 = 3;

/// Vanilla `Mob.getAmbientSoundInterval`, which the panda does not override.
const AMBIENT_SOUND_INTERVAL: i32 = 80;

pub const FLAG_SNEEZE: u8 = 2;
pub const FLAG_ROLL: u8 = 4;
pub const FLAG_SIT: u8 = 8;
pub const FLAG_ON_BACK: u8 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum PandaGene {
    #[default]
    Normal = 0,
    Lazy = 1,
    Worried = 2,
    Playful = 3,
    Brown = 4,
    Weak = 5,
    Aggressive = 6,
}

impl PandaGene {
    #[must_use]
    pub const fn from_id(id: u8) -> Self {
        match id {
            1 => Self::Lazy,
            2 => Self::Worried,
            3 => Self::Playful,
            4 => Self::Brown,
            5 => Self::Weak,
            6 => Self::Aggressive,
            _ => Self::Normal,
        }
    }

    #[must_use]
    pub const fn id(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Lazy => "lazy",
            Self::Worried => "worried",
            Self::Playful => "playful",
            Self::Brown => "brown",
            Self::Weak => "weak",
            Self::Aggressive => "aggressive",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name {
            "lazy" => Self::Lazy,
            "worried" => Self::Worried,
            "playful" => Self::Playful,
            "brown" => Self::Brown,
            "weak" => Self::Weak,
            "aggressive" => Self::Aggressive,
            _ => Self::Normal,
        }
    }

    /// Vanilla `Panda.Gene.getRandom`.
    #[must_use]
    pub fn random_gene() -> Self {
        let mut rng = rand::rng();
        match rng.random_range(0..16) {
            0 => Self::Lazy,
            1 => Self::Worried,
            2 => Self::Playful,
            4 => Self::Aggressive,
            3 | 5..=8 => Self::Weak,
            9..=10 => Self::Brown,
            _ => Self::Normal,
        }
    }

    /// Vanilla recessive genes only show when both genes match.
    #[must_use]
    pub const fn is_recessive(self) -> bool {
        matches!(self, Self::Brown | Self::Weak)
    }

    /// Vanilla `Panda.Gene.getVariantFromGenes`.
    #[must_use]
    pub const fn variant_from_genes(main: Self, hidden: Self) -> Self {
        if main.is_recessive() && main.id() != hidden.id() {
            Self::Normal
        } else {
            main
        }
    }
}

pub struct PandaEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    /// Vanilla `Mob.ambientSoundTime`.
    ambient_sound_time: AtomicI32,
    pub main_gene: AtomicU8,
    pub hidden_gene: AtomicU8,
    pub flags: AtomicU8,
    pub eat_counter: AtomicI32,
    pub sneeze_counter: AtomicI32,
    pub unhappy_counter: AtomicI32,
    /// Vanilla `gotBamboo`: given bamboo while having an attack target keeps the revenge goal.
    got_bamboo: AtomicBool,
    /// Vanilla `didBite`: a calmer variant bit something, also keeping the revenge target.
    did_bite: AtomicBool,
    /// Vanilla `rollCounter`.
    roll_counter: AtomicI32,
    /// Vanilla `rollDelta`.
    roll_delta: AtomicCell<Vector3<f64>>,
    /// Vanilla `PandaLookAtPlayerGoal.setTarget` slot for the breed goal.
    forced_look_target: Mutex<Option<Arc<dyn EntityBase>>>,
}

impl PandaEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let panda = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            ambient_sound_time: AtomicI32::new(0),
            main_gene: AtomicU8::new(PandaGene::random_gene().id()),
            hidden_gene: AtomicU8::new(PandaGene::random_gene().id()),
            flags: AtomicU8::new(0),
            eat_counter: AtomicI32::new(0),
            sneeze_counter: AtomicI32::new(0),
            unhappy_counter: AtomicI32::new(0),
            got_bamboo: AtomicBool::new(false),
            did_bite: AtomicBool::new(false),
            roll_counter: AtomicI32::new(0),
            roll_delta: AtomicCell::new(Vector3::new(0.0, 0.0, 0.0)),
            forced_look_target: Mutex::new(None),
        };
        let mob_arc = Arc::new(panda);

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Vanilla Panda.registerGoals; the order decides which goal wins a shared priority.
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(2, Box::new(PandaPanicGoal::default()));
            goal_selector.add_goal(2, Box::new(PandaBreedGoal::default()));
            // Vanilla PandaAttackGoal(speed 1.2): the panda only attacks while it can act.
            goal_selector.add_goal(
                3,
                Box::new(MeleeAttackGoal::new(1.2, true).gated_by(panda_can_perform_action)),
            );
            goal_selector.add_goal(4, Box::new(TemptGoal::new(1.0, TEMPT_ITEMS, false)));
            goal_selector.add_goal(
                6,
                Box::new(
                    (AvoidEntityGoal::new(&EntityType::PLAYER, 8.0, 2.0, 2.0))
                        .gated_by(panda_worried_and_can_act),
                ),
            );
            goal_selector.add_goal(
                6,
                Box::new(
                    (AvoidEntityGoal::avoids_category(&MobCategory::MONSTER, 4.0, 2.0, 2.0))
                        .gated_by(panda_worried_and_can_act),
                ),
            );
            goal_selector.add_goal(7, Box::new(PandaSitGoal::default()));
            goal_selector.add_goal(8, Box::new(PandaLieOnBackGoal::default()));
            goal_selector.add_goal(8, Box::new(PandaSneezeGoal));
            goal_selector.add_goal(9, Box::new(PandaLookAtPlayerGoal::default()));
            goal_selector.add_goal(10, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(12, Box::new(PandaRollGoal));
            goal_selector.add_goal(13, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(14, Box::new(WanderAroundGoal::new(1.0)));

            // Vanilla PandaHurtByTargetGoal(this).setAlertOthers, alerting aggressive pandas.
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            target_selector.add_goal(
                1,
                Box::new(
                    RevengeGoal::new(true)
                        .alerting_others()
                        .alerting_only(is_aggressive_panda)
                        .continuing_while(panda_keeps_grudge),
                ),
            );
        };

        {
            // Vanilla Panda constructor: no movement while the panda cannot act.
            let mut move_control = mob_arc
                .mob_entity
                .move_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *move_control = Box::new(PandaMoveControl::default());
        };

        // Vanilla Panda constructor, `!isBaby()` is false there: loot (and eat) from the ground.
        mob_arc.mob_entity.set_can_pick_up_loot(true);

        mob_arc
    }

    #[must_use]
    pub fn get_main_gene(&self) -> PandaGene {
        PandaGene::from_id(self.main_gene.load(Ordering::Relaxed))
    }

    pub fn set_main_gene(&self, gene: PandaGene) {
        self.main_gene.store(gene.id(), Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::panda::MAIN_GENE_ID,
            gene.id() as i8,
        );
    }

    #[must_use]
    pub fn get_hidden_gene(&self) -> PandaGene {
        PandaGene::from_id(self.hidden_gene.load(Ordering::Relaxed))
    }

    pub fn set_hidden_gene(&self, gene: PandaGene) {
        self.hidden_gene.store(gene.id(), Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::panda::HIDDEN_GENE_ID,
            gene.id() as i8,
        );
    }

    /// Vanilla `Panda.getVariant`.
    #[must_use]
    pub fn get_variant(&self) -> PandaGene {
        PandaGene::variant_from_genes(self.get_main_gene(), self.get_hidden_gene())
    }

    #[must_use]
    pub fn is_lazy(&self) -> bool {
        self.get_variant() == PandaGene::Lazy
    }

    #[must_use]
    pub fn is_worried(&self) -> bool {
        self.get_variant() == PandaGene::Worried
    }

    #[must_use]
    pub fn is_playful(&self) -> bool {
        self.get_variant() == PandaGene::Playful
    }

    #[must_use]
    pub fn is_weak(&self) -> bool {
        self.get_variant() == PandaGene::Weak
    }

    /// Vanilla `Panda.isAggressive`.
    #[must_use]
    pub fn is_aggressive(&self) -> bool {
        self.get_variant() == PandaGene::Aggressive
    }

    /// Vanilla `Panda.isScared`.
    #[must_use]
    pub fn is_scared(&self) -> bool {
        self.is_worried() && self.get_entity().world.load().is_thundering()
    }

    #[must_use]
    pub fn has_flag(&self, flag: u8) -> bool {
        (self.flags.load(Ordering::Relaxed) & flag) != 0
    }

    fn set_flag(&self, flag: u8, val: bool) {
        let current = self.flags.load(Ordering::Relaxed);
        let new_flags = if val { current | flag } else { current & !flag };
        self.flags.store(new_flags, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::panda::DATA_ID_FLAGS,
            new_flags as i8,
        );
    }

    #[must_use]
    pub fn is_sneezing(&self) -> bool {
        self.has_flag(FLAG_SNEEZE)
    }

    /// Vanilla `Panda.sneeze`: turning the sneeze off clears the counter.
    pub fn sneeze(&self, val: bool) {
        self.set_flag(FLAG_SNEEZE, val);
        if !val {
            self.set_sneeze_counter(0);
        }
    }

    pub fn get_sneeze_counter(&self) -> i32 {
        self.sneeze_counter.load(Relaxed)
    }

    pub fn set_sneeze_counter(&self, value: i32) {
        self.sneeze_counter.store(value, Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::panda::SNEEZE_COUNTER,
            VarInt(value),
        );
    }

    /// Vanilla `getUnhappyCounter`.
    pub fn get_unhappy_counter(&self) -> i32 {
        self.unhappy_counter.load(Relaxed)
    }

    pub fn set_unhappy_counter(&self, value: i32) {
        self.unhappy_counter.store(value, Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::panda::UNHAPPY_COUNTER,
            VarInt(value),
        );
    }

    #[must_use]
    pub fn is_sitting(&self) -> bool {
        self.has_flag(FLAG_SIT)
    }

    pub fn set_sitting(&self, val: bool) {
        self.set_flag(FLAG_SIT, val);
    }

    #[must_use]
    pub fn is_on_back(&self) -> bool {
        self.has_flag(FLAG_ON_BACK)
    }

    pub fn set_on_back(&self, val: bool) {
        self.set_flag(FLAG_ON_BACK, val);
    }

    #[must_use]
    pub fn is_rolling(&self) -> bool {
        self.has_flag(FLAG_ROLL)
    }

    pub fn set_rolling(&self, val: bool) {
        self.set_flag(FLAG_ROLL, val);
    }

    /// Vanilla `Panda.eat`: the synced eat counter doubles as the eating flag.
    fn eat(&self, value: bool) {
        self.set_eat_counter(i32::from(value));
    }

    #[must_use]
    pub fn is_eating(&self) -> bool {
        self.eat_counter.load(Relaxed) > 0
    }

    pub fn get_eat_counter(&self) -> i32 {
        self.eat_counter.load(Relaxed)
    }

    pub fn set_eat_counter(&self, value: i32) {
        self.eat_counter.store(value, Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::panda::EAT_COUNTER,
            VarInt(value),
        );
    }

    /// Vanilla `Panda.canPerformAction`.
    #[must_use]
    pub fn can_perform_action(&self) -> bool {
        !self.is_on_back()
            && !self.is_scared()
            && !self.is_eating()
            && !self.is_rolling()
            && !self.is_sitting()
    }

    /// Vanilla `Panda.setAttributes`: the weak and lazy genes come with weaker stats.
    pub fn set_attributes(&self) {
        let living = &self.mob_entity.living_entity;
        if self.is_weak() {
            living.set_attribute_base(&Attributes::MAX_HEALTH, 10.0);
        }
        if self.is_lazy() {
            living.set_attribute_base(&Attributes::MOVEMENT_SPEED, 0.07);
        }
    }

    /// Vanilla `Panda.setGeneFromParents`, run on the freshly created baby.
    #[allow(clippy::too_many_lines)] // mirrors the Java match order one to one
    pub fn set_gene_from_parents(&self, parent1: &Self, parent2: Option<&Self>) {
        let mut rng = rand::rng();
        let Some(parent2) = parent2 else {
            if rng.random_range(0..2) == 0 {
                self.set_main_gene(parent1.get_one_of_genes_randomly());
                self.set_hidden_gene(PandaGene::random_gene());
            } else {
                self.set_main_gene(PandaGene::random_gene());
                self.set_hidden_gene(parent1.get_one_of_genes_randomly());
            }
            if rng.random_range(0..32) == 0 {
                self.set_main_gene(PandaGene::random_gene());
            }
            if rng.random_range(0..32) == 0 {
                self.set_hidden_gene(PandaGene::random_gene());
            }
            return;
        };

        if rng.random_range(0..2) == 0 {
            self.set_main_gene(parent1.get_one_of_genes_randomly());
            self.set_hidden_gene(parent2.get_one_of_genes_randomly());
        } else {
            self.set_main_gene(parent2.get_one_of_genes_randomly());
            self.set_hidden_gene(parent1.get_one_of_genes_randomly());
        }

        if rng.random_range(0..32) == 0 {
            self.set_main_gene(PandaGene::random_gene());
        }
        if rng.random_range(0..32) == 0 {
            self.set_hidden_gene(PandaGene::random_gene());
        }
    }

    /// Vanilla `Panda.getOneOfGenesRandomly`.
    fn get_one_of_genes_randomly(&self) -> PandaGene {
        if rand::rng().random_range(0..2) == 0 {
            self.get_main_gene()
        } else {
            self.get_hidden_gene()
        }
    }

    fn get_main_hand_item(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
    }

    /// Vanilla `Panda.tryToSit`.
    fn try_to_sit(&self) {
        if !self.mob_entity.living_entity.entity.is_in_water() {
            self.mob_entity
                .living_entity
                .movement_input
                .store(Vector3::new(0.0, 0.0, 0.0));
            self.mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
            self.set_sitting(true);
        }
    }

    /// Vanilla `Panda.canPickUpAndEat`.
    fn can_pick_up_and_eat(item: &ItemEntity) -> bool {
        let stack = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        stack.item_count > 0
            && stack
                .item
                .has_tag(&tag::Item::MINECRAFT_PANDA_EATS_FROM_GROUND)
            && item.get_entity().is_alive()
            && item.get_pickup_delay() == 0
    }

    /// Vanilla `Panda.pickUpItem`: the whole stack moves to the main hand.
    fn pick_up_item(&self, item: &ItemEntity) {
        if !self.get_main_hand_item().is_empty() {
            return;
        }

        let count = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .item_count;
        if count == 0
            || !self
                .mob_entity
                .living_entity
                .pickup(item.get_entity(), u32::from(count))
        {
            return;
        }

        let taken = item
            .get_item_stack()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .split(count);
        self.mob_entity
            .set_item_slot(&EquipmentSlot::MAIN_HAND, taken);
        self.mob_entity
            .set_guaranteed_drop(&EquipmentSlot::MAIN_HAND);
        item.get_entity().remove();
    }

    /// The shared `Mob.aiStep` loot scan, gated on the mobile griefing rule like vanilla.
    fn pick_up_loose_items(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        if !self.mob_entity.can_pick_up_loot() || !entity.is_alive() {
            return;
        }
        let world = entity.world.load();
        if !world.level_info.load().game_rules.mob_griefing {
            return;
        }

        let reach = entity.bounding_box.load().expand(
            ITEM_PICKUP_REACH.0,
            ITEM_PICKUP_REACH.1,
            ITEM_PICKUP_REACH.2,
        );
        for candidate in world.get_entities_at_box(&reach) {
            let Some(item) = candidate.get_item_entity() else {
                continue;
            };
            if Self::can_pick_up_and_eat(item) {
                self.pick_up_item(item);
            }
        }
    }

    /// Item entities around the panda that `can_pick_up_and_eat`, for the sit goal.
    fn get_eatable_items(&self, distance: f64) -> Vec<Arc<dyn EntityBase>> {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let region = entity.bounding_box.load().expand_all(distance);

        world
            .get_entities_at_box(&region)
            .into_iter()
            .filter(|candidate| {
                candidate
                    .get_item_entity()
                    .is_some_and(Self::can_pick_up_and_eat)
            })
            .collect()
    }

    /// Vanilla `Mob.baseTick`: the ambient sound fires on the growing chance.
    fn tick_ambient_sound(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        if entity.is_alive()
            && rand::random_range(0..1000) < self.ambient_sound_time.fetch_add(1, Ordering::Relaxed)
        {
            self.reset_ambient_sound_time();
            let world = entity.world.load();
            world.play_sound(
                self.get_ambient_sound(),
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
        }
    }

    fn reset_ambient_sound_time(&self) {
        self.ambient_sound_time
            .store(-AMBIENT_SOUND_INTERVAL, Ordering::Relaxed);
    }

    /// Vanilla `Panda.getAmbientSound`: the gene picks between the three ambient sounds.
    #[must_use]
    fn get_ambient_sound(&self) -> Sound {
        if self.is_aggressive() {
            Sound::EntityPandaAggressiveAmbient
        } else if self.is_worried() {
            Sound::EntityPandaWorriedAmbient
        } else {
            Sound::EntityPandaAmbient
        }
    }

    /// Vanilla `Panda.tick`, the server side. Vanilla's sitAmount/onBackAmount/rollAmount lerp
    /// state is read only by the client model, which recomputes it locally.
    #[allow(clippy::too_many_lines)] // mirrors vanilla's Panda.tick
    fn tick_panda(&self) {
        let mob_entity = &self.mob_entity;
        let entity = &mob_entity.living_entity.entity;
        let world = entity.world.load();

        self.tick_ambient_sound();

        if self.is_worried() {
            if world.is_thundering() && !entity.is_in_water() {
                self.set_sitting(true);
                self.eat(false);
            } else if !self.is_eating() {
                self.set_sitting(false);
            }
        }

        let target = mob_entity.get_target();
        if target.is_none() {
            self.got_bamboo.store(false, Relaxed);
            self.did_bite.store(false, Relaxed);
        }

        let unhappy_counter = self.get_unhappy_counter();
        if unhappy_counter > 0 {
            if let Some(target) = &target {
                // Vanilla lookAt(target, 90, 90)
                mob_entity
                    .look_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .look_at_entity_with_range(target, 90.0, 90.0);
            }
            if unhappy_counter == 29 || unhappy_counter == 14 {
                world.play_sound(
                    Sound::EntityPandaCantBreed,
                    SoundCategory::Neutral,
                    &entity.pos.load(),
                );
            }
            self.set_unhappy_counter(unhappy_counter - 1);
        }

        if self.is_sneezing() {
            let sneeze_counter = self.get_sneeze_counter() + 1;
            self.set_sneeze_counter(sneeze_counter);
            if sneeze_counter > 20 {
                self.sneeze(false);
                self.after_sneeze();
            } else if sneeze_counter == 1 {
                world.play_sound(
                    Sound::EntityPandaPreSneeze,
                    SoundCategory::Neutral,
                    &entity.pos.load(),
                );
            }
        }

        if self.is_rolling() {
            self.handle_roll();
        } else {
            self.roll_counter.store(0, Relaxed);
        }

        if self.is_sitting() {
            entity.set_pitch(0.0);
        }

        self.handle_eating();
    }

    /// Vanilla `Panda.handleRoll`.
    fn handle_roll(&self) {
        let counter = self.roll_counter.fetch_add(1, Relaxed) + 1;
        if counter > 32 {
            self.set_rolling(false);
            return;
        }

        let entity = &self.mob_entity.living_entity.entity;
        let mut movement = entity.velocity.load();
        if counter == 1 {
            let angle = f64::from(entity.yaw.load()).to_radians();
            let multiplier = if self.is_baby() { 0.1 } else { 0.2 };
            let roll_delta = Vector3::new(
                movement.x + -angle.sin() * multiplier,
                0.0,
                movement.z + angle.cos() * multiplier,
            );
            self.roll_delta.store(roll_delta);
            movement = Vector3::new(roll_delta.x, 0.27, roll_delta.z);
        } else if counter == 7 || counter == 15 || counter == 23 {
            let y = if entity.on_ground.load(Relaxed) {
                0.27
            } else {
                movement.y
            };
            movement = Vector3::new(0.0, y, 0.0);
        } else {
            let roll_delta = self.roll_delta.load();
            movement = Vector3::new(roll_delta.x, movement.y, roll_delta.z);
        }
        entity.velocity.store(movement);
        entity.velocity_dirty.store(true, Ordering::SeqCst);
    }

    /// Vanilla `Panda.afterSneeze`.
    #[allow(clippy::too_many_lines)]
    fn after_sneeze(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();

        let nose_pos = {
            let body_angle = f64::from(entity.body_yaw.load()).to_radians();
            let width = f64::from(entity.entity_dimension.load().width);
            let extent = f64::midpoint(width, 1.0);
            Vector3::new(
                pos.x - extent * body_angle.sin(),
                entity.get_eye_y() - 0.1,
                pos.z + extent * body_angle.cos(),
            )
        };
        world.spawn_particles(
            Particle::Sneeze,
            nose_pos,
            1,
            Vector3::new(0.0, 0.0, 0.0),
            1.0,
        );
        world.play_sound(Sound::EntityPandaSneeze, SoundCategory::Neutral, &pos);

        // Vanilla makes every nearby grown panda jump when it sneezes.
        for candidate in world
            .get_entities_at_box(&entity.bounding_box.load().expand_all(10.0))
            .into_iter()
            .filter(|candidate| candidate.get_entity().entity_type == &EntityType::PANDA)
        {
            let Some(other_mob) = candidate.get_mob() else {
                continue;
            };
            let Some(panda) = other_mob.as_panda() else {
                continue;
            };
            let other_entity = candidate.get_entity();
            if other_entity.age.load(Relaxed) < 0
                || !other_entity.on_ground.load(Relaxed)
                || other_entity.is_in_water()
                || !panda.can_perform_action()
            {
                continue;
            }
            other_mob.get_mob_entity().living_entity.jump();
        }

        // Vanilla drops the sneeze gift under the `mobDrops` gamerule.
        if world.level_info.load().game_rules.mob_drops
            && let Some(loot_table) = world.get_loot_table("minecraft:gameplay/panda_sneeze")
        {
            let seed: i64 = rand::random();
            let block_pos = entity.block_pos.load();
            for stack in loot_table.generate_loot(seed) {
                world.drop_stack(&block_pos, stack);
            }
        }
    }

    /// Vanilla `Panda.handleEating`.
    fn handle_eating(&self) {
        let held = self.get_main_hand_item();
        if !self.is_eating()
            && self.is_sitting()
            && !self.is_scared()
            && !held.is_empty()
            && rand::random_range(0..80) == 1
        {
            self.eat(true);
        } else if held.is_empty() || !self.is_sitting() {
            self.eat(false);
        }

        if !self.is_eating() {
            return;
        }

        self.add_eating_particles();
        let eat_counter = self.get_eat_counter();
        if eat_counter > 80 && rand::random_range(0..20) == 1 {
            if eat_counter > 100
                && self
                    .get_main_hand_item()
                    .item
                    .has_tag(&tag::Item::MINECRAFT_PANDA_EATS_FROM_GROUND)
            {
                let entity = &self.mob_entity.living_entity.entity;
                let world = entity.world.load();
                self.mob_entity
                    .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
                world.emit_game_event(GameEvent::Eat.name(), entity.pos.load());
                self.set_sitting(false);
            }
            self.eat(false);
        } else {
            self.set_eat_counter(eat_counter + 1);
        }
    }

    /// Vanilla `Panda.addEatingParticles`; the item crumbs need item-stack particle data that
    /// Pumpkin cannot send yet, so only the chewing sound plays.
    fn add_eating_particles(&self) {
        if self.get_eat_counter() % 5 == 0 {
            let entity = &self.mob_entity.living_entity.entity;
            let world = entity.world.load();
            let volume = if rand::random_range(0..2) == 0 {
                0.5
            } else {
                1.0
            };
            let pitch = (rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0;
            world.play_sound_fine(
                Sound::EntityPandaEat,
                SoundCategory::Neutral,
                &entity.pos.load(),
                volume,
                pitch,
            );
        }
    }

    /// Vanilla `Panda.setInLove`.
    fn set_in_love(&self, player: &Arc<Player>) {
        let mob_entity = &self.mob_entity;
        let entity = &mob_entity.living_entity.entity;
        let world = entity.world.load();
        mob_entity.set_love_ticks(600, Some(player.gameprofile.id));
        world.send_entity_status(
            entity,
            EntityStatus::InLoveHearts,
            Some(ActorEventID::InLoveHearts),
        );
        world.spawn_particle(
            entity.pos.load() + Vector3::new(0.0, f64::from(entity.height()), 0.0),
            Vector3::new(0.5, 0.5, 0.5),
            1.0,
            7,
            Particle::Heart,
        );
    }

    /// Vanilla `PandaLookAtPlayerGoal.setTarget`, used by the breed goal while unbreedable.
    fn set_forced_look_target(&self, target: Option<Arc<dyn EntityBase>>) {
        *self
            .forced_look_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target;
    }

    fn take_forced_look_target(&self) -> Option<Arc<dyn EntityBase>> {
        self.forced_look_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

impl AgeableMob for PandaEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn on_child_from_breeding(
        &self,
        child: &Arc<dyn EntityBase>,
        partner: Option<&dyn EntityBase>,
    ) {
        // Vanilla Panda.getBreedOffspring: genes are mixed from both parents.
        let Some(baby) = child.get_mob().and_then(Mob::as_panda) else {
            return;
        };
        let partner = partner
            .and_then(EntityBase::get_mob)
            .and_then(Mob::as_panda);
        baby.set_gene_from_parents(self, partner);
        baby.set_attributes();
    }
}

impl Animal for PandaEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        // Vanilla Panda.isFood: only `ItemTags.PANDA_FOOD`
        item_stack.item.has_tag(&tag::Item::MINECRAFT_PANDA_FOOD)
    }
}

impl crate::entity::custom_sound::CustomSound for PandaEntity {
    fn death_sound(&self) -> Option<Sound> {
        Some(Sound::EntityPandaDeath)
    }

    fn hurt_sound(&self) -> Option<Sound> {
        Some(Sound::EntityPandaHurt)
    }
}

impl Mob for PandaEntity {
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_custom_sound(&self) -> Option<&dyn crate::entity::custom_sound::CustomSound> {
        Some(self)
    }

    fn as_panda(&self) -> Option<&PandaEntity> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_string("MainGene", self.get_main_gene().as_str().to_string());
        nbt.put_string("HiddenGene", self.get_hidden_gene().as_str().to_string());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(main) = nbt.get_string("MainGene") {
            self.set_main_gene(PandaGene::from_name(main));
        }
        if let Some(hidden) = nbt.get_string("HiddenGene") {
            self.set_hidden_gene(PandaGene::from_name(hidden));
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
        self.tick_panda();
    }

    fn post_tick(&self) {
        self.pick_up_loose_items();
    }

    fn on_damage(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        _source: Option<&dyn EntityBase>,
    ) {
        // vanilla Panda.hurtServer clears sitting on any damage.
        self.set_sitting(false);
    }

    fn on_attack(&self, _target: &dyn EntityBase) {
        // vanilla Panda.doHurtTarget + playAttackSound.
        if !self.is_aggressive() {
            self.did_bite.store(true, Relaxed);
        }
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityPandaBite,
            SoundCategory::Neutral,
            &entity.pos.load(),
        );
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<crate::world::World>,
        group_data: Option<crate::entity::mob::spawn::SpawnGroupData>,
    ) -> Option<crate::entity::mob::spawn::SpawnGroupData> {
        self.get_mob_entity().finalize_spawn_base();
        // Vanilla Panda.finalizeSpawn: fresh genes per spawn, then the weak/lazy stat fixes.
        self.set_main_gene(PandaGene::random_gene());
        self.set_hidden_gene(PandaGene::random_gene());
        self.set_attributes();
        group_data
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::panda::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::MAIN_GENE_ID,
            self.main_gene.load(Relaxed) as i8,
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::HIDDEN_GENE_ID,
            self.hidden_gene.load(Relaxed) as i8,
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::DATA_ID_FLAGS,
            self.flags.load(Relaxed) as i8,
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::UNHAPPY_COUNTER,
            VarInt(self.unhappy_counter.load(Relaxed)),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::SNEEZE_COUNTER,
            VarInt(self.sneeze_counter.load(Relaxed)),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::panda::EAT_COUNTER,
            VarInt(self.eat_counter.load(Relaxed)),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        // Vanilla Panda.mobInteract
        if self.is_scared() {
            return false;
        }
        if self.is_on_back() {
            self.set_on_back(false);
            return true;
        }
        let mob_entity = &self.mob_entity;
        if self.is_food(item_stack) {
            if mob_entity.get_target().is_some() {
                self.got_bamboo.store(true, Relaxed);
            }
            if self.can_age_up() {
                // vanilla usePlayerItem + ageUp(speedUpSeconds, true)
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                self.age_up(
                    Self::get_speed_up_seconds_when_feeding(-self.get_age()),
                    true,
                );
            } else {
                if self.is_baby() {
                    return false;
                }
                if self.get_age() == 0 && !mob_entity.is_in_love() {
                    // vanilla usePlayerItem + setInLove
                    item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                    self.set_in_love(player);
                } else {
                    if self.is_sitting() || self.get_entity().is_in_water() {
                        return false;
                    }
                    // Vanilla trades the held item for the fed one and starts eating.
                    self.try_to_sit();
                    self.eat(true);
                    let held = self.get_main_hand_item();
                    if !held.is_empty() && !player.has_infinite_materials() {
                        let world = self.get_entity().world.load();
                        world.drop_stack(&self.get_entity().block_pos.load(), held);
                    }
                    mob_entity.set_item_slot(
                        &EquipmentSlot::MAIN_HAND,
                        ItemStack::new(1, item_stack.item),
                    );
                    item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                }
            }
            return true;
        }

        // Vanilla forwards a baby fed the golden dandelion to super.mobInteract; that steps
        // into the AgeableMob age lock, which Pumpkin has not ported yet.
        let holding_dandelion = {
            let inventory = player.inventory();
            inventory.held_item().item == &Item::GOLDEN_DANDELION
                || inventory.off_hand_item().item == &Item::GOLDEN_DANDELION
        };
        self.is_baby() && holding_dandelion && mob_entity.mob_interact(player, item_stack)
    }
}

// Free filters shared by the goals (vanilla's `PandaAttackGoal`, `PandaAvoidGoal`,
// `PandaHurtByTargetGoal` and `PandaMoveControl` overrides, plus the `canPerformAction` helper).
fn panda_can_perform_action(mob: &dyn Mob) -> bool {
    mob.as_panda().is_some_and(PandaEntity::can_perform_action)
}

fn panda_worried_and_can_act(mob: &dyn Mob) -> bool {
    mob.as_panda()
        .is_some_and(|panda| panda.is_worried() && panda.can_perform_action())
}

fn is_aggressive_panda(mob: &dyn Mob) -> bool {
    mob.as_panda().is_some_and(PandaEntity::is_aggressive)
}

fn panda_keeps_grudge(mob: &dyn Mob) -> bool {
    mob.as_panda()
        .is_none_or(|panda| !panda.got_bamboo.load(Relaxed) && !panda.did_bite.load(Relaxed))
}

/// Vanilla `PandaMoveControl`: movement is driven only while the panda can perform an action.
#[derive(Default)]
pub(crate) struct PandaMoveControl {
    /// The inherited vanilla base control.
    inner: MoveControl,
}

impl Control for PandaMoveControl {}

impl MoveControlTrait for PandaMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        if panda_can_perform_action(mob) {
            self.inner.tick(mob);
        } else {
            // Vanilla skips MoveControl entirely here; clear stale navigator input so the
            // panda stands still while sitting, lying, rolling or eating.
            mob.get_mob_entity()
                .living_entity
                .movement_input
                .store(Vector3::new(0.0, 0.0, 0.0));
        }
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.inner.set_wanted_position(x, y, z, speed_modifier);
    }

    fn strafe(&mut self, forward: f32, right: f32) {
        self.inner.strafe(forward, right);
    }

    fn has_wanted(&self) -> bool {
        self.inner.has_wanted()
    }
}

/// Vanilla `PandaPanicGoal`: panics only on `PANIC_ENVIRONMENTAL_CAUSES` and never while sitting.
struct PandaPanicGoal {
    panic_goal: EscapeDangerGoal,
}

impl Default for PandaPanicGoal {
    fn default() -> Self {
        Self {
            panic_goal: *EscapeDangerGoal::new(2.0)
                .with_panic_tag(&tag::DamageType::MINECRAFT_PANIC_ENVIRONMENTAL_CAUSES),
        }
    }
}

impl crate::entity::ai::goal::Goal for PandaPanicGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.panic_goal.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        // vanilla PandaPanicGoal.canContinueToUse: sitting panicking pandas stop fleeing.
        if mob.as_panda().is_some_and(PandaEntity::is_sitting) {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
            return false;
        }
        self.panic_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.panic_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.panic_goal.stop(mob);
    }

    fn controls(&self) -> crate::entity::ai::goal::Controls {
        self.panic_goal.controls()
    }
}

/// Vanilla `PandaBreedGoal`: pandas only breed with bamboo around and get grumpy without it.
struct PandaBreedGoal {
    breed_goal: BreedGoal,
    /// Vanilla `unhappyCooldown`, measured against the world age.
    unhappy_cooldown: i64,
}

impl Default for PandaBreedGoal {
    fn default() -> Self {
        Self {
            breed_goal: *BreedGoal::new(1.0),
            unhappy_cooldown: 0,
        }
    }
}

impl PandaBreedGoal {
    /// Vanilla `PandaBreedGoal.canFindBamboo`, diamond scan from the Java loop.
    fn can_find_bamboo(mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let base = entity.block_pos.load();

        for y in 0..CAN_FIND_BAMBOO_HEIGHT {
            for radius in 0..CAN_FIND_BAMBOO_RADIUS {
                // Outer loop walks x: 0, 1, -1, 2, -2 ..., mirror of the Java `for` update.
                let mut x = 0;
                while x <= radius {
                    // Inside the diamond the ring starts at the radius, at the edges at 0.
                    let mut z = if x < radius && x > -radius { radius } else { 0 };
                    loop {
                        let pos = BlockPos::new(base.0.x + x, base.0.y + y, base.0.z + z);
                        if world.get_block(&pos).id == pumpkin_data::Block::BAMBOO.id {
                            return true;
                        }
                        // Mirror of the Java `for` update: r, -r, 1+r ... until past `radius`.
                        z = if z > 0 { -z } else { 1 - z };
                        if z > radius {
                            break;
                        }
                    }
                    x = if x > 0 { -x } else { 1 - x };
                }
            }
        }

        false
    }
}

impl crate::entity::ai::goal::Goal for PandaBreedGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        // Order mirrors vanilla: the shared mate search first, then the panda quirks.
        if !self.breed_goal.can_start(mob) {
            return false;
        }
        let Some(panda) = mob.as_panda() else {
            return false;
        };
        if panda.get_unhappy_counter() != 0 {
            return false;
        }

        if !Self::can_find_bamboo(mob) {
            let world_age = mob.get_entity().world.load().get_world_age();
            if self.unhappy_cooldown <= world_age {
                panda.set_unhappy_counter(32);
                self.unhappy_cooldown = world_age + 600;
                if !mob.get_mob_entity().is_no_ai() {
                    // Vanilla stares at the nearest player while unbreedable; `BREED_TARGETING`
                    // is `TargetingConditions.forNonCombat().range(8.0)`, so sensor line of
                    // sight applies.
                    let player = {
                        let pos = mob.get_entity().pos.load();
                        let world = mob.get_entity().world.load();
                        let targeting = TargetPredicate::create_non_attackable()
                            .set_base_max_distance(BREED_PLAYER_RANGE);
                        world.get_nearest_player(pos, BREED_PLAYER_RANGE, |player| {
                            targeting.copy().test(&world, Some(mob), player.as_ref())
                        })
                    };
                    panda.set_forced_look_target(player.map(|p| p as Arc<dyn EntityBase>));
                }
            }
            return false;
        }
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.breed_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.breed_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.breed_goal.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.breed_goal.tick(mob);
    }

    fn controls(&self) -> crate::entity::ai::goal::Controls {
        self.breed_goal.controls()
    }
}

/// Vanilla `PandaSitGoal`: pandas with a bamboo supply sit down and nibble for a while.
#[derive(Default)]
struct PandaSitGoal {
    cooldown: i64,
}

impl PandaSitGoal {
    fn can_sit(&self, mob: &dyn Mob, panda: &PandaEntity) -> bool {
        // Vanilla canUse through stop: the condition chain shares one `tick` time source.
        let world_age = mob.get_entity().world.load().get_world_age();
        self.cooldown <= world_age
            && !panda.is_baby()
            && !panda.get_entity().is_in_water()
            && panda.can_perform_action()
            && panda.get_unhappy_counter() <= 0
    }

    fn drop_held_item(&mut self, panda: &PandaEntity, world_age: i64) {
        // Vanilla stop: drop the held item, then wait longer as a lazy gene.
        let held = panda.get_main_hand_item();
        if !held.is_empty() {
            let entity = panda.get_entity();
            let world = entity.world.load();
            world.drop_stack(&entity.block_pos.load(), held);
            panda
                .mob_entity
                .set_item_slot(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
            let wait_ticks = i64::from(
                if panda.is_lazy() {
                    rand::rng().random_range(0..50) + 10
                } else {
                    rand::rng().random_range(0..150) + 10
                } * 20,
            );
            self.cooldown = world_age + wait_ticks;
        }
        panda.set_sitting(false);
    }
}

impl crate::entity::ai::goal::Goal for PandaSitGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = mob.as_panda() else {
            return false;
        };
        if !self.can_sit(mob, panda) {
            return false;
        }
        if !panda.get_main_hand_item().is_empty() {
            return true;
        }
        !panda.get_eatable_items(6.0).is_empty()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        // Vanilla canContinueToUse.
        let Some(panda) = mob.as_panda() else {
            return false;
        };
        let mut rng = rand::rng();
        !panda.get_entity().is_in_water()
            && (panda.is_lazy() || rng.random_range(0..to_goal_ticks(600)) != 1)
            && rng.random_range(0..to_goal_ticks(2000)) != 1
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(panda) = mob.as_panda()
            && !panda.is_sitting()
            && !panda.get_main_hand_item().is_empty()
        {
            panda.try_to_sit();
        }
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(panda) = mob.as_panda() else {
            return;
        };
        if panda.get_main_hand_item().is_empty() {
            // Vanilla walks to the first nearby snack it can pick up.
            if let Some(item) = panda.get_eatable_items(8.0).into_iter().next() {
                let mob_pos = mob.get_entity().pos.load();
                let item_pos = item.get_entity().pos.load();
                mob.get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_progress(NavigatorGoal::new(mob_pos, item_pos, 1.2));
            }
        } else {
            panda.try_to_sit();
        }
        self.cooldown = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let Some(panda) = mob.as_panda() else {
            return;
        };
        let world_age = mob.get_entity().world.load().get_world_age();
        self.drop_held_item(panda, world_age);
    }

    fn controls(&self) -> crate::entity::ai::goal::Controls {
        crate::entity::ai::goal::Controls::MOVE
    }
}

/// Vanilla `PandaLieOnBackGoal`: lazy pandas roll onto their back for a while.
#[derive(Default)]
struct PandaLieOnBackGoal {
    cooldown: i64,
}

impl crate::entity::ai::goal::Goal for PandaLieOnBackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = mob.as_panda() else {
            return false;
        };
        let world_age = mob.get_entity().world.load().get_world_age();
        self.cooldown < world_age
            && panda.is_lazy()
            && panda.can_perform_action()
            && rand::rng().random_range(0..to_goal_ticks(400)) == 1
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        // Vanilla canContinueToUse.
        let Some(panda) = mob.as_panda() else {
            return false;
        };
        let mut rng = rand::rng();
        !panda.get_entity().is_in_water()
            && (panda.is_lazy() || rng.random_range(0..to_goal_ticks(600)) != 1)
            && rng.random_range(0..to_goal_ticks(2000)) != 1
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(panda) = mob.as_panda() {
            panda.set_on_back(true);
        }
        self.cooldown = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(panda) = mob.as_panda() {
            panda.set_on_back(false);
        }
        let world_age = mob.get_entity().world.load().get_world_age();
        self.cooldown = world_age + 200;
    }
}

/// Vanilla `PandaSneezeGoal`: weak baby pandas sneeze a lot more often.
struct PandaSneezeGoal;

impl crate::entity::ai::goal::Goal for PandaSneezeGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = mob.as_panda() else {
            return false;
        };
        if !panda.is_baby() || !panda.can_perform_action() {
            return false;
        }
        let mut rng = rand::rng();
        (panda.is_weak() && rng.random_range(0..to_goal_ticks(500)) == 1)
            || rng.random_range(0..to_goal_ticks(6000)) == 1
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        // vanilla canContinueToUse: the sneeze runs through the entity tick.
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(panda) = mob.as_panda() {
            panda.sneeze(true);
        }
    }
}

/// Vanilla `PandaLookAtPlayerGoal`: looks at players when able to act, showing the mood only
/// once its forced target is set by the breed goal.
#[derive(Default)]
struct PandaLookAtPlayerGoal {
    target: Option<Arc<dyn EntityBase>>,
    look_time: i32,
}

impl crate::entity::ai::goal::Goal for PandaLookAtPlayerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if mob.get_random().random::<f32>() >= LOOK_PROBABILITY {
            return false;
        }
        let Some(panda) = mob.as_panda() else {
            return false;
        };
        // Vanilla field `lookAt`: a fresh forced target from the breed goal replaces the
        // queued one and survives failed attempts until this goal stops.
        self.target = panda
            .take_forced_look_target()
            .or_else(|| self.target.clone());
        if self.target.is_none() {
            // Vanilla `lookAtContext`: `forNonCombat().range(6.0)` plus `notRiding(this)`.
            self.target = {
                let world = mob.get_entity().world.load();
                let mob_pos = mob.get_entity().pos.load();
                let look_predicate = TargetPredicate::create_non_attackable()
                    .set_base_max_distance(f64::from(LOOK_RANGE));
                world
                    .get_nearest_player(mob_pos, f64::from(LOOK_RANGE), |player| {
                        EntityPredicate::Rides(mob.get_entity()).test(player.get_entity())
                            && look_predicate
                                .copy()
                                .test(&world, Some(mob), player.as_ref())
                    })
                    .map(|p: Arc<Player>| p as Arc<dyn EntityBase>)
            };
        }
        self.target.is_some() && panda_can_perform_action(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        // vanilla: lookAt != null && super.canContinueToUse()
        let Some(target) = self.target.clone() else {
            return false;
        };
        if !target.get_entity().is_alive() {
            return false;
        }
        let mob_pos = mob.get_entity().pos.load();
        let target_pos = target.get_entity().pos.load();
        let dist_sq = mob_pos.squared_distance_to_vec(&target_pos) as f32;
        dist_sq <= 36.0 && self.look_time > 0
    }

    fn start(&mut self, mob: &dyn Mob) {
        // vanilla LookAtPlayerGoal.start: 40 + up to 40 more ticks.
        self.look_time = self.get_tick_count(40 + mob.get_random().random_range(0..40));
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        self.target = None;
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = &self.target else {
            return;
        };
        if target.get_entity().is_alive() {
            let target_pos = target.get_entity().pos.load();
            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at(
                    mob,
                    target_pos.x,
                    target.get_entity().get_eye_y(),
                    target_pos.z,
                );
            self.look_time -= 1;
        }
    }

    fn controls(&self) -> crate::entity::ai::goal::Controls {
        crate::entity::ai::goal::Controls::LOOK
    }
}

/// Vanilla `PandaRollGoal`: baby and playful pandas roll once they stand by an edge.
struct PandaRollGoal;

impl crate::entity::ai::goal::Goal for PandaRollGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(panda) = mob.as_panda() else {
            return false;
        };
        if !(panda.is_baby() || panda.is_playful()) {
            return false;
        }
        if !panda.get_entity().on_ground.load(Relaxed) {
            return false;
        }
        if !panda.can_perform_action() {
            return false;
        }

        let entity = panda.get_entity();
        let angle = f64::from(entity.yaw.load()).to_radians();
        let x_dir = -angle.sin();
        let z_dir = angle.cos();
        let x_step = if x_dir.abs() > 0.5 {
            x_dir.signum() as i32
        } else {
            0
        };
        let z_step = if z_dir.abs() > 0.5 {
            z_dir.signum() as i32
        } else {
            0
        };

        let pos = entity.block_pos.load();
        let edge = BlockPos::new(pos.0.x + x_step, pos.0.y - 1, pos.0.z + z_step);
        if entity.world.load().get_block_state(&edge).is_air() {
            return true;
        }

        let mut rng = rand::rng();
        (panda.is_playful() && rng.random_range(0..to_goal_ticks(60)) == 1)
            || rng.random_range(0..to_goal_ticks(500)) == 1
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        // vanilla canContinueToUse: the rolling continues in the entity tick.
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(panda) = mob.as_panda() {
            panda.set_rolling(true);
        }
    }

    fn can_stop(&self) -> bool {
        // vanilla isInterruptable: false
        false
    }

    fn controls(&self) -> crate::entity::ai::goal::Controls {
        crate::entity::ai::goal::Controls::MOVE
            | crate::entity::ai::goal::Controls::LOOK
            | crate::entity::ai::goal::Controls::JUMP
    }
}

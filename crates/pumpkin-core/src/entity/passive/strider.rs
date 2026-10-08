use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::Block;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;

use crate::block::PathComputationType;
use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        Controls, Goal, ParentHandle,
        breed::BreedGoal,
        escape_danger::EscapeDangerGoal,
        follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal},
        swim::SwimGoal,
        tempt::TemptGoal,
        wander_around::WanderAroundGoal,
    },
    attributes::{Modifier, ModifierOperation},
    item_steerable::{ItemBasedSteering, ItemSteerable},
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

const TEMPT_ITEMS: &[&Item] = &[&Item::WARPED_FUNGUS, &Item::WARPED_FUNGUS_ON_A_STICK];

/// Vanilla `Strider.SUFFOCATING_MODIFIER_ID`.
const SUFFOCATING_MODIFIER_ID: &str = "minecraft:suffocating";
/// Vanilla `Strider.SUFFOCATING_MODIFIER` amount (add-multiplied-base).
const SUFFOCATING_MODIFIER_AMOUNT: f64 = -0.34;

pub struct StriderEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub steering: ItemBasedSteering,
    pub saddled: AtomicBool,
    pub suffocating: AtomicBool,
}

impl StriderEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let strider = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            steering: ItemBasedSteering::default(),
            saddled: AtomicBool::new(false),
            suffocating: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(strider);
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
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.65));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.4, TEMPT_ITEMS, false)));
            goal_selector.add_goal(4, StriderGoToLavaGoal::new());
            goal_selector.add_goal(5, Box::new(FollowParentGoal::new(1.1)));
            goal_selector.add_goal(6, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                7,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::STRIDER, 8.0),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn is_suffocating(&self) -> bool {
        self.suffocating.load(Ordering::Relaxed)
    }

    pub fn set_suffocating(&self, suffocating: bool) {
        self.suffocating.store(suffocating, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::strider::DATA_SUFFOCATING,
            suffocating,
        );

        // Vanilla `Strider.setSuffocating`: -34% base movement speed while suffocating.
        self.mob_entity
            .living_entity
            .update_attribute(&Attributes::MOVEMENT_SPEED, |inst| {
                if suffocating {
                    inst.add_or_replace_modifier(Modifier {
                        id: SUFFOCATING_MODIFIER_ID.to_string(),
                        amount: SUFFOCATING_MODIFIER_AMOUNT,
                        operation: ModifierOperation::MultiplyBase,
                    });
                } else {
                    inst.remove_modifier(SUFFOCATING_MODIFIER_ID);
                }
            });
    }

    fn is_in_lava(&self) -> bool {
        self.get_entity().touching_lava.load(Ordering::SeqCst)
    }

    /// Vanilla `Strider.floatStrider`: while inside lava, either stand on the lava's
    /// surface or float up slowly.
    fn float_strider(&self) {
        if !self.is_in_lava() {
            return;
        }

        let entity = self.get_entity();
        let block_pos = entity.block_pos.load();
        let world = entity.world.load();
        let above_is_lava = world
            .get_fluid_and_fluid_state(&block_pos.up())
            .0
            .has_tag(&tag::Fluid::MINECRAFT_LAVA);
        // `getLiquidCollisionShape` is `Block.column(16, 0, 8)`, i.e. the lava block's
        // top half; vanilla `CollisionContext.isAbove` is `entityBottom > y + 0.5 - 1e-5`.
        let above_liquid_shape =
            entity.bounding_box.load().min.y > f64::from(block_pos.0.y) + 0.5 - 1.0E-5;

        if above_liquid_shape && !above_is_lava {
            entity.on_ground.store(true, Ordering::SeqCst);
        } else {
            let velocity = entity.velocity.load();
            entity.set_velocity(pumpkin_util::math::vector3::Vector3::new(
                velocity.x * 0.5,
                velocity.y * 0.5 + 0.05,
                velocity.z * 0.5,
            ));
        }
    }
}

/// Vanilla `Strider.StriderGoToLavaGoal`: walk towards nearby lava and stop once inside it.
pub struct StriderGoToLavaGoal {
    move_to_target_pos_goal: MoveToTargetPosGoal<Self>,
}

impl StriderGoToLavaGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        let mut this = Box::new(Self {
            move_to_target_pos_goal: MoveToTargetPosGoal::new(ParentHandle::none(), 1.0, 8, 2),
        });

        // SAFETY: `this` heap allocation address is pinned in Box and outlives `ParentHandle` references.
        this.move_to_target_pos_goal.move_to_target_pos = unsafe { ParentHandle::new(&this) };

        this
    }
}

impl Goal for StriderGoToLavaGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.move_to_target_pos_goal.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.move_to_target_pos_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.move_to_target_pos_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.move_to_target_pos_goal.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.move_to_target_pos_goal.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.move_to_target_pos_goal.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.move_to_target_pos_goal.controls()
    }
}

impl MoveToTargetPos for StriderGoToLavaGoal {
    fn is_target_pos(&self, world: Arc<World>, block_pos: BlockPos) -> bool {
        world.get_block(&block_pos).id == Block::LAVA.id
            && world.block_registry.is_pathfindable(
                world.get_block(&block_pos.up()),
                world.get_block_state(&block_pos.up()),
                PathComputationType::Land,
            )
    }

    /// Vanilla `StriderGoToLavaGoal.getMoveToTarget` returns the lava block itself,
    /// not the block above it.
    fn get_move_to_target(&self, block_pos: &BlockPos) -> BlockPos {
        *block_pos
    }

    /// Vanilla `StriderGoToLavaGoal.shouldRecalculatePath` uses a 20 tick interval.
    fn should_recalculate_path(&self, try_ticks: i32) -> bool {
        try_ticks % 20 == 0
    }

    fn can_use(&self, mob: &dyn Mob) -> bool {
        Self::not_in_lava(mob)
    }

    fn can_continue_to_use(&self, mob: &dyn Mob) -> bool {
        Self::not_in_lava(mob)
    }

    /// Vanilla `StriderGoToLavaGoal.canContinueToUse` replaces the base method instead of
    /// calling super, so the `tryTicks`/`maxStayTicks` give-up bounds do not apply.
    fn continue_respects_try_ticks(&self) -> bool {
        false
    }
}

impl StriderGoToLavaGoal {
    fn not_in_lava(mob: &dyn Mob) -> bool {
        !mob.get_living_entity()
            .is_some_and(|living| living.is_in_lava())
    }
}

impl AgeableMob for StriderEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for StriderEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_STRIDER_FOOD)
            || item_stack.item == &Item::WARPED_FUNGUS
    }
}

impl Mob for StriderEntity {
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_bool("Saddle", self.is_saddled());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(saddle) = nbt.get_bool("Saddle") {
            self.set_saddled(saddle);
        } else if let Some(saddle_byte) = nbt.get_byte("Saddle") {
            self.set_saddled(saddle_byte == 1);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_item_steerable(&self) -> Option<&dyn ItemSteerable> {
        Some(self)
    }

    fn is_saddled(&self) -> bool {
        self.saddled.load(Ordering::Relaxed)
    }

    fn can_be_saddled(&self) -> bool {
        self.mob_entity.living_entity.entity.is_alive() && !self.is_baby()
    }

    fn set_saddled(&self, saddled: bool) {
        self.saddled.store(saddled, Ordering::Relaxed);
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();

        // Vanilla `Strider.tick`: a strider suffocates unless it is standing in/on a warm
        // block, inside lava, or riding another non-suffocating strider.
        let entity = self.get_entity();
        if self.mob_entity.is_no_ai() {
            return;
        }
        let pos = entity.pos.load();
        let block_pos = entity.block_pos.load();
        let world = entity.world.load();
        let (block_inside, _) = world.get_block_and_state(&block_pos);
        let on_pos = entity
            .get_supporting_block_pos()
            .unwrap_or_else(|| BlockPos::containing(pos.x, pos.y - 0.2, pos.z));
        let (block_on, _) = world.get_block_and_state(&on_pos);
        let in_warm_blocks = block_inside.has_tag(&tag::Block::MINECRAFT_STRIDER_WARM_BLOCKS)
            || block_on.has_tag(&tag::Block::MINECRAFT_STRIDER_WARM_BLOCKS)
            || self.is_in_lava();
        let on_warm_strider = entity.get_vehicle().is_some_and(|vehicle| {
            vehicle
                .cast_any()
                .downcast_ref::<StriderEntity>()
                .is_some_and(|strider| !strider.is_suffocating())
        });
        self.set_suffocating(!in_warm_blocks && !on_warm_strider);
    }

    fn post_tick(&self) {
        self.float_strider();
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::strider::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::strider::DATA_SUFFOCATING,
            self.is_suffocating(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let item = item_stack.get_item();

        if item == &Item::SADDLE && self.can_be_saddled() && !self.is_saddled() {
            self.set_saddled(true);
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            let entity = self.get_entity();
            let world = entity.world.load();
            let pos = entity.pos.load();
            world.play_sound(Sound::EntityStriderSaddle, SoundCategory::Neutral, &pos);
            return true;
        }

        if self.is_saddled() && !self.is_food(item_stack) {
            let world = player.world();
            let ent = &self.mob_entity.living_entity.entity;
            if let Some(vehicle) = world.get_entity_by_id(ent.entity_id)
                && let Some(passenger) = world.get_player_by_id(player.entity_id())
            {
                ent.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
                return true;
            }
        }

        self.animal_interact(player, item_stack, Sound::EntityStriderAmbient)
    }
}

impl ItemSteerable for StriderEntity {
    fn boost(&self) -> bool {
        self.steering.boost()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

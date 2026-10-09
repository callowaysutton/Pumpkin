use std::sync::Arc;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{active_target::ActiveTargetGoal, bow_attack::BowAttackGoal},
    ai::pathfinder::node::PathType,
    mob::{
        Mob, MobEntity, equipment::RegionalDifficulty, skeleton::SkeletonEntityBase,
        spawn::SpawnGroupData,
    },
};
use crate::world::World;

pub struct WitherSkeletonEntity {
    entity: Arc<SkeletonEntityBase>,
}

impl WitherSkeletonEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let skeleton = SkeletonEntityBase::new(entity);

        let mut goal_selector = skeleton
            .mob_entity
            .goals_selector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A wither skeleton spawns with a stone sword, so vanilla's
        // `reassessWeaponGoal` never adds the ranged bow goal.
        goal_selector.remove_goals::<BowAttackGoal>();
        drop(goal_selector);

        let mut target_selector = skeleton
            .mob_entity
            .target_selector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Vanilla `WitherSkeleton.registerGoals` adds a piglin target on top of the
        // skeleton goals (vanilla targets `AbstractPiglin`, so both piglin types).
        target_selector.add_goal(
            3,
            ActiveTargetGoal::with_default(&skeleton.mob_entity, &EntityType::PIGLIN, true),
        );
        target_selector.add_goal(
            3,
            ActiveTargetGoal::with_default(&skeleton.mob_entity, &EntityType::PIGLIN_BRUTE, true),
        );
        drop(target_selector);

        let mut navigator = skeleton
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Vanilla `WitherSkeleton` constructor: its pathfinder may cross lava.
        navigator.set_pathfinding_malus(PathType::Lava, 8.0);
        drop(navigator);

        Arc::new(Self { entity: skeleton })
    }
}

impl Mob for WitherSkeletonEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.entity.finalize_spawn(world, group_data);
        // Vanilla `AbstractSkeleton.finalizeSpawn` populates the equipment slots through
        // the subclass override.
        let pos = self.entity.mob_entity.living_entity.entity.pos.load();
        let difficulty = RegionalDifficulty::at(world, pos);
        self.populate_default_equipment_slots(world, &difficulty);
        // Vanilla `WitherSkeleton.finalizeSpawn`: melee damage base of 4.0.
        self.entity
            .mob_entity
            .living_entity
            .set_attribute_base(&Attributes::ATTACK_DAMAGE, 4.0);
        group_data
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        // Vanilla `WitherSkeleton.populateDefaultEquipmentSlots`: no armor roll; it always
        // enters the world with a stone sword.
        let living = &self.entity.mob_entity.living_entity;
        let mut equipment = living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        equipment.put(
            &EquipmentSlot::MAIN_HAND,
            ItemStack::new(1, &Item::STONE_SWORD),
        );
    }

    fn populate_default_equipment_enchantments(&self, _difficulty: &RegionalDifficulty) {
        // Vanilla `WitherSkeleton.populateDefaultEquipmentEnchantments` is empty: the sword
        // spawns unenchanted.
    }

    fn on_attack(&self, target: &dyn EntityBase) {
        // Vanilla `WitherSkeleton.doHurtTarget`: a successful melee hit applies
        // WITHER for 200 ticks to living targets, regardless of difficulty.
        if let Some(living) = target.get_living_entity() {
            living.add_effect(Effect {
                effect_type: &StatusEffect::WITHER,
                duration: 200,
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: false,
            });
        }
    }
}

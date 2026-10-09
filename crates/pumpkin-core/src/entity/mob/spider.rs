use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::potion::Effect;
use pumpkin_util::difficulty::Difficulty;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::{
    Entity, EntityBase,
    ai::{
        goal::{
            active_target::{ActiveTargetGoal, TargetCondition},
            avoid_entity::AvoidEntityGoal,
            leap_at_target::LeapAtTargetGoal,
            look_around::RandomLookAroundGoal,
            look_at_entity::LookAtEntityGoal,
            revenge::RevengeGoal,
            spider_attack::SpiderAttackGoal,
            swim::SwimGoal,
            wander_around::WanderAroundGoal,
        },
        pathfinder::Navigator,
    },
    mob::{
        Mob, MobEntity,
        equipment::RegionalDifficulty,
        spawn::{SpawnGroupData, finalize_spawn},
    },
    passive::armadillo::ArmadilloEntity,
    r#type::from_type,
};
use crate::world::World;

pub struct SpiderEntity {
    pub mob_entity: MobEntity,
    pub is_climbing: AtomicBool,
}

impl SpiderEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let spider = Self {
            mob_entity,
            is_climbing: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(spider);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut navigator = mob_arc
                .mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *navigator = Navigator::wall_climber();
        };

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(1, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                2,
                Box::new(
                    AvoidEntityGoal::new(&EntityType::ARMADILLO, 6.0, 1.0, 1.2)
                        .with_avoid_predicate(|entity| {
                            entity
                                .cast_any()
                                .downcast_ref::<ArmadilloEntity>()
                                .is_none_or(|armadillo| !armadillo.is_scared())
                        }),
                ),
            );
            goal_selector.add_goal(3, Box::new(LeapAtTargetGoal::new(0.4)));
            goal_selector.add_goal(4, SpiderAttackGoal::new(1.0, true));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(0.8)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(6, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true)
                    .when(TargetCondition::NoDaylight),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::IRON_GOLEM, true)
                    .when(TargetCondition::NoDaylight),
            );
        };

        mob_arc
    }

    pub fn is_climbing(&self) -> bool {
        self.is_climbing.load(Ordering::Relaxed)
    }

    pub fn set_climbing(&self, climbing: bool) {
        set_climbing_flag(&self.mob_entity, &self.is_climbing, climbing);
    }
}

/// Vanilla `Spider.tick`: climb while pressed against a wall. Shared by cave spiders,
/// which inherit `Spider` in vanilla.
pub fn spider_climbing_tick(mob_entity: &MobEntity, is_climbing: &AtomicBool) {
    let entity = &mob_entity.living_entity.entity;
    if !entity.is_alive() {
        return;
    }

    let vel = entity.velocity.load();
    let is_colliding_horizontally = vel.x.abs() < 1e-4 && vel.z.abs() < 1e-4;
    set_climbing_flag(mob_entity, is_climbing, is_colliding_horizontally);
}

fn set_climbing_flag(mob_entity: &MobEntity, is_climbing: &AtomicBool, climbing: bool) {
    if is_climbing.swap(climbing, Ordering::Relaxed) != climbing {
        let flags = i8::from(climbing);
        mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::spider::DATA_FLAGS_ID, flags);
    }
}

/// Vanilla `Spider.finalizeSpawn` (also used by the cave spider): a skeleton jockey,
/// and on hard difficulty a random effect shared by the whole spawn group.
pub fn finalize_spider_spawn(
    mob: &MobEntity,
    world: &Arc<World>,
    group_data: Option<SpawnGroupData>,
) -> Option<SpawnGroupData> {
    mob.finalize_spawn_base();
    let mut rng = rand::rng();
    let entity = &mob.living_entity.entity;
    let pos = entity.pos.load();

    if rng.random_range(0..100) == 0 {
        let skeleton = from_type(&EntityType::SKELETON, pos, world, Uuid::new_v4());
        skeleton.get_entity().set_rotation(entity.yaw.load(), 0.0);
        finalize_spawn(&skeleton, world, None);
        mob.add_pending_rider(skeleton);
    }

    let group_data = group_data.unwrap_or_else(|| {
        let difficulty = RegionalDifficulty::at(world, pos);
        let effect = (difficulty.base_difficulty == Difficulty::Hard
            && rng.random::<f32>() < 0.1 * difficulty.special_multiplier)
            .then(|| match rng.random_range(0..5) {
                0 | 1 => &StatusEffect::SPEED,
                2 => &StatusEffect::STRENGTH,
                3 => &StatusEffect::REGENERATION,
                _ => &StatusEffect::INVISIBILITY,
            });
        SpawnGroupData::SpiderEffects(effect)
    });

    let SpawnGroupData::SpiderEffects(effect) = &group_data;
    if let Some(effect_type) = effect {
        mob.living_entity.add_effect(Effect {
            effect_type,
            duration: -1,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        });
    }
    Some(group_data)
}

impl Mob for SpiderEntity {
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        finalize_spider_spawn(&self.mob_entity, world, group_data)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        spider_climbing_tick(&self.mob_entity, &self.is_climbing);
    }

    fn on_climbable(&self) -> bool {
        self.is_climbing()
    }
}

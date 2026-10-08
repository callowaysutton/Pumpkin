use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::potion::Effect;
use pumpkin_protocol::java::client::play::{CGameEvent, GameEvent};

use crate::entity::ai::control::guardian_move_control::GuardianMoveControl;
use crate::entity::ai::goal::guardian_attack::{Guardian, GuardianAttackGoal};
use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        active_target::ActiveTargetGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, move_towards_restriction::MoveTowardsRestrictionGoal,
        swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
};

pub struct ElderGuardianEntity {
    pub mob_entity: MobEntity,
    tick_count: AtomicI32,
    /// Trips the stall goal after a beam, vanilla `GuardianAttackGoal.stop`.
    wander_trigger: Arc<AtomicBool>,
}

impl ElderGuardianEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let wander_trigger = Arc::new(AtomicBool::new(false));
        let guardian = Self {
            mob_entity,
            tick_count: AtomicI32::new(0),
            wander_trigger: wander_trigger.clone(),
        };
        let mob_arc = Arc::new(guardian);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        // Vanilla `ElderGuardian` constructor: `setPersistenceRequired()`.
        mob_arc
            .mob_entity
            .persistence_required
            .store(true, Ordering::Relaxed);

        {
            let mut navigator = mob_arc
                .mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            navigator.set_pathfinding_malus(PathType::Water, 0.0);
        };

        {
            let mut move_control = mob_arc
                .mob_entity
                .move_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *move_control = Box::new(GuardianMoveControl::default());
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

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            // Vanilla `Guardian.registerGoals`; the elder stroll interval is 400.
            let guardian_weak: Weak<Self> = Arc::downgrade(&mob_arc);
            goal_selector.add_goal(4, Box::new(GuardianAttackGoal::new(guardian_weak)));
            goal_selector.add_goal(5, Box::new(MoveTowardsRestrictionGoal::new(1.0)));
            {
                let mut wander = WanderAroundGoal::with_interval(1.0, 400);
                wander.set_shared_trigger(wander_trigger);
                goal_selector.add_goal(7, Box::new(wander));
            };
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(
                8,
                Box::new(LookAtEntityGoal::new(
                    mob_weak.clone(),
                    &EntityType::ELDER_GUARDIAN,
                    12.0,
                    0.01,
                    false,
                )),
            );
            goal_selector.add_goal(9, Box::new(RandomLookAroundGoal::default()));

            // Vanilla targets players, squid, glow squid and axolotls, but only from
            // more than 3 blocks away (the beam needs the range).
            let weak_for_predicate = Arc::downgrade(&mob_arc);
            let guardian_pos_predicate =
                move |target: &crate::entity::living::LivingEntity,
                      _world: &crate::world::World| {
                    let entity_type = target.entity.entity_type;
                    if entity_type != &EntityType::PLAYER
                        && entity_type != &EntityType::SQUID
                        && entity_type != &EntityType::GLOW_SQUID
                        && entity_type != &EntityType::AXOLOTL
                    {
                        return false;
                    }
                    let Some(guardian) = weak_for_predicate.upgrade() else {
                        return false;
                    };
                    let guardian_pos = guardian.get_entity().pos.load();
                    let target_pos = target.entity.pos.load();
                    guardian_pos.squared_distance_to_vec(&target_pos) > 9.0
                };
            target_selector.add_goal(
                1,
                ActiveTargetGoal::predicated(&mob_arc.mob_entity, 10, true, guardian_pos_predicate),
            );
        };

        mob_arc
    }
}

impl Guardian for ElderGuardianEntity {
    fn attack_duration(&self) -> i32 {
        60
    }

    fn is_elder(&self) -> bool {
        true
    }

    fn trigger_random_stroll(&self) {
        self.wander_trigger.store(true, Ordering::Relaxed);
    }
}

impl Mob for ElderGuardianEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        if !entity.is_alive() {
            return;
        }

        // Vanilla `ElderGuardian.customServerAiStep`: the fatigue pulse...
        let ticks = self.tick_count.fetch_add(1, Ordering::Relaxed) + 1;
        if ticks % 1200 == 0 {
            let pos = entity.pos.load();
            let world = entity.world.load();
            let players = world.get_nearby_players(pos, 50.0);
            let packet = CGameEvent::new(
                GameEvent::PlayElderGuardianMobAppearance,
                if entity.is_silent() { 0.0 } else { 1.0 },
            );

            for player in players {
                player.try_send_client_packet(&packet);
                player.living_entity.add_effect(Effect {
                    effect_type: &StatusEffect::MINING_FATIGUE,
                    duration: 6000,
                    amplifier: 2,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                });
            }
        }

        // ... and the home it is pulled back to, vanilla `setHomeTo(blockPosition(), 16)`.
        if !self.mob_entity.has_position_target() {
            self.mob_entity
                .position_target
                .store(entity.block_pos.load());
            self.mob_entity
                .position_target_range
                .store(16, Ordering::Relaxed);
        }
    }

    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        // Vanilla `Guardian.hurtServer`: a hit always makes it swim elsewhere.
        self.trigger_random_stroll();
        if let Some(src) = source
            && let Some(living) = src.get_living_entity()
        {
            let _ = living.damage(src, 2.0, DamageType::THORNS);
        }
    }
}

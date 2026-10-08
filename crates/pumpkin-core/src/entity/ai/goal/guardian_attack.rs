use std::sync::Weak;

use pumpkin_data::entity::EntityStatus;
use pumpkin_data::{damage::DamageType, entity::EntityType, tracked_data};
use pumpkin_util::Difficulty;

use crate::entity::Entity;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::mob::Mob;

/// The parts of a guardian the laser beam goal needs. Implemented by both
/// `GuardianEntity` and `ElderGuardianEntity`, which are separate structs here.
pub trait Guardian: Mob + Send + Sync {
    /// Vanilla `Guardian.getAttackDuration`: 80 ticks, 60 for the elder guardian.
    fn attack_duration(&self) -> i32;

    /// Vanilla `GuardianAttackGoal.elder`: the elder guardian does not drop the beam
    /// when the target comes closer than 3 blocks.
    fn is_elder(&self) -> bool;

    /// Vanilla `GuardianAttackGoal.stop`: `randomStrollGoal.trigger()`, so the guardian
    /// swims somewhere else after a beam.
    fn trigger_random_stroll(&self);
}

/// Vanilla `Guardian.GuardianAttackGoal`: charges a laser beam at the target.
///
/// After `attack_duration()` ticks it damages the target with indirect magic, plus the
/// melee hit vanilla adds. The beam itself is drawn by the client from the
/// `DATA_ID_ATTACK_TARGET` synced value, which this goal sets.
pub struct GuardianAttackGoal<G: Guardian + ?Sized> {
    guardian: Weak<G>,
    attack_time: i32,
}

impl<G: Guardian + ?Sized> GuardianAttackGoal<G> {
    #[must_use]
    pub const fn new(guardian: Weak<G>) -> Self {
        Self {
            guardian,
            attack_time: 0,
        }
    }

    fn stop_navigation(guardian: &G) {
        guardian
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }
}

impl<G: Guardian + ?Sized + 'static> Goal for GuardianAttackGoal<G> {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let Some(guardian) = self.guardian.upgrade() else {
            return false;
        };
        guardian
            .get_mob_entity()
            .get_target()
            .is_some_and(|target| target.get_entity().is_alive())
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        let Some(guardian) = self.guardian.upgrade() else {
            return false;
        };
        // Vanilla `super.canContinueToUse()`: the target must still be alive.
        let Some(target) = guardian.get_mob_entity().get_target() else {
            return false;
        };
        if !target.get_entity().is_alive() {
            return false;
        }
        if guardian.is_elder() {
            return true;
        }
        let guardian_pos = guardian.get_entity().pos.load();
        let target_pos = target.get_entity().pos.load();
        guardian_pos.squared_distance_to_vec(&target_pos) > 9.0
    }

    fn start(&mut self, _mob: &dyn Mob) {
        let Some(guardian) = self.guardian.upgrade() else {
            return;
        };
        self.attack_time = -10;
        Self::stop_navigation(&*guardian);
        if let Some(target) = guardian.get_mob_entity().get_target() {
            guardian
                .get_mob_entity()
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_entity_with_range(&target, 90.0, 90.0);
        }
        guardian
            .get_entity()
            .velocity_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        let Some(guardian) = self.guardian.upgrade() else {
            return;
        };
        set_active_attack_target(guardian.get_entity(), 0);
        guardian.get_mob_entity().set_target(None);
        guardian.trigger_random_stroll();
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let Some(guardian) = self.guardian.upgrade() else {
            return;
        };
        let Some(target) = guardian.get_mob_entity().get_target() else {
            return;
        };

        Self::stop_navigation(&*guardian);
        guardian
            .get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 90.0, 90.0);

        if !guardian.has_line_of_sight(target.get_entity()) {
            guardian.get_mob_entity().set_target(None);
            return;
        }

        self.attack_time += 1;
        if self.attack_time == 0 {
            let entity = guardian.get_entity();
            set_active_attack_target(entity, entity.entity_id);
            if !entity.is_silent() {
                entity.world.load().broadcast_entity_event(
                    entity,
                    EntityStatus::GuardianAttackSound,
                    None,
                );
            }
        } else if self.attack_time >= guardian.attack_duration() {
            let world = guardian.get_entity().world.load();
            let mut magic_damage = 1.0;
            if world.level_info.load().difficulty == Difficulty::Hard {
                magic_damage += 2.0;
            }
            if guardian.is_elder() {
                magic_damage += 2.0;
            }

            let _ = target.damage_with_context(
                target.as_ref(),
                magic_damage,
                DamageType::INDIRECT_MAGIC,
                None,
                Some(guardian.get_entity()),
                Some(guardian.get_entity()),
            );
            // Vanilla `doHurtTarget`: the melee hit that follows the beam.
            guardian
                .get_mob_entity()
                .try_attack(guardian.get_entity(), target.as_ref());
            guardian.get_mob_entity().set_target(None);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// Vanilla `Guardian.setActiveAttackTarget`: the synced entity id the client draws the
/// beam towards, 0 meaning "no beam".
fn set_active_attack_target(entity: &Entity, target_id: i32) {
    let key = if entity.entity_type == &EntityType::ELDER_GUARDIAN {
        tracked_data::elder_guardian::DATA_ID_ATTACK_TARGET
    } else {
        tracked_data::guardian::DATA_ID_ATTACK_TARGET
    };
    entity.set_synced_data(key, target_id);
}

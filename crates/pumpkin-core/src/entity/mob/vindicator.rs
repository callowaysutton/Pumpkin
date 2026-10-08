use std::sync::{Arc, Weak};

use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::Sound;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        Controls, Goal, active_target::ActiveTargetGoal, break_door::BreakDoorGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, open_door::OpenDoorGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            HoldGroundAttackGoal, ObtainRaidLeaderBannerGoal, PathfindToRaidGoal, Raider,
            RaiderCelebrationGoal, RaiderData, RaiderMoveThroughVillageGoal,
        },
    },
};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct VindicatorEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    pub is_johnny: AtomicBool,
}

impl VindicatorEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let vindicator = Self {
            mob_entity,
            raider_data: RaiderData::default(),
            is_johnny: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(vindicator);
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
            goal_selector.add_goal(1, Box::new(OpenDoorGoal::new(false)));
            goal_selector.add_goal(2, Box::new(VindicatorBreakDoorGoal::new()));
            goal_selector.add_goal(1, Box::new(ObtainRaidLeaderBannerGoal));
            goal_selector.add_goal(2, Box::new(HoldGroundAttackGoal::new(10.0)));
            goal_selector.add_goal(3, Box::new(MeleeAttackGoal::new(1.0, false)));
            goal_selector.add_goal(4, Box::new(LongDistancePatrolGoal::new(0.7, 0.595)));
            goal_selector.add_goal(4, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(4, Box::new(PathfindToRaidGoal::default()));
            goal_selector.add_goal(5, Box::new(RaiderCelebrationGoal));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            target_selector.add_goal(
                1,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::VILLAGER, true),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::IRON_GOLEM, true),
            );
            target_selector.add_goal(
                4,
                Box::new(VindicatorJohnnyAttackGoal::new(&mob_arc.mob_entity)),
            );
        };

        mob_arc
    }
}

impl Mob for VindicatorEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }

    fn is_johnny(&self) -> bool {
        if self.is_johnny.load(Ordering::Relaxed) {
            return true;
        }
        // Vanilla `Vindicator.setCustomName` flips `isJohnny` when a custom name equal to
        // "Johnny" is applied. Pumpkin has no custom-name hook, so derive it lazily here:
        // a nametag or NBT name is picked up on the next goal evaluation.
        let entity = self.get_entity();
        if let Some(name) = &**entity.custom_name.load()
            && name.clone().get_text() == "Johnny"
        {
            self.is_johnny.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);
        if self.is_johnny() {
            nbt.put_bool("Johnny", true);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);
        if nbt.get_bool("Johnny").unwrap_or(false) {
            self.is_johnny.store(true, Ordering::Relaxed);
        }
    }
}

impl PatrollingMonster for VindicatorEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }
}

impl Raider for VindicatorEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityVindicatorCelebrate
    }
}

/// Vanilla `Vindicator.VindicatorBreakDoorGoal`: only while an active raid, and only on
/// Normal or Hard difficulty.
struct VindicatorBreakDoorGoal {
    break_door_goal: BreakDoorGoal,
}

impl VindicatorBreakDoorGoal {
    fn new() -> Self {
        let predicate: crate::entity::ai::goal::break_door::DifficultyPredicate =
            Arc::new(|d| matches!(d, Difficulty::Normal | Difficulty::Hard));
        Self {
            // Vanilla passes 6 as the door break time.
            break_door_goal: BreakDoorGoal::with_door_break_time(6, predicate),
        }
    }
}

impl Goal for VindicatorBreakDoorGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(raider) = mob.as_raider() else {
            return false;
        };
        raider.has_active_raid()
            && mob
                .get_random()
                .random_range(0..crate::entity::ai::goal::to_goal_ticks(10))
                == 0
            && self.break_door_goal.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.as_raider().is_some_and(Raider::has_active_raid)
            && self.break_door_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.break_door_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.break_door_goal.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.break_door_goal.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Vindicator.VindicatorJohnnyAttackGoal`: a Johnny vindicator attacks every
/// attackable living entity within follow range.
struct VindicatorJohnnyAttackGoal {
    active_target_goal: Box<ActiveTargetGoal>,
}

impl VindicatorJohnnyAttackGoal {
    fn new(mob_entity: &MobEntity) -> Self {
        Self {
            // reciprocal chance 0: every search attempt. `attackable()` excludes only
            // armor stands in vanilla.
            active_target_goal: ActiveTargetGoal::predicated(
                mob_entity,
                0,
                true,
                |target, _world| target.entity.entity_type != &EntityType::ARMOR_STAND,
            ),
        }
    }
}

impl Goal for VindicatorJohnnyAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.is_johnny() && self.active_target_goal.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.is_johnny() && self.active_target_goal.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.active_target_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.active_target_goal.stop(mob);
    }

    fn controls(&self) -> Controls {
        self.active_target_goal.controls()
    }
}

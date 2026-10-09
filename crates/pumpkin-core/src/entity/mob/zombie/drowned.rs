use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::Block;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Hand;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::ai::goal::ranged_attack::RangedAttackGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::ai::util::default_random_pos;
use crate::entity::custom_sound::CustomSound;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::zombie::ZombieEntityBase;
use crate::entity::mob::{Mob, MobEntity, RangedAttackMob};
use crate::entity::projectile::arrow::ArrowPickup;
use crate::entity::projectile::trident::TridentEntity;
use crate::entity::{Entity, EntityBase};
use crate::world::World;

/// Vanilla `Drowned`.
///
/// The shared zombie goals registered by [`ZombieEntityBase`] are kept and the
/// drowned-specific ones are layered on top, exactly like `Drowned.addBehaviourGoals`
/// does after `Zombie.registerGoals`.
pub struct DrownedEntity {
    pub entity: Arc<ZombieEntityBase>,
    searching_for_land: AtomicBool,
    /// Trident aim error in hundredths; `-1` means "use the difficulty default".
    ranged_attack_uncertainty: AtomicI32,
}

impl DrownedEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        Self::with_can_break_doors(entity, false)
    }

    #[must_use]
    pub fn with_can_break_doors(entity: Entity, can_break_doors: bool) -> Arc<Self> {
        let entity = ZombieEntityBase::with_can_break_doors(entity, can_break_doors);
        let drowned = Self {
            entity,
            searching_for_land: AtomicBool::new(false),
            ranged_attack_uncertainty: AtomicI32::new(-1),
        };
        let mob_arc = Arc::new(drowned);
        // Vanilla `Drowned.createNavigation`: amphibious path navigation plus a
        // water malus of 0 so the drowned doesn't avoid water.
        mob_arc
            .entity
            .mob_entity
            .set_navigator(crate::entity::ai::pathfinder::Navigator::amphibious(false));
        let ranged_weak: Weak<dyn RangedAttackMob> = {
            let ranged_arc: Arc<dyn RangedAttackMob> = mob_arc.clone();
            Arc::downgrade(&ranged_arc)
        };

        {
            let mut goal_selector = mob_arc
                .entity
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Vanilla `Drowned.addBehaviourGoals` overrides
            // `Zombie.addBehaviourGoals` instead of adding to it, so the plain
            // zombie melee goal is replaced by the drowned pair.
            let mut stopped = goal_selector.remove_goals::<ZombieAttackGoal>();
            for goal in &mut stopped {
                goal.stop(mob_arc.as_ref());
            }

            // Vanilla replaces the shared Zombie melee goal at priority 2 with
            // `DrownedAttackGoal` and adds the trident goal beside it.
            goal_selector.add_goal(1, Box::new(DrownedGoToWaterGoal::new(1.0)));
            goal_selector.add_goal(2, DrownedTridentAttackGoal::new(ranged_weak, 1.0, 40, 10.0));
            goal_selector.add_goal(2, DrownedAttackGoal::new(1.0, false));
            goal_selector.add_goal(5, Box::new(DrownedGoToBeachGoal::new(1.0)));
            goal_selector.add_goal(6, Box::new(DrownedSwimUpGoal::new(1.0)));
        };

        {
            let mut target_selector = mob_arc
                .entity
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Vanilla `Drowned.addBehaviourGoals` targets axolotls as well, which
            // the shared zombie target list does not include.
            target_selector.add_goal(
                3,
                crate::entity::ai::goal::active_target::ActiveTargetGoal::with_default(
                    &mob_arc.entity.mob_entity,
                    &EntityType::AXOLOTL,
                    true,
                ),
            );
        };

        mob_arc
    }

    pub fn set_searching_for_land(&self, searching_for_land: bool) {
        self.searching_for_land
            .store(searching_for_land, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_searching_for_land(&self) -> bool {
        self.searching_for_land.load(Ordering::Relaxed)
    }

    /// Vanilla `Drowned.wantsToSwim`: swimming unless it is heading for land
    /// while none of its targets are in water.
    #[must_use]
    pub fn wants_to_swim(&self) -> bool {
        if self.is_searching_for_land() {
            return true;
        }
        self.entity
            .mob_entity
            .get_target()
            .is_some_and(|target| target.get_entity().is_in_water())
    }

    /// Vanilla `Drowned.okTarget`: any target at night, only submerged ones in daylight.
    #[must_use]
    pub fn ok_target(mob: &dyn Mob, target: Option<&Arc<dyn EntityBase>>) -> bool {
        let Some(target) = target else {
            return false;
        };
        !mob.get_entity().world.load().is_bright_outside() || target.get_entity().is_in_water()
    }

    /// Vanilla `Drowned.closeToNextPos`: the current path node is under 2 blocks away.
    #[must_use]
    fn close_to_next_pos(mob: &dyn Mob) -> bool {
        let navigator = mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(path) = navigator.get_path() else {
            return false;
        };
        let target = path.get_target();
        let dist = mob.get_entity().pos.load().squared_distance_to(
            f64::from(target.0.x),
            f64::from(target.0.y),
            f64::from(target.0.z),
        );
        dist < 4.0
    }

    #[must_use]
    fn main_hand_item(mob: &dyn Mob) -> ItemStack {
        mob.get_mob_entity()
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&pumpkin_data::data_component_impl::EquipmentSlot::MAIN_HAND)
    }

    #[must_use]
    pub fn ranged_attack_uncertainty(&self, difficulty: i32) -> f32 {
        let stored = self.ranged_attack_uncertainty.load(Ordering::Relaxed);
        if stored < 0 {
            // Vanilla `RangedAttackMob.rangedAttackUncertainty`.
            (14 - difficulty * 4) as f32
        } else {
            stored as f32 / 100.0
        }
    }

    pub fn set_ranged_attack_uncertainty(&self, uncertainty: f32) {
        self.ranged_attack_uncertainty
            .store((uncertainty * 100.0) as i32, Ordering::Relaxed);
    }
}

impl Mob for DrownedEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn spawn_as_baby(&self) -> bool {
        self.entity.spawn_as_baby()
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<crate::entity::mob::spawn::SpawnGroupData>,
    ) -> Option<crate::entity::mob::spawn::SpawnGroupData> {
        self.get_mob_entity().finalize_spawn_base();
        // Vanilla `Drowned.finalizeSpawn`: 3% of drowned carry a nautilus shell
        // in the offhand and always drop it.
        let living = &self.entity.mob_entity.living_entity;
        let off_hand_empty = living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&pumpkin_data::data_component_impl::EquipmentSlot::OFF_HAND)
            .is_empty();
        if off_hand_empty && rand::random::<f32>() < 0.03 {
            self.entity.mob_entity.set_item_slot_and_drop_when_killed(
                &pumpkin_data::data_component_impl::EquipmentSlot::OFF_HAND,
                ItemStack::new(1, &Item::NAUTILUS_SHELL),
            );
        }
        group_data
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        // Vanilla `Drowned.populateDefaultEquipmentSlots` overrides the zombie
        // weapon roll entirely, sometimes granting a trident instead.
        if rand::random::<f32>() > 0.9 {
            let roll = rand::random_range(0..16);
            let item = if roll < 10 {
                &Item::TRIDENT
            } else {
                &Item::FISHING_ROD
            };
            self.get_mob_entity().set_item_slot(
                &pumpkin_data::data_component_impl::EquipmentSlot::MAIN_HAND,
                ItemStack::new(1, item),
            );
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.mob_write_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.entity.mob_read_nbt(nbt);
    }

    /// Vanilla `Drowned.isPushedByFluid`: swimming drowned ignore the current.
    fn is_pushed_by_fluids(&self) -> bool {
        !self.get_entity().is_swimming()
    }

    /// Vanilla `Drowned.updateSwimming`.
    fn update_swimming(&self) {
        let entity = self.get_entity();
        let effective_ai = !self.get_mob_entity().is_no_ai();
        entity.set_swimming(effective_ai && entity.is_under_water() && self.wants_to_swim());
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }
}

impl CustomSound for DrownedEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        let is_water = self.get_entity().is_in_water();
        Some(if is_water {
            Sound::EntityDrownedHurtWater
        } else {
            Sound::EntityDrownedHurt
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        let is_water = self.get_entity().is_in_water();
        Some(if is_water {
            Sound::EntityDrownedDeathWater
        } else {
            Sound::EntityDrownedDeath
        })
    }
}

// ---------------------------------------------------------------------------
// Vanilla `Drowned.performRangedAttack`
// ---------------------------------------------------------------------------

impl RangedAttackMob for DrownedEntity {
    fn perform_ranged_attack(&self, target: &Arc<dyn EntityBase>, _power: f32) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let pos = entity.pos.load();

        let main_hand = Self::main_hand_item(self);
        let trident_stack = if main_hand.item == &Item::TRIDENT {
            main_hand
        } else {
            ItemStack::new(1, &Item::TRIDENT)
        };

        let trident_entity = Entity::new(world.clone(), pos, &EntityType::TRIDENT);
        let trident = TridentEntity::new_shot(
            trident_entity,
            entity,
            trident_stack.clone(),
            ArrowPickup::Allowed,
        );
        trident.apply_on_projectile_spawned(&trident_stack);

        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        let target_height = f64::from(target_entity.entity_dimension.load().height);
        let trident_pos = trident.get_entity().pos.load();

        let xd = target_pos.x - pos.x;
        let yd = target_pos.y + target_height / 3.0 - trident_pos.y;
        let zd = target_pos.z - pos.z;
        let horizontal_distance = xd.hypot(zd);

        let difficulty = world.level_info.load().difficulty as i32;
        let uncertainty = f64::from(self.ranged_attack_uncertainty(difficulty));
        trident.set_velocity(xd, yd + horizontal_distance * 0.2, zd, 1.6, uncertainty);

        if !entity.is_silent() {
            world.play_sound(Sound::EntityDrownedShoot, SoundCategory::Hostile, &pos);
        }

        let trident_arc: Arc<dyn EntityBase> = Arc::new(trident);
        world.spawn_entity(trident_arc);
    }
}

// ---------------------------------------------------------------------------
// Vanilla `Drowned.DrownedAttackGoal`
// ---------------------------------------------------------------------------

/// Vanilla `Drowned.DrownedAttackGoal`: a [`ZombieAttackGoal`] gated on `okTarget`.
pub struct DrownedAttackGoal {
    inner: ZombieAttackGoal,
}

impl DrownedAttackGoal {
    #[must_use]
    pub fn new(speed: f64, pause_when_mob_idle: bool) -> Box<Self> {
        Box::new(Self {
            inner: *ZombieAttackGoal::new(speed, pause_when_mob_idle),
        })
    }

    fn ok_target(mob: &dyn Mob) -> bool {
        let target = mob.get_mob_entity().get_target();
        DrownedEntity::ok_target(mob, target.as_ref())
    }
}

impl Goal for DrownedAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.inner.can_start(mob) && Self::ok_target(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob) && Self::ok_target(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.inner.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}

// ---------------------------------------------------------------------------
// Vanilla `Drowned.DrownedTridentAttackGoal`
// ---------------------------------------------------------------------------

/// Vanilla `Drowned.DrownedTridentAttackGoal`: a [`RangedAttackGoal`] that only
/// runs while a trident is held, raising the arm for the duration.
pub struct DrownedTridentAttackGoal {
    inner: RangedAttackGoal,
}

impl DrownedTridentAttackGoal {
    #[must_use]
    pub fn new(
        mob: Weak<dyn RangedAttackMob>,
        speed_modifier: f64,
        attack_interval: i32,
        attack_radius: f32,
    ) -> Box<Self> {
        Box::new(Self {
            inner: RangedAttackGoal::new(mob, speed_modifier, attack_interval, attack_radius),
        })
    }
}

impl Goal for DrownedTridentAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.inner.can_start(mob) && DrownedEntity::main_hand_item(mob).item == &Item::TRIDENT
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.inner.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.inner.start(mob);
        mob.get_mob_entity().set_attacking(true);
        let living = &mob.get_mob_entity().living_entity;
        let stack = DrownedEntity::main_hand_item(mob);
        living.set_active_hand(Hand::Right, stack, 72000);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.inner.stop(mob);
        mob.get_mob_entity().living_entity.clear_active_hand();
        mob.get_mob_entity().set_attacking(false);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.inner.tick(mob);
    }

    fn should_run_every_tick(&self) -> bool {
        self.inner.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}

// ---------------------------------------------------------------------------
// Vanilla `Drowned.DrownedGoToWaterGoal`
// ---------------------------------------------------------------------------

/// Vanilla `Drowned.DrownedGoToWaterGoal`: walks a drowning mob back into water.
pub struct DrownedGoToWaterGoal {
    speed_modifier: f64,
    wanted_x: f64,
    wanted_y: f64,
    wanted_z: f64,
    has_target: bool,
}

impl DrownedGoToWaterGoal {
    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            wanted_x: 0.0,
            wanted_y: 0.0,
            wanted_z: 0.0,
            has_target: false,
        }
    }

    /// Vanilla `DrownedGoToWaterGoal.getWaterPos`.
    fn water_pos(mob: &dyn Mob) -> Option<Vector3<f64>> {
        let world = mob.get_entity().world.load();
        let base = mob.get_entity().block_pos.load();
        for _ in 0..10 {
            let dx = rand::random_range(0..20) - 10;
            let dy = 2 - rand::random_range(0..8);
            let dz = rand::random_range(0..20) - 10;
            let pos = BlockPos::new(base.0.x + dx, base.0.y + dy, base.0.z + dz);
            if world.get_block(&pos).id == Block::WATER.id {
                return Some(Vector3::new(
                    f64::from(pos.0.x) + 0.5,
                    f64::from(pos.0.y),
                    f64::from(pos.0.z) + 0.5,
                ));
            }
        }
        None
    }
}

impl Goal for DrownedGoToWaterGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !mob.get_entity().world.load().is_bright_outside() || mob.get_entity().is_in_water() {
            return false;
        }
        let Some(pos) = Self::water_pos(mob) else {
            return false;
        };
        self.wanted_x = pos.x;
        self.wanted_y = pos.y;
        self.wanted_z = pos.z;
        self.has_target = true;
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_idle()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if !self.has_target {
            return;
        }
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal {
                current_progress: mob.get_entity().pos.load(),
                destination: Vector3::new(self.wanted_x, self.wanted_y, self.wanted_z),
                speed: self.speed_modifier,
            });
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

// ---------------------------------------------------------------------------
// Vanilla `Drowned.DrownedGoToBeachGoal`
// ---------------------------------------------------------------------------

/// Vanilla `Drowned.DrownedGoToBeachGoal`: swims a submerged drowned to a dry
/// block near the surface so it can leave the water at night.
pub struct DrownedGoToBeachGoal {
    speed_modifier: f64,
    target_pos: BlockPos,
    trying_time: i32,
    safe_waiting_time: i32,
    cooldown: i32,
    reached: bool,
}

impl DrownedGoToBeachGoal {
    const RANGE: i32 = 8;
    const MAX_Y_DIFFERENCE: i32 = 2;
    const GIVE_UP_TICKS: i32 = 1200;
    const STAY_TICKS: i32 = 1200;
    const INTERVAL_TICKS: i32 = 200;

    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            target_pos: BlockPos::new(0, 0, 0),
            trying_time: 0,
            safe_waiting_time: 0,
            cooldown: 0,
            reached: false,
        }
    }

    /// Vanilla `DrownedGoToBeachGoal.isValidTarget`: two air blocks over a block
    /// the drowned can stand on.
    fn is_valid_target(_mob: &dyn Mob, world: &World, pos: &BlockPos) -> bool {
        let above = pos.up();
        if !world.get_block_state(&above).is_air() || !world.get_block_state(&above.up()).is_air() {
            return false;
        }
        // Vanilla `BlockState.entityCanStandOn`.
        world.get_block_state(pos).is_full_cube()
    }

    fn can_target(&self, mob: &dyn Mob) -> bool {
        let world = mob.get_entity().world.load();
        Self::is_valid_target(mob, &world, &self.target_pos)
    }

    /// Vanilla `MoveToBlockGoal.findNearestBlock`.
    fn find_target_pos(&mut self, mob: &dyn Mob) -> bool {
        let world = mob.get_entity().world.load();
        let base = mob.get_entity().block_pos.load();
        let mut found = false;

        for k in -Self::MAX_Y_DIFFERENCE..=Self::MAX_Y_DIFFERENCE {
            for l in 0..=Self::RANGE {
                let mut m = 0;
                while m <= l {
                    let mut n = if m < l && m > -l { l } else { 0 };
                    while n <= l {
                        let pos = BlockPos::new(base.0.x + m, base.0.y + k - 1, base.0.z + n);
                        if Self::is_valid_target(mob, &world, &pos) {
                            self.target_pos = pos;
                            found = true;
                            break;
                        }
                        n = if n > 0 { -n } else { 1 - n };
                    }
                    if found {
                        break;
                    }
                    m = if m > 0 { -m } else { 1 - m };
                }
                if found {
                    break;
                }
            }
            if found {
                break;
            }
        }
        found
    }

    fn move_to_target(&self, mob: &dyn Mob) {
        let target = self.target_pos.up();
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal {
                current_progress: mob.get_entity().pos.load(),
                destination: Vector3::new(
                    f64::from(target.0.x) + 0.5,
                    f64::from(target.0.y),
                    f64::from(target.0.z) + 0.5,
                ),
                speed: self.speed_modifier,
            });
    }
}

impl Goal for DrownedGoToBeachGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return false;
        }
        self.cooldown = i32::midpoint(
            Self::INTERVAL_TICKS,
            mob.get_random().random_range(0..Self::INTERVAL_TICKS),
        );

        let world = mob.get_entity().world.load();
        if world.is_bright_outside()
            || !mob.get_entity().is_in_water()
            || mob.get_entity().pos.load().y < f64::from(world.sea_level - 3)
        {
            return false;
        }
        self.find_target_pos(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.trying_time >= -self.safe_waiting_time
            && self.trying_time <= Self::GIVE_UP_TICKS
            && self.can_target(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        // Vanilla `DrownedGoToBeachGoal.start` clears the land search flag.
        if let Some(drowned) = mob.cast_any().downcast_ref::<DrownedEntity>() {
            drowned.set_searching_for_land(false);
        }
        self.move_to_target(mob);
        self.trying_time = 0;
        let bound = mob.get_random().random_range(0..Self::STAY_TICKS);
        self.safe_waiting_time = mob.get_random().random_range(0..bound) + Self::STAY_TICKS;
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let target = self.target_pos.up();
        let center = Vector3::new(
            f64::from(target.0.x) + 0.5,
            f64::from(target.0.y) + 0.5,
            f64::from(target.0.z) + 0.5,
        );
        if center.squared_distance_to_vec(&mob.get_entity().pos.load()) < 1.0 {
            self.reached = true;
            self.trying_time -= 1;
        } else {
            self.reached = false;
            self.trying_time += 1;
            if self.trying_time % 40 == 0 {
                self.move_to_target(mob);
            }
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::JUMP
    }
}

// ---------------------------------------------------------------------------
// Vanilla `Drowned.DrownedSwimUpGoal`
// ---------------------------------------------------------------------------

/// Vanilla `Drowned.DrownedSwimUpGoal`: brings a drowned below the sea floor back
/// up towards the surface.
pub struct DrownedSwimUpGoal {
    speed_modifier: f64,
    stuck: bool,
}

impl DrownedSwimUpGoal {
    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            stuck: false,
        }
    }

    fn can_use(mob: &dyn Mob, sea_level: i32) -> bool {
        let world = mob.get_entity().world.load();
        !world.is_bright_outside()
            && mob.get_entity().is_in_water()
            && mob.get_entity().pos.load().y < f64::from(sea_level - 2)
    }
}

impl Goal for DrownedSwimUpGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        Self::can_use(mob, mob.get_entity().world.load().sea_level)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !self.stuck && Self::can_use(mob, mob.get_entity().world.load().sea_level)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let world = mob.get_entity().world.load();
        let sea_level = world.sea_level;
        let y = mob.get_entity().pos.load().y;
        if y >= f64::from(sea_level - 1) {
            return;
        }

        let navigation_done = mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_idle();
        if !navigation_done && !DrownedEntity::close_to_next_pos(mob) {
            return;
        }

        let next_pos = default_random_pos::get_pos_towards(
            mob,
            4,
            8,
            Vector3::new(
                mob.get_entity().pos.load().x,
                f64::from(sea_level - 1),
                mob.get_entity().pos.load().z,
            ),
            std::f64::consts::FRAC_PI_2,
        );
        let Some(next_pos) = next_pos else {
            self.stuck = true;
            return;
        };

        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal {
                current_progress: mob.get_entity().pos.load(),
                destination: next_pos,
                speed: self.speed_modifier,
            });
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.stuck = false;
        if let Some(drowned) = mob.cast_any().downcast_ref::<DrownedEntity>() {
            drowned.set_searching_for_land(true);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(drowned) = mob.cast_any().downcast_ref::<DrownedEntity>() {
            drowned.set_searching_for_land(false);
        }
    }
}

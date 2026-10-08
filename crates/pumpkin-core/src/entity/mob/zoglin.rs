use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::entity_status::EntityStatus;
use pumpkin_data::environment_attribute::Activity;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::int_provider::UniformIntProvider;

use crate::entity::ai::brain::activity::ActivityData;
use crate::entity::ai::brain::behavior::{
    BehaviorControl, DoNothing, GateBehavior, LookAtTargetSink, OrderPolicy, RunningPolicy,
    downcast_mob, melee_attack::MeleeAttack, move_to_target_sink::MoveToTargetSink,
    random_stroll::RandomStroll, set_entity_look_target_sometimes::SetEntityLookTargetSometimes,
    set_walk_target_from_attack_target_if_target_out_of_reach::SetWalkTargetFromAttackTargetIfTargetOutOfReach,
    set_walk_target_from_look_target::SetWalkTargetFromLookTarget, start_attacking::StartAttacking,
    stop_attacking_if_target_invalid::StopAttackingIfTargetInvalid,
    utils::is_other_target_much_further_away_than_current_attack_target,
};
use crate::entity::ai::brain::memory::{PackedMemories, types};
use crate::entity::ai::brain::sensing::{SensorType, is_entity_attackable};
use crate::entity::ai::brain::{Brain, BrainProvider, BrainTick};
use crate::entity::mob::{Mob, MobEntity, hoglin::hurt_and_throw_target};
use crate::entity::{Entity, EntityBase};
use crate::world::World;

pub struct ZoglinEntity {
    pub mob_entity: MobEntity,
    pub is_baby: AtomicBool,
    /// Vanilla `Zoglin.attackAnimationRemainingTicks`: ticks left on the broadcast attack
    /// animation before the next hit can re-fire it.
    attack_animation_remaining_ticks: AtomicI32,
}

impl ZoglinEntity {
    pub const XP_REWARD: u32 = 5;
    /// Vanilla `HoglinBase.ATTACK_ANIMATION_DURATION`.
    const ATTACK_ANIMATION_DURATION: i32 = 10;
    const BABY_ATTACK_DAMAGE: f64 = 0.5;
    /// Vanilla `Zoglin.setAttackTarget`: the memory expires after `ATTACK_DURATION`.
    const ATTACK_TARGET_TTL: i64 = 200;
    /// Vanilla `Zoglin.hurtServer`: only re-target when the attacker is within this much of
    /// the current attack target.
    const ATTACK_TARGET_CUT_OFF_DISTANCE: f64 = 4.0;
    pub const BABY_DIMENSIONS: EntityDimensions = EntityDimensions {
        width: 0.75,
        height: 0.85,
        eye_height: 0.625,
    };
}

/// Vanilla `Zoglin.initIdleActivity`: `UniformInt.of(30, 60)` between curious looks.
const ZOGLIN_LOOK_SOMETIMES_INTERVAL: UniformIntProvider = UniformIntProvider::new(30, 60);

/// Vanilla `Zoglin.ATTACK_INTERVAL`.
const ATTACK_INTERVAL: i32 = 40;
/// Vanilla `Zoglin.BABY_ATTACK_INTERVAL`.
const BABY_ATTACK_INTERVAL: i32 = 15;

/// Vanilla `Zoglin.BRAIN_PROVIDER`: the visible-entities and player sensors; the memories
/// the activities need are registered with them.
static ZOGLIN_BRAIN_PROVIDER: BrainProvider = BrainProvider {
    memory_types: &[],
    sensor_types: &[
        SensorType::NearestLivingEntities,
        SensorType::NearestPlayers,
    ],
    activities: |_mob| {
        vec![
            init_core_activity(),
            init_idle_activity(),
            init_fight_activity(),
        ]
    },
};

/// Vanilla `Zoglin.initCoreActivity`.
fn init_core_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Core,
        0,
        vec![
            Box::new(LookAtTargetSink::core()) as Box<dyn BehaviorControl>,
            Box::new(MoveToTargetSink::core()),
        ],
    )
}

/// Vanilla `Zoglin.initIdleActivity`: keep hunting for a target, glance around, and amble.
fn init_idle_activity() -> ActivityData {
    ActivityData::with_priority_start(
        Activity::Idle,
        10,
        vec![
            Box::new(StartAttacking::create(
                "StartAttacking",
                Box::new(find_nearest_valid_attack_target),
            )),
            Box::new(SetEntityLookTargetSometimes::create(
                "SetEntityLookTargetSometimes",
                8.0,
                ZOGLIN_LOOK_SOMETIMES_INTERVAL,
            )),
            // Vanilla `RunOne`: ordered, one child per pick, 2 : 2 : 1 weights.
            Box::new(GateBehavior::new(
                "RunOne",
                Vec::new(),
                Vec::new(),
                OrderPolicy::Ordered,
                RunningPolicy::RunOne,
                vec![
                    (
                        Box::new(RandomStroll::stroll("RandomStroll", 0.4))
                            as Box<dyn BehaviorControl>,
                        2,
                    ),
                    (
                        Box::new(SetWalkTargetFromLookTarget::create(
                            "SetWalkTargetFromLookTarget",
                            0.4,
                            3,
                        )),
                        2,
                    ),
                    (Box::new(DoNothing::new(30, 60)), 1),
                ],
            )),
        ],
    )
}

/// Vanilla `Zoglin.initFightActivity`: gated on the attack-target memory, which vanilla erases
/// again when the activity stops (`MemoryModuleType.ATTACK_TARGET` single-memory ctor).
fn init_fight_activity() -> ActivityData {
    ActivityData::with_gate_memory(
        Activity::Fight,
        10,
        vec![
            Box::new(SetWalkTargetFromAttackTargetIfTargetOutOfReach::create(
                "SetWalkTargetFromAttackTargetIfTargetOutOfReach",
                1.0,
            )) as Box<dyn BehaviorControl>,
            // Vanilla `BehaviorBuilder.triggerIf(Zoglin::isAdult, MeleeAttack.create(40))`.
            Box::new(MeleeAttack::create(
                "MeleeAttack",
                is_adult_at_tick,
                ATTACK_INTERVAL,
            )),
            // Vanilla `BehaviorBuilder.triggerIf(Zoglin::isBaby, MeleeAttack.create(15))`.
            Box::new(MeleeAttack::create(
                "MeleeAttackBaby",
                is_baby_at_tick,
                BABY_ATTACK_INTERVAL,
            )),
            Box::new(StopAttackingIfTargetInvalid::create(
                "StopAttackingIfTargetInvalid",
            )),
        ],
        types::ATTACK_TARGET.id(),
    )
}

impl ZoglinEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        {
            let mut attributes = mob_entity
                .living_entity
                .attributes
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(health) = attributes.get_mut(&Attributes::MAX_HEALTH.id) {
                health.base_value = 40.0;
                health.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(speed) = attributes.get_mut(&Attributes::MOVEMENT_SPEED.id) {
                speed.base_value = 0.3;
                speed.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(knockback_res) = attributes.get_mut(&Attributes::KNOCKBACK_RESISTANCE.id) {
                knockback_res.base_value = 0.6;
                knockback_res.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(attack_kb) = attributes.get_mut(&Attributes::ATTACK_KNOCKBACK.id) {
                attack_kb.base_value = 1.0;
                attack_kb.dirty.store(true, Ordering::Relaxed);
            }
            if let Some(damage) = attributes.get_mut(&Attributes::ATTACK_DAMAGE.id) {
                damage.base_value = 6.0;
                damage.dirty.store(true, Ordering::Relaxed);
            }
        }
        mob_entity.living_entity.health.store(40.0);

        let zoglin = Self {
            mob_entity,
            is_baby: AtomicBool::new(false),
            attack_animation_remaining_ticks: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(zoglin);

        // Vanilla `LivingEntity` constructor: the brain exists before any NBT load replaces it.
        let brain = ZOGLIN_BRAIN_PROVIDER.make_brain(mob_arc.as_ref(), &PackedMemories::empty());
        *mob_arc
            .mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = brain;

        mob_arc
    }

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.is_baby.load(Ordering::Relaxed)
    }

    /// Vanilla `Zoglin.setBaby`; growing up keeps the lowered attack damage, as in vanilla.
    pub fn set_baby(&self, baby: bool) {
        self.mob_entity
            .set_baby_flag(&self.is_baby, tracked_data::zoglin::DATA_BABY_ID, baby);
        let living = &self.mob_entity.living_entity;
        living.entity.entity_dimension.store(if baby {
            Self::BABY_DIMENSIONS
        } else {
            Entity::type_dimensions(living.entity.entity_type)
        });
        if baby {
            living.set_attribute_base(&Attributes::ATTACK_DAMAGE, Self::BABY_ATTACK_DAMAGE);
        }
    }

    /// Vanilla `Zoglin.playAngrySound`.
    fn play_angry_sound(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        entity.world.load().play_sound(
            Sound::EntityZoglinAngry,
            SoundCategory::Hostile,
            &entity.pos.load(),
        );
    }

    /// Vanilla `Zoglin.updateActivity`: fight while an attack target is set, idle otherwise.
    fn update_activity(&self) {
        let mut brain = self
            .mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let old_activity = brain.get_active_non_core_activity();
        brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
        let new_activity = brain.get_active_non_core_activity();
        if new_activity == Some(Activity::Fight) && old_activity != Some(Activity::Fight) {
            self.play_angry_sound();
        }
        let attacking = brain.has_memory_value(types::ATTACK_TARGET.id());
        drop(brain);
        self.mob_entity.set_attacking(attacking);
    }

    /// Vanilla `Zoglin.setAttackTarget`: a 200-tick, expiring attack target.
    fn set_attack_target(&self, target: Arc<dyn EntityBase>) {
        let mut brain = self
            .mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        brain.erase(types::CANT_REACH_WALK_TARGET_SINCE.id());
        brain.set_with_expiry(types::ATTACK_TARGET, target, Self::ATTACK_TARGET_TTL);
    }
}

/// Vanilla `Zoglin.findNearestValidAttackTarget`: the closest visible entity that is
/// neither a zoglin nor a creeper, still attackable.
fn find_nearest_valid_attack_target(tick: &mut BrainTick<'_>) -> Option<Arc<dyn EntityBase>> {
    let ctx = tick.visibility();
    let visible = tick.brain.get(types::NEAREST_VISIBLE_LIVING_ENTITIES)?;
    visible.find_closest(&ctx, |target| {
        let entity = target.get_entity();
        entity.entity_type != &EntityType::ZOGLIN
            && entity.entity_type != &EntityType::CREEPER
            && is_entity_attackable(&ctx, target.as_ref())
    })
}

impl Mob for ZoglinEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `Zoglin.customServerAiStep`: brain tick followed by the activity flip.
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        self.mob_entity.tick_brain(self);
        self.update_activity();
    }

    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        ZOGLIN_BRAIN_PROVIDER.make_brain(self, packed)
    }

    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        group_data: Option<crate::entity::mob::spawn::SpawnGroupData>,
    ) -> Option<crate::entity::mob::spawn::SpawnGroupData> {
        if rand::random::<f32>() < 0.2 {
            self.set_baby(true);
        }
        self.mob_entity.finalize_spawn_base();
        group_data
    }

    fn spawn_as_baby(&self) -> bool {
        self.set_baby(true);
        true
    }

    /// Vanilla `Zoglin.aiStep`: run the attack animation countdown down.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        if self
            .attack_animation_remaining_ticks
            .load(Ordering::Relaxed)
            > 0
        {
            self.attack_animation_remaining_ticks
                .fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Vanilla `Zoglin.hurtServer`: chase the attacker back unless it is much further away
    /// from the current attack target.
    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        let Some(attacker) = source else {
            return;
        };
        if attacker.get_living_entity().is_none() || !self.can_attack(attacker) {
            return;
        }
        let brain = self
            .mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Vanilla `hurtServer` returns early past this check while `BehaviorUtils` decides;
        // keep the read without holding it afterwards.
        if is_other_target_much_further_away_than_current_attack_target(
            &brain,
            self.get_entity(),
            attacker,
            Self::ATTACK_TARGET_CUT_OFF_DISTANCE,
        ) {
            return;
        }
        drop(brain);
        let entity_id = attacker.get_entity().entity_id;
        let world = self.get_entity().world.load();
        if let Some(attacker) = world.get_entity_by_id(entity_id) {
            self.set_attack_target(attacker);
        }
    }

    /// Vanilla `Zoglin.doHurtTarget`: raise the attack animation, broadcast it, play the
    /// attack sound, and run `HoglinBase.hurtAndThrowTarget`.
    fn do_hurt_target(&self, target: &dyn EntityBase) -> bool {
        self.attack_animation_remaining_ticks
            .store(Self::ATTACK_ANIMATION_DURATION, Ordering::Relaxed);
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        world.send_entity_status(entity, EntityStatus::StartAttacking, None);
        world.play_sound(
            Sound::EntityZoglinAttack,
            SoundCategory::Hostile,
            &entity.pos.load(),
        );
        hurt_and_throw_target(self, self.is_baby(), target)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("IsBaby", self.is_baby());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_baby(nbt.get_bool("IsBaby").unwrap_or(false));
    }
}

/// Vanilla `Zoglin.isBaby`, read through the ticked body like vanilla's
/// `BehaviorBuilder.triggerIf(Zoglin::isBaby, ...)`.
fn is_baby_at_tick(tick: &BrainTick<'_>) -> bool {
    downcast_mob::<ZoglinEntity>(tick.mob).is_some_and(ZoglinEntity::is_baby)
}

/// Vanilla `Zoglin.isAdult`, read through the ticked body like vanilla's trigger on it.
fn is_adult_at_tick(tick: &BrainTick<'_>) -> bool {
    !is_baby_at_tick(tick)
}

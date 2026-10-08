use super::BrainTick;
use super::memory::MemoryModuleId;

pub mod do_nothing;
pub mod gate;
pub mod look_at_target_sink;
pub mod melee_attack;
pub mod move_to_target_sink;
pub mod one_shot;
pub mod random_stroll;
pub mod set_entity_look_target_sometimes;
pub mod set_walk_target_from_attack_target_if_target_out_of_reach;
pub mod set_walk_target_from_look_target;
pub mod shuffling_list;
pub mod start_attacking;
pub mod stop_attacking_if_target_invalid;
pub mod timed;
pub mod utils;

pub use do_nothing::DoNothing;
pub use gate::{GateBehavior, OrderPolicy, RunningPolicy, run_one, run_one_with_conditions};
pub use look_at_target_sink::LookAtTargetSink;
pub use move_to_target_sink::MoveToTargetSink;
pub use one_shot::{OneShot, Trigger};
pub use set_entity_look_target_sometimes::SetEntityLookTargetSometimes;
pub use shuffling_list::ShufflingList;
pub use timed::{Behavior, DEFAULT_DURATION, Timed};

/// Downcasts the ticked mob to its concrete type, standing in for vanilla's generic behavior
/// parameter (vanilla compiles `<T extends Mob>` where brains need the mob's own members).
#[must_use]
pub fn downcast_mob<T: 'static>(mob: &dyn crate::entity::mob::Mob) -> Option<&T> {
    (mob as &dyn std::any::Any).downcast_ref::<T>()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Stopped,
    Running,
}

pub trait BehaviorControl: Send + Sync {
    fn status(&self) -> Status;
    fn required_memories(&self) -> &[MemoryModuleId];
    fn try_start(&mut self, tick: &mut BrainTick<'_>) -> bool;
    fn tick_or_stop(&mut self, tick: &mut BrainTick<'_>);
    fn do_stop(&mut self, tick: &mut BrainTick<'_>);
    fn debug_string(&self) -> String;
}

pub struct BehaviorEntry {
    pub priority: i32,
    pub activity: pumpkin_data::environment_attribute::Activity,
    pub behavior: Box<dyn BehaviorControl>,
}

#[must_use]
pub fn required_memories_of(
    conditions: &[(MemoryModuleId, super::memory::MemoryStatus)],
) -> Box<[MemoryModuleId]> {
    conditions.iter().map(|(id, _)| *id).collect()
}

use std::sync::Arc;

use crate::entity::EntityBase;
use crate::entity::ai::brain::memory::nearest_visible::NearestVisibleLivingEntities;
use crate::entity::ai::brain::memory::types;
use crate::entity::ai::brain::{BrainTick, VisibilityContext};

use super::{MemoryModuleId, Sensor};

/// Vanilla `NearestLivingEntitySensor`: the living entities around the mob, closest first,
/// and the visibility wrapper over the same list.
#[derive(Default)]
pub struct NearestLivingEntitySensor;

const REQUIRED: [MemoryModuleId; 2] = [
    types::NEAREST_LIVING_ENTITIES.id(),
    types::NEAREST_VISIBLE_LIVING_ENTITIES.id(),
];

impl Sensor for NearestLivingEntitySensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &REQUIRED
    }

    /// Vanilla `NearestLivingEntitySensor.doTick`.
    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let nearby = {
            let ctx: VisibilityContext<'_> = tick.visibility();
            let mob = tick.mob.get_entity();
            let mob_pos = mob.pos.load();
            let inflated = mob.bounding_box.load().expand_all(ctx.follow_range);
            let mut nearby: Vec<Arc<dyn EntityBase>> = ctx.world.get_all_at_box(&inflated);
            let mob_id = mob.entity_id;
            nearby.retain(|candidate| {
                candidate.get_entity().entity_id != mob_id
                    && candidate.get_entity().is_alive()
                    && candidate.get_living_entity().is_some()
            });
            nearby.sort_by(|first, second| {
                let first_distance = first
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&mob_pos);
                let second_distance = second
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&mob_pos);
                first_distance
                    .partial_cmp(&second_distance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            nearby
        };

        tick.brain
            .set(types::NEAREST_LIVING_ENTITIES, nearby.clone());
        tick.brain.set(
            types::NEAREST_VISIBLE_LIVING_ENTITIES,
            NearestVisibleLivingEntities::new(nearby),
        );
    }
}

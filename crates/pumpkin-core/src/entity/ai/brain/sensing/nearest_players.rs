use std::sync::Arc;

use crate::entity::EntityBase;
use crate::entity::ai::brain::memory::types;
use crate::entity::ai::brain::sensing::{is_entity_attackable, is_entity_targetable};
use crate::entity::ai::brain::{BrainTick, VisibilityContext};
use crate::entity::player::Player;

use super::{MemoryModuleId, Sensor};

/// Vanilla `PlayerSensor`: players around the mob within the follow range, plus their
/// targetable and attackable subsets.
#[derive(Default)]
pub struct NearestPlayersSensor;

const REQUIRED: [MemoryModuleId; 4] = [
    types::NEAREST_PLAYERS.id(),
    types::NEAREST_VISIBLE_PLAYER.id(),
    types::NEAREST_VISIBLE_ATTACKABLE_PLAYER.id(),
    types::NEAREST_VISIBLE_ATTACKABLE_PLAYERS.id(),
];

impl Sensor for NearestPlayersSensor {
    fn requires(&self) -> &'static [MemoryModuleId] {
        &REQUIRED
    }

    /// Vanilla `PlayerSensor.doTick`.
    fn do_tick(&mut self, tick: &mut BrainTick<'_>) {
        let (nearby, visible, attackable) = {
            let ctx: VisibilityContext<'_> = tick.visibility();
            let mob_pos = tick.mob.get_entity().pos.load();
            let follow_range = ctx.follow_range;
            let mut nearby: Vec<Arc<Player>> = ctx
                .world
                .players
                .load()
                .iter()
                .filter(|player| {
                    !player.is_spectator()
                        && player
                            .get_entity()
                            .pos
                            .load()
                            .squared_distance_to_vec(&mob_pos)
                            <= follow_range * follow_range
                })
                .cloned()
                .collect();
            sort_by_distance(&mut nearby, &mob_pos);

            let visible: Vec<Arc<Player>> = nearby
                .iter()
                .filter(|player| is_entity_targetable(&ctx, player.as_ref()))
                .cloned()
                .collect();
            let attackable: Vec<Arc<Player>> = visible
                .iter()
                .filter(|player| is_entity_attackable(&ctx, player.as_ref()))
                .cloned()
                .collect();
            (nearby, visible, attackable)
        };

        tick.brain.set(types::NEAREST_PLAYERS, nearby);
        tick.brain
            .set_optional(types::NEAREST_VISIBLE_PLAYER, visible.first().cloned());
        tick.brain.set(
            types::NEAREST_VISIBLE_ATTACKABLE_PLAYERS,
            attackable.clone(),
        );
        tick.brain.set_optional(
            types::NEAREST_VISIBLE_ATTACKABLE_PLAYER,
            attackable.first().cloned(),
        );
    }
}

/// Vanilla `Comparator.comparingDouble(body::distanceToSqr)`: closest player first.
fn sort_by_distance(
    players: &mut [Arc<Player>],
    mob_pos: &pumpkin_util::math::vector3::Vector3<f64>,
) {
    players.sort_by(|first, second| {
        let first_distance = first
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(mob_pos);
        let second_distance = second
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(mob_pos);
        first_distance
            .partial_cmp(&second_distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

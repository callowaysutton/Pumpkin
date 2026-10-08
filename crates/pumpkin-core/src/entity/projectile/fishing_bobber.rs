use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::entity::item::ItemEntity;
use crate::entity::projectile::{ProjectileHit, is_projectile};
use crate::world::loot::{LootContextParameters, generate_loot_from_handle};
use crate::{
    entity::{Entity, EntityBase, living::LivingEntity, player::Player},
    server::Server,
};
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;

pub struct FishingBobberEntity {
    pub entity: Entity,
    pub owner_id: i32,
    pub hooked_entity_id: AtomicI32,
    pub in_ground: AtomicBool,
    pub has_hit: AtomicBool,
    pub wait_countdown: AtomicI32,
    pub bite_countdown: AtomicI32,
}

impl FishingBobberEntity {
    const WATER_INERTIA: f64 = 0.8;
    const AIR_INERTIA: f64 = 0.92;
    const GRAVITY: f64 = 0.03;

    pub fn new(entity: Entity, owner: &Player) -> Self {
        let mut owner_pos = owner.living_entity.entity.pos.load();
        owner_pos.y += owner.living_entity.entity.get_eye_height() - 0.1;
        entity.pos.store(owner_pos);

        Self {
            entity,
            owner_id: owner.living_entity.entity.entity_id,
            hooked_entity_id: AtomicI32::new(0),
            in_ground: AtomicBool::new(false),
            has_hit: AtomicBool::new(false),
            wait_countdown: AtomicI32::new(rand::random::<i32>().abs() % 600 + 100),
            bite_countdown: AtomicI32::new(0),
        }
    }

    /// Pulls a hooked entity towards the bobber's owner. Ports vanilla
    /// `FishingHook#pullEntity`.
    fn pull_entity(&self, entity: &dyn EntityBase) {
        let world = self.entity.world.load();
        let Some(owner) = world.get_entity_by_id(self.owner_id) else {
            return;
        };
        let owner_pos = owner.get_entity().pos.load();
        let bobber_pos = self.entity.pos.load();
        let delta = (owner_pos - bobber_pos).multiply(0.1, 0.1, 0.1);
        entity.get_entity().add_velocity(delta);
    }

    /// Ports vanilla `FishingHook#retrieve`. Returns the durability damage the
    /// rod should take (0 when nothing was caught).
    pub fn retrieve(&self, player: &Player) -> i32 {
        let world = self.entity.world.load();
        let hooked_id = self.hooked_entity_id.load(Ordering::Relaxed);
        let mut damage = 0;

        if hooked_id != 0
            && let Some(hooked) = world.get_entity_by_id(hooked_id)
        {
            self.pull_entity(hooked.as_ref());
            damage = if hooked.get_entity().entity_type == &EntityType::ITEM {
                3
            } else {
                5
            };
        } else if self.bite_countdown.load(Ordering::Relaxed) > 0 {
            let rod = player.inventory().held_item();
            let params = LootContextParameters {
                position: Some(self.entity.pos.load()),
                tool: Some(rod),
                this_entity: Some(self.entity.entity_type),
                luck: 0.0,
                ..LootContextParameters::default()
            };

            if let Some(loot_table) = world.get_loot_table("minecraft:gameplay/fishing") {
                let seed: i64 = rand::random();
                let owner_pos = player.get_entity().pos.load();
                let bobber_pos = self.entity.pos.load();
                let delta = owner_pos - bobber_pos;
                let speed = delta.length().sqrt() * 0.08;

                for stack in generate_loot_from_handle(&loot_table, seed, &params) {
                    let velocity = delta.multiply(0.1, 0.1, 0.1).add_raw(0.0, speed, 0.0);
                    let item_entity = Entity::new(world.clone(), bobber_pos, &EntityType::ITEM);
                    let mut item_event =
                        crate::plugin::api::events::entity::item_spawn::ItemSpawnEvent::new(
                            item_entity.entity_id,
                            bobber_pos,
                            stack.item.registry_key.to_string(),
                        );
                    if let Some(server) = world.server.upgrade() {
                        server
                            .plugin_manager
                            .fire_blocking(&server, &mut item_event);
                    }
                    if item_event.cancelled {
                        continue;
                    }
                    let item = Arc::new(ItemEntity::new_with_velocity(
                        item_entity,
                        stack.clone(),
                        velocity,
                        ItemEntity::DEFAULT_PICKUP_DELAY,
                    ));
                    world.spawn_entity(item);

                    let orb_pos = Vector3::new(owner_pos.x, owner_pos.y + 0.5, owner_pos.z + 0.5);
                    ExperienceOrbEntity::spawn(&world, orb_pos, rand::random_range(1..=6) as u32);

                    if stack
                        .item
                        .has_tag(&pumpkin_data::tag::Item::MINECRAFT_FISHES)
                    {
                        player.increment_stat(
                            pumpkin_data::statistic::StatisticCategory::Custom,
                            pumpkin_data::statistic::CustomStatistic::FishCaught as i32,
                            1,
                        );
                    }

                    player.trigger_advancement(
                        crate::entity::player::advancement::trigger::AdvancementTrigger::FishedItem {
                            item_id: format!("minecraft:{}", stack.item.registry_key),
                        },
                    );
                }
            }

            damage = 1;
        }

        if self.in_ground.load(Ordering::Relaxed) {
            damage = 2;
        }

        self.entity.remove();
        damage
    }

    #[expect(clippy::too_many_lines)]
    pub fn process_tick(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load();

        if self.in_ground.load(Ordering::Relaxed) {
            return;
        }

        let hooked_id = self.hooked_entity_id.load(Ordering::Relaxed);
        if hooked_id != 0 {
            if let Some(hooked) = world.get_entity_by_id(hooked_id) {
                if hooked.get_entity().removed.load(Ordering::Relaxed) {
                    self.hooked_entity_id.store(0, Ordering::Relaxed);
                } else {
                    let mut hooked_pos = hooked.get_entity().pos.load();
                    hooked_pos.y += hooked.get_entity().get_eye_height() * 0.8;
                    entity.set_pos(hooked_pos);
                    return;
                }
            } else {
                self.hooked_entity_id.store(0, Ordering::Relaxed);
            }
        }

        let mut velocity = entity.velocity.load();
        let start_pos = entity.pos.load();

        if entity.touching_water.load(Ordering::Relaxed) {
            velocity.y += 0.02; // Buoyancy

            let bite = self.bite_countdown.load(Ordering::Relaxed);
            if bite > 0 {
                self.bite_countdown.store(bite - 1, Ordering::Relaxed);
                if bite % 5 == 0 {
                    world.spawn_particle(
                        entity.pos.load(),
                        Vector3::new(0.1f32, 0.1f32, 0.1f32),
                        0.0,
                        5,
                        pumpkin_data::particle::Particle::Bubble,
                    );
                }
            } else {
                let wait = self.wait_countdown.load(Ordering::Relaxed);
                if wait > 0 {
                    self.wait_countdown.store(wait - 1, Ordering::Relaxed);
                } else {
                    // Start bite
                    self.bite_countdown.store(40, Ordering::Relaxed);
                    self.wait_countdown
                        .store(rand::random::<i32>().abs() % 600 + 100, Ordering::Relaxed);

                    world.play_sound(
                        Sound::EntityFishingBobberSplash,
                        SoundCategory::Neutral,
                        &entity.pos.load(),
                    );
                }
            }
        } else {
            velocity.y -= Self::GRAVITY;
        }

        let inertia = if entity.touching_water.load(Ordering::Relaxed) {
            Self::WATER_INERTIA
        } else {
            Self::AIR_INERTIA
        };
        velocity = velocity.multiply(inertia, inertia, inertia);
        entity.velocity.store(velocity);

        let new_pos = start_pos.add(&velocity);

        let search_box = BoundingBox::new(
            Vector3::new(
                start_pos.x.min(new_pos.x),
                start_pos.y.min(new_pos.y),
                start_pos.z.min(new_pos.z),
            ),
            Vector3::new(
                start_pos.x.max(new_pos.x),
                start_pos.y.max(new_pos.y),
                start_pos.z.max(new_pos.z),
            ),
        )
        .expand(0.3, 0.3, 0.3);

        // Basic block collision to stop bobber
        let (block_cols, _) = world.get_block_collisions(search_box, caller);
        if !block_cols.is_empty() {
            self.in_ground.store(true, Ordering::Relaxed);
            entity.velocity.store(Vector3::new(0.0, 0.0, 0.0));
            return;
        }

        entity.set_pos(new_pos);

        let candidates = world.get_entities_at_box(&search_box);
        for cand in candidates {
            if cand.get_entity().entity_id == self.owner_id
                || cand.get_entity().entity_id == entity.entity_id
            {
                continue;
            }

            if is_projectile(cand.get_entity().entity_type) {
                continue;
            }

            let ebb = cand.get_entity().bounding_box.load().expand(0.3, 0.3, 0.3);
            if ebb.intersects(&search_box) {
                self.hooked_entity_id
                    .store(cand.get_entity().entity_id, Ordering::Relaxed);
                entity.set_synced_data(
                    pumpkin_data::tracked_data::fishing_bobber::HOOKED_ENTITY,
                    cand.get_entity().entity_id + 1,
                );
                return;
            }
        }
    }
}

impl EntityBase for FishingBobberEntity {
    fn get_owner_id(&self) -> Option<i32> {
        Some(self.owner_id)
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
    fn on_hit(&self, _hit: ProjectileHit) {
        self.has_hit.store(true, Ordering::Relaxed);
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.process_tick(caller);
    }
}

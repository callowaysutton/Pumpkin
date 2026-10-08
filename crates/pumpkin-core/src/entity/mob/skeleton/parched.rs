use std::sync::Arc;

use pumpkin_data::effect::StatusEffect;

use crate::entity::{
    Entity,
    mob::{Mob, MobEntity, skeleton::SkeletonEntityBase},
    projectile::arrow::ArrowEntity,
};

pub struct ParchedSkeletonEntity {
    entity: Arc<SkeletonEntityBase>,
}

impl ParchedSkeletonEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = SkeletonEntityBase::new(entity);
        let parched = Self { entity };
        Arc::new(parched)
    }
}

impl Mob for ParchedSkeletonEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    /// Vanilla `Parched#getArrow`: the arrows it fires inflict Weakness for 600 ticks.
    fn modify_fired_arrow(&self, arrow: &ArrowEntity) {
        arrow.add_effect(&StatusEffect::WEAKNESS, 600, 0);
    }

    /// Vanilla `Parched#getHardAttackInterval` / `#getAttackInterval`.
    fn bow_attack_interval(&self, hard: bool) -> Option<i32> {
        Some(if hard { 50 } else { 70 })
    }

    /// Vanilla `Parched#canBeAffected`: a Parched is immune to Weakness.
    fn can_be_affected(&self, effect: &pumpkin_data::potion::Effect) -> bool {
        effect.effect_type != &StatusEffect::WEAKNESS
    }
}

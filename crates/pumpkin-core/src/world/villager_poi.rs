use std::{collections::HashMap, sync::Weak};

use pumpkin_data::{
    Block, BlockState,
    block_properties::BedPart,
    block_properties::BedProperties,
    tag::{self, Taggable},
    villager::VillagerProfession,
};
use pumpkin_util::math::position::BlockPos;

use crate::entity::EntityBase;

/// Vanilla POI type for a bed's head: `minecraft:home`.
pub const POI_TYPE_HOME: &str = "minecraft:home";
/// Vanilla POI type for a bell: `minecraft:meeting`.
pub const POI_TYPE_MEETING: &str = "minecraft:meeting";
/// Maximum number of tickets a `minecraft:meeting` POI has in vanilla.
pub const MEETING_MAX_TICKETS: i32 = 32;

struct JobSite {
    profession: VillagerProfession,
    owner: Option<Weak<dyn EntityBase>>,
}

#[derive(Default)]
pub struct VillagerPoiStorage {
    job_sites: HashMap<BlockPos, JobSite>,
}

impl VillagerPoiStorage {
    fn live_owner(owner: &Weak<dyn EntityBase>) -> Option<std::sync::Arc<dyn EntityBase>> {
        owner.upgrade().filter(|entity| {
            entity
                .get_living_entity()
                .is_none_or(|living| living.health.load() > 0.0)
        })
    }

    pub fn update_block(&mut self, position: BlockPos, block: &Block) {
        let Some(profession) = profession_for_block(block) else {
            self.job_sites.remove(&position);
            return;
        };
        let site = self.job_sites.entry(position).or_insert(JobSite {
            profession,
            owner: None,
        });
        if site.profession != profession {
            *site = JobSite {
                profession,
                owner: None,
            };
        }
    }

    pub fn claim(
        &mut self,
        position: BlockPos,
        block: &Block,
        owner: Weak<dyn EntityBase>,
        expected_profession: Option<VillagerProfession>,
    ) -> Option<VillagerProfession> {
        self.update_block(position, block);
        let site = self.job_sites.get_mut(&position)?;
        if expected_profession.is_some_and(|profession| profession != site.profession) {
            return None;
        }
        if let Some(current_owner) = site.owner.as_ref().and_then(Self::live_owner)
            && current_owner.get_entity().entity_uuid != owner.upgrade()?.get_entity().entity_uuid
        {
            return None;
        }
        site.owner = Some(owner);
        Some(site.profession)
    }

    pub fn release(&mut self, position: BlockPos, owner: uuid::Uuid) {
        let Some(site) = self.job_sites.get_mut(&position) else {
            return;
        };
        if site
            .owner
            .as_ref()
            .and_then(Weak::upgrade)
            .is_none_or(|current| current.get_entity().entity_uuid == owner)
        {
            site.owner = None;
        }
    }

    #[must_use]
    pub fn available_job_sites(
        &self,
        origin: BlockPos,
        radius: i32,
        expected_profession: Option<VillagerProfession>,
    ) -> Vec<BlockPos> {
        let radius_squared = i64::from(radius).pow(2);
        let mut sites = self
            .job_sites
            .iter()
            .filter(|(_, site)| {
                expected_profession.is_none_or(|profession| profession == site.profession)
                    && site.owner.as_ref().and_then(Self::live_owner).is_none()
            })
            .filter_map(|(position, _)| {
                let delta = position.0 - origin.0;
                let distance_squared = i64::from(delta.x).pow(2)
                    + i64::from(delta.y).pow(2)
                    + i64::from(delta.z).pow(2);
                (distance_squared <= radius_squared).then_some((distance_squared, *position))
            })
            .collect::<Vec<_>>();
        sites.sort_unstable_by_key(|(distance, _)| *distance);
        sites.into_iter().map(|(_, position)| position).collect()
    }

    #[must_use]
    pub fn get_nearest_job_site(&self, origin: BlockPos, radius: i32) -> Option<BlockPos> {
        let radius_squared = i64::from(radius).pow(2);
        let mut closest = None;
        let mut closest_dist = radius_squared;

        for position in self.job_sites.keys() {
            let delta = position.0 - origin.0;
            let distance_squared =
                i64::from(delta.x).pow(2) + i64::from(delta.y).pow(2) + i64::from(delta.z).pow(2);
            if distance_squared <= closest_dist {
                closest = Some(*position);
                closest_dist = distance_squared;
            }
        }
        closest
    }
}

#[must_use]
pub fn profession_for_block(block: &Block) -> Option<VillagerProfession> {
    match block {
        block if block == &Block::COMPOSTER => Some(VillagerProfession::Farmer),
        block if block == &Block::LECTERN => Some(VillagerProfession::Librarian),
        block if block == &Block::BLAST_FURNACE => Some(VillagerProfession::Armorer),
        block if block == &Block::SMOKER => Some(VillagerProfession::Butcher),
        block if block == &Block::CARTOGRAPHY_TABLE => Some(VillagerProfession::Cartographer),
        block if block == &Block::BREWING_STAND => Some(VillagerProfession::Cleric),
        block if block == &Block::BARREL => Some(VillagerProfession::Fisherman),
        block if block == &Block::FLETCHING_TABLE => Some(VillagerProfession::Fletcher),
        block
            if [
                &Block::CAULDRON,
                &Block::WATER_CAULDRON,
                &Block::LAVA_CAULDRON,
                &Block::POWDER_SNOW_CAULDRON,
            ]
            .contains(&block) =>
        {
            Some(VillagerProfession::Leatherworker)
        }
        block if block == &Block::STONECUTTER => Some(VillagerProfession::Mason),
        block if block == &Block::LOOM => Some(VillagerProfession::Shepherd),
        block if block == &Block::SMITHING_TABLE => Some(VillagerProfession::Toolsmith),
        block if block == &Block::GRINDSTONE => Some(VillagerProfession::Weaponsmith),
        _ => None,
    }
}

/// Vanilla `PoiTypes.forState` for the POI types that make up the `minecraft:village` tag
/// but are not villager job sites: bed heads (`minecraft:home`) and bells (`minecraft:meeting`).
#[must_use]
pub fn village_poi_type_for_block(
    block: &Block,
    state: &BlockState,
) -> Option<(&'static str, i32)> {
    if block == &Block::BELL {
        return Some((POI_TYPE_MEETING, MEETING_MAX_TICKETS));
    }
    if block.has_tag(&tag::Block::MINECRAFT_BEDS)
        && BedProperties::from_state_id(state.id).r#part == BedPart::Head
    {
        return Some((POI_TYPE_HOME, 1));
    }
    None
}

/// Whether `poi_type` (a `minecraft:`-prefixed POI type id) is in the `minecraft:village` tag.
#[must_use]
pub fn is_village_poi_type(poi_type: &str) -> bool {
    let Some(name) = poi_type.strip_prefix("minecraft:") else {
        return false;
    };
    tag::PointOfInterestType::MINECRAFT_VILLAGE
        .0
        .contains(&name)
}

#[must_use]
pub fn poi_type_for_block(block: &Block) -> Option<&'static str> {
    Some(match profession_for_block(block)? {
        VillagerProfession::Armorer => "minecraft:armorer",
        VillagerProfession::Butcher => "minecraft:butcher",
        VillagerProfession::Cartographer => "minecraft:cartographer",
        VillagerProfession::Cleric => "minecraft:cleric",
        VillagerProfession::Farmer => "minecraft:farmer",
        VillagerProfession::Fisherman => "minecraft:fisherman",
        VillagerProfession::Fletcher => "minecraft:fletcher",
        VillagerProfession::Leatherworker => "minecraft:leatherworker",
        VillagerProfession::Librarian => "minecraft:librarian",
        VillagerProfession::Mason => "minecraft:mason",
        VillagerProfession::Shepherd => "minecraft:shepherd",
        VillagerProfession::Toolsmith => "minecraft:toolsmith",
        VillagerProfession::Weaponsmith => "minecraft:weaponsmith",
        VillagerProfession::None | VillagerProfession::Nitwit => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_villager_workstation_maps_to_its_profession() {
        let workstations = [
            (&Block::COMPOSTER, VillagerProfession::Farmer),
            (&Block::LECTERN, VillagerProfession::Librarian),
            (&Block::BLAST_FURNACE, VillagerProfession::Armorer),
            (&Block::SMOKER, VillagerProfession::Butcher),
            (&Block::CARTOGRAPHY_TABLE, VillagerProfession::Cartographer),
            (&Block::BREWING_STAND, VillagerProfession::Cleric),
            (&Block::BARREL, VillagerProfession::Fisherman),
            (&Block::FLETCHING_TABLE, VillagerProfession::Fletcher),
            (&Block::CAULDRON, VillagerProfession::Leatherworker),
            (&Block::STONECUTTER, VillagerProfession::Mason),
            (&Block::LOOM, VillagerProfession::Shepherd),
            (&Block::SMITHING_TABLE, VillagerProfession::Toolsmith),
            (&Block::GRINDSTONE, VillagerProfession::Weaponsmith),
        ];

        for (block, profession) in workstations {
            assert_eq!(profession_for_block(block), Some(profession));
            assert_eq!(
                poi_type_for_block(block).unwrap(),
                format!(
                    "minecraft:{}",
                    profession.translation_key().rsplit('.').next().unwrap()
                )
            );
        }
        assert_eq!(profession_for_block(&Block::DIRT), None);
        assert_eq!(poi_type_for_block(&Block::DIRT), None);
    }

    #[test]
    fn only_bed_heads_and_bells_are_village_pois() {
        let head = BedProperties {
            r#part: BedPart::Head,
            ..BedProperties::default(&Block::RED_BED)
        }
        .to_state_id(&Block::RED_BED);
        let foot = BedProperties {
            r#part: BedPart::Foot,
            ..BedProperties::default(&Block::RED_BED)
        }
        .to_state_id(&Block::RED_BED);

        assert_eq!(
            village_poi_type_for_block(&Block::RED_BED, BlockState::from_id(head)),
            Some((POI_TYPE_HOME, 1))
        );
        assert_eq!(
            village_poi_type_for_block(&Block::RED_BED, BlockState::from_id(foot)),
            None
        );
        assert_eq!(
            village_poi_type_for_block(
                &Block::BELL,
                BlockState::from_id(Block::BELL.default_state.id)
            ),
            Some((POI_TYPE_MEETING, MEETING_MAX_TICKETS))
        );
        assert_eq!(
            village_poi_type_for_block(
                &Block::STONE,
                BlockState::from_id(Block::STONE.default_state.id)
            ),
            None
        );
    }

    #[test]
    fn village_tag_covers_job_sites_homes_and_meetings() {
        assert!(is_village_poi_type(POI_TYPE_HOME));
        assert!(is_village_poi_type(POI_TYPE_MEETING));
        assert!(is_village_poi_type("minecraft:farmer"));
        assert!(!is_village_poi_type("minecraft:beehive"));
        assert!(!is_village_poi_type("minecraft:nether_portal"));
        assert!(!is_village_poi_type("farmer"));
    }

    #[test]
    fn available_job_sites_respect_profession_and_vanilla_search_radius() {
        let origin = BlockPos::new(0, 64, 0);
        let lectern = BlockPos::new(48, 64, 0);
        let too_far = BlockPos::new(49, 64, 0);
        let composter = BlockPos::new(1, 64, 0);
        let mut storage = VillagerPoiStorage::default();

        storage.update_block(lectern, &Block::LECTERN);
        storage.update_block(too_far, &Block::LECTERN);
        storage.update_block(composter, &Block::COMPOSTER);

        assert_eq!(
            storage.available_job_sites(origin, 48, Some(VillagerProfession::Librarian)),
            vec![lectern]
        );
        assert_eq!(
            storage.available_job_sites(origin, 48, Some(VillagerProfession::Farmer)),
            vec![composter]
        );
    }
}

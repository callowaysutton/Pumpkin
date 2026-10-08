use std::sync::Arc;

use pumpkin_data::{
    Block, BlockDirection, BlockStateId,
    block_properties::{HorizontalFacing, RailShape, RailShapeStraight},
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::{BlockAccessor, BlockFlags};

use crate::world::World;

use super::{HorizontalFacingRailExt, Rail, RailElevation, RailProperties, StraightRailShapeExt};

use super::super::block_receives_redstone_power;

/// Vanilla `PoweredRailBlock#findPoweredRailSignal`: walks one step along a powered rail line and
/// checks whether the rail there carries a power signal.
///
/// `block` is the rail block being searched for (powered rail or activator rail); the two share
/// this logic in vanilla because both use `PoweredRailBlock`.
pub(super) fn find_powered_rail_signal(
    world: &World,
    block: &Block,
    pos: &BlockPos,
    state: &RailProperties,
    forward: bool,
    search_depth: u8,
) -> bool {
    if search_depth >= 8 {
        return false;
    }

    let mut x = pos.0.x;
    let mut y = pos.0.y;
    let mut z = pos.0.z;
    let mut check_below = true;
    let mut shape = state.shape();

    match shape {
        RailShape::NorthSouth => {
            if forward {
                z += 1;
            } else {
                z -= 1;
            }
        }
        RailShape::EastWest => {
            if forward {
                x -= 1;
            } else {
                x += 1;
            }
        }
        RailShape::AscendingEast => {
            if forward {
                x -= 1;
            } else {
                x += 1;
                y += 1;
                check_below = false;
            }
            shape = RailShape::EastWest;
        }
        RailShape::AscendingWest => {
            if forward {
                x -= 1;
                y += 1;
                check_below = false;
            } else {
                x += 1;
            }
            shape = RailShape::EastWest;
        }
        RailShape::AscendingNorth => {
            if forward {
                z += 1;
            } else {
                z -= 1;
                y += 1;
                check_below = false;
            }
            shape = RailShape::NorthSouth;
        }
        RailShape::AscendingSouth => {
            if forward {
                z += 1;
                y += 1;
                check_below = false;
            } else {
                z -= 1;
            }
            shape = RailShape::NorthSouth;
        }
        _ => return false,
    }

    if is_same_rail_with_power(
        world,
        block,
        &BlockPos::new(x, y, z),
        forward,
        search_depth,
        shape,
    ) {
        return true;
    }

    check_below
        && is_same_rail_with_power(
            world,
            block,
            &BlockPos::new(x, y - 1, z),
            forward,
            search_depth,
            shape,
        )
}

/// Vanilla `PoweredRailBlock#isSameRailWithPower`: true when the rail at `pos` is the same rail
/// block as `block`, points the right way, is powered and either has a direct redstone signal or
/// continues the signal along the line.
fn is_same_rail_with_power(
    world: &World,
    block: &Block,
    pos: &BlockPos,
    forward: bool,
    search_depth: u8,
    dir: RailShape,
) -> bool {
    if world.get_block(pos) != block {
        return false;
    }

    let state_id = world.get_block_state_id(pos);
    let rail_props = RailProperties::new(state_id, block);
    let my_shape = rail_props.shape();

    match dir {
        RailShape::EastWest => {
            if matches!(
                my_shape,
                RailShape::NorthSouth | RailShape::AscendingNorth | RailShape::AscendingSouth
            ) {
                return false;
            }
        }
        RailShape::NorthSouth => {
            if matches!(
                my_shape,
                RailShape::EastWest | RailShape::AscendingEast | RailShape::AscendingWest
            ) {
                return false;
            }
        }
        _ => {}
    }

    if !rail_props.is_powered() {
        return false;
    }

    if block_receives_redstone_power(world, pos) {
        return true;
    }

    find_powered_rail_signal(world, block, pos, &rail_props, forward, search_depth + 1)
}

/// Vanilla `PoweredRailBlock#updateState`: recompute whether this rail should be powered from its
/// direct redstone signal or from a signal carried along a line of the same powered rail block.
pub(super) fn update_powered_state(world: &Arc<World>, block: &Block, pos: &BlockPos) {
    let state_id = world.get_block_state_id(pos);
    let mut rail_props = RailProperties::new(state_id, block);
    let current_powered = rail_props.is_powered();

    let should_power = block_receives_redstone_power(world, pos)
        || find_powered_rail_signal(world, block, pos, &rail_props, true, 0)
        || find_powered_rail_signal(world, block, pos, &rail_props, false, 0);

    if should_power != current_powered {
        rail_props.set_powered(should_power);
        world.set_block_state(pos, rail_props.to_state_id(block), BlockFlags::NOTIFY_ALL);

        // Vanilla `level.updateNeighborsAt(pos.below(), this)` and the slope case above.
        world.update_neighbors(&pos.down(), None);

        if rail_props.shape().is_ascending() {
            world.update_neighbors(&pos.up(), None);
        }
    }
}

pub(super) fn rail_placement_is_valid(world: &World, block: &Block, pos: &BlockPos) -> bool {
    if !can_place_rail_at(world, pos) {
        return false;
    }

    let state_id = world.get_block_state_id(pos);
    let rail_props = RailProperties::new(state_id, block);
    let rail_leaning_direction = match rail_props.shape() {
        RailShape::AscendingNorth => Some(HorizontalFacing::North),
        RailShape::AscendingSouth => Some(HorizontalFacing::South),
        RailShape::AscendingEast => Some(HorizontalFacing::East),
        RailShape::AscendingWest => Some(HorizontalFacing::West),
        _ => None,
    };

    if let Some(direction) = rail_leaning_direction
        && !can_place_rail_at(world, &pos.offset(direction.to_offset()).up())
    {
        return false;
    }

    true
}

pub(super) fn can_place_rail_at(world: &dyn BlockAccessor, pos: &BlockPos) -> bool {
    let state = world.get_block_state(&pos.down());
    state.is_side_solid(BlockDirection::Up)
}

pub(super) fn compute_placed_rail_shape(
    world: &World,
    block_pos: &BlockPos,
    player_facing: HorizontalFacing,
) -> RailShapeStraight {
    // Use the same sophisticated logic as normal rails, but adapted for straight rails
    // Check each direction for rail connections, similar to normal rail placement

    // Check East first
    if let Some(east_rail) = Rail::find_if_unlocked(world, block_pos, HorizontalFacing::East) {
        // Check for opposite connection (West) to form a straight line
        if let Some(west_rail) = Rail::find_if_unlocked(world, block_pos, HorizontalFacing::West) {
            // We have connections in both East and West
            if east_rail.elevation == RailElevation::Up {
                return RailShapeStraight::AscendingEast;
            } else if west_rail.elevation == RailElevation::Up {
                return RailShapeStraight::AscendingWest;
            }
            return RailShapeStraight::EastWest;
        }
        // Only East connection
        if east_rail.elevation == RailElevation::Up {
            return RailShapeStraight::AscendingEast;
        }
        return RailShapeStraight::EastWest;
    }

    // Check South
    if let Some(south_rail) = Rail::find_if_unlocked(world, block_pos, HorizontalFacing::South) {
        // Check for opposite connection (North) to form a straight line
        if let Some(north_rail) = Rail::find_if_unlocked(world, block_pos, HorizontalFacing::North)
        {
            // We have connections in both South and North
            if south_rail.elevation == RailElevation::Up {
                return RailShapeStraight::AscendingSouth;
            } else if north_rail.elevation == RailElevation::Up {
                return RailShapeStraight::AscendingNorth;
            }
            return RailShapeStraight::NorthSouth;
        }
        // Only South connection
        if south_rail.elevation == RailElevation::Up {
            return RailShapeStraight::AscendingSouth;
        }
        return RailShapeStraight::NorthSouth;
    }

    // Check West
    if let Some(west_rail) = Rail::find_if_unlocked(world, block_pos, HorizontalFacing::West) {
        if west_rail.elevation == RailElevation::Up {
            return RailShapeStraight::AscendingWest;
        }
        return RailShapeStraight::EastWest;
    }

    // Check North
    if let Some(north_rail) = Rail::find_if_unlocked(world, block_pos, HorizontalFacing::North) {
        if north_rail.elevation == RailElevation::Up {
            return RailShapeStraight::AscendingNorth;
        }
        return RailShapeStraight::NorthSouth;
    }

    // No connections found, use player facing direction
    player_facing.to_rail_shape_flat()
}

pub(super) fn update_flanking_rails_shape(
    world: &Arc<World>,
    block: &Block,
    state_id: BlockStateId,
    block_pos: &BlockPos,
) {
    for direction in RailProperties::new(state_id, block).directions() {
        let Some(mut flanking_rail) =
            Rail::find_with_elevation(world, block_pos.offset(direction.to_offset()))
        else {
            // Skip non-rail blocks
            continue;
        };

        let new_shape =
            compute_flanking_rail_new_shape(world, &flanking_rail, direction.opposite());

        if new_shape != flanking_rail.properties.shape() {
            flanking_rail.properties.set_shape(new_shape);
            world.set_block_state(
                &flanking_rail.position,
                flanking_rail.properties.to_state_id(flanking_rail.block),
                BlockFlags::NOTIFY_ALL,
            );
        }
    }
}

fn compute_flanking_rail_new_shape(
    world: &World,
    rail: &Rail,
    flanking_from: HorizontalFacing,
) -> RailShape {
    let mut connected_towards = Vec::with_capacity(2);
    let mut is_already_connected_to_elevated_rail = false;

    for neighbor_direction in rail.properties.directions() {
        if neighbor_direction == flanking_from {
            // Rails pointing to where the player placed are not connected
            continue;
        }

        let Some(maybe_connected_rail) =
            Rail::find_with_elevation(world, rail.position.offset(neighbor_direction.to_offset()))
        else {
            // Rails pointing to non-rail blocks are not connected
            continue;
        };

        if maybe_connected_rail
            .properties
            .directions()
            .into_iter()
            .any(|d| d == neighbor_direction.opposite())
        {
            // Rails pointing to other rails that are pointing back are connected
            connected_towards.push(neighbor_direction);
            is_already_connected_to_elevated_rail =
                maybe_connected_rail.elevation == RailElevation::Up;
        }
    }

    let new_neighbor_directions = match connected_towards.len() {
        2 => {
            // Do not update rails that are locked (aka fully connected)
            return rail.properties.shape();
        }
        1 => [connected_towards[0], flanking_from],
        0 => [flanking_from, flanking_from.opposite()],
        _ => {
            tracing::error!(
                "Rails only have two sides, but got {}",
                connected_towards.len()
            );
            return rail.properties.shape();
        }
    };

    // Handle rails that want to be straight
    if new_neighbor_directions
        .iter()
        .all(|d| *d == flanking_from || *d == flanking_from.opposite())
    {
        if rail.elevation == RailElevation::Down {
            if is_already_connected_to_elevated_rail {
                // Prioritize the South/West ascending
                return match flanking_from {
                    HorizontalFacing::South | HorizontalFacing::North => RailShape::AscendingSouth,
                    HorizontalFacing::West | HorizontalFacing::East => RailShape::AscendingWest,
                };
            }

            return flanking_from.to_rail_shape_ascending_towards().as_shape();
        } else if is_already_connected_to_elevated_rail {
            return connected_towards[0]
                .to_rail_shape_ascending_towards()
                .as_shape();
        }

        // Reset the shape to flat even if the rail already had good directions
        return rail.get_new_rail_shape(new_neighbor_directions[0], new_neighbor_directions[1]);
    }

    // Handle straight rails that want to curve
    if !rail.properties.can_curve() {
        return if new_neighbor_directions[0] == HorizontalFacing::North
            || new_neighbor_directions[0] == HorizontalFacing::South
        {
            if rail.elevation == RailElevation::Down {
                // The rail is down so it should be ascending
                flanking_from.to_rail_shape_ascending_towards().as_shape()
            } else {
                rail.get_new_rail_shape(new_neighbor_directions[0], new_neighbor_directions[1])
            }
        } else {
            rail.properties.shape()
        };
    }

    rail.get_new_rail_shape(new_neighbor_directions[0], new_neighbor_directions[1])
}

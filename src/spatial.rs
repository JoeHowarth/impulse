//! Spatial hierarchy primitives for high-precision positioning.
//!
//! Uses big_space's nested grid system for sub-meter precision at solar system scale.
//!
//! Two types of spatial entities:
//! - [`GridNode`]: Can have high-precision children (bodies, arenas)
//! - [`GridLeaf`]: High-precision entity without high-precision children (ships, projectiles)

use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;

/// Default grid cell size in meters (100m cells give sub-mm precision within a cell).
pub const GRID_CELL_SIZE: f32 = 100.0;

/// Default switching threshold - how far past cell edge before recentering.
pub const GRID_SWITCH_THRESHOLD: f32 = 10.0;

/// A spatial node that can contain high-precision children.
///
/// Use for: celestial bodies, tactical arenas, or anything that has children
/// needing CellCoord positioning.
///
/// Automatically inserts: CellCoord, Transform, GlobalTransform
/// Note: You must also add a Grid component for children to use CellCoord.
#[derive(Component, Default, Debug)]
#[require(CellCoord)]
pub struct GridNode;

impl GridNode {
    /// Create a default Grid for this node's children.
    pub fn default_grid() -> Grid {
        Grid::new(GRID_CELL_SIZE, GRID_SWITCH_THRESHOLD)
    }

    /// Create a GridNode with a custom grid configuration.
    pub fn with_grid(cell_size: f32, switch_threshold: f32) -> (Self, Grid) {
        (Self, Grid::new(cell_size, switch_threshold))
    }

    /// Position this node within a parent's grid.
    /// Returns components needed for spawning (excluding Grid - add that separately).
    pub fn at_position(position: DVec3, parent_grid: &Grid) -> (Self, CellCoord, Transform) {
        let (cell, local) = parent_grid.translation_to_grid(position);
        (Self, cell, Transform::from_translation(local))
    }
}

/// A high-precision spatial leaf node.
///
/// Use for: ships, projectiles, or entities that don't have high-precision children.
/// Children of a GridLeaf will be low-precision (using standard Transform propagation).
///
/// Automatically inserts: CellCoord, Transform, GlobalTransform
#[derive(Component, Default, Debug)]
#[require(CellCoord)]
pub struct GridLeaf;

impl GridLeaf {
    /// Position this leaf within a parent's grid.
    /// Returns components needed for spawning.
    pub fn at_position(position: DVec3, parent_grid: &Grid) -> (Self, CellCoord, Transform) {
        let (cell, local) = parent_grid.translation_to_grid(position);
        (Self, cell, Transform::from_translation(local))
    }

    /// Position this leaf at a cell coordinate with local offset.
    pub fn at_cell(cell: CellCoord, local_offset: Vec3) -> (Self, CellCoord, Transform) {
        (Self, cell, Transform::from_translation(local_offset))
    }
}

/// Extension trait for Commands to spawn spatial entities ergonomically.
pub trait SpatialCommands {
    /// Spawn a GridNode (interior node that can have high-precision children).
    fn spawn_grid_node(
        &mut self,
        position: DVec3,
        parent_grid: &Grid,
        parent: Entity,
        bundle: impl Bundle,
    ) -> Entity;

    /// Spawn a GridLeaf (high-precision leaf node).
    fn spawn_grid_leaf(
        &mut self,
        position: DVec3,
        parent_grid: &Grid,
        parent: Entity,
        bundle: impl Bundle,
    ) -> Entity;
}

impl SpatialCommands for Commands<'_, '_> {
    fn spawn_grid_node(
        &mut self,
        position: DVec3,
        parent_grid: &Grid,
        parent: Entity,
        bundle: impl Bundle,
    ) -> Entity {
        let (node, cell, transform) = GridNode::at_position(position, parent_grid);
        self.spawn((node, cell, transform, ChildOf(parent), bundle))
            .id()
    }

    fn spawn_grid_leaf(
        &mut self,
        position: DVec3,
        parent_grid: &Grid,
        parent: Entity,
        bundle: impl Bundle,
    ) -> Entity {
        let (leaf, cell, transform) = GridLeaf::at_position(position, parent_grid);
        self.spawn((leaf, cell, transform, ChildOf(parent), bundle))
            .id()
    }
}

//! Spatial hierarchy primitives for high-precision positioning.
//!
//! Uses big_space's nested grid system for sub-meter precision at solar system scale.
//!
//! Two types of spatial entities:
//! - [`GridNode`]: Can have high-precision children (bodies, arenas)
//! - [`GridLeaf`]: High-precision entity without high-precision children (ships, projectiles)
//!
//! World position tracking:
//! - [`TrackedWorldPosition`]: Component storing last frame's world position
//! - [`BigSpaceHierarchy`]: System param to compute current world position on demand

use bevy::ecs::system::SystemParam;
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

// ============================================================================
// World Position Tracking
// ============================================================================

/// Stores the world position from the previous frame.
///
/// Add this component to entities that need frame-to-frame position deltas
/// (e.g., for camera tracking of moving bodies).
///
/// Updated automatically in PostUpdate by [`sync_tracked_world_positions`].
#[derive(Component, Default, Debug)]
pub struct TrackedWorldPosition {
    /// World position at the end of the previous frame
    pub last_frame: DVec3,
}

/// System param for computing world positions in the big_space hierarchy.
///
/// Walks from an entity up to the root, accumulating position through nested grids.
/// Excludes Camera3d entities to avoid query conflicts with camera manipulation systems.
#[derive(SystemParam)]
pub struct BigSpaceHierarchy<'w, 's> {
    root: Query<'w, 's, (Entity, &'static Grid), With<BigSpace>>,
    spatial: Query<
        'w,
        's,
        (
            &'static Transform,
            Option<&'static CellCoord>,
            Option<&'static ChildOf>,
        ),
        Without<Camera3d>,
    >,
    grids: Query<'w, 's, &'static Grid>,
}

impl BigSpaceHierarchy<'_, '_> {
    /// Compute the world position (DVec3) of an entity by walking up the hierarchy.
    ///
    /// Returns None if the entity isn't in the big_space hierarchy or queries fail.
    pub fn world_position(&self, entity: Entity) -> Option<DVec3> {
        let (root_entity, root_grid) = self.root.single().ok()?;

        // Build chain from entity up to root
        let mut chain = Vec::new();
        let mut current = entity;

        loop {
            let (transform, cell, parent) = self.spatial.get(current).ok()?;
            chain.push((current, transform.clone(), cell.cloned()));

            if let Some(child_of) = parent {
                if child_of.0 == root_entity {
                    break; // Reached root
                }
                current = child_of.0;
            } else {
                break; // No parent, assume at root level
            }
        }

        // Walk down the chain (from root toward entity), accumulating position
        // Start at root position (origin)
        let mut world_pos = DVec3::ZERO;
        let mut current_grid: &Grid = root_grid;

        // Process chain in reverse (from closest-to-root to entity)
        for (ent, transform, cell) in chain.into_iter().rev() {
            // Compute this entity's position relative to parent using parent's grid
            let local_pos = if let Some(cell) = cell {
                current_grid.grid_position_double(&cell, &transform)
            } else {
                transform.translation.as_dvec3()
            };

            world_pos += local_pos;

            // If this entity has a grid, use it for children
            if let Ok(grid) = self.grids.get(ent) {
                current_grid = grid;
            }
        }

        Some(world_pos)
    }
}

/// Updates [`TrackedWorldPosition`] components with current world position.
///
/// Run this in PostUpdate so `last_frame` captures the position at end of frame.
/// Next frame, compute current position on-demand and diff against `last_frame`.
pub fn sync_tracked_world_positions(
    hierarchy: BigSpaceHierarchy,
    mut query: Query<(Entity, &mut TrackedWorldPosition)>,
) {
    for (entity, mut tracked) in &mut query {
        if let Some(pos) = hierarchy.world_position(entity) {
            tracked.last_frame = pos;
        }
    }
}

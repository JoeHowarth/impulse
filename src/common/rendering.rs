//! Common rendering systems that run in both strategic and tactical modes.
//!
//! Contains body position updates, visibility calculations, and orbit rendering.

use astrora_core::core::{Vector3, elements::coe_to_rv};
use bevy::{gizmos::GizmoAsset, math::DVec3, platform::collections::HashMap, prelude::*};
use big_space::prelude::{BigSpace, CellCoord, Grid};

use crate::common::simulation::SimulationTime;
use crate::model::{Body, propagate_elliptic};

// ============================================================================
// Constants
// ============================================================================

/// Number of line segments per orbit (higher = smoother, more memory)
const ORBIT_SEGMENTS: usize = 10000;

/// LOD: bodies closer than this screen distance to parent are invisible
const LOD_MIN_SCREEN_DIST: f32 = 5.0;

/// LOD: bodies farther than this screen distance from parent are fully visible
const LOD_MAX_SCREEN_DIST: f32 = 25.0;

// ============================================================================
// Components
// ============================================================================

/// Computed per-frame data for a body (visibility, display size, cached helio position).
/// Written by update_body_positions, read by rendering and UI systems.
#[derive(Component, Default)]
pub struct ComputedBody {
    /// High-precision heliocentric position (f64) - used for distance calculations
    pub helio_pos: DVec3,
    pub visibility: f32,
    pub display_size: f32,
}

/// Links an orbit gizmo entity to its parent body for position updates.
#[derive(Component)]
pub struct OrbitGizmo;

/// Marker for body shape entities (circles representing celestial bodies).
#[derive(Component)]
pub struct BodyShape;

// ============================================================================
// Coordinate Conversion
// ============================================================================

/// Converts physics coordinates (meters) to visual coordinates (1:1 meters).
pub fn phys_vec_to_vec3(v: Vector3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// Converts physics coordinates (meters) to DVec3 (f64).
fn phys_to_dvec3(v: Vector3) -> DVec3 {
    DVec3::new(v.x, v.y, v.z)
}

// ============================================================================
// Position Resolution
// ============================================================================

/// Recursively resolves a body's absolute heliocentric position in f64.
fn resolve_helio_position(
    entity: Entity,
    bodies: &Query<(Entity, &Body)>,
    cache: &mut HashMap<Entity, DVec3>,
    t: f64,
) -> DVec3 {
    // Return cached position if available
    if let Some(&pos) = cache.get(&entity) {
        return pos;
    }

    // Find the body component for this entity
    let body = bodies.iter().find(|(e, _)| *e == entity).map(|(_, b)| b);

    let Some(body) = body else {
        return DVec3::ZERO;
    };

    // Resolve parent's position first
    let parent_pos = body
        .parent_entity
        .map(|p| resolve_helio_position(p, bodies, cache, t))
        .unwrap_or(DVec3::ZERO);

    // Calculate local position relative to parent
    let local_pos = if let Some(parent_entity) = body.parent_entity {
        // Find parent body
        let parent_body = bodies
            .iter()
            .find(|(e, _)| *e == parent_entity)
            .map(|(_, b)| b);

        if let Some(parent_body) = parent_body {
            propagate_elliptic(body.orbital_elements, parent_body.std_grav_param, t)
                .ok()
                .map(|elems| {
                    let (r_vec, _) = coe_to_rv(&elems, parent_body.std_grav_param);
                    phys_to_dvec3(r_vec)
                })
                .unwrap_or_default()
        } else {
            DVec3::ZERO
        }
    } else {
        DVec3::ZERO
    };

    let abs_pos = parent_pos + local_pos;
    cache.insert(entity, abs_pos);
    abs_pos
}

// ============================================================================
// Visibility Calculation
// ============================================================================

/// Calculate visibility (0.0-1.0) based on screen-space separation from parent (f64 version).
fn calculate_visibility_f64(
    body: &Body,
    body_pos: DVec3,
    positions: &HashMap<Entity, DVec3>,
    cam_scale: f32,
) -> f32 {
    // Bodies without parents (Sun) are always visible
    let Some(parent_entity) = body.parent_entity else {
        return 1.0;
    };

    let parent_pos = positions
        .get(&parent_entity)
        .copied()
        .unwrap_or(DVec3::ZERO);
    let world_dist = (body_pos - parent_pos).length();

    // Convert to approximate screen distance
    // For orthographic: screen_dist ≈ world_dist / cam_scale
    let screen_dist = (world_dist / cam_scale as f64) as f32;

    // Smooth fade between thresholds
    ((screen_dist - LOD_MIN_SCREEN_DIST) / (LOD_MAX_SCREEN_DIST - LOD_MIN_SCREEN_DIST))
        .clamp(0.0, 1.0)
}

/// Computes the display radius for a body based on camera scale.
/// Bodies are shown at fixed screen fraction when zoomed out, physical size when zoomed in.
fn compute_display_size(body: &Body, cam_scale: f32) -> f32 {
    let log_radius = (body.radius as f32).log10();
    let log_scaled_size = ((log_radius - 4.0).max(1.0) * 1.5).min(8.0) * cam_scale;
    let phys_radius = body.radius as f32;
    log_scaled_size.max(phys_radius)
}

// ============================================================================
// Systems
// ============================================================================

/// Spawns BodyShape circle meshes for all bodies.
pub fn spawn_body_circles(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    bodies: Query<(Entity, &Body), Without<BodyShape>>,
) {
    for (entity, body) in &bodies {
        let mesh = meshes.add(Circle::new(1.0)); // Unit circle, scale via Transform
        let material = materials.add(StandardMaterial {
            base_color: body.color,
            unlit: true,
            ..default()
        });

        commands.spawn((
            BodyShape,
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::default(),
            ChildOf(entity),
        ));
    }
}

/// Computes positions, visibility, and display sizes for all bodies.
/// Updates CellCoord + Transform for big_space integration.
/// Runs once per frame before orbit updates and rendering.
pub fn update_body_positions(
    body_query: Query<(Entity, &Body)>,
    mut spatial_query: Query<(&mut ComputedBody, &mut CellCoord, &mut Transform)>,
    camera_query: Query<&Projection, With<Camera3d>>,
    grid_query: Query<&Grid, With<BigSpace>>,
    sim_time: Res<SimulationTime>,
) {
    let cam_scale = camera_query
        .single()
        .map(|p| match p {
            Projection::Orthographic(ortho) => ortho.scale,
            _ => 1.0,
        })
        .expect("No camera found");

    let Ok(grid) = grid_query.single() else {
        warn!("No BigSpace grid found");
        return;
    };

    let t = sim_time.sim_time;

    // Build position cache using entity keys (f64 for precision)
    let mut helio_positions: HashMap<Entity, DVec3> = HashMap::new();

    // First pass: compute all heliocentric positions in f64
    for (entity, _) in body_query.iter() {
        resolve_helio_position(entity, &body_query, &mut helio_positions, t);
    }

    // Second pass: update CellCoord, Transform, and ComputedBody
    for (entity, body) in body_query.iter() {
        let helio_pos = helio_positions[&entity];

        let (mut computed, mut cell, mut transform) = spatial_query
            .get_mut(entity)
            .inspect_err(|e| {
                error!(
                    "No ComputedBody, CellCoord, or Transform found for entity: {:?}",
                    e
                )
            })
            .expect("No ComputedBody, CellCoord, or Transform found for entity");

        // Convert heliocentric position to CellCoord + local Transform
        let (new_cell, local) = grid.translation_to_grid(helio_pos);
        *cell = new_cell;
        transform.translation = local;

        // Cache heliocentric position for other systems
        computed.helio_pos = helio_pos;

        // Compute visibility using f64 positions
        computed.visibility =
            calculate_visibility_f64(body, helio_pos, &helio_positions, cam_scale);
        computed.display_size = compute_display_size(body, cam_scale);
    }
}

/// Updates body shape scale based on computed display size.
pub fn update_body_shape_scale(
    mut body_shapes: Query<(&mut Transform, &ChildOf), With<BodyShape>>,
    computed_bodies: Query<&ComputedBody>,
) {
    for (mut transform, child_of) in body_shapes.iter_mut() {
        let computed_body = computed_bodies.get(child_of.0).unwrap();
        transform.scale = Vec3::splat(computed_body.display_size);
    }
}

/// Create a GizmoAsset containing the orbit linestrip.
pub fn create_orbit_gizmo_asset(body: &Body, parent_body: &Body) -> GizmoAsset {
    let mut gizmo = GizmoAsset::new();

    let period = body
        .orbital_elements
        .period(parent_body.std_grav_param)
        .unwrap_or(0.0);
    let step_dt = period / ORBIT_SEGMENTS as f64;

    // Collect orbit points in local coordinates (relative to parent)
    let mut points = Vec::with_capacity(ORBIT_SEGMENTS + 1);
    for i in 0..ORBIT_SEGMENTS {
        let t = i as f64 * step_dt;
        if let Ok(elems) = propagate_elliptic(body.orbital_elements, parent_body.std_grav_param, t)
        {
            let (r_local, _) = coe_to_rv(&elems, parent_body.std_grav_param);
            points.push(phys_vec_to_vec3(r_local));
        }
    }

    // Close the orbit loop
    if !points.is_empty() {
        points.push(points[0]);
    }

    let color = Color::srgba(1.0, 1.0, 1.0, 0.3);
    gizmo.linestrip(points, color);

    gizmo
}

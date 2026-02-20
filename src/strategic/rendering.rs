//! Strategic mode rendering.
//!
//! Fleet shapes, objective rings, plan markers, and transfer arc visualization.

use std::sync::atomic::{AtomicUsize, Ordering};

use bevy::gizmos::GizmoAsset;
use bevy::math::{DVec3, Isometry3d};
use bevy::prelude::*;
use bevy_vector_shapes::prelude::*;
use big_space::prelude::*;

use crate::camera::{BigSpaceRoot, CameraScale};
use crate::common::{ComputedBody, SimulationTime};
use crate::model::{
    Body, CombatState, ComputedFleetPosition, Faction, Fleet, FleetLocation, FlightPlan,
    LogicalShip, MU_SUN, Selected, TransferSolution, leg_source, propagate_kepler_full, ship_count,
};
use crate::phys_vec_to_vec3;

use super::transfer_lut::TransferLut;
use super::transfer_vis::{self, HoveredTransferArc, TransferArcType};

// ============================================================================
// Rendering Components
// ============================================================================

/// Marker for objective ring entities (retained shape showing enemy presence at a body).
/// These are spawned as children of Body entities.
#[derive(Component)]
pub struct ObjectiveRing;

/// Marker for fleet visual entities (retained shape for strategic map).
/// Links the shape to its logical fleet entity.
#[derive(Component)]
pub struct FleetShape {
    pub fleet_entity: Entity,
    /// True if shape was spawned for InTransit (has CellCoord, parented to BigSpace).
    /// False if spawned for AtBody (parented to body entity).
    pub is_transit_shape: bool,
}

/// Marker for flight plan waypoint gizmos.
/// Spawned as children of target body entities.
#[derive(Component)]
pub struct PlanMarker {
    /// Which fleet's plan this marker belongs to
    pub fleet: Entity,
    /// Index in the flight plan
    pub leg_index: usize,
}

// ============================================================================
// Colors and Constants
// ============================================================================

/// Player fleet colors (green)
pub const FLEET_PLAYER_SELECTED: Color = Color::srgb(0.4, 1.0, 0.4);
pub const FLEET_PLAYER_UNSELECTED: Color = Color::srgba(0.3, 0.8, 0.3, 0.6);

/// Enemy fleet colors (imperial red)
pub const FLEET_ENEMY_SELECTED: Color = Color::srgb(1.0, 0.3, 0.3);
pub const FLEET_ENEMY_UNSELECTED: Color = Color::srgba(0.8, 0.2, 0.2, 0.6);

/// Fleet size in pixels (scale = world units per pixel)
const FLEET_SIZE_PIXELS: f32 = 10.0;

/// Offset distance in pixels
const FLEET_OFFSET_PIXELS: f32 = 10.0;

/// Enemy marker color (matches fleet color)
const ENEMY_MARKER_COLOR: Color = Color::srgba(0.8, 0.2, 0.2, 0.6);

/// Queue waypoint marker color (cyan, dimmed)
const QUEUE_MARKER_COLOR: Color = Color::srgba(0.3, 0.8, 0.8, 0.7);

/// Size of plan marker in pixels (scaled by cam_scale)
const PLAN_MARKER_SIZE_PIXELS: f32 = 8.0;

// ============================================================================
// Fleet Name Generation
// ============================================================================

/// Counter for generating unique fleet names
static FLEET_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Generate a unique fleet name based on NATO phonetic alphabet
pub fn generate_fleet_name() -> String {
    const NAMES: &[&str] = &[
        "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliet", "Kilo", "Lima", "Mike",
        "November", "Oscar", "Papa", "Quebec", "Romeo", "Sierra", "Tango", "Uniform", "Victor",
        "Whiskey", "Xray", "Yankee", "Zulu",
    ];
    let idx = FLEET_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as usize;
    if idx < NAMES.len() {
        NAMES[idx].to_string()
    } else {
        format!("Fleet-{}", idx + 1)
    }
}

// ============================================================================
// State Transition Systems
// ============================================================================

/// Despawn strategic-only marker entities when entering tactical mode.
pub fn despawn_strategic_markers(
    mut commands: Commands,
    fleet_shapes: Query<Entity, With<FleetShape>>,
    rings: Query<Entity, With<ObjectiveRing>>,
) {
    for entity in &fleet_shapes {
        commands.entity(entity).despawn();
    }
    for entity in &rings {
        commands.entity(entity).despawn();
    }
}

// ============================================================================
// Fleet Position Systems
// ============================================================================

/// Computes visual positions for all fleets, offsetting multiple fleets at the same body.
/// Returns a map from fleet entity to (world_position, velocity_direction).
/// Note: Uses GlobalTransform for body positions (camera-relative via big_space).
pub fn compute_fleet_positions<F: bevy::ecs::query::QueryFilter>(
    ships: &Query<(Entity, &Fleet, &FleetLocation, Option<&Selected>, &Faction), F>,
    bodies: &Query<&GlobalTransform, With<Body>>,
    cam_scale: f32,
) -> bevy::platform::collections::HashMap<Entity, (Vec3, Vec3)> {
    use bevy::platform::collections::HashMap;
    use std::f32::consts::PI;

    let mut positions = HashMap::new();
    let offset_distance = cam_scale * FLEET_OFFSET_PIXELS;

    // First pass: count fleets at each body
    let mut fleets_at_body: HashMap<Entity, Vec<Entity>> = HashMap::new();
    for (fleet_entity, _, location, _, _) in ships.iter() {
        if let FleetLocation::AtBody(body) = location {
            fleets_at_body.entry(*body).or_default().push(fleet_entity);
        }
    }

    // Second pass: compute positions with offsets
    for (fleet_entity, _, location, is_selected, _) in ships.iter() {
        let size_mult = if is_selected.is_some() { 1.3 } else { 1.0 };

        let (position, velocity_dir) = match location {
            FleetLocation::AtBody(body) => {
                let body_pos = bodies
                    .get(*body)
                    .map(|gt| gt.translation())
                    .unwrap_or(Vec3::ZERO);

                // Get index of this fleet among all fleets at this body
                let fleets_here = fleets_at_body
                    .get(body)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let fleet_index = fleets_here
                    .iter()
                    .position(|e| *e == fleet_entity)
                    .unwrap_or(0);
                let fleet_count = fleets_here.len();

                // Compute offset angle for this fleet
                let offset = if fleet_count == 1 {
                    // Single fleet: offset to the right
                    Vec3::new(offset_distance * size_mult, 0.0, 0.0)
                } else {
                    // Multiple fleets: fan out in a semicircle (top half)
                    let angle = PI * 0.25
                        + (fleet_index as f32 / (fleet_count - 1).max(1) as f32) * PI * 0.5;
                    let x = offset_distance * size_mult * angle.cos();
                    let y = offset_distance * size_mult * angle.sin();
                    Vec3::new(x, y, 0.0)
                };

                (body_pos + offset, Vec3::new(0.0, 1.0, 0.0))
            }
            FleetLocation::InTransit { .. } => {
                continue;
            }
        };

        positions.insert(fleet_entity, (position, velocity_dir));
    }

    positions
}

/// Updates ComputedFleetPosition components for all fleets.
/// Run this before rendering to have positions available.
pub fn update_fleet_positions(
    mut commands: Commands,
    fleets: Query<(Entity, &Fleet, &FleetLocation, Option<&Selected>, &Faction)>,
    bodies: Query<&GlobalTransform, With<Body>>,
    cam_scale: Res<CameraScale>,
    shapes: Query<(&FleetShape, &GlobalTransform)>,
) {
    let positions = compute_fleet_positions(&fleets, &bodies, cam_scale.0);
    let mut positions = positions;

    for (fleet_shape, transform) in &shapes {
        if !fleet_shape.is_transit_shape {
            continue;
        }
        let position = transform.translation();
        let velocity_dir = transform.rotation() * Vec3::Y;
        positions.insert(fleet_shape.fleet_entity, (position, velocity_dir));
    }

    for (fleet_entity, _, _, _, _) in &fleets {
        if let Some((position, velocity_dir)) = positions.get(&fleet_entity) {
            commands.entity(fleet_entity).insert(ComputedFleetPosition {
                position: *position,
                velocity_dir: *velocity_dir,
            });
        }
    }
}

// ============================================================================
// Sync Systems
// ============================================================================

/// Syncs fleet shape entities with fleet positions.
/// Spawns shapes for new fleets, updates existing shapes, despawns orphaned shapes.
/// - AtBody fleets: shape is child of body entity (inherits body's Transform)
/// - InTransit fleets: shape has CellCoord + Transform from orbital position
pub fn sync_fleet_shapes(
    mut commands: Commands,
    combat: Res<CombatState>,
    big_space_root: Res<BigSpaceRoot>,
    grid_query: Query<&Grid, With<BigSpace>>,
    fleets: Query<(Entity, &Fleet, &FleetLocation, Option<&Selected>, &Faction)>,
    bodies: Query<&GlobalTransform, With<Body>>,
    existing_shapes: Query<(Entity, &FleetShape)>,
    mut shape_transforms: Query<&mut Transform, With<FleetShape>>,
    sim_time: Res<SimulationTime>,
    cam_scale: Res<CameraScale>,
) {
    use bevy::platform::collections::{HashMap, HashSet};

    // Skip during tactical mode
    if combat.active {
        return;
    }

    let Ok(grid) = grid_query.single() else {
        return;
    };

    let cam_scale = cam_scale.0;
    let fleet_size = cam_scale * FLEET_SIZE_PIXELS;
    let computed_positions = compute_fleet_positions(&fleets, &bodies, cam_scale);

    let at_body_local_offset = |fleet_entity: Entity, body: Entity| -> Vec3 {
        let Some((fleet_world_pos, _)) = computed_positions.get(&fleet_entity) else {
            return Vec3::new(cam_scale * 15.0, 0.0, 0.2);
        };
        let Ok(body_transform) = bodies.get(body) else {
            return Vec3::new(cam_scale * 15.0, 0.0, 0.2);
        };
        let body_world_pos = body_transform.translation();
        Vec3::new(
            fleet_world_pos.x - body_world_pos.x,
            fleet_world_pos.y - body_world_pos.y,
            0.2,
        )
    };

    // Track which fleets currently exist and have shapes
    let mut fleets_with_shapes: HashMap<Entity, Entity> = HashMap::new();
    for (shape_entity, fleet_shape) in &existing_shapes {
        fleets_with_shapes.insert(fleet_shape.fleet_entity, shape_entity);
    }

    // Track which fleets we've processed
    let mut processed_fleets: HashSet<Entity> = HashSet::new();

    // Process all fleets
    for (fleet_entity, _fleet, location, is_selected, faction) in &fleets {
        processed_fleets.insert(fleet_entity);

        let is_selected = is_selected.is_some();
        let size_mult = if is_selected { 1.3 } else { 1.0 };
        let color = match (faction, is_selected) {
            (Faction::Player, true) => FLEET_PLAYER_SELECTED,
            (Faction::Player, false) => FLEET_PLAYER_UNSELECTED,
            (Faction::Enemy, true) => FLEET_ENEMY_SELECTED,
            (Faction::Enemy, false) => FLEET_ENEMY_UNSELECTED,
        };

        // Compute velocity direction for triangle orientation
        let velocity_dir = match location {
            FleetLocation::AtBody(_) => Vec3::Y, // Default up when stationary
            FleetLocation::InTransit {
                solution,
                departure_time,
                ..
            } => {
                let elapsed = sim_time.sim_time - departure_time;
                if elapsed >= 0.0 {
                    if let Some((_, v_vec)) = propagate_kepler_full(
                        solution.departure_pos,
                        solution.departure_vel,
                        MU_SUN,
                        elapsed,
                    ) {
                        phys_vec_to_vec3(v_vec).normalize_or_zero()
                    } else {
                        Vec3::Y
                    }
                } else {
                    Vec3::Y
                }
            }
        };

        let rotation = if velocity_dir.length_squared() > 0.001 {
            Quat::from_rotation_arc(Vec3::Y, velocity_dir)
        } else {
            Quat::IDENTITY
        };

        // Build triangle vertices (scaled, Vec2 for bevy_vector_shapes)
        let half_base = fleet_size * 0.5 * size_mult;
        let height = fleet_size * size_mult;
        let v_top = Vec2::new(0.0, height * 0.5);
        let v_left = Vec2::new(-half_base, -height * 0.5);
        let v_right = Vec2::new(half_base, -height * 0.5);

        let is_in_transit = matches!(location, FleetLocation::InTransit { .. });

        if let Some(&shape_entity) = fleets_with_shapes.get(&fleet_entity) {
            // Check if we have a matching shape
            let shape_info = existing_shapes.get(shape_entity).ok();

            // If shape type doesn't match location type, despawn and let respawn happen
            if let Some((_, fleet_shape)) = shape_info {
                if fleet_shape.is_transit_shape != is_in_transit {
                    // Location type changed - despawn old shape, spawn new one below
                    commands.entity(shape_entity).despawn();
                    // Fall through to spawn new shape
                } else {
                    // Update existing shape
                    if let Ok(mut transform) = shape_transforms.get_mut(shape_entity) {
                        match location {
                            FleetLocation::AtBody(body) => {
                                // Shape is child of body - just update local offset and rotation
                                transform.translation = at_body_local_offset(fleet_entity, *body);
                                transform.rotation = rotation;
                            }
                            FleetLocation::InTransit {
                                solution,
                                departure_time,
                                ..
                            } => {
                                // Compute position from orbital mechanics
                                let elapsed = sim_time.sim_time - departure_time;
                                if elapsed >= 0.0 {
                                    if let Some((r_vec, _)) = propagate_kepler_full(
                                        solution.departure_pos,
                                        solution.departure_vel,
                                        MU_SUN,
                                        elapsed,
                                    ) {
                                        // Convert nalgebra Vector3 to DVec3, then to CellCoord + local
                                        let helio_pos = DVec3::new(r_vec.x, r_vec.y, r_vec.z);
                                        let (cell, local) = grid.translation_to_grid(helio_pos);
                                        // Update CellCoord component
                                        commands.entity(shape_entity).insert(cell);
                                        transform.translation = local;
                                        transform.translation.z = 0.2; // Slight Z offset for visibility
                                    }
                                }
                                transform.rotation = rotation;
                            }
                        }
                    }

                    // Update triangle component and color
                    commands.entity(shape_entity).insert((
                        TriangleComponent::new(
                            &ShapeConfig {
                                color,
                                hollow: false,
                                ..ShapeConfig::default_3d()
                            },
                            v_top,
                            v_left,
                            v_right,
                        ),
                        ShapeFill {
                            color,
                            ty: FillType::Fill,
                        },
                    ));
                    continue; // Shape updated, move to next fleet
                }
            }
        }

        // Spawn new shape (either no existing shape, or old one was despawned due to type change)
        {
            match location {
                FleetLocation::AtBody(body) => {
                    // Spawn as child of body entity
                    let local_transform = Transform::from_translation(
                        at_body_local_offset(fleet_entity, *body),
                    )
                    .with_rotation(rotation);
                    let config = ShapeConfig {
                        color,
                        thickness: cam_scale * 1.0,
                        hollow: false,
                        transform: local_transform,
                        ..ShapeConfig::default_3d()
                    };
                    commands.spawn((
                        ShapeBundle::triangle(&config, v_top, v_left, v_right).insert_3d(),
                        FleetShape {
                            fleet_entity,
                            is_transit_shape: false,
                        },
                        ChildOf(*body),
                    ));
                }
                FleetLocation::InTransit {
                    solution,
                    departure_time,
                    ..
                } => {
                    // Compute position and spawn with CellCoord
                    let elapsed = sim_time.sim_time - departure_time;
                    let helio_pos = if elapsed >= 0.0 {
                        if let Some((r_vec, _)) = propagate_kepler_full(
                            solution.departure_pos,
                            solution.departure_vel,
                            MU_SUN,
                            elapsed,
                        ) {
                            // Convert nalgebra Vector3 to bevy DVec3
                            DVec3::new(r_vec.x, r_vec.y, r_vec.z)
                        } else {
                            DVec3::ZERO
                        }
                    } else {
                        DVec3::ZERO
                    };

                    let (cell, local) = grid.translation_to_grid(helio_pos);
                    let local_transform =
                        Transform::from_translation(local + Vec3::Z * 0.2).with_rotation(rotation);
                    let config = ShapeConfig {
                        color,
                        thickness: cam_scale * 1.0,
                        hollow: false,
                        transform: local_transform,
                        ..ShapeConfig::default_3d()
                    };

                    commands.spawn((
                        ShapeBundle::triangle(&config, v_top, v_left, v_right).insert_3d(),
                        FleetShape {
                            fleet_entity,
                            is_transit_shape: true,
                        },
                        cell,
                        ChildOf(big_space_root.0),
                    ));
                }
            }
        }
    }

    // Despawn shapes for fleets that no longer exist
    for (shape_entity, fleet_shape) in &existing_shapes {
        if !processed_fleets.contains(&fleet_shape.fleet_entity) {
            commands.entity(shape_entity).despawn();
        }
    }
}

/// Syncs objective ring entities with enemy fleet presence.
/// Spawns rings as children of bodies with enemies, despawns when enemies leave.
/// Ring size updates each frame based on camera scale.
pub fn sync_objective_rings(
    mut commands: Commands,
    combat: Res<CombatState>,
    fleets: Query<(Entity, &FleetLocation, &Faction)>,
    fleet_children: Query<&Children>,
    ships: Query<&LogicalShip>,
    bodies: Query<(Entity, &ComputedBody), With<Body>>,
    existing_rings: Query<(Entity, &ChildOf), With<ObjectiveRing>>,
    mut ring_shapes: Query<(&mut DiscComponent, &mut ShapeFill), With<ObjectiveRing>>,
    cam_scale: Res<CameraScale>,
) {
    use bevy::platform::collections::HashSet;

    // Hide rings during tactical mode
    if combat.active {
        // Could set Visibility::Hidden instead of despawning, but for now just skip updates
        return;
    }

    let cam_scale = cam_scale.0;

    // Collect bodies with enemy fleets
    let mut enemy_bodies: HashSet<Entity> = HashSet::new();
    for (fleet_entity, location, faction) in &fleets {
        if *faction != Faction::Enemy {
            continue;
        }
        if ship_count(fleet_entity, &fleet_children, &ships) == 0 {
            continue;
        }
        if let FleetLocation::AtBody(body) = location {
            enemy_bodies.insert(*body);
        }
    }

    // Track which bodies already have rings
    let mut bodies_with_rings: HashSet<Entity> = HashSet::new();
    for (ring_entity, child_of) in &existing_rings {
        let parent = child_of.parent();
        if enemy_bodies.contains(&parent) {
            // Body still has enemies - keep ring, update size
            bodies_with_rings.insert(parent);
            if let Ok((mut disc, mut fill)) = ring_shapes.get_mut(ring_entity) {
                // Update radius based on body's display size + offset
                if let Ok((_, computed)) = bodies.get(parent) {
                    disc.radius = computed.display_size + cam_scale * 5.0;
                    fill.ty = FillType::Stroke(cam_scale * 1.5, ThicknessType::World);
                }
            }
        } else {
            // No enemies at this body - despawn ring
            commands.entity(ring_entity).despawn();
        }
    }

    // Spawn rings for bodies that need them but don't have one
    for body_entity in &enemy_bodies {
        if bodies_with_rings.contains(body_entity) {
            continue;
        }
        let Ok((_, computed)) = bodies.get(*body_entity) else {
            continue;
        };

        let ring_radius = computed.display_size + cam_scale * 5.0;
        let config = ShapeConfig {
            color: ENEMY_MARKER_COLOR,
            thickness: cam_scale * 1.5,
            hollow: true,
            transform: Transform::from_xyz(0.0, 0.0, 0.1), // Slight Z offset
            ..ShapeConfig::default_3d()
        };

        commands.spawn((
            ShapeBundle::circle(&config, ring_radius).insert_3d(),
            ObjectiveRing,
            ChildOf(*body_entity),
        ));
    }
}

/// Syncs plan marker gizmo entities with flight plan state.
/// Spawns markers as children of target body entities, despawns when legs are removed.
/// Uses Transform.scale to adjust size based on camera scale.
pub fn sync_plan_markers(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    fleets: Query<(Entity, &FlightPlan)>,
    existing_markers: Query<(Entity, &PlanMarker, &ChildOf)>,
    mut marker_transforms: Query<&mut Transform, With<PlanMarker>>,
    cam_scale: Res<CameraScale>,
) {
    use bevy::platform::collections::HashSet;

    let cam_scale = cam_scale.0;
    let marker_scale = cam_scale * PLAN_MARKER_SIZE_PIXELS;

    // Build set of desired markers: (fleet, leg_index, target_body)
    let mut desired: HashSet<(Entity, usize, Entity)> = HashSet::new();
    for (fleet_entity, plan) in &fleets {
        for (leg_index, leg) in plan.legs.iter().enumerate() {
            desired.insert((fleet_entity, leg_index, leg.target));
        }
    }

    // Track which desired markers already exist
    let mut existing_set: HashSet<(Entity, usize)> = HashSet::new();

    // Update existing markers or despawn if no longer needed
    for (marker_entity, marker, child_of) in &existing_markers {
        let parent_body = child_of.parent();
        let key = (marker.fleet, marker.leg_index, parent_body);

        if desired.contains(&key) {
            // Marker still valid - update scale
            existing_set.insert((marker.fleet, marker.leg_index));
            if let Ok(mut transform) = marker_transforms.get_mut(marker_entity) {
                transform.scale = Vec3::splat(marker_scale);
            }
        } else {
            // Marker no longer needed - despawn
            commands.entity(marker_entity).despawn();
        }
    }

    // Spawn markers for legs that don't have one
    for (fleet_entity, plan) in &fleets {
        for (leg_index, leg) in plan.legs.iter().enumerate() {
            if existing_set.contains(&(fleet_entity, leg_index)) {
                continue;
            }

            // Create a unit circle gizmo asset (radius 1.0, scaled by Transform)
            let mut gizmo = GizmoAsset::new();
            gizmo.circle(Isometry3d::IDENTITY, 1.0, QUEUE_MARKER_COLOR);

            commands.spawn((
                Gizmo {
                    handle: gizmo_assets.add(gizmo),
                    depth_bias: 0.08,
                    ..default()
                },
                PlanMarker {
                    fleet: fleet_entity,
                    leg_index,
                },
                Transform::from_xyz(0.0, 0.0, 0.15).with_scale(Vec3::splat(marker_scale)),
                ChildOf(leg.target),
            ));
        }
    }
}

/// Syncs Transfer visualization entities to match FleetLocation + committed legs.
/// - InTransit -> one Transfer for active flight
/// - Committed legs -> one Transfer each for future arcs
pub fn sync_transfer_entities(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    fleets: Query<(Entity, &FleetLocation, &FlightPlan)>,
    lut: Res<TransferLut>,
    transfers: Query<(Entity, &transfer_vis::Transfer), Without<HoveredTransferArc>>,
    bodies: Query<&Body>,
    cam_scale: Res<CameraScale>,
) {
    for (fleet_entity, location, plan) in &fleets {
        // Build list of (source, target, solution, departure_time) for active visualizations
        let mut active: Vec<(Entity, Entity, TransferSolution, f64, TransferArcType)> = Vec::new();

        // Add active transfer if InTransit
        if let FleetLocation::InTransit {
            source,
            target,
            solution,
            departure_time,
        } = location
        {
            active.push((
                *source,
                *target,
                solution.clone(),
                *departure_time,
                TransferArcType::Committed,
            ));
        }

        for (i, leg) in plan.legs.iter().enumerate() {
            let source = leg_source(location, plan, i);

            let [src_body, tgt_body] = bodies
                .get_many([source, leg.target])
                .expect("Source and target bodies not found");

            let solution = lut
                .get_transfer(
                    source,
                    leg.target,
                    &src_body.orbital_elements,
                    &tgt_body.orbital_elements,
                    leg.departure_day,
                    leg.tof_days,
                )
                .expect("No transfer solution found for leg");

            let departure_time = leg.departure_day as f64 * 86400.0;
            let ty = if i < plan.committed_count {
                TransferArcType::Committed
            } else {
                TransferArcType::Preview
            };
            active.push((source, leg.target, solution, departure_time, ty));
        }

        // Find existing Transfer entities for this fleet
        let fleet_transfers: Vec<_> = transfers
            .iter()
            .filter(|(_, t)| t.fleet == fleet_entity)
            .collect();

        // Despawn entities that don't match any active transfer
        for (transfer_entity, transfer) in &fleet_transfers {
            let has_match = active.iter().any(|(_, target, _, dep_time, _)| {
                *target == transfer.target && (*dep_time - transfer.departure_time).abs() < 1.0
            });

            if !has_match {
                commands.entity(*transfer_entity).despawn();
            }
        }

        // Spawn entities for active transfers that don't have one
        for (source, target, solution, departure_time, arc_type) in &active {
            let transfer_ent = fleet_transfers.iter().find(|(_, t)| {
                t.target == *target && (t.departure_time - *departure_time).abs() < 1.0
            });

            match transfer_ent {
                Some((transfer_entity, t)) => {
                    // Wrong type of transfer arc, despawn and recreate as an active transfer arc
                    if t.arc_type == *arc_type {
                        continue;
                    }
                    commands.entity(*transfer_entity).despawn();
                }
                None => {}
            }
            let parent_entity = bodies
                .get(*source)
                .map(|b| b.parent_entity)
                .expect("Body has no parent")
                .expect("Body has no parent");
            transfer_vis::spawn_transfer_visualization(
                &mut commands,
                &mut gizmo_assets,
                parent_entity,
                fleet_entity,
                *source,
                *target,
                solution,
                *departure_time,
                cam_scale.0,
                *arc_type,
            );
        }

        // Debug logging
        static DEDUP_LOG_THRESHOLD: AtomicUsize = AtomicUsize::new(0);
        let dedup_log_threshold = DEDUP_LOG_THRESHOLD.load(Ordering::Relaxed);
        if dedup_log_threshold < 10 {
            DEDUP_LOG_THRESHOLD.fetch_add(1, Ordering::Relaxed);
        } else {
            let names: Vec<String> = active
                .iter()
                .map(|(src, tgt, _, _, _)| {
                    let src_name = bodies.get(*src).map(|b| b.name.as_str()).unwrap_or("?");
                    let tgt_name = bodies.get(*tgt).map(|b| b.name.as_str()).unwrap_or("?");
                    format!("{}->{}", src_name, tgt_name)
                })
                .collect();
            debug!("sync_transfer_entities: [{}]", names.join(", "));
            DEDUP_LOG_THRESHOLD.store(0, Ordering::Relaxed);
        }
    }
}

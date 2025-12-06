use std::{cell::LazyCell, time::Duration};

use astrora_core::core::{Vector3, elements::coe_to_rv};
use avian3d::prelude::Position;
use bevy::{
    asset::Assets,
    camera::visibility::NoFrustumCulling,
    diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin},
    gizmos::{
        GizmoAsset,
        config::{GizmoConfigStore, GizmoLineJoint},
    },
    math::{DVec3, Vec3},
    platform::collections::HashMap,
    prelude::*,
    transform::TransformPlugin,
    window::PrimaryWindow,
};
use bevy_pancam::{PanCam, PanCamPlugin};
use bevy_vector_shapes::{prelude as shape_prelude, prelude::*};
use big_space::prelude::*;

mod camera;
mod orbital_data;
mod physics;
mod picking;
mod ship;
mod simulation;
mod tactical;
mod transfer;
mod transfer_lut;
mod transfer_vis;
mod ui;

use orbital_data::{Body, PlanetaryElements, propagate_elliptic};
use simulation::SimulationTime;

use crate::camera::spawn_camera;

// ============================================================================
// Constants
// ============================================================================

/// Number of line segments per orbit (higher = smoother, more memory)
const ORBIT_SEGMENTS: usize = 10000;

/// Astronomical unit in meters (lazy-loaded from orbital_data)
const AU: LazyCell<f64> = LazyCell::new(|| orbital_data::AU);

/// LOD: bodies closer than this screen distance to parent are invisible
const LOD_MIN_SCREEN_DIST: f32 = 5.0;

/// LOD: bodies farther than this screen distance from parent are fully visible
const LOD_MAX_SCREEN_DIST: f32 = 25.0;

// ============================================================================
// Type Aliases (for clarity when storing Entity references)
// ============================================================================

/// Entity reference to a Body entity
type BodyEntity = Entity;

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
struct OrbitGizmo;

#[derive(Component)]
struct BodyShape;

fn main() {
    let start_day = simulation::parse_start_day();
    if start_day != 0 {
        eprintln!("Starting simulation at day {}", start_day);
    }

    App::new()
        // Disable Bevy's TransformPlugin - big_space replaces it
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(BigSpaceDefaultPlugins)
        .add_plugins(PanCamPlugin)
        .add_plugins(ShapePlugin::default())
        .add_plugins(physics::TacticalPhysicsPlugin)
        .add_plugins((
            // Performance diagnostics - logs to console every X seconds
            FrameTimeDiagnosticsPlugin::default(),
            LogDiagnosticsPlugin {
                wait_duration: Duration::from_secs(10),
                ..default()
            },
        ))
        .insert_resource(SimulationTime::from_start_day(start_day))
        .init_resource::<ui::TransferPopup>()
        .init_resource::<ui::FleetKeyState>()
        .init_resource::<ship::VictoryState>()
        .init_resource::<ship::CombatState>()
        .init_resource::<picking::BoxSelection>()
        .add_systems(
            Startup,
            (
                spawn_camera,
                ApplyDeferred, // Ensure BigSpaceRoot resource is available
                setup,
                ApplyDeferred,
                spawn_body_circles,
                init_parent_entities,
                transfer_lut::init_transfer_lut,
                configure_gizmos,
            )
                .chain(),
        )
        // Fleet and simulation systems (run first)
        .add_systems(
            Update,
            (
                (
                    camera::update_camera_scale,
                    simulation::handle_time_controls,
                    update_body_positions,
                    // Flight plan modifications
                    ship::commit_plan,
                    ship::cancel_last_leg,
                    // Fleet management
                    ship::split_fleet,
                    ship::merge_fleets,
                    // Objective tracking
                    ship::check_objectives,
                    // Transfer execution (runs before expire so committed legs depart first)
                    ship::execute_departure,
                    ship::check_arrival,
                    // Combat detection (after arrival)
                    ship::detect_combat,
                    // Tactical mode entry (after combat detection)
                    tactical::enter_tactical_mode,
                    // Keep tactical arena synced with body position
                    tactical::update_arena_position,
                    // Expire uncommitted legs whose departure_day passed
                    ship::expire_stale_uncommitted_legs,
                    // Sync Transfer entities to FleetLocation + committed legs
                    ship::sync_transfer_entities,
                    transfer_vis::check_transfer_expiration,
                )
                    .chain(),
                // UI interaction systems
                (
                    handle_body_click,
                    // Tactical picking (box selection must run before click to set drag state)
                    picking::update_box_selection,
                    picking::handle_tactical_click,
                    picking::handle_tactical_move_order,
                    ui::handle_fleet_number_keys,
                    ui::handle_popup_spawn,
                    ui::update_popup_options,
                    ui::update_popup_position,
                    ui::handle_close_button,
                    ui::handle_escape_key,
                    ui::handle_option_hover,
                    transfer_vis::update_hovered_arc,
                    ui::handle_option_selection,
                )
                    .chain(),
                // Rendering systems (run last)
                (
                    camera::animate_camera,
                    // update_orbit_positions,
                    // transfer_vis::update_transfer_arc_positions,
                    // render_system,
                    update_body_shape_scale,
                    ship::update_fleet_positions,
                    ship::sync_fleet_shapes,
                    // ship::render_fleets, // TODO: Remove after verifying sync_fleet_shapes works
                    tactical::update_ship_movement,
                    // tactical::render_visual_ships, // Now using retained gizmos spawned with VisualShip
                    tactical::render_move_markers,
                    picking::sync_box_selection,
                    ship::sync_objective_rings,
                    ship::sync_plan_markers,
                    // ship::render_plan_arcs,
                    // TODO: remove me - now handled by spawn_transfer_visualization in sync_transfer_entities system
                    // transfer_vis::render_burn_arrows,
                    ui::update_labels,
                    ui::update_time_ui,
                    ui::update_transfer_panel,
                    ui::update_fleet_tabs,
                    ui::update_victory_overlay,
                )
                    .chain(),
            )
                .chain(),
        )
        .run();
}

fn spawn_body_circles(
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
            dbg!(ChildOf(entity)),
        ));
    }
}

fn setup(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    big_space_root: Res<camera::BigSpaceRoot>,
) {
    // Load planetary data
    let bodies = PlanetaryElements::get_planetary_elements();
    commands.insert_resource(PlanetaryElements {
        bodies: bodies.clone(),
    });

    // First pass: spawn all bodies as children of BigSpace with CellCoord
    let mut body_entities: HashMap<String, BodyEntity> = HashMap::new();
    for body in bodies.values() {
        // Spawn body as child of BigSpace root with spatial components
        // Initial CellCoord is origin - update_body_positions will compute correct position
        let entity = commands
            .spawn((
                body.clone(),
                Visibility::Visible,
                ComputedBody::default(),
                CellCoord::default(), // Will be updated by update_body_positions
                Transform::default(), // Will be updated by update_body_positions
                ChildOf(dbg!(big_space_root.0)), // Parent to BigSpace root
            ))
            .id();
        body_entities.insert(body.name.clone(), entity);
    }

    // Player fleet at Venus (for testing f32 precision - closer to sun = smaller coordinates)
    if let Some(&venus) = body_entities.get("Venus") {
        commands
            .spawn((
                ship::Fleet {
                    delta_v_remaining: 500_000.0,
                    name: "Alpha".to_string(),
                },
                ship::FleetLocation::AtBody(venus),
                ship::Faction::Player,
                ship::Selected,
                ship::FlightPlan::default(),
            ))
            .with_children(|builder| {
                for _ in 0..10 {
                    builder.spawn(ship::LogicalShip);
                }
            });
    }

    // Enemy garrison at Mercury (for testing f32 precision)
    if let Some(&mercury) = body_entities.get("Mercury") {
        commands
            .spawn((
                ship::Fleet {
                    delta_v_remaining: 500_000.0,
                    name: "Mercury Garrison".to_string(),
                },
                ship::FleetLocation::AtBody(mercury),
                ship::Faction::Enemy,
                ship::FlightPlan::default(),
            ))
            .with_children(|builder| {
                for _ in 0..10 {
                    builder.spawn(ship::LogicalShip);
                }
            });
    }

    // Second pass: spawn labels and orbit gizmos (now we can look up parent entities)
    for body in bodies.values() {
        let body_entity = body_entities[&body.name];
        ui::spawn_body_label(&mut commands, &body.name, body_entity);
        // Body -> Body shape
        // let mut config = shape_prelude::ShapeConfig::default_3d();
        // config.color = body.color.clone();

        // let mut shape_bundle = shape_prelude::ShapeBundle::circle(&config, 1E10 as f32);
        // shape_bundle.visibility = Visibility::Visible;
        // commands
        //     .entity(dbg!(body_entity))
        //     .with_child((BodyShape, shape_bundle, NoFrustumCulling));

        // Create orbit gizmo if body has a parent
        // Gizmos stay outside BigSpace - their Transform is set from parent's GlobalTransform
        let Some(parent_name) = &body.parent_name else {
            continue;
        };
        let Some(parent_entity) = body_entities.get(parent_name) else {
            continue;
        };
        let Some(parent_body) = bodies.get(parent_name) else {
            continue;
        };
        // Parent Body -> Orbit gizmo
        commands.entity(*parent_entity).with_child((
            Gizmo {
                handle: gizmo_assets.add(create_orbit_gizmo_asset(body, parent_body)),
                depth_bias: 0.1,
                ..default()
            },
            OrbitGizmo,
            Transform::default(),
        ));
    }

    // Spawn time control panel
    ui::spawn_time_panel(&mut commands);

    // Spawn transfer info panel
    ui::spawn_transfer_panel(&mut commands);

    // Spawn fleet tabs at bottom center
    ui::spawn_fleet_tabs(&mut commands);
}

/// Links parent entities in Body components after all bodies are spawned.
/// This must run after ApplyDeferred to ensure all entities exist.
/// TODO: Can we just do this in the setup function? We just need entity ids, they don't need to actually be spawned.
fn init_parent_entities(mut bodies: Query<(Entity, &mut Body)>) {
    // Build entity map by name
    let entity_map: HashMap<String, Entity> = bodies
        .iter()
        .map(|(entity, body)| (body.name.clone(), entity))
        .collect();

    // Link parent entities
    for (_, mut body) in bodies.iter_mut() {
        if let Some(parent_name) = &body.parent_name {
            body.parent_entity = entity_map.get(parent_name).copied();
        }
    }
}

/// Configure gizmo line settings for orbit rendering.
fn configure_gizmos(mut config_store: ResMut<GizmoConfigStore>) {
    let (config, _) = config_store.config_mut::<DefaultGizmoConfigGroup>();
    config.line.joints = GizmoLineJoint::None;
}

/// Computes positions, visibility, and display sizes for all bodies.
/// Updates CellCoord + Transform for big_space integration.
/// Runs once per frame before orbit updates and rendering.
fn update_body_positions(
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

        let (
            mut computed,
            mut cell,
            mut transform,
            // mut position
        ) = spatial_query
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

        // Update physics position
        // position.0 = helio_pos;

        // Cache heliocentric position for other systems
        computed.helio_pos = helio_pos;

        // Compute visibility using f64 positions
        computed.visibility =
            calculate_visibility_f64(body, helio_pos, &helio_positions, cam_scale);
        computed.display_size = compute_display_size(body, cam_scale);
    }
}

// ============================================================================
// Coordinate Conversion & Position Resolution
// ============================================================================

/// Converts physics coordinates (meters) to visual coordinates (1:1 meters).
pub fn phys_vec_to_vec3(v: Vector3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// Converts physics coordinates (meters) to DVec3 (f64).
fn phys_to_dvec3(v: Vector3) -> DVec3 {
    DVec3::new(v.x, v.y, v.z)
}

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

// ============================================================================
// Visibility & Rendering
// ============================================================================

/// Create a GizmoAsset containing the orbit linestrip (points at origin, will be translated by Transform).
fn create_orbit_gizmo_asset(body: &Body, parent_body: &Body) -> GizmoAsset {
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

fn update_body_shape_scale(
    mut body_shapes: Query<(&mut Transform, &ChildOf), With<BodyShape>>,
    computed_bodies: Query<&ComputedBody>,
) {
    for (mut transform, child_of) in body_shapes.iter_mut() {
        let computed_body = computed_bodies.get(child_of.0).unwrap();
        transform.scale = Vec3::splat(computed_body.display_size);
    }
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
// Body Click Detection
// ============================================================================

// /// Computes the display radius for a body based on camera scale.
// /// Bodies are shown at fixed screen fraction when zoomed out, physical size when zoomed in.
// fn compute_display_size(body: &Body, cam_scale: f32) -> f32 {
//     let phys_radius = body.radius as f32;
//     // Screen-space size: ~20 pixels (scale = world units per pixel)
//     let screen_size = cam_scale * 20.0;
//     phys_radius.max(screen_size)
// }

/// Detects clicks on fleets or bodies (strategic mode only).
/// - Click: select fleet if one is at click location
/// - Shift+click: open transfer popup for selected fleet
fn handle_body_click(
    mut commands: Commands,
    mouse_button: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    body_query: Query<(Entity, &Body, &ComputedBody, &GlobalTransform)>,
    fleet_query: Query<(Entity, &ship::Fleet, &ship::FleetLocation, &ship::Faction)>,
    fleet_positions: Query<(Entity, &ship::ComputedFleetPosition, &ship::Faction)>,
    selected_query: Query<Entity, With<ship::Selected>>,
    mut popup: ResMut<ui::TransferPopup>,
    combat: Res<ship::CombatState>,
) {
    // Skip in tactical mode - picking::handle_tactical_click handles that
    if combat.active {
        return;
    }

    if !mouse_button.just_pressed(MouseButton::Left) {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };

    let shift_held = keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);

    if shift_held {
        // Shift+click: open transfer popup for body
        let selected_fleet = fleet_query
            .iter()
            .filter(|(_, _, _, faction)| **faction == ship::Faction::Player)
            .find(|(e, _, _, _)| selected_query.get(*e).is_ok());
        let current_entity = selected_fleet.map(|(_, _, loc, _)| loc.effective_body());

        let mut best_match: Option<(Entity, f32)> = None;
        for (entity, _body, computed, global_transform) in body_query.iter() {
            if computed.visibility < 0.01 {
                continue;
            }
            if current_entity == Some(entity) {
                continue;
            }

            let Ok(screen_pos) =
                camera.world_to_viewport(camera_transform, global_transform.translation())
            else {
                continue;
            };

            let screen_dist = cursor_pos.distance(screen_pos);
            let click_radius = computed.display_size * 2.0 + 10.0;
            if screen_dist <= click_radius {
                match &best_match {
                    None => best_match = Some((entity, screen_dist)),
                    Some((_, best_dist)) if screen_dist < *best_dist => {
                        best_match = Some((entity, screen_dist))
                    }
                    _ => {}
                }
            }
        }

        if let Some((clicked_entity, _)) = best_match {
            let body_name = body_query
                .get(clicked_entity)
                .map(|(_, b, _, _)| b.name.clone())
                .unwrap_or_default();
            info!("Shift+clicked on body: {}", body_name);
            popup.target_entity = Some(clicked_entity);
        }
    } else {
        // Click: select fleet if there's one at click location
        let mut best_match: Option<(Entity, f32)> = None;
        for (fleet_entity, computed, faction) in fleet_positions.iter() {
            // Only consider player fleets for selection
            if *faction != ship::Faction::Player {
                continue;
            }
            let Ok(screen_pos) = camera.world_to_viewport(camera_transform, computed.position)
            else {
                continue;
            };

            let screen_dist = cursor_pos.distance(screen_pos);
            if screen_dist <= 20.0 {
                match &best_match {
                    None => best_match = Some((fleet_entity, screen_dist)),
                    Some((_, best_dist)) if screen_dist < *best_dist => {
                        best_match = Some((fleet_entity, screen_dist))
                    }
                    _ => {}
                }
            }
        }

        if let Some((fleet_entity, _)) = best_match {
            let fleet_name = fleet_query
                .get(fleet_entity)
                .map(|(_, f, _, _)| f.name.clone())
                .unwrap_or_default();
            info!("Selected fleet: {}", fleet_name);
            for old_selected in selected_query.iter() {
                commands.entity(old_selected).remove::<ship::Selected>();
            }
            commands.entity(fleet_entity).insert(ship::Selected);
        }
    }
}

use std::cell::LazyCell;

use astrora_core::core::{Vector3, elements::coe_to_rv};
use bevy::{
    asset::Assets,
    diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin},
    gizmos::{
        GizmoAsset,
        config::{GizmoConfigStore, GizmoLineJoint},
    },
    math::Vec3,
    platform::collections::HashMap,
    prelude::*,
    window::PrimaryWindow,
};
use bevy_pancam::{PanCam, PanCamPlugin};
use bevy_vector_shapes::prelude::*;

mod orbital_data;
mod ship;
mod simulation;
mod transfer;
mod transfer_lut;
mod transfer_vis;
mod ui;

use orbital_data::{Body, PlanetaryElements, propagate_elliptic};
use simulation::SimulationTime;

// ============================================================================
// Constants
// ============================================================================

/// Number of line segments per orbit (higher = smoother, more memory)
const ORBIT_SEGMENTS: usize = 10000;

/// Visual scaling factor: 1 AU = this many world units
const VISUAL_SCALE: f64 = 100.0;

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

/// Computed per-frame data for a body (position, visibility, display size).
/// Written by update_body_positions, read by rendering and UI systems.
#[derive(Component, Default)]
pub struct ComputedBody {
    pub position: Vec3,
    pub visibility: f32,
    pub display_size: f32,
}

/// Links an orbit gizmo entity to its parent body for position updates.
#[derive(Component)]
struct OrbitGizmo {
    parent: BodyEntity,
}

fn main() {
    let start_day = simulation::parse_start_day();
    if start_day != 0 {
        eprintln!("Starting simulation at day {}", start_day);
    }

    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(PanCamPlugin)
        .add_plugins(ShapePlugin::default())
        // Performance diagnostics - logs to console every second
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_plugins(LogDiagnosticsPlugin::default())
        .insert_resource(SimulationTime::from_start_day(start_day))
        .init_resource::<ui::TransferPopup>()
        .add_systems(
            Startup,
            (
                setup,
                ApplyDeferred,
                init_parent_entities,
                transfer_lut::init_transfer_lut,
                configure_gizmos,
            )
                .chain(),
        )
        // Ship and simulation systems (run first)
        .add_systems(
            Update,
            (
                simulation::handle_time_controls,
                // Flight plan modifications
                ship::commit_plan,
                ship::cancel_last_leg,
                // Fleet management
                ship::split_fleet,
                ship::merge_fleets,
                // Transfer execution (runs before expire so committed legs depart first)
                ship::execute_departure,
                ship::check_arrival,
                // Expire uncommitted legs whose departure_day passed
                ship::expire_stale_legs,
                // Sync Transfer entities to ShipLocation + committed legs
                ship::sync_transfer_entities,
                transfer_vis::check_transfer_expiration,
                update_body_positions,
            )
                .chain(),
        )
        // UI interaction systems
        .add_systems(
            Update,
            (
                handle_body_click,
                ui::handle_fleet_number_keys,
                ui::handle_popup_spawn,
                ui::update_popup_options,
                ui::update_popup_position,
                ui::handle_close_button,
                ui::handle_escape_key,
                ui::handle_option_hover,
                transfer_vis::update_preview_arc,
                ui::handle_option_selection,
            )
                .chain()
                .after(update_body_positions),
        )
        // Rendering systems (run last)
        .add_systems(
            Update,
            (
                update_orbit_positions,
                render_system,
                ship::render_ship,
                ship::render_departure_markers,
                ship::render_plan_markers,
                ship::render_plan_arcs,
                transfer_vis::render_burn_arrows,
                ui::update_labels,
                ui::update_time_ui,
                ui::update_transfer_panel,
                ui::update_fleet_tabs,
            )
                .chain()
                .after(ui::handle_option_selection),
        )
        .run();
}

fn setup(mut commands: Commands, mut gizmo_assets: ResMut<Assets<GizmoAsset>>) {
    // Spawn camera
    commands.spawn((
        Camera3d::default(),
        Projection::from(OrthographicProjection {
            far: 100_000.0,
            near: 0.1,
            scale: 1.0,
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 0.0, 1000.0).looking_at(Vec3::ZERO, Vec3::Y),
        PanCam::default(),
    ));

    // Load planetary data
    let bodies = PlanetaryElements::get_planetary_elements();
    commands.insert_resource(PlanetaryElements {
        bodies: bodies.clone(),
    });

    // First pass: spawn all bodies and build name -> entity map
    let mut body_entities: HashMap<String, BodyEntity> = HashMap::new();
    for body in bodies.values() {
        let entity = commands.spawn((body.clone(), ComputedBody::default())).id();
        body_entities.insert(body.name.clone(), entity);
    }

    // Spawn player fleets at different locations
    // Fleet 1: Main fleet at Earth (selected by default)
    if let Some(&earth) = body_entities.get("Earth") {
        commands.spawn((
            ship::Fleet {
                delta_v_remaining: 500_000.0,
                name: "Alpha".to_string(),
                ship_count: 10,
            },
            ship::ShipLocation::AtBody(earth),
            ship::PlayerControlled,
            ship::Selected,
            ship::FlightPlan::default(),
        ));
    }

    // Fleet 2: At Mars
    if let Some(&mars) = body_entities.get("Mars") {
        commands.spawn((
            ship::Fleet {
                delta_v_remaining: 400_000.0,
                name: "Bravo".to_string(),
                ship_count: 5,
            },
            ship::ShipLocation::AtBody(mars),
            ship::PlayerControlled,
            ship::FlightPlan::default(),
        ));
    }

    // Fleet 3: At Jupiter
    if let Some(&jupiter) = body_entities.get("Jupiter") {
        commands.spawn((
            ship::Fleet {
                delta_v_remaining: 300_000.0,
                name: "Charlie".to_string(),
                ship_count: 3,
            },
            ship::ShipLocation::AtBody(jupiter),
            ship::PlayerControlled,
            ship::FlightPlan::default(),
        ));
    }

    // Second pass: spawn labels and orbit gizmos (now we can look up parent entities)
    for body in bodies.values() {
        let body_entity = body_entities[&body.name];
        ui::spawn_body_label(&mut commands, &body.name, body_entity);

        // Create orbit gizmo if body has a parent
        if let Some(parent_name) = &body.parent_name {
            if let Some(&parent_entity) = body_entities.get(parent_name) {
                if let Some(parent_body) = bodies.get(parent_name) {
                    let orbit_asset = create_orbit_gizmo_asset(body, parent_body);
                    commands.spawn((
                        Gizmo {
                            handle: gizmo_assets.add(orbit_asset),
                            depth_bias: 0.1,
                            ..default()
                        },
                        OrbitGizmo {
                            parent: parent_entity,
                        },
                    ));
                }
            }
        }
    }

    // Note: Parent entities will be linked after all entities are spawned
    // This happens in a deferred system (init_parent_entities) that runs after setup

    // Spawn time control panel
    ui::spawn_time_panel(&mut commands);

    // Spawn transfer info panel
    ui::spawn_transfer_panel(&mut commands);

    // Spawn fleet tabs at bottom center
    ui::spawn_fleet_tabs(&mut commands);
}

/// Links parent entities in Body components after all bodies are spawned.
/// This must run after ApplyDeferred to ensure all entities exist.
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
            points.push(phys_to_visual(r_local));
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

/// Computes positions, visibility, and display sizes for all bodies.
/// Runs once per frame before orbit updates and rendering.
fn update_body_positions(
    body_query: Query<(Entity, &Body)>,
    mut computed_query: Query<&mut ComputedBody>,
    camera_query: Query<&Projection, With<Camera3d>>,
    sim_time: Res<SimulationTime>,
) {
    let cam_scale = camera_query
        .single()
        .map(|p| match p {
            Projection::Orthographic(ortho) => ortho.scale,
            _ => 1.0,
        })
        .unwrap_or(1.0);

    let t = sim_time.sim_time;

    // Build position cache using entity keys
    let mut positions: HashMap<Entity, Vec3> = HashMap::new();

    // First pass: compute all positions
    for (entity, _) in body_query.iter() {
        resolve_position_with_queries(entity, &body_query, &mut positions, t);
    }

    // Second pass: update ComputedBody components with computed positions and metadata
    for (entity, body) in body_query.iter() {
        if let Ok(mut computed) = computed_query.get_mut(entity) {
            let pos = positions.get(&entity).copied().unwrap_or(Vec3::ZERO);
            computed.position = pos;
            computed.visibility = calculate_visibility(body, pos, &positions, cam_scale);
            computed.display_size = compute_display_size(body, cam_scale);
        }
    }
}

/// Update orbit gizmo Transform positions to match their parent body positions.
fn update_orbit_positions(
    mut orbit_query: Query<(&OrbitGizmo, &mut Transform)>,
    body_query: Query<&ComputedBody>,
) {
    for (orbit_gizmo, mut transform) in &mut orbit_query {
        let parent_pos = body_query
            .get(orbit_gizmo.parent)
            .map(|c| c.position)
            .unwrap_or(Vec3::ZERO);
        transform.translation = parent_pos;
    }
}

// ============================================================================
// Coordinate Conversion & Position Resolution
// ============================================================================

/// Converts physics coordinates (meters) to visual coordinates.
pub fn phys_to_visual(v: Vector3) -> Vec3 {
    let scale = VISUAL_SCALE / *AU;
    Vec3::new(
        (v.x * scale) as f32,
        (v.y * scale) as f32,
        (v.z * scale) as f32,
    )
}

/// Recursively resolves a body's absolute position in visual coordinates using entity keys.
fn resolve_position_with_queries(
    entity: Entity,
    bodies: &Query<(Entity, &Body)>,
    cache: &mut HashMap<Entity, Vec3>,
    t: f64,
) -> Vec3 {
    // Return cached position if available
    if let Some(&pos) = cache.get(&entity) {
        return pos;
    }

    // Find the body component for this entity
    let body = bodies.iter().find(|(e, _)| *e == entity).map(|(_, b)| b);

    let Some(body) = body else {
        return Vec3::ZERO;
    };

    // Resolve parent's position first
    let parent_pos = body
        .parent_entity
        .map(|p| resolve_position_with_queries(p, bodies, cache, t))
        .unwrap_or(Vec3::ZERO);

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
                    r_vec
                })
                .unwrap_or_default()
        } else {
            Vector3::default()
        }
    } else {
        Vector3::default()
    };

    let abs_pos = parent_pos + phys_to_visual(local_pos);
    cache.insert(entity, abs_pos);
    abs_pos
}

// ============================================================================
// Visibility & Rendering
// ============================================================================

/// Calculate visibility (0.0-1.0) based on screen-space separation from parent.
fn calculate_visibility(
    body: &Body,
    body_pos: Vec3,
    positions: &HashMap<Entity, Vec3>,
    cam_scale: f32,
) -> f32 {
    // Bodies without parents (Sun) are always visible
    let Some(parent_entity) = body.parent_entity else {
        return 1.0;
    };

    let parent_pos = positions.get(&parent_entity).copied().unwrap_or(Vec3::ZERO);
    let world_dist = (body_pos - parent_pos).length();

    // Convert to approximate screen distance
    // For orthographic: screen_dist ≈ world_dist / cam_scale
    let screen_dist = world_dist / cam_scale;

    // Smooth fade between thresholds
    ((screen_dist - LOD_MIN_SCREEN_DIST) / (LOD_MAX_SCREEN_DIST - LOD_MIN_SCREEN_DIST))
        .clamp(0.0, 1.0)
}

/// Draws all visible bodies. Positions and visibility are precomputed by update_body_positions.
fn render_system(body_query: Query<(&Body, &ComputedBody)>, mut painter: ShapePainter) {
    for (body, computed) in body_query.iter() {
        if computed.visibility < 0.01 {
            continue;
        }

        painter.set_translation(computed.position);
        let base_color = body.color.to_srgba();
        painter.set_color(Color::srgba(
            base_color.red,
            base_color.green,
            base_color.blue,
            computed.visibility,
        ));
        painter.circle(computed.display_size);
    }
}

/// Computes the display radius for a body based on camera scale.
/// Uses logarithmic scaling so all bodies remain visible, with a minimum of true physical size.
fn compute_display_size(body: &Body, cam_scale: f32) -> f32 {
    let log_radius = (body.radius as f32).log10();
    let log_scaled_size = ((log_radius - 4.0).max(1.0) * 1.5).min(8.0) * cam_scale;
    let phys_radius = (body.radius * VISUAL_SCALE / *AU) as f32;
    log_scaled_size.max(phys_radius)
}

// ============================================================================
// Body Click Detection
// ============================================================================

/// Detects clicks on fleets or bodies.
/// - Click: select fleet if one is at click location
/// - Shift+click: open transfer popup for selected fleet
fn handle_body_click(
    mut commands: Commands,
    mouse_button: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    body_query: Query<(Entity, &Body, &ComputedBody)>,
    fleet_query: Query<(Entity, &ship::Fleet, &ship::ShipLocation, Option<&ship::Selected>), With<ship::PlayerControlled>>,
    selected_query: Query<Entity, With<ship::Selected>>,
    bodies_for_pos: Query<&ComputedBody>,
    sim_time: Res<simulation::SimulationTime>,
    mut popup: ResMut<ui::TransferPopup>,
) {
    if !mouse_button.just_pressed(MouseButton::Left) {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else { return };
    let Ok((camera, camera_transform)) = camera_query.single() else { return };

    let shift_held = keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);

    if shift_held {
        // Shift+click: open transfer popup for body
        let selected_fleet = fleet_query.iter().find(|(e, _, _, _)| selected_query.get(*e).is_ok());
        let current_entity = selected_fleet.map(|(_, _, loc, _)| loc.effective_body());

        let mut best_match: Option<(Entity, f32)> = None;
        for (entity, _body, computed) in body_query.iter() {
            if computed.visibility < 0.01 {
                continue;
            }
            if current_entity == Some(entity) {
                continue;
            }

            let Ok(screen_pos) = camera.world_to_viewport(camera_transform, computed.position) else {
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
                .map(|(_, b, _)| b.name.clone())
                .unwrap_or_default();
            info!("Shift+clicked on body: {}", body_name);
            popup.target_entity = Some(clicked_entity);
        }
    } else {
        // Click: select fleet if there's one at click location
        // Use computed positions with offsets for proper hit detection
        let fleet_positions = ship::compute_fleet_positions(&fleet_query, &bodies_for_pos, &sim_time);

        let mut best_match: Option<(Entity, f32)> = None;
        for (fleet_entity, _, _, _) in fleet_query.iter() {
            let Some((world_pos, _)) = fleet_positions.get(&fleet_entity) else { continue };
            let Ok(screen_pos) = camera.world_to_viewport(camera_transform, *world_pos) else { continue };

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

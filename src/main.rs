#![allow(unused_imports)]

use std::cell::LazyCell;

use astrora_core::core::{Vector3, elements::coe_to_rv};
use bevy::{
    asset::Assets,
    diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin},
    gizmos::{config::{GizmoConfigStore, GizmoLineJoint}, GizmoAsset},
    input::mouse::MouseButtonInput,
    math::Vec3,
    platform::collections::HashMap,
    prelude::*,
    window::PrimaryWindow,
};
use bevy_pancam::{PanCam, PanCamPlugin};
use bevy_vector_shapes::prelude::*;

mod orbital_data;
mod simulation;
mod transfer;
mod transfer_vis;
mod ui;

// ============================================================================
// Transfer Planning Resources
// ============================================================================

/// Which body the player is currently "at" for transfer planning.
/// Transfers are computed FROM this body to clicked targets.
#[derive(Resource)]
pub struct CurrentBody {
    pub entity: Entity,
    pub name: String,
}

/// Transfer popup state - tracks if a popup is open and for which target.
#[derive(Resource, Default)]
pub struct TransferPopup {
    pub target_entity: Option<Entity>,
    pub popup_entity: Option<Entity>,
    /// Cached options for the currently displayed popup
    pub options: Vec<ui::TransferOption>,
    /// Which option index is being hovered (for preview)
    pub hovered_option: Option<usize>,
    /// Day when options were last computed (for live updates)
    pub options_computed_day: i32,
}

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
        .init_resource::<transfer_vis::ActiveTransfer>()
        .init_resource::<transfer_vis::TransferCache>()
        .init_resource::<TransferPopup>()
        .add_systems(
            Startup,
            (
                setup,
                ApplyDeferred,
                transfer_vis::init_transfer_cache,
                configure_gizmos,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                simulation::handle_time_controls,
                transfer_vis::update_transfer_cache,
                transfer_vis::check_transfer_expiration,
                update_body_positions,
                handle_body_click,
                ui::handle_popup_spawn,
                ui::update_popup_options,
                ui::update_popup_position,
                ui::handle_close_button,
                ui::handle_escape_key,
                ui::handle_option_hover,
                transfer_vis::update_transfer_visualization,
                ui::handle_option_selection,
                update_orbit_positions,
                render_system,
                transfer_vis::render_burn_arrows,
                ui::update_labels,
                ui::update_time_ui,
                ui::update_transfer_panel,
            )
                .chain(),
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
    let mut earth_entity: Option<Entity> = None;
    for body in bodies.values() {
        let entity = commands.spawn((body.clone(), ComputedBody::default())).id();
        body_entities.insert(body.name.clone(), entity);
        if body.name == "Earth" {
            earth_entity = Some(entity);
        }
    }

    // Initialize CurrentBody resource (start at Earth)
    if let Some(entity) = earth_entity {
        commands.insert_resource(CurrentBody {
            entity,
            name: "Earth".to_string(),
        });
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
                        OrbitGizmo { parent: parent_entity },
                    ));
                }
            }
        }
    }

    // Spawn time control panel
    ui::spawn_time_panel(&mut commands);

    // Spawn transfer info panel
    ui::spawn_transfer_panel(&mut commands);
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
        if let Ok(elems) = propagate_elliptic(body.orbital_elements, parent_body.std_grav_param, t) {
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
    mut body_query: Query<(&Body, &mut ComputedBody)>,
    res_elements: Res<PlanetaryElements>,
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

    // Build position cache (still need string keys for recursive resolution)
    let mut positions: HashMap<String, Vec3> = HashMap::new();
    positions.insert("Sun".to_string(), Vec3::ZERO);

    for (body, _) in body_query.iter() {
        resolve_position(&body.name, &res_elements.bodies, &mut positions, t);
    }

    // Update ComputedBody components
    for (body, mut computed) in body_query.iter_mut() {
        let pos = positions.get(&body.name).copied().unwrap_or(Vec3::ZERO);
        computed.position = pos;
        computed.visibility = calculate_visibility(body, pos, &positions, cam_scale);
        computed.display_size = compute_display_size(body, cam_scale);
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

/// Recursively resolves a body's absolute position in visual coordinates.
fn resolve_position(
    name: &str,
    bodies: &HashMap<String, Body>,
    cache: &mut HashMap<String, Vec3>,
    t: f64,
) -> Vec3 {
    // Return cached position if available
    if let Some(&pos) = cache.get(name) {
        return pos;
    }

    let Some(body) = bodies.get(name) else {
        return Vec3::ZERO;
    };

    // Resolve parent's position first
    let parent_pos = body
        .parent_name
        .as_ref()
        .map(|p| resolve_position(p, bodies, cache, t))
        .unwrap_or(Vec3::ZERO);

    // Calculate local position relative to parent
    let local_pos = if let Some(parent_name) = &body.parent_name {
        if let Some(parent_body) = bodies.get(parent_name) {
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
    cache.insert(name.to_string(), abs_pos);
    abs_pos
}

// ============================================================================
// Visibility & Rendering
// ============================================================================

/// Calculate visibility (0.0-1.0) based on screen-space separation from parent.
fn calculate_visibility(
    body: &Body,
    body_pos: Vec3,
    positions: &HashMap<String, Vec3>,
    cam_scale: f32,
) -> f32 {
    // Bodies without parents (Sun) are always visible
    let Some(parent_name) = &body.parent_name else {
        return 1.0;
    };

    let parent_pos = positions.get(parent_name).copied().unwrap_or(Vec3::ZERO);
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

/// Detects clicks on bodies and opens transfer popup for valid targets.
/// Only bodies with the same parent as CurrentBody are valid targets.
fn handle_body_click(
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    body_query: Query<(Entity, &Body, &ComputedBody)>,
    current_body: Option<Res<CurrentBody>>,
    mut popup: ResMut<TransferPopup>,
) {
    // Only process left clicks
    if !mouse_button.just_pressed(MouseButton::Left) {
        return;
    }

    // Get cursor position
    let Ok(window) = windows.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else { return };

    // Get camera for world projection
    let Ok((camera, camera_transform)) = camera_query.single() else { return };

    // Get current body's parent
    let Some(ref current) = current_body else { return };
    let current_parent = body_query
        .iter()
        .find(|(e, _, _)| *e == current.entity)
        .and_then(|(_, b, _)| b.parent_name.clone());

    // Find the closest body to the click that:
    // 1. Is visible
    // 2. Has the same parent as current body (same SOI)
    // 3. Is not the current body
    let mut best_match: Option<(Entity, f32)> = None; // (entity, screen_distance)

    for (entity, body, computed) in body_query.iter() {
        // Skip invisible bodies
        if computed.visibility < 0.01 {
            continue;
        }

        // Skip current body
        if entity == current.entity {
            continue;
        }

        // Skip bodies with different parent (different SOI)
        if body.parent_name != current_parent {
            continue;
        }

        // Project body position to screen space
        let Ok(screen_pos) = camera.world_to_viewport(camera_transform, computed.position) else {
            continue;
        };

        // Check distance from cursor to body center
        let screen_dist = cursor_pos.distance(screen_pos);

        // Check if click is within the body's display radius (with some tolerance)
        let click_radius = computed.display_size * 2.0 + 10.0; // Extra tolerance for small bodies
        if screen_dist <= click_radius {
            // Track the closest match
            match &best_match {
                None => best_match = Some((entity, screen_dist)),
                Some((_, best_dist)) if screen_dist < *best_dist => {
                    best_match = Some((entity, screen_dist))
                }
                _ => {}
            }
        }
    }

    // Handle the click
    if let Some((clicked_entity, _)) = best_match {
        let body_name = body_query
            .get(clicked_entity)
            .map(|(_, b, _)| b.name.clone())
            .unwrap_or_default();

        info!("Clicked on body: {} (entity {:?})", body_name, clicked_entity);

        // Store for popup (popup spawning will be added in next step)
        popup.target_entity = Some(clicked_entity);
    }
}


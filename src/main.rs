#![allow(unused_imports)]

use std::cell::LazyCell;

use astrora_core::core::{
    Vector3,
    elements::{OrbitalElements, coe_to_rv},
};
use bevy::{
    asset::Assets,
    color::palettes::css::WHITE,
    diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin},
    gizmos::{config::{GizmoConfigStore, GizmoLineJoint}, GizmoAsset},
    math::Vec3,
    platform::collections::HashMap,
    prelude::*,
};
use bevy_pancam::{PanCam, PanCamPlugin};
use bevy_vector_shapes::prelude::*;

mod orbital_data;
mod simulation;
mod ui;

use orbital_data::{Body, PlanetaryElements, propagate_elliptic};
use simulation::SimulationTime;
use ui::BodyPositions;

/// Links an orbit gizmo entity to its parent body for position updates.
#[derive(Component)]
struct OrbitGizmo {
    /// Name of the parent body this orbit is centered on
    parent_name: String,
}

// --- Config ---
const ORBIT_SEGMENTS: usize = 10000;
const VISUAL_SCALE: f64 = 100.0; // Visual scaling factor relative to AU

// Lazy load AU from orbital_data
const AU: LazyCell<f64> = LazyCell::new(|| orbital_data::AU);

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(PanCamPlugin)
        .add_plugins(ShapePlugin::default())
        // Performance diagnostics - logs to console every second
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_plugins(LogDiagnosticsPlugin::default())
        .init_resource::<BodyPositions>()
        .init_resource::<SimulationTime>()
        .add_systems(Startup, (setup, configure_gizmos))
        .add_systems(
            Update,
            (
                simulation::handle_time_controls,
                update_orbit_positions,
                render_system,
                ui::update_labels,
                ui::update_time_ui,
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

    // Load planetary data (cached static, cloned into resource)
    let bodies = PlanetaryElements::get_planetary_elements();
    commands.insert_resource(PlanetaryElements {
        bodies: bodies.clone(),
    });

    // Spawn bodies and their orbit gizmos
    for body in bodies.values() {
        // Spawn the body entity
        commands.spawn(body.clone());
        ui::spawn_body_label(&mut commands, &body.name);

        // Create orbit gizmo if body has a parent
        if let Some(parent_name) = &body.parent_name {
            if let Some(parent_body) = bodies.get(parent_name) {
                let orbit_asset = create_orbit_gizmo_asset(body, parent_body);
                commands.spawn((
                    Gizmo {
                        handle: gizmo_assets.add(orbit_asset),
                        // Push orbits slightly behind other geometry (small positive = behind, but not at far plane)
                        depth_bias: 0.1,
                        ..default()
                    },
                    OrbitGizmo {
                        parent_name: parent_name.clone(),
                    },
                ));
            }
        }
    }

    // Spawn time control panel
    ui::spawn_time_panel(&mut commands);
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

    // Log point count for debugging
    info!(
        "Creating orbit gizmo for {} with {} points",
        body.name,
        points.len()
    );

    // Draw with semi-transparent white
    let color = Color::srgba(1.0, 1.0, 1.0, 0.3);
    gizmo.linestrip(points, color);

    gizmo
}

/// Update orbit gizmo Transform positions to match their parent body positions.
fn update_orbit_positions(
    mut orbit_query: Query<(&OrbitGizmo, &mut Transform)>,
    res_elements: Res<PlanetaryElements>,
    sim_time: Res<SimulationTime>,
) {
    let sim_time_seconds = sim_time.sim_time;
    let mut position_cache: HashMap<String, Vec3> = HashMap::new();
    position_cache.insert("Sun".to_string(), Vec3::ZERO);

    for (orbit_gizmo, mut transform) in &mut orbit_query {
        let parent_pos = resolve_position(
            &orbit_gizmo.parent_name,
            &res_elements.bodies,
            &mut position_cache,
            sim_time_seconds,
        );
        transform.translation = parent_pos;
    }
}

// ============================================================================
// Rendering
// ============================================================================

/// Converts physics coordinates (meters) to visual coordinates.
fn phys_to_visual(v: Vector3) -> Vec3 {
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

// LOD thresholds for screen-space distance (in pixels roughly)
const LOD_MIN_SCREEN_DIST: f32 = 5.0; // Below this = invisible
const LOD_MAX_SCREEN_DIST: f32 = 25.0; // Above this = fully visible

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

/// Main render system: draws bodies (orbits are now handled by retained gizmos).
fn render_system(
    body_query: Query<&Body>,
    res_elements: Res<PlanetaryElements>,
    mut painter: ShapePainter,
    camera_query: Query<(&Camera, &GlobalTransform, &Projection), With<Camera3d>>,
    mut body_positions: ResMut<BodyPositions>,
    sim_time: Res<SimulationTime>,
    mut frame_count: Local<u32>,
) {
    let Ok((_camera, _camera_transform, projection)) = camera_query.single() else {
        return;
    };

    let cam_scale = match projection {
        Projection::Orthographic(ortho) => ortho.scale,
        _ => 1.0,
    };

    let sim_time_seconds = sim_time.sim_time;

    // Build position and visibility caches
    let mut positions: HashMap<String, Vec3> = HashMap::new();
    let mut visibility: HashMap<String, f32> = HashMap::new();
    positions.insert("Sun".to_string(), Vec3::ZERO);
    visibility.insert("Sun".to_string(), 1.0);

    // First pass: calculate positions and visibility
    for body in body_query.iter() {
        let pos = resolve_position(&body.name, &res_elements.bodies, &mut positions, sim_time_seconds);
        let vis = calculate_visibility(body, pos, &positions, cam_scale);
        visibility.insert(body.name.clone(), vis);
    }

    // Second pass: render bodies
    let mut display_sizes: HashMap<String, f32> = HashMap::new();

    for body in body_query.iter() {
        let pos = *positions.get(&body.name).unwrap_or(&Vec3::ZERO);
        let vis = *visibility.get(&body.name).unwrap_or(&0.0);

        if vis < 0.01 {
            continue;
        }

        // Draw body
        let display_radius = compute_display_size(body, cam_scale);
        display_sizes.insert(body.name.clone(), display_radius);
        draw_body(&mut painter, body, pos, cam_scale, vis);
    }

    // Log timing and debug info every 60 frames (~1 second)
    *frame_count += 1;
    if *frame_count >= 60 {
        *frame_count = 0;
        info!("Render: cam_scale={:.4}", cam_scale);
    }

    // Store positions, visibility, and display sizes for label system
    body_positions.positions = positions;
    body_positions.visibility = visibility;
    body_positions.display_sizes = display_sizes;
}

/// Computes the display radius for a body (without drawing).
fn compute_display_size(body: &Body, cam_scale: f32) -> f32 {
    // Use logarithmic scaling for radius so all bodies are visible
    // log10(radius) ranges from ~4 (Phobos) to ~9 (Sun)
    // Scale to reasonable display sizes (1.5 to ~8 base units)
    let log_radius = (body.radius as f32).log10();
    let log_scaled_size = ((log_radius - 4.0).max(1.0) * 1.5).min(8.0) * cam_scale;

    // Calculate actual physical radius in visual coordinates
    let phys_radius = (body.radius * VISUAL_SCALE / *AU) as f32;

    // Use whichever is larger - ensures body is at least its true size when zoomed in
    log_scaled_size.max(phys_radius)
}

/// Draws a body circle.
fn draw_body(painter: &mut ShapePainter, body: &Body, pos: Vec3, cam_scale: f32, visibility: f32) {
    painter.set_translation(pos);

    // Apply visibility fade to body color
    let base_color = body.color.to_srgba();
    painter.set_color(Color::srgba(base_color.red, base_color.green, base_color.blue, visibility));

    let display_size = compute_display_size(body, cam_scale);
    painter.circle(display_size);
}


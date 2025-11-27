#![allow(unused_imports)]

use std::cell::LazyCell;

use astrora_core::core::{
    Vector3,
    elements::{OrbitalElements, coe_to_rv},
};
use bevy::{
    color::palettes::css::WHITE,
    diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin},
    gizmos::config::{GizmoConfigStore, GizmoLineJoint},
    math::Vec3,
    platform::collections::HashMap,
    prelude::*,
};
use bevy_pancam::{PanCam, PanCamPlugin};
use bevy_vector_shapes::prelude::*;
use big_space::prelude::*;

mod orbital_data;
mod simulation;
mod ui;

use orbital_data::{Body, PlanetaryElements, propagate_elliptic};
use simulation::SimulationTime;
use ui::BodyPositions;

/// Pre-computed orbit path in local coordinates (relative to parent).
/// Computed once at startup, translated at render time.
#[derive(Component)]
struct CachedOrbit {
    /// Points along the orbit in local coordinates (meters, then scaled to visual)
    points: Vec<Vec3>,
}

// --- Config ---
const ORBIT_SEGMENTS: usize = 10000;
const VISUAL_SCALE: f64 = 100.0; // Visual scaling factor relative to AU

// Lazy load AU from orbital_data
const AU: LazyCell<f64> = LazyCell::new(|| orbital_data::AU);

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(PanCamPlugin)
        .add_plugins(ShapePlugin::default())
        .add_plugins(BigSpaceDefaultPlugins)
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
                render_system,
                ui::update_labels,
                ui::update_time_ui,
            )
                .chain(),
        )
        .run();
}

fn setup(mut commands: Commands) {
    // Spawn camera
    commands.spawn((
        Camera3d::default(),
        Projection::from(OrthographicProjection {
            far: 100_000.0,
            near: -100_000.0,
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

    // Spawn bodies with pre-computed orbit cache
    for body in bodies.values() {
        let cached_orbit = compute_orbit_cache(body, &bodies);
        commands.spawn((body.clone(), cached_orbit));
        ui::spawn_body_label(&mut commands, &body.name);
    }

    // Spawn time control panel
    ui::spawn_time_panel(&mut commands);
}

/// Configure gizmo line settings for orbit rendering.
fn configure_gizmos(mut config_store: ResMut<GizmoConfigStore>) {
    let (config, _) = config_store.config_mut::<DefaultGizmoConfigGroup>();
    config.line.joints = GizmoLineJoint::None;
}

/// Pre-compute orbit points for a body (in local visual coordinates).
fn compute_orbit_cache(body: &Body, all_bodies: &HashMap<String, Body>) -> CachedOrbit {
    let mut points = Vec::with_capacity(ORBIT_SEGMENTS + 1);

    // Only compute if body has a parent
    if let Some(parent_name) = &body.parent_name {
        if let Some(parent_body) = all_bodies.get(parent_name) {
            let period = body
                .orbital_elements
                .period(parent_body.std_grav_param)
                .unwrap_or(0.0);
            let step_dt = period / ORBIT_SEGMENTS as f64;

            // Don't include endpoint (t=period) since it overlaps with start (t=0)
            for i in 0..ORBIT_SEGMENTS {
                let t = i as f64 * step_dt;
                if let Ok(elems) =
                    propagate_elliptic(body.orbital_elements, parent_body.std_grav_param, t)
                {
                    let (r_local, _) = coe_to_rv(&elems, parent_body.std_grav_param);
                    points.push(phys_to_visual(r_local));
                }
            }
        }
    }

    CachedOrbit { points }
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

/// Calculate "importance" (0.0-1.0) based on gravitational parameter relative to max in frame.
/// This makes orbits scale relative to the dominant body currently visible.
fn calculate_importance(mu: f64, max_mu: f64) -> f32 {
    let log_mu = (mu as f32).log10();
    let log_max = (max_mu as f32).log10();
    // Scale relative to max, with a floor so tiny bodies don't completely disappear
    // Using log scale: (log(mu) - 5) / (log(max) - 5) gives reasonable spread
    let floor = 5.0;
    ((log_mu - floor) / (log_max - floor)).clamp(0.2, 1.0)
}

/// Main render system: draws bodies and their orbits.
fn render_system(
    query: Query<(&Body, &CachedOrbit)>,
    res_elements: Res<PlanetaryElements>,
    mut painter: ShapePainter,
    mut gizmos: Gizmos,
    camera_query: Query<(&Camera, &GlobalTransform, &Projection), With<Camera3d>>,
    mut body_positions: ResMut<BodyPositions>,
    sim_time: Res<SimulationTime>,
    mut frame_count: Local<u32>,
) {
    use std::time::Instant;

    let Ok((camera, camera_transform, projection)) = camera_query.single() else {
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

    let mut total_position_time = std::time::Duration::ZERO;
    let mut total_orbit_time = std::time::Duration::ZERO;

    // First pass: calculate positions, visibility, and find max mass among bodies actually in viewport
    let mut max_mu_in_frame: f64 = 0.0;
    for (body, _) in query.iter() {
        let t0 = Instant::now();
        let pos = resolve_position(&body.name, &res_elements.bodies, &mut positions, sim_time_seconds);
        total_position_time += t0.elapsed();

        let vis = calculate_visibility(body, pos, &positions, cam_scale);
        visibility.insert(body.name.clone(), vis);

        // Only include in max_mu if body is actually in the viewport (with margin)
        // This ensures when zoomed into Earth, Jupiter doesn't dominate max_mu
        if vis > 0.01 && body.parent_name.is_some() {
            if let Ok(viewport_pos) = camera.world_to_viewport(camera_transform, pos) {
                // Check if within viewport with some margin
                if let Some(viewport) = camera.logical_viewport_size() {
                    let margin = 200.0; // Allow some off-screen bodies
                    if viewport_pos.x >= -margin
                        && viewport_pos.x <= viewport.x + margin
                        && viewport_pos.y >= -margin
                        && viewport_pos.y <= viewport.y + margin
                    {
                        max_mu_in_frame = max_mu_in_frame.max(body.std_grav_param);
                    }
                }
            }
        }
    }

    // Fallback: if no orbiting bodies in viewport, use largest planet (Jupiter-scale)
    if max_mu_in_frame == 0.0 {
        max_mu_in_frame = 1.0e17; // Roughly Jupiter's mu
    }

    // Second pass: render orbits and bodies
    let mut display_sizes: HashMap<String, f32> = HashMap::new();

    for (body, cached_orbit) in query.iter() {
        let pos = *positions.get(&body.name).unwrap_or(&Vec3::ZERO);
        let vis = *visibility.get(&body.name).unwrap_or(&0.0);

        if vis < 0.01 {
            continue;
        }

        let importance = calculate_importance(body.std_grav_param, max_mu_in_frame);

        // Draw orbit
        if let Some(parent_name) = &body.parent_name {
            let parent_pos = *positions.get(parent_name).unwrap_or(&Vec3::ZERO);
            let t1 = Instant::now();
            draw_cached_orbit(&mut gizmos, cached_orbit, parent_pos, vis, importance);
            total_orbit_time += t1.elapsed();
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
        info!(
            "Render: cam_scale={:.4}, max_mu={:.2e}",
            cam_scale,
            max_mu_in_frame
        );
        // Log a few specific bodies for debugging
        for name in ["Earth", "Moon", "Jupiter", "Io"] {
            if let Some(&vis) = visibility.get(name) {
                if let Some(body) = res_elements.bodies.get(name) {
                    let imp = calculate_importance(body.std_grav_param, max_mu_in_frame);
                    info!("  {}: vis={:.2}, imp={:.2}, mu={:.2e}", name, vis, imp, body.std_grav_param);
                }
            }
        }
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

/// Draw orbit from pre-computed cache using gizmos linestrip.
fn draw_cached_orbit(
    gizmos: &mut Gizmos,
    cached: &CachedOrbit,
    parent_pos: Vec3,
    visibility: f32,
    importance: f32,
) {
    if cached.points.is_empty() {
        return;
    }

    // Orbit alpha based on LOD visibility and body importance
    let orbit_alpha = 0.4 * visibility * importance;
    let color = Color::srgba(1.0, 1.0, 1.0, orbit_alpha);

    // Build world-space points including closing point
    let points = cached
        .points
        .iter()
        .map(|&p| parent_pos + p)
        .chain(std::iter::once(parent_pos + cached.points[0]));

    gizmos.linestrip(points, color);
}

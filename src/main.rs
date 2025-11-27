#![allow(unused_imports)]

use std::cell::LazyCell;

use astrora_core::core::{
    Vector3,
    elements::{OrbitalElements, coe_to_rv},
};
use bevy::{math::Vec3, platform::collections::HashMap, prelude::*};
use bevy_pancam::{PanCam, PanCamPlugin};
use bevy_vector_shapes::prelude::*;
use big_space::prelude::*;

mod orbital_data;
mod simulation;
mod ui;

use orbital_data::{Body, PlanetaryElements, propagate_elliptic};
use simulation::SimulationTime;
use ui::BodyPositions;

// --- Config ---
const ORBIT_SEGMENTS: usize = 1000;
const VISUAL_SCALE: f64 = 100.0; // Visual scaling factor relative to AU

// Lazy load AU from orbital_data
const AU: LazyCell<f64> = LazyCell::new(|| orbital_data::AU);

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(PanCamPlugin)
        .add_plugins(ShapePlugin::default())
        .add_plugins(BigSpaceDefaultPlugins)
        .init_resource::<BodyPositions>()
        .init_resource::<SimulationTime>()
        .add_systems(Startup, setup)
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

    // Spawn bodies and their labels
    for body in bodies.values() {
        commands.spawn(body.clone());
        ui::spawn_body_label(&mut commands, &body.name);
    }

    // Spawn time control panel
    ui::spawn_time_panel(&mut commands);
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

/// Main render system: draws bodies and their orbits.
fn render_system(
    query: Query<&Body>,
    res_elements: Res<PlanetaryElements>,
    mut painter: ShapePainter,
    camera_query: Query<&Projection, With<Camera3d>>,
    mut body_positions: ResMut<BodyPositions>,
    sim_time: Res<SimulationTime>,
) {
    let cam_scale = camera_query
        .single()
        .map(|proj| match proj {
            Projection::Orthographic(ortho) => ortho.scale,
            _ => 1.0,
        })
        .unwrap_or(1.0);

    let sim_time_seconds = sim_time.sim_time;

    // Build position cache
    let mut positions: HashMap<String, Vec3> = HashMap::new();
    positions.insert("Sun".to_string(), Vec3::ZERO);

    for body in query.iter() {
        let pos = resolve_position(&body.name, &res_elements.bodies, &mut positions, sim_time_seconds);

        // Draw body
        draw_body(&mut painter, body, pos, cam_scale);

        // Draw orbit
        if let Some(parent_name) = &body.parent_name {
            if let Some(parent_body) = res_elements.bodies.get(parent_name) {
                let parent_pos = *positions.get(parent_name).unwrap_or(&Vec3::ZERO);
                draw_orbit(&mut painter, body, parent_body, parent_pos, cam_scale);
            }
        }
    }

    // Store positions for label system
    body_positions.positions = positions;
}

fn draw_body(painter: &mut ShapePainter, body: &Body, pos: Vec3, cam_scale: f32) {
    painter.set_translation(pos);

    // Color and size based on body type
    if body.name == "Sun" {
        painter.set_color(Color::srgb(1.0, 1.0, 0.0));
        painter.circle(5.0 * cam_scale);
    } else if body.parent_name.as_deref() == Some("Sun") {
        // Planets
        painter.set_color(Color::srgb(0.2, 0.6, 1.0));
        painter.circle(3.0 * cam_scale);
    } else {
        // Moons
        painter.set_color(Color::srgb(0.7, 0.7, 0.7));
        painter.circle(1.5 * cam_scale);
    }
}

fn draw_orbit(
    painter: &mut ShapePainter,
    body: &Body,
    parent_body: &Body,
    parent_pos: Vec3,
    cam_scale: f32,
) {
    painter.set_translation(Vec3::ZERO);

    // Fade orbits when zoomed in
    let orbit_alpha = (0.15 * cam_scale).clamp(0.05, 0.25);
    painter.set_color(Color::srgba(1.0, 1.0, 1.0, orbit_alpha));

    // Use pixel-based thickness so lines stay thin regardless of zoom
    painter.thickness_type = ThicknessType::Pixels;
    painter.thickness = 1.0;

    // Calculate orbit path
    let period = body
        .orbital_elements
        .period(parent_body.std_grav_param)
        .unwrap_or(0.0);
    let step_dt = period / ORBIT_SEGMENTS as f64;

    let mut prev_point = None;

    for i in 0..=ORBIT_SEGMENTS {
        let t_orbit = i as f64 * step_dt;

        if let Ok(orbit_el) =
            propagate_elliptic(body.orbital_elements, parent_body.std_grav_param, t_orbit)
        {
            let (r_local, _) = coe_to_rv(&orbit_el, parent_body.std_grav_param);
            let world_point = parent_pos + phys_to_visual(r_local);

            if let Some(prev) = prev_point {
                painter.line(prev, world_point);
            }
            prev_point = Some(world_point);
        }
    }
}

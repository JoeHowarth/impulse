//! Common UI components and systems that run in both modes.
//!
//! Contains HUD elements (date/speed/zoom), body labels, and victory overlay.

use bevy::prelude::*;

use crate::common::rendering::ComputedBody;
use crate::common::simulation::SimulationTime;
use crate::model::VictoryState;

// ============================================================================
// Components
// ============================================================================

/// Links a UI label to its body entity
#[derive(Component)]
pub struct BodyLabel {
    pub body: Entity,
}

/// Marker for the simulation date text
#[derive(Component)]
pub struct SimDateText;

/// Marker for the speed/status text
#[derive(Component)]
pub struct SimSpeedText;

/// Marker for the zoom scale text
#[derive(Component)]
pub struct ZoomScaleText;

/// Marker for the victory overlay
#[derive(Component)]
pub struct VictoryOverlay;

// ============================================================================
// Startup Systems
// ============================================================================

/// Spawns the time control UI panel in the bottom-left corner.
pub fn spawn_time_panel(commands: &mut Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(16.0),
                left: Val::Px(16.0),
                padding: UiRect::all(Val::Px(12.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
            BorderRadius::all(Val::Px(8.0)),
        ))
        .with_children(|parent| {
            // Date display
            parent.spawn((
                Text::new("J2000 + 0d"),
                TextFont {
                    font_size: 16.0,
                    ..default()
                },
                TextColor(Color::srgba(0.9, 0.95, 1.0, 1.0)),
                SimDateText,
            ));

            // Speed display
            parent.spawn((
                Text::new("1x"),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::srgba(0.6, 0.7, 0.8, 1.0)),
                SimSpeedText,
            ));

            // Zoom scale display
            parent.spawn((
                Text::new("100px = 1 AU"),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgba(0.5, 0.6, 0.7, 1.0)),
                ZoomScaleText,
            ));

            // Controls hint
            parent.spawn((
                Text::new("[P] pause  [+/-] speed"),
                TextFont {
                    font_size: 10.0,
                    ..default()
                },
                TextColor(Color::srgba(0.5, 0.5, 0.6, 0.8)),
            ));
        });
}

/// Spawns a label for a body that will track its screen position.
pub fn spawn_body_label(commands: &mut Commands, name: &str, body_entity: Entity) {
    commands.spawn((
        Text::new(name),
        TextFont {
            font_size: 12.0,
            ..default()
        },
        TextColor(Color::srgba(1.0, 1.0, 1.0, 0.8)),
        TextLayout::default().with_no_wrap(),
        Node {
            position_type: PositionType::Absolute,
            ..default()
        },
        BodyLabel { body: body_entity },
    ));
}

// ============================================================================
// Update Systems
// ============================================================================

/// Updates body label positions by projecting world coordinates to screen space.
pub fn update_labels(
    mut labels: Query<(&mut Node, &mut TextColor, &BodyLabel)>,
    bodies: Query<(&ComputedBody, &GlobalTransform)>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };

    for (mut node, mut text_color, label) in &mut labels {
        let Ok((computed, body_transform)) = bodies.get(label.body) else {
            continue;
        };

        // Update label alpha based on visibility
        text_color.0 = Color::srgba(1.0, 1.0, 1.0, 0.8 * computed.visibility);

        // Project right edge of body to screen (offset by radius in world x)
        // Use GlobalTransform for camera-relative positioning
        let body_pos = body_transform.translation();
        let right_edge_world = body_pos + Vec3::new(computed.display_size, 0.0, 0.0);
        if let Ok(edge_screen) = camera.world_to_viewport(camera_transform, right_edge_world) {
            node.left = Val::Px(edge_screen.x + 4.0);
            node.top = Val::Px(edge_screen.y - 6.0);
        }
    }
}

/// Updates the time control UI with current simulation state.
pub fn update_time_ui(
    sim_time: Res<SimulationTime>,
    cam_scale: Res<crate::camera::CameraScale>,
    mut date_query: Query<
        &mut Text,
        (
            With<SimDateText>,
            Without<SimSpeedText>,
            Without<ZoomScaleText>,
        ),
    >,
    mut speed_query: Query<
        &mut Text,
        (
            With<SimSpeedText>,
            Without<SimDateText>,
            Without<ZoomScaleText>,
        ),
    >,
    mut zoom_query: Query<
        &mut Text,
        (
            With<ZoomScaleText>,
            Without<SimDateText>,
            Without<SimSpeedText>,
        ),
    >,
) {
    // Convert simulation seconds to days since J2000
    let days = sim_time.sim_time / (60.0 * 60.0 * 24.0);
    let years = days / 365.25;

    // Update date text
    if let Ok(mut text) = date_query.single_mut() {
        if years.abs() >= 1.0 {
            **text = format!("J2000 {:+.2}y", years);
        } else {
            **text = format!("J2000 {:+.1}d", days);
        }
    }

    // Update speed text
    if let Ok(mut text) = speed_query.single_mut() {
        let status = if sim_time.paused { "PAUSED" } else { "RUNNING" };
        let scale_str = crate::common::simulation::format_time_scale(sim_time.time_scale);
        **text = format!("{} {}", scale_str, status);
    }

    // Update zoom scale text
    // cam_scale.0 = world units (meters) per pixel
    // So 100 pixels = cam_scale.0 * 100 meters
    if let Ok(mut text) = zoom_query.single_mut() {
        let meters_per_100px = cam_scale.0 * 100.0;
        **text = format!("100px = {}", format_distance(meters_per_100px as f64));
    }
}

/// Spawns or updates the victory overlay when victory is achieved.
pub fn update_victory_overlay(
    mut commands: Commands,
    victory: Res<VictoryState>,
    existing: Query<Entity, With<VictoryOverlay>>,
) {
    // Only show if victory achieved and overlay doesn't exist
    if !victory.victory_achieved {
        return;
    }

    if !existing.is_empty() {
        return; // Already showing
    }

    // Calculate days to victory
    let victory_days = victory.victory_time.unwrap_or(0.0) / 86400.0;

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(20.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.1, 0.0, 0.85)),
            VictoryOverlay,
        ))
        .with_children(|parent| {
            // Victory title
            parent.spawn((
                Text::new("VICTORY"),
                TextFont {
                    font_size: 64.0,
                    ..default()
                },
                TextColor(Color::srgb(0.3, 1.0, 0.3)),
            ));

            // Subtitle
            parent.spawn((
                Text::new("All objectives completed!"),
                TextFont {
                    font_size: 24.0,
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.9, 0.8, 0.9)),
            ));

            // Time taken
            parent.spawn((
                Text::new(format!("Completed in {:.1} days", victory_days)),
                TextFont {
                    font_size: 18.0,
                    ..default()
                },
                TextColor(Color::srgba(0.6, 0.7, 0.6, 0.8)),
            ));
        });
}

// ============================================================================
// Helpers
// ============================================================================

/// Formats a distance in meters to a human-readable string.
fn format_distance(meters: f64) -> String {
    const AU: f64 = 1.495978707e11; // meters per AU
    const KM: f64 = 1000.0;

    if meters >= AU * 0.1 {
        format!("{:.2} AU", meters / AU)
    } else if meters >= KM * 1_000_000.0 {
        format!("{:.1}M km", meters / (KM * 1_000_000.0))
    } else if meters >= KM * 1000.0 {
        format!("{:.0}k km", meters / (KM * 1000.0))
    } else if meters >= KM {
        format!("{:.1} km", meters / KM)
    } else {
        format!("{:.0} m", meters)
    }
}

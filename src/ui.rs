//! UI components and systems for the orbital simulation.

use bevy::{platform::collections::HashMap, prelude::*};

use crate::simulation::SimulationTime;

/// Links a UI label to a body entity by name
#[derive(Component)]
pub struct BodyLabel {
    pub body_name: String,
}

/// Marker for the simulation date text
#[derive(Component)]
pub struct SimDateText;

/// Marker for the speed/status text
#[derive(Component)]
pub struct SimSpeedText;

/// Cached body positions and visibility for the current frame.
/// Updated by the render system, read by the label system.
#[derive(Resource, Default)]
pub struct BodyPositions {
    pub positions: HashMap<String, Vec3>,
    /// Visibility (0.0 = hidden, 1.0 = fully visible) based on screen-space separation
    pub visibility: HashMap<String, f32>,
    /// Display radius in world units for each body (for label positioning)
    pub display_sizes: HashMap<String, f32>,
}

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
pub fn spawn_body_label(commands: &mut Commands, name: &str) {
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
        BodyLabel {
            body_name: name.to_string(),
        },
    ));
}

/// Updates body label positions by projecting world coordinates to screen space.
pub fn update_labels(
    body_positions: Res<BodyPositions>,
    mut labels: Query<(&mut Node, &mut TextColor, &BodyLabel)>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };

    for (mut node, mut text_color, label) in &mut labels {
        let Some(&world_pos) = body_positions.positions.get(&label.body_name) else {
            continue;
        };

        // Get visibility (default to 1.0 for bodies without parents like Sun)
        let visibility = body_positions
            .visibility
            .get(&label.body_name)
            .copied()
            .unwrap_or(1.0);

        // Update label alpha based on visibility
        text_color.0 = Color::srgba(1.0, 1.0, 1.0, 0.8 * visibility);

        // Get display radius for this body (default to 0 for small offset)
        let display_radius = body_positions
            .display_sizes
            .get(&label.body_name)
            .copied()
            .unwrap_or(0.0);

        // Project right edge of body to screen (offset by radius in world x)
        let right_edge_world = world_pos + Vec3::new(display_radius, 0.0, 0.0);
        if let Ok(edge_screen) = camera.world_to_viewport(camera_transform, right_edge_world) {
            // Small padding from edge
            node.left = Val::Px(edge_screen.x + 4.0);
            node.top = Val::Px(edge_screen.y - 6.0);
        }
    }
}

/// Updates the time control UI with current simulation state.
pub fn update_time_ui(
    sim_time: Res<SimulationTime>,
    mut date_query: Query<&mut Text, (With<SimDateText>, Without<SimSpeedText>)>,
    mut speed_query: Query<&mut Text, (With<SimSpeedText>, Without<SimDateText>)>,
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
        let scale = sim_time.time_scale;

        // Format scale nicely
        let scale_str = if scale >= 1.0 {
            format!("{:.0}x", scale)
        } else {
            // Format without trailing zeros (0.125 not 0.125000)
            let num = format!("{:.3}", scale)
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string();
            format!("{}x", num)
        };

        **text = format!("{} {}", scale_str, status);
    }
}

//! UI components and systems for the orbital simulation.

use bevy::prelude::*;

use crate::simulation::SimulationTime;
use crate::transfer_vis::{ActiveTransfer, Transfer};
use crate::orbital_data::Body;
use crate::ComputedBody;

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

/// Updates body label positions by projecting world coordinates to screen space.
pub fn update_labels(
    mut labels: Query<(&mut Node, &mut TextColor, &BodyLabel)>,
    bodies: Query<&ComputedBody>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };

    for (mut node, mut text_color, label) in &mut labels {
        let Ok(computed) = bodies.get(label.body) else {
            continue;
        };

        // Update label alpha based on visibility
        text_color.0 = Color::srgba(1.0, 1.0, 1.0, 0.8 * computed.visibility);

        // Project right edge of body to screen (offset by radius in world x)
        let right_edge_world = computed.position + Vec3::new(computed.display_size, 0.0, 0.0);
        if let Ok(edge_screen) = camera.world_to_viewport(camera_transform, right_edge_world) {
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

// ============================================================================
// Transfer Info Panel
// ============================================================================

/// Marker for the transfer info panel container
#[derive(Component)]
pub struct TransferInfoPanel;

/// Marker for the transfer title text (e.g., "Earth → Mars")
#[derive(Component)]
pub struct TransferTitleText;

/// Marker for the transfer stats text (delta-v, TOF)
#[derive(Component)]
pub struct TransferStatsText;

/// Spawns the transfer info UI panel in the bottom-right corner.
pub fn spawn_transfer_panel(commands: &mut Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(16.0),
                right: Val::Px(16.0),
                padding: UiRect::all(Val::Px(12.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
            BorderRadius::all(Val::Px(8.0)),
            TransferInfoPanel,
        ))
        .with_children(|parent| {
            // Title (e.g., "Earth → Mars Transfer")
            parent.spawn((
                Text::new("No Transfer"),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::srgba(1.0, 0.6, 0.2, 1.0)), // Orange to match arc
                TransferTitleText,
            ));

            // Stats display
            parent.spawn((
                Text::new(""),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.85, 0.9, 1.0)),
                TransferStatsText,
            ));
        });
}

/// Updates the transfer info panel with current transfer data.
pub fn update_transfer_panel(
    active_transfer: Res<ActiveTransfer>,
    transfers: Query<&Transfer>,
    bodies: Query<&Body>,
    mut title_query: Query<&mut Text, (With<TransferTitleText>, Without<TransferStatsText>)>,
    mut stats_query: Query<&mut Text, (With<TransferStatsText>, Without<TransferTitleText>)>,
    mut logged: Local<bool>,
) {
    let Some(transfer_entity) = active_transfer.entity else {
        // No active transfer
        if !*logged {
            info!("update_transfer_panel: no active_transfer.entity");
            *logged = true;
        }
        if let Ok(mut title) = title_query.single_mut() {
            **title = "No Transfer".to_string();
        }
        if let Ok(mut stats) = stats_query.single_mut() {
            **stats = String::new();
        }
        return;
    };

    if !*logged {
        info!("update_transfer_panel: active_transfer.entity = {:?}", transfer_entity);
    }

    let Ok(transfer) = transfers.get(transfer_entity) else {
        if !*logged {
            info!("update_transfer_panel: transfers.get() failed for {:?}", transfer_entity);
            *logged = true;
        }
        return;
    };

    if !*logged {
        info!("update_transfer_panel: found transfer, updating UI");
        *logged = true;
    }

    // Get body names
    let source_name = bodies
        .get(transfer.source)
        .map(|b| b.name.as_str())
        .unwrap_or("???");
    let target_name = bodies
        .get(transfer.target)
        .map(|b| b.name.as_str())
        .unwrap_or("???");

    // Update title
    if let Ok(mut title) = title_query.single_mut() {
        **title = format!("{} -> {}", source_name, target_name);
    }

    // Update stats
    if let Ok(mut stats) = stats_query.single_mut() {
        let sol = &transfer.solution;
        let tof_days = sol.time_of_flight / (24.0 * 3600.0);

        **stats = format!(
            "Dep dv: {:.0} m/s\nArr dv: {:.0} m/s\nTotal: {:.0} m/s\nTOF: {:.0} days",
            sol.departure_dv.norm(),
            sol.arrival_dv.norm(),
            sol.total_dv,
            tof_days
        );
    }
}

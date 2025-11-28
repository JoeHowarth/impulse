//! UI components and systems for the orbital simulation.

use bevy::prelude::*;
use bevy::asset::Assets;
use bevy::gizmos::GizmoAsset;

use crate::simulation::SimulationTime;
use crate::transfer_vis::{ActiveTransfer, Transfer, TransferCache, find_best_transfer_in_range};
use crate::transfer::TransferSolution;
use crate::orbital_data::Body;
use crate::{ComputedBody, TransferPopup};

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

/// Marker for ship status text
#[derive(Component)]
pub struct ShipStatusText;

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
                row_gap: Val::Px(6.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
            BorderRadius::all(Val::Px(8.0)),
            TransferInfoPanel,
        ))
        .with_children(|parent| {
            // Ship status (location + delta-v)
            parent.spawn((
                Text::new("Ship: Earth | 10,000 m/s"),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgba(0.3, 0.9, 0.9, 1.0)), // Cyan to match ship
                ShipStatusText,
            ));

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

/// Updates the transfer info panel with current transfer data and ship status.
pub fn update_transfer_panel(
    active_transfer: Res<ActiveTransfer>,
    transfers: Query<&Transfer>,
    bodies: Query<&Body>,
    ships: Query<(&crate::ship::Ship, &crate::ship::ShipState)>,
    scheduled_transfers: Res<crate::ship::ScheduledTransfers>,
    sim_time: Res<SimulationTime>,
    mut ship_query: Query<&mut Text, (With<ShipStatusText>, Without<TransferTitleText>, Without<TransferStatsText>)>,
    mut title_query: Query<&mut Text, (With<TransferTitleText>, Without<ShipStatusText>, Without<TransferStatsText>)>,
    mut stats_query: Query<&mut Text, (With<TransferStatsText>, Without<TransferTitleText>, Without<ShipStatusText>)>,
) {
    // Update ship status
    if let Some((ship, state)) = ships.iter().next() {
        let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

        let location = match state {
            crate::ship::ShipState::Orbiting { body } => {
                bodies.get(*body).map(|b| b.name.as_str()).unwrap_or("???").to_string()
            }
            crate::ship::ShipState::Transferring { target, arrival_time, .. } => {
                let target_name = bodies.get(*target).map(|b| b.name.as_str()).unwrap_or("???");
                let arrival_day = (*arrival_time / 86400.0).floor() as i32;
                let days_to_arrival = arrival_day - current_day;
                format!("-> {} ({} days)", target_name, days_to_arrival)
            }
        };

        // Check for scheduled transfer
        let scheduled_info = if let Some(scheduled) = scheduled_transfers.transfers.first() {
            let days_to_departure = scheduled.departure_day - current_day;
            let target_name = bodies.get(scheduled.target).map(|b| b.name.as_str()).unwrap_or("???");
            format!(" | Dep to {} in {}d", target_name, days_to_departure)
        } else {
            String::new()
        };

        if let Ok(mut text) = ship_query.single_mut() {
            **text = format!(
                "Ship: {} | {:.0} m/s{}",
                location,
                ship.delta_v_remaining,
                scheduled_info
            );
        }
    }

    // Update transfer info
    let Some(transfer_entity) = active_transfer.entity else {
        // No active transfer
        if let Ok(mut title) = title_query.single_mut() {
            **title = "No Transfer".to_string();
        }
        if let Ok(mut stats) = stats_query.single_mut() {
            **stats = String::new();
        }
        return;
    };

    let Ok(transfer) = transfers.get(transfer_entity) else {
        return;
    };

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

// ============================================================================
// Transfer Popup UI
// ============================================================================

/// Marker for the transfer popup container
#[derive(Component)]
pub struct TransferPopupUI;

/// Marker for transfer option buttons with their index
#[derive(Component)]
pub struct TransferOptionButton {
    pub index: usize,
}

/// Marker for the close button
#[derive(Component)]
pub struct ClosePopupButton;

/// A transfer option for the popup menu
pub struct TransferOption {
    pub label: String,
    pub departure_day: i32,
    pub solution: TransferSolution,
}

/// Spawns a transfer popup near the clicked body position.
/// `available_dv` is used to grey out options that require more delta-v.
pub fn spawn_transfer_popup(
    commands: &mut Commands,
    target_name: &str,
    options: &[TransferOption],
    screen_pos: Vec2,
    available_dv: f64,
) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(screen_pos.x + 20.0),
                top: Val::Px(screen_pos.y - 40.0),
                padding: UiRect::all(Val::Px(12.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                min_width: Val::Px(180.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.1, 0.1, 0.15, 0.95)),
            BorderRadius::all(Val::Px(8.0)),
            TransferPopupUI,
        ))
        .with_children(|parent| {
            // Header row: "Transfer to [Target]" + [X]
            parent
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|row| {
                    row.spawn((
                        Text::new(format!("Transfer to {}", target_name)),
                        TextFont {
                            font_size: 14.0,
                            ..default()
                        },
                        TextColor(Color::srgba(1.0, 0.7, 0.3, 1.0)),
                    ));

                    // Close button
                    row.spawn((
                        Button,
                        Node {
                            width: Val::Px(24.0),
                            height: Val::Px(24.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            margin: UiRect::left(Val::Px(12.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.4, 0.2, 0.2, 1.0)),
                        BorderRadius::all(Val::Px(4.0)),
                        ClosePopupButton,
                    ))
                    .with_child((
                        Text::new("X"),
                        TextFont {
                            font_size: 12.0,
                            ..default()
                        },
                        TextColor(Color::srgba(1.0, 0.8, 0.8, 1.0)),
                    ));
                });

            // Option buttons
            for (i, opt) in options.iter().enumerate() {
                let tof_days = (opt.solution.time_of_flight / 86400.0) as i32;
                let dv = opt.solution.total_dv as i32;
                let dep_in = opt.departure_day; // Relative to current day
                let affordable = opt.solution.total_dv <= available_dv;

                // Grey out unaffordable options
                let bg_color = if affordable {
                    Color::srgba(0.2, 0.25, 0.35, 1.0)
                } else {
                    Color::srgba(0.15, 0.15, 0.18, 1.0)
                };
                let text_color = if affordable {
                    Color::srgba(0.9, 0.9, 0.95, 1.0)
                } else {
                    Color::srgba(0.5, 0.5, 0.55, 1.0)
                };

                parent
                    .spawn((
                        Button,
                        Node {
                            padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                            ..default()
                        },
                        BackgroundColor(bg_color),
                        BorderRadius::all(Val::Px(4.0)),
                        TransferOptionButton { index: i },
                    ))
                    .with_child((
                        Text::new(format!(
                            "{}: {} m/s, {}d TOF (dep +{}d)",
                            opt.label, dv, tof_days, dep_in
                        )),
                        TextFont {
                            font_size: 11.0,
                            ..default()
                        },
                        TextColor(text_color),
                    ));
            }

            // Show message if no options
            if options.is_empty() {
                parent.spawn((
                    Text::new("No transfers available"),
                    TextFont {
                        font_size: 11.0,
                        ..default()
                    },
                    TextColor(Color::srgba(0.6, 0.6, 0.7, 1.0)),
                ));
            }
        })
        .id()
}

/// Builds transfer options from the cache for a specific target.
pub fn build_transfer_options(
    cache: &TransferCache,
    target_name: &str,
    current_day: i32,
) -> Vec<TransferOption> {
    let mut options = Vec::new();

    // 1. Now (tomorrow to avoid immediate expiration)
    // We use current_day + 1 so the transfer doesn't expire immediately
    if let Some((dep_day, sol)) = find_best_transfer_in_range(cache, target_name, current_day + 1, current_day + 3) {
        options.push(TransferOption {
            label: "Now".to_string(),
            departure_day: dep_day - current_day,
            solution: sol.clone(),
        });
    }

    // 2. Best in 30 days
    if let Some((dep_day, sol)) = find_best_transfer_in_range(cache, target_name, current_day, current_day + 30) {
        // Only add if different from "Now" option
        let is_different = options.first().map_or(true, |o| {
            (o.solution.total_dv - sol.total_dv).abs() > 100.0 || (dep_day - current_day) > 2
        });
        if is_different {
            options.push(TransferOption {
                label: "Best 30d".to_string(),
                departure_day: dep_day - current_day,
                solution: sol.clone(),
            });
        }
    }

    // 3. Best in 180 days
    if let Some((dep_day, sol)) = find_best_transfer_in_range(cache, target_name, current_day, current_day + 180) {
        // Only add if different from previous options
        let is_different = options.iter().all(|o| {
            (o.solution.total_dv - sol.total_dv).abs() > 100.0
        });
        if is_different {
            options.push(TransferOption {
                label: "Best 180d".to_string(),
                departure_day: dep_day - current_day,
                solution: sol.clone(),
            });
        }
    }

    options
}

/// Despawns the transfer popup if it exists.
pub fn despawn_transfer_popup(commands: &mut Commands, popup: &mut TransferPopup) {
    if let Some(entity) = popup.popup_entity.take() {
        commands.entity(entity).despawn();
    }
    popup.target_entity = None;
    popup.hovered_option = None;
}

/// System to handle popup spawning when target is set.
pub fn handle_popup_spawn(
    mut commands: Commands,
    mut popup: ResMut<TransferPopup>,
    cache: Res<TransferCache>,
    sim_time: Res<SimulationTime>,
    bodies: Query<(&Body, &ComputedBody)>,
    player_query: Query<&crate::ship::Ship, With<crate::ship::PlayerControlled>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    // Check if we need to spawn a popup
    let Some(target_entity) = popup.target_entity else {
        return;
    };

    // If popup already exists, don't spawn again
    if popup.popup_entity.is_some() {
        return;
    }

    // Get target body info
    let Ok((target_body, target_computed)) = bodies.get(target_entity) else {
        popup.target_entity = None;
        return;
    };

    // Get screen position
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };

    let screen_pos = match camera.world_to_viewport(camera_transform, target_computed.position) {
        Ok(pos) => pos,
        Err(_) => {
            popup.target_entity = None;
            return;
        }
    };

    // Get available delta-v from player ship
    let available_dv = player_query.single().map(|s| s.delta_v_remaining).unwrap_or(0.0);

    // Build transfer options
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;
    let options = build_transfer_options(&cache, &target_body.name, current_day);

    info!("Spawning popup for {} with {} options", target_body.name, options.len());

    // Store options in popup resource for later selection handling
    popup.options = options;
    popup.options_computed_day = current_day;

    // Spawn the popup (pass reference to stored options)
    let popup_entity = spawn_transfer_popup(
        &mut commands,
        &target_body.name,
        &popup.options,
        screen_pos,
        available_dv,
    );

    popup.popup_entity = Some(popup_entity);
}

/// System to update popup position to follow target body.
pub fn update_popup_position(
    popup: Res<TransferPopup>,
    bodies: Query<&ComputedBody>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut popup_nodes: Query<&mut Node, With<TransferPopupUI>>,
) {
    // Only process if popup is open
    let Some(target_entity) = popup.target_entity else {
        return;
    };
    let Some(_popup_entity) = popup.popup_entity else {
        return;
    };

    // Get target body position
    let Ok(target_computed) = bodies.get(target_entity) else {
        return;
    };

    // Get screen position
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };
    let Ok(screen_pos) = camera.world_to_viewport(camera_transform, target_computed.position) else {
        return;
    };

    // Update popup position
    for mut node in &mut popup_nodes {
        node.left = Val::Px(screen_pos.x + 20.0);
        node.top = Val::Px(screen_pos.y - 40.0);
    }
}

/// System to update popup options when simulation day changes.
/// Rebuilds options and respawns popup UI to show updated values.
pub fn update_popup_options(
    mut commands: Commands,
    mut popup: ResMut<TransferPopup>,
    cache: Res<TransferCache>,
    sim_time: Res<SimulationTime>,
    bodies: Query<(&Body, &ComputedBody)>,
    player_query: Query<&crate::ship::Ship, With<crate::ship::PlayerControlled>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    // Only process if popup is open
    let Some(target_entity) = popup.target_entity else {
        return;
    };
    let Some(_popup_entity) = popup.popup_entity else {
        return;
    };

    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    // Check if day has changed
    if current_day == popup.options_computed_day {
        return;
    }

    // Get target body info
    let Ok((target_body, target_computed)) = bodies.get(target_entity) else {
        return;
    };

    // Get screen position for respawning popup
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };
    let screen_pos = match camera.world_to_viewport(camera_transform, target_computed.position) {
        Ok(pos) => pos,
        Err(_) => return,
    };

    // Get available delta-v from player ship
    let available_dv = player_query.single().map(|s| s.delta_v_remaining).unwrap_or(0.0);

    // Rebuild options
    let options = build_transfer_options(&cache, &target_body.name, current_day);

    // Preserve hover state if still valid
    let preserved_hover = popup.hovered_option.filter(|&idx| idx < options.len());

    // Despawn old popup
    if let Some(entity) = popup.popup_entity.take() {
        commands.entity(entity).despawn();
    }

    // Update popup state
    popup.options = options;
    popup.options_computed_day = current_day;
    popup.hovered_option = preserved_hover;

    // Respawn popup with new options
    let new_popup_entity = spawn_transfer_popup(
        &mut commands,
        &target_body.name,
        &popup.options,
        screen_pos,
        available_dv,
    );
    popup.popup_entity = Some(new_popup_entity);
}

/// System to handle close button clicks.
pub fn handle_close_button(
    mut commands: Commands,
    mut popup: ResMut<TransferPopup>,
    interactions: Query<&Interaction, (Changed<Interaction>, With<ClosePopupButton>)>,
) {
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            despawn_transfer_popup(&mut commands, &mut popup);
        }
    }
}

/// System to handle ESC key to close popup.
pub fn handle_escape_key(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut popup: ResMut<TransferPopup>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        if popup.popup_entity.is_some() {
            despawn_transfer_popup(&mut commands, &mut popup);
        }
    }
}

/// System to track hover state on option buttons for live preview.
pub fn handle_option_hover(
    mut popup: ResMut<TransferPopup>,
    interactions: Query<(&Interaction, &TransferOptionButton)>,
) {
    // Only process if popup is open
    if popup.popup_entity.is_none() {
        return;
    }

    // Find which button (if any) is being hovered
    let mut hovered = None;
    for (interaction, button) in &interactions {
        if *interaction == Interaction::Hovered || *interaction == Interaction::Pressed {
            hovered = Some(button.index);
            break;
        }
    }

    // Only update if changed to avoid unnecessary resource mutation
    if popup.hovered_option != hovered {
        popup.hovered_option = hovered;
    }
}

/// System to handle transfer option button selection.
/// When a button is clicked, schedule the transfer (deduct delta-v on departure).
pub fn handle_option_selection(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    mut popup: ResMut<TransferPopup>,
    mut active_transfer: ResMut<ActiveTransfer>,
    mut scheduled_transfers: ResMut<crate::ship::ScheduledTransfers>,
    player_query: Query<(Entity, &crate::ship::Ship, &crate::ship::ShipState), With<crate::ship::PlayerControlled>>,
    sim_time: Res<SimulationTime>,
    interactions: Query<(&Interaction, &TransferOptionButton), Changed<Interaction>>,
    // For despawning old transfer
    old_transfers: Query<Entity, With<Transfer>>,
    old_arcs: Query<Entity, With<crate::transfer_vis::TransferArc>>,
    old_markers: Query<Entity, With<crate::transfer_vis::BurnMarker>>,
) {
    use bevy::gizmos::GizmoAsset;
    use crate::transfer_vis::{Transfer, TransferArc, BurnMarker};

    for (interaction, button) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }

        // Get the selected option
        let Some(option) = popup.options.get(button.index) else {
            warn!("Invalid option index: {}", button.index);
            continue;
        };

        // Get player ship and current body
        let Ok((ship_entity, ship, ship_state)) = player_query.single() else {
            warn!("No player ship found");
            continue;
        };

        let source_entity = match ship_state {
            crate::ship::ShipState::Orbiting { body } => *body,
            crate::ship::ShipState::Transferring { .. } => {
                warn!("Ship is in transit");
                continue;
            }
        };

        let Some(target_entity) = popup.target_entity else {
            warn!("No target entity set");
            continue;
        };

        // Check if ship has enough delta-v
        if ship.delta_v_remaining < option.solution.total_dv {
            warn!(
                "Insufficient delta-v! Need {:.0} m/s, have {:.0} m/s",
                option.solution.total_dv, ship.delta_v_remaining
            );
            continue;
        }

        info!(
            "Scheduling transfer: {} (dep +{}d, {} m/s, remaining after: {:.0} m/s)",
            option.label,
            option.departure_day,
            option.solution.total_dv as i32,
            ship.delta_v_remaining - option.solution.total_dv
        );

        // Calculate absolute departure day
        let current_day = (sim_time.sim_time / 86400.0).floor() as i32;
        let departure_day = current_day + option.departure_day;
        let departure_time = departure_day as f64 * 86400.0;

        // Schedule the transfer (delta-v deducted when it executes)
        scheduled_transfers.transfers.push(crate::ship::ScheduledTransfer {
            ship: ship_entity,
            target: target_entity,
            solution: option.solution.clone(),
            departure_day,
        });

        // Despawn old transfer entities
        for entity in old_transfers.iter() {
            commands.entity(entity).despawn();
        }
        for entity in old_arcs.iter() {
            commands.entity(entity).despawn();
        }
        for entity in old_markers.iter() {
            commands.entity(entity).despawn();
        }

        // Spawn the new transfer visualization
        let transfer_entity = spawn_transfer_from_solution(
            &mut commands,
            &mut gizmo_assets,
            source_entity,
            target_entity,
            &option.solution,
            departure_time,
        );

        active_transfer.entity = Some(transfer_entity);
        active_transfer.user_selected = true; // Prevent auto-update from overwriting

        // Close the popup
        despawn_transfer_popup(&mut commands, &mut popup);

        // Only handle one click per frame
        break;
    }
}

/// Spawns a transfer visualization from a solution.
/// Returns the transfer entity.
fn spawn_transfer_from_solution(
    commands: &mut Commands,
    gizmo_assets: &mut ResMut<Assets<GizmoAsset>>,
    source_entity: Entity,
    target_entity: Entity,
    solution: &TransferSolution,
    departure_time: f64,
) -> Entity {
    use crate::transfer_vis::{Transfer, TransferArc, BurnMarker};
    use crate::phys_to_visual;
    use crate::transfer::propagate_kepler;
    use crate::orbital_data::MU_SUN;

    const TRANSFER_ARC_SEGMENTS: usize = 500;
    const TRANSFER_COLOR: Color = Color::srgba(1.0, 0.6, 0.2, 0.8);

    // Create the transfer arc gizmo
    let mut gizmo = GizmoAsset::new();
    let tof = solution.time_of_flight;
    let step_dt = tof / TRANSFER_ARC_SEGMENTS as f64;

    let mut points = Vec::with_capacity(TRANSFER_ARC_SEGMENTS + 1);
    points.push(phys_to_visual(solution.departure_pos));

    let r0 = solution.departure_pos;
    let v0 = solution.departure_vel;

    for i in 1..TRANSFER_ARC_SEGMENTS {
        let t = i as f64 * step_dt;
        if let Some(r_vec) = propagate_kepler(r0, v0, MU_SUN, t) {
            points.push(phys_to_visual(r_vec));
        }
    }
    points.push(phys_to_visual(solution.arrival_pos));
    gizmo.linestrip(points, TRANSFER_COLOR);

    // Spawn the Transfer entity
    let transfer_entity = commands
        .spawn(Transfer {
            source: source_entity,
            target: target_entity,
            solution: solution.clone(),
            departure_time,
        })
        .id();

    // Spawn the arc gizmo
    commands.spawn((
        Gizmo {
            handle: gizmo_assets.add(gizmo),
            depth_bias: 0.05,
            ..default()
        },
        TransferArc { transfer: transfer_entity },
    ));

    // Spawn burn markers
    commands.spawn(BurnMarker {
        transfer: transfer_entity,
        is_departure: true,
        delta_v: solution.departure_dv,
    });
    commands.spawn(BurnMarker {
        transfer: transfer_entity,
        is_departure: false,
        delta_v: solution.arrival_dv,
    });

    transfer_entity
}

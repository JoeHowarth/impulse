//! UI components and systems for the orbital simulation.

use bevy::prelude::*;

use crate::simulation::SimulationTime;
use crate::transfer_cache::{TransferCache, find_best_transfer_in_range, is_source_cached, request_cache_for_source, PendingCacheCompute};
use crate::transfer::TransferSolution;
use crate::orbital_data::Body;
use crate::ComputedBody;

// ============================================================================
// Resources
// ============================================================================

/// Transfer popup state - tracks if a popup is open and for which target.
#[derive(Resource, Default)]
pub struct TransferPopup {
    pub target_entity: Option<Entity>,
    pub popup_entity: Option<Entity>,
    /// Cached options for the currently displayed popup
    pub options: Vec<TransferOption>,
    /// Which option index is being hovered (for preview)
    pub hovered_option: Option<usize>,
    /// Day when options were last computed (for live updates)
    pub options_computed_day: i32,
    /// Source body name when waiting for cache computation
    pub waiting_for_cache: Option<String>,
}

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

/// Marker for ship status header text
#[derive(Component)]
pub struct ShipStatusText;

/// Marker for the flight plan rows text
#[derive(Component)]
pub struct FlightPlanText;

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
                min_width: Val::Px(280.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
            BorderRadius::all(Val::Px(8.0)),
            TransferInfoPanel,
        ))
        .with_children(|parent| {
            // Ship status header (location + delta-v)
            parent.spawn((
                Text::new("Ship: Earth | 500,000 m/s"),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgba(0.3, 0.9, 0.9, 1.0)), // Cyan to match ship
                ShipStatusText,
            ));

            // Flight plan rows (dynamically updated)
            parent.spawn((
                Text::new(""),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgba(0.9, 0.9, 0.9, 1.0)),
                FlightPlanText,
            ));
        });
}

/// Updates the transfer info panel with current transfer data and ship status.
/// Shows the selected fleet's info.
pub fn update_transfer_panel(
    bodies: Query<&Body>,
    selected_query: Query<(&crate::ship::Fleet, &crate::ship::ShipLocation, &crate::ship::FlightPlan), With<crate::ship::Selected>>,
    sim_time: Res<SimulationTime>,
    cache: Res<TransferCache>,
    mut ship_query: Query<&mut Text, (With<ShipStatusText>, Without<FlightPlanText>)>,
    mut plan_query: Query<&mut Text, (With<FlightPlanText>, Without<ShipStatusText>)>,
) {
    let Ok((fleet, location, plan)) = selected_query.single() else {
        // No fleet selected - clear panel
        if let Ok(mut text) = ship_query.single_mut() {
            **text = "No fleet selected".to_string();
        }
        if let Ok(mut text) = plan_query.single_mut() {
            **text = String::new();
        }
        return;
    };

    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;

    // Build header: fleet name, ship count, location, delta-v
    let location_name = match location {
        crate::ship::ShipLocation::AtBody(body) => {
            bodies.get(*body).map(|b| b.name.clone()).unwrap_or_else(|_| "???".into())
        }
        crate::ship::ShipLocation::InTransit { target, solution, departure_time } => {
            let target_name = bodies.get(*target).map(|b| b.name.as_str()).unwrap_or("???");
            let arrival_day = ((*departure_time + solution.time_of_flight) / 86400.0).floor() as i32;
            let days_left = arrival_day - current_day;
            format!("→ {} ({}d)", target_name, days_left)
        }
    };

    if let Ok(mut text) = ship_query.single_mut() {
        **text = format!("{} ({} ships) @ {} | {:.0} m/s", fleet.name, fleet.ship_count, location_name, fleet.delta_v_remaining);
    }

    // Build flight plan rows
    if let Ok(mut text) = plan_query.single_mut() {
        if plan.legs.is_empty() {
            **text = String::new();
        } else {
            let mut rows = Vec::new();
            let mut running_dv = fleet.delta_v_remaining;

            for (i, leg) in plan.legs.iter().enumerate() {
                let target_name = bodies.get(leg.target).map(|b| b.name.as_str()).unwrap_or("???");
                let source = crate::ship::leg_source(location, plan, i);

                // Look up solution to get delta-v
                let (dv, tof) = if let Some(sol) = crate::ship::leg_solution(&cache, source, leg) {
                    (sol.total_dv, leg.tof_days)
                } else {
                    // Solution not in cache - show what we have
                    (0.0, leg.tof_days)
                };

                running_dv -= dv;
                let arrival_day = leg.departure_day + tof;

                // Mark uncommitted legs
                let status = if i < plan.committed_count { "" } else { " *" };

                rows.push(format!(
                    "→ {:<8} {:>4}d {:>6.0} dv {:>7.0} rem{}",
                    target_name, arrival_day, dv, running_dv, status
                ));
            }

            // Add hint line
            let uncommitted = plan.legs.len() - plan.committed_count;
            if uncommitted > 0 {
                rows.push(format!("* = uncommitted ({}) | Enter=commit, N=cancel", uncommitted));
            }

            **text = rows.join("\n");
        }
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
    pub tof_days: i32,  // Exact cache key - must match for leg_solution lookup
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

/// Spawns a popup showing "Computing transfers from [source]..." message.
pub fn spawn_computing_popup(
    commands: &mut Commands,
    target_name: &str,
    source_name: &str,
    screen_pos: Vec2,
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

            // Computing message
            parent.spawn((
                Text::new(format!("Computing transfers from {}...", source_name)),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgba(0.7, 0.7, 0.8, 1.0)),
            ));
        })
        .id()
}

/// Builds transfer options from the cache for a specific source and target.
pub fn build_transfer_options(
    cache: &TransferCache,
    source_entity: Entity,
    target_entity: Entity,
    current_day: i32,
) -> Vec<TransferOption> {
    let mut options = Vec::new();

    // 1. Now (tomorrow to avoid immediate expiration)
    // We use current_day + 1 so the transfer doesn't expire immediately
    if let Some((dep_day, tof_days, sol)) = find_best_transfer_in_range(cache, source_entity, target_entity, current_day + 1, current_day + 3) {
        options.push(TransferOption {
            label: "Now".to_string(),
            departure_day: dep_day - current_day,
            tof_days,
            solution: sol.clone(),
        });
    }

    // 2. Best in 30 days
    if let Some((dep_day, tof_days, sol)) = find_best_transfer_in_range(cache, source_entity, target_entity, current_day, current_day + 30) {
        // Only add if different from "Now" option
        let is_different = options.first().map_or(true, |o| {
            (o.solution.total_dv - sol.total_dv).abs() > 100.0 || (dep_day - current_day) > 2
        });
        if is_different {
            options.push(TransferOption {
                label: "Best 30d".to_string(),
                departure_day: dep_day - current_day,
                tof_days,
                solution: sol.clone(),
            });
        }
    }

    // 3. Best in 180 days
    if let Some((dep_day, tof_days, sol)) = find_best_transfer_in_range(cache, source_entity, target_entity, current_day, current_day + 180) {
        // Only add if different from previous options
        let is_different = options.iter().all(|o| {
            (o.solution.total_dv - sol.total_dv).abs() > 100.0
        });
        if is_different {
            options.push(TransferOption {
                label: "Best 180d".to_string(),
                departure_day: dep_day - current_day,
                tof_days,
                solution: sol.clone(),
            });
        }
    }

    // 4. Best in 500 days (full search window)
    if let Some((dep_day, tof_days, sol)) = find_best_transfer_in_range(cache, source_entity, target_entity, current_day, current_day + 500) {
        // Only add if different from previous options
        let is_different = options.iter().all(|o| {
            (o.solution.total_dv - sol.total_dv).abs() > 100.0
        });
        if is_different {
            options.push(TransferOption {
                label: "Best 500d".to_string(),
                departure_day: dep_day - current_day,
                tof_days,
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
    popup.waiting_for_cache = None;
}

/// System to handle popup spawning when target is set.
pub fn handle_popup_spawn(
    mut commands: Commands,
    mut popup: ResMut<TransferPopup>,
    cache: Res<TransferCache>,
    sim_time: Res<SimulationTime>,
    bodies: Query<(&Body, &ComputedBody)>,
    player_query: Query<(Entity, &crate::ship::Fleet, &crate::ship::ShipLocation, &crate::ship::FlightPlan), With<crate::ship::Selected>>,
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

    // Get player ship state to find source body
    let Ok((_ship_entity, ship, location, plan)) = player_query.single() else {
        return;
    };

    // Determine source entity: where we'd depart from for next leg
    // This is the effective body after all current legs
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;
    let next_leg_index = plan.legs.len();
    let source_entity = crate::ship::leg_source(location, plan, next_leg_index);
    let base_day = crate::ship::leg_base_day(location, plan, next_leg_index, current_day);

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
    let available_dv = ship.delta_v_remaining;

    // Check if source is cached - if not, show "computing" message
    let source_name = bodies
        .get(source_entity)
        .map(|(b, _)| b.name.clone())
        .unwrap_or_else(|_| "Unknown".to_string());

    if !is_source_cached(&cache, source_entity) {
        info!("Spawning popup for {} - waiting for {} cache", target_body.name, source_name);

        popup.options = Vec::new();
        popup.options_computed_day = base_day;
        popup.waiting_for_cache = Some(source_name.clone());

        // Spawn popup with "computing" message
        let popup_entity = spawn_computing_popup(
            &mut commands,
            &target_body.name,
            &source_name,
            screen_pos,
        );

        popup.popup_entity = Some(popup_entity);
        return;
    }

    // Build transfer options
    let options = build_transfer_options(&cache, source_entity, target_entity, base_day);

    info!("Spawning popup for {} with {} options", target_body.name, options.len());

    // Store options in popup resource for later selection handling
    popup.options = options;
    popup.options_computed_day = base_day;
    popup.waiting_for_cache = None;

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

/// System to refresh popup when cache becomes available.
/// If popup is waiting for cache and the source is now cached, despawn and respawn with real options.
pub fn refresh_popup_on_cache_ready(
    mut commands: Commands,
    mut popup: ResMut<TransferPopup>,
    cache: Res<TransferCache>,
    sim_time: Res<SimulationTime>,
    bodies: Query<(&Body, &ComputedBody)>,
    player_query: Query<(Entity, &crate::ship::Fleet, &crate::ship::ShipLocation, &crate::ship::FlightPlan), With<crate::ship::Selected>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    // Only check if we're waiting for cache
    if popup.waiting_for_cache.is_none() {
        return;
    }
    let Some(target_entity) = popup.target_entity else {
        return;
    };

    // Get player ship state to find source body
    let Ok((_ship_entity, ship, location, plan)) = player_query.single() else {
        return;
    };

    // Determine source entity and base day using helpers
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;
    let next_leg_index = plan.legs.len();
    let source_entity = crate::ship::leg_source(location, plan, next_leg_index);
    let base_day = crate::ship::leg_base_day(location, plan, next_leg_index, current_day);

    // Check if source is now cached
    if !is_source_cached(&cache, source_entity) {
        return; // Still waiting
    }

    info!("Cache for source now ready, refreshing popup");

    // Get target body info
    let Ok((target_body, target_computed)) = bodies.get(target_entity) else {
        despawn_transfer_popup(&mut commands, &mut popup);
        return;
    };

    // Get screen position
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };
    let screen_pos = match camera.world_to_viewport(camera_transform, target_computed.position) {
        Ok(pos) => pos,
        Err(_) => {
            despawn_transfer_popup(&mut commands, &mut popup);
            return;
        }
    };

    // Get available delta-v
    let available_dv = ship.delta_v_remaining;

    // Build transfer options
    let options = build_transfer_options(&cache, source_entity, target_entity, base_day);

    info!("Refreshed popup for {} with {} options", target_body.name, options.len());

    // Despawn old popup
    if let Some(entity) = popup.popup_entity.take() {
        commands.entity(entity).despawn();
    }

    // Update popup state
    popup.options = options;
    popup.options_computed_day = base_day;
    popup.waiting_for_cache = None;

    // Spawn new popup with real options
    let new_popup_entity = spawn_transfer_popup(
        &mut commands,
        &target_body.name,
        &popup.options,
        screen_pos,
        available_dv,
    );
    popup.popup_entity = Some(new_popup_entity);
}

/// System to update popup options when simulation day changes.
/// Rebuilds options and respawns popup UI to show updated values.
pub fn update_popup_options(
    mut commands: Commands,
    mut popup: ResMut<TransferPopup>,
    cache: Res<TransferCache>,
    sim_time: Res<SimulationTime>,
    bodies: Query<(&Body, &ComputedBody)>,
    player_query: Query<(Entity, &crate::ship::Fleet, &crate::ship::ShipLocation, &crate::ship::FlightPlan), With<crate::ship::Selected>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    // Only process if popup is open
    let Some(target_entity) = popup.target_entity else {
        return;
    };
    let Some(_popup_entity) = popup.popup_entity else {
        return;
    };

    // Get player ship state to find source body
    let Ok((_ship_entity, ship, location, plan)) = player_query.single() else {
        return;
    };

    // Determine source entity and base day using helpers
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;
    let next_leg_index = plan.legs.len();
    let source_entity = crate::ship::leg_source(location, plan, next_leg_index);
    let base_day = crate::ship::leg_base_day(location, plan, next_leg_index, current_day);

    // Check if base day has changed
    if base_day == popup.options_computed_day {
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
    let available_dv = ship.delta_v_remaining;

    // Rebuild options
    let options = build_transfer_options(&cache, source_entity, target_entity, base_day);

    // Preserve hover state if still valid
    let preserved_hover = popup.hovered_option.filter(|&idx| idx < options.len());

    // Despawn old popup
    if let Some(entity) = popup.popup_entity.take() {
        commands.entity(entity).despawn();
    }

    // Update popup state
    popup.options = options;
    popup.options_computed_day = base_day;
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
/// Click appends new leg as uncommitted. Use Enter to commit.
pub fn handle_option_selection(
    mut commands: Commands,
    mut popup: ResMut<TransferPopup>,
    mut player_query: Query<(Entity, &crate::ship::Fleet, &crate::ship::ShipLocation, &mut crate::ship::FlightPlan), With<crate::ship::Selected>>,
    sim_time: Res<SimulationTime>,
    interactions: Query<(&Interaction, &TransferOptionButton), Changed<Interaction>>,
    bodies: Query<&crate::orbital_data::Body>,
    bodies_with_entity: Query<(Entity, &crate::orbital_data::Body)>,
    cache_res: Res<TransferCache>,
    pending_tasks: Query<&PendingCacheCompute>,
) {
    for (interaction, button) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }

        // Get the selected option
        let Some(option) = popup.options.get(button.index) else {
            warn!("Invalid option index: {}", button.index);
            continue;
        };

        // Get player ship and flight plan
        let Ok((_ship_entity, ship, location, mut plan)) = player_query.single_mut() else {
            warn!("No player ship found");
            continue;
        };

        let Some(target_entity) = popup.target_entity else {
            warn!("No target entity set");
            continue;
        };

        let current_day = (sim_time.sim_time / 86400.0).floor() as i32;
        let cache = cache_res.as_ref();

        // Source is where we'd depart from after all current legs
        let next_leg_index = plan.legs.len();
        let source_entity = crate::ship::leg_source(location, &plan, next_leg_index);
        let base_day = crate::ship::leg_base_day(location, &plan, next_leg_index, current_day);

        let departure_day = base_day + option.departure_day;
        let tof_days = option.tof_days;

        // Calculate total delta-v needed (all existing legs + this new one)
        let existing_dv: f64 = plan.legs.iter()
            .enumerate()
            .filter_map(|(i, leg)| {
                let src = crate::ship::leg_source(location, &plan, i);
                crate::ship::leg_solution(cache, src, leg).map(|s| s.total_dv)
            })
            .sum();

        let total_required = existing_dv + option.solution.total_dv;

        if ship.delta_v_remaining < total_required {
            warn!(
                "Insufficient delta-v! Need {:.0} m/s, have {:.0} m/s",
                total_required, ship.delta_v_remaining
            );
            continue;
        }

        let target_name = bodies.get(target_entity).map(|b| b.name.as_str()).unwrap_or("???");
        let source_name = bodies.get(source_entity).map(|b| b.name.as_str()).unwrap_or("???");

        info!(
            "Queueing leg {} -> {} (dep day {}, {} m/s)",
            source_name, target_name, departure_day, option.solution.total_dv as i32
        );

        plan.legs.push_back(crate::ship::PlannedLeg {
            target: target_entity,
            departure_day,
            tof_days,
        });

        // Proactively compute cache for target body
        let arrival_day = departure_day + tof_days;
        if !is_source_cached(cache, target_entity) {
            info!(
                "Proactively computing cache for {} (arrival day {})",
                target_name, arrival_day
            );
            request_cache_for_source(
                &mut commands,
                &bodies_with_entity,
                &pending_tasks,
                cache,
                target_entity,
                arrival_day,
            );
        }

        // Close the popup
        despawn_transfer_popup(&mut commands, &mut popup);

        // Only handle one click per frame
        break;
    }
}


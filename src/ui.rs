//! UI components and systems for the orbital simulation.

use bevy::prelude::*;

use crate::ComputedBody;
use crate::orbital_data::{Body, MU_SUN};
use crate::phys_vec_to_vec3;
use crate::simulation::SimulationTime;
use crate::transfer::{TransferSolution, propagate_kepler_full};
use crate::transfer_lut::TransferLut;

// ============================================================================
// Resources
// ============================================================================

/// Tracks fleet number key state for double-tap detection.
#[derive(Resource, Default)]
pub struct FleetKeyState {
    /// Last number key pressed (if any)
    pub last_key: Option<KeyCode>,
    /// Time of last key press (in seconds)
    pub last_press_time: f64,
}

/// Double-tap threshold in seconds
const DOUBLE_TAP_THRESHOLD: f64 = 0.35;

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

/// Marker for the zoom scale text
#[derive(Component)]
pub struct ZoomScaleText;

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
        let scale_str = crate::simulation::format_time_scale(sim_time.time_scale);
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

// ============================================================================
// Transfer Info Panel
// ============================================================================

/// Marker for the transfer info panel container
#[derive(Component)]
pub struct TransferInfoPanel;

/// Marker for ship status header text
#[derive(Component)]
pub struct FleetStatusText;

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
            // Fleet status header (location + delta-v)
            parent.spawn((
                Text::new("Fleet: Earth | 500,000 m/s"),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgba(0.3, 0.9, 0.9, 1.0)), // Cyan to match ship
                FleetStatusText,
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
    selected_query: Query<
        (
            Entity,
            &crate::ship::Fleet,
            &crate::ship::FleetLocation,
            &crate::ship::FlightPlan,
        ),
        With<crate::ship::Selected>,
    >,
    children_query: Query<&Children>,
    logical_ships: Query<&crate::ship::LogicalShip>,
    sim_time: Res<SimulationTime>,
    lut: Res<TransferLut>,
    mut fleet_status_query: Query<&mut Text, (With<FleetStatusText>, Without<FlightPlanText>)>,
    mut plan_query: Query<&mut Text, (With<FlightPlanText>, Without<FleetStatusText>)>,
) {
    let Ok((fleet_entity, fleet, location, plan)) = selected_query.single() else {
        // No fleet selected - clear panel
        if let Ok(mut text) = fleet_status_query.single_mut() {
            **text = "No fleet selected".to_string();
        }
        if let Ok(mut text) = plan_query.single_mut() {
            **text = String::new();
        }
        return;
    };

    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;
    let ship_count = crate::ship::ship_count(fleet_entity, &children_query, &logical_ships);

    // Build header: fleet name, ship count, location, delta-v
    let location_name = match location {
        crate::ship::FleetLocation::AtBody(body) => bodies
            .get(*body)
            .map(|b| b.name.clone())
            .unwrap_or_else(|_| "???".into()),
        crate::ship::FleetLocation::InTransit {
            source: _,
            target,
            solution,
            departure_time,
        } => {
            let target_name = bodies
                .get(*target)
                .map(|b| b.name.as_str())
                .unwrap_or("???");
            let arrival_day =
                ((*departure_time + solution.time_of_flight) / 86400.0).floor() as i32;
            let days_left = arrival_day - current_day;
            format!("→ {} ({}d)", target_name, days_left)
        }
    };

    if let Ok(mut text) = fleet_status_query.single_mut() {
        **text = format!(
            "{} ({} ships) @ {} | {:.0} m/s",
            fleet.name, ship_count, location_name, fleet.delta_v_remaining
        );
    }

    // Build flight plan rows
    if let Ok(mut text) = plan_query.single_mut() {
        if plan.legs.is_empty() {
            **text = String::new();
        } else {
            let mut rows = Vec::new();
            let mut running_dv = fleet.delta_v_remaining;

            for (i, leg) in plan.legs.iter().enumerate() {
                let target_name = bodies
                    .get(leg.target)
                    .map(|b| b.name.as_str())
                    .unwrap_or("???");
                let source = crate::ship::leg_source(location, plan, i);

                // Look up solution to get delta-v
                let dv = if let (Ok(source_body), Ok(target_body)) =
                    (bodies.get(source), bodies.get(leg.target))
                {
                    lut.get_transfer(
                        source,
                        leg.target,
                        &source_body.orbital_elements,
                        &target_body.orbital_elements,
                        leg.departure_day,
                        leg.tof_days,
                    )
                    .map(|s| s.total_dv)
                    .unwrap_or(0.0)
                } else {
                    0.0
                };

                running_dv -= dv;
                let arrival_day = leg.departure_day + leg.tof_days;

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
                rows.push(format!(
                    "* = uncommitted ({}) | Enter=commit, N=cancel",
                    uncommitted
                ));
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
    pub tof_days: i32,
    /// Full transfer solution from LUT
    pub solution: TransferSolution,
}

/// Spawns a transfer popup near the clicked body position.
/// `available_dv` is used to grey out options that require more delta-v.
pub fn spawn_transfer_popup(
    commands: &mut Commands,
    source_name: &str,
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
                min_width: Val::Px(200.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.1, 0.1, 0.15, 0.95)),
            BorderRadius::all(Val::Px(8.0)),
            TransferPopupUI,
        ))
        .with_children(|parent| {
            // Header row: "Source → Target" + [X]
            parent
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|row| {
                    row.spawn((
                        Text::new(format!("{} → {}", source_name, target_name)),
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
                            opt.label, dv, opt.tof_days, dep_in
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

/// Builds transfer options from the LUT for a specific source and target.
pub fn build_transfer_options(
    lut: &TransferLut,
    source_entity: Entity,
    target_entity: Entity,
    source_elements: &astrora_core::core::elements::OrbitalElements,
    target_elements: &astrora_core::core::elements::OrbitalElements,
    current_day: i32,
) -> Vec<TransferOption> {
    let mut options = Vec::new();

    // 1. Now (tomorrow to avoid immediate expiration)
    if let Some((dep_day, tof_days, sol)) = lut.find_best_transfer(
        source_entity,
        target_entity,
        source_elements,
        target_elements,
        current_day + 1,
        current_day + 3,
    ) {
        options.push(TransferOption {
            label: "Now".to_string(),
            departure_day: dep_day - current_day,
            tof_days,
            solution: sol,
        });
    }

    // 2. Best in 30 days
    if let Some((dep_day, tof_days, sol)) = lut.find_best_transfer(
        source_entity,
        target_entity,
        source_elements,
        target_elements,
        current_day,
        current_day + 30,
    ) {
        let is_different = options.first().map_or(true, |o| {
            (o.solution.total_dv - sol.total_dv).abs() > 100.0 || (dep_day - current_day) > 2
        });
        if is_different {
            options.push(TransferOption {
                label: "Best 30d".to_string(),
                departure_day: dep_day - current_day,
                tof_days,
                solution: sol,
            });
        }
    }

    // 3. Best in 180 days
    if let Some((dep_day, tof_days, sol)) = lut.find_best_transfer(
        source_entity,
        target_entity,
        source_elements,
        target_elements,
        current_day,
        current_day + 180,
    ) {
        let is_different = options
            .iter()
            .all(|o| (o.solution.total_dv - sol.total_dv).abs() > 100.0);
        if is_different {
            options.push(TransferOption {
                label: "Best 180d".to_string(),
                departure_day: dep_day - current_day,
                tof_days,
                solution: sol,
            });
        }
    }

    // 4. Best in 500 days (full search window)
    if let Some((dep_day, tof_days, sol)) = lut.find_best_transfer(
        source_entity,
        target_entity,
        source_elements,
        target_elements,
        current_day,
        current_day + 500,
    ) {
        let is_different = options
            .iter()
            .all(|o| (o.solution.total_dv - sol.total_dv).abs() > 100.0);
        if is_different {
            options.push(TransferOption {
                label: "Best 500d".to_string(),
                departure_day: dep_day - current_day,
                tof_days,
                solution: sol,
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
    lut: Res<TransferLut>,
    sim_time: Res<SimulationTime>,
    bodies: Query<(&Body, &ComputedBody, &GlobalTransform)>,
    player_query: Query<
        (
            Entity,
            &crate::ship::Fleet,
            &crate::ship::FleetLocation,
            &crate::ship::FlightPlan,
        ),
        With<crate::ship::Selected>,
    >,
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
    let current_day = (sim_time.sim_time / 86400.0).floor() as i32;
    let next_leg_index = plan.legs.len();
    let source_entity = crate::ship::leg_source(location, plan, next_leg_index);
    let base_day = crate::ship::leg_base_day(location, plan, next_leg_index, current_day);

    // Get source body orbital elements
    let Ok((source_body, _, _)) = bodies.get(source_entity) else {
        popup.target_entity = None;
        return;
    };

    // Get target body info
    let Ok((target_body, _target_computed, target_transform)) = bodies.get(target_entity) else {
        popup.target_entity = None;
        return;
    };

    // Get screen position (using GlobalTransform which is camera-relative via big_space)
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };

    let screen_pos =
        match camera.world_to_viewport(camera_transform, target_transform.translation()) {
            Ok(pos) => pos,
            Err(_) => {
                popup.target_entity = None;
                return;
            }
        };

    // Get available delta-v from player ship
    let available_dv = ship.delta_v_remaining;

    // Build transfer options (LUT is always ready - no "computing" state)
    let options = build_transfer_options(
        &lut,
        source_entity,
        target_entity,
        &source_body.orbital_elements,
        &target_body.orbital_elements,
        base_day,
    );

    info!(
        "Spawning popup for {} with {} options",
        target_body.name,
        options.len()
    );

    // Store options in popup resource for later selection handling
    popup.options = options;
    popup.options_computed_day = base_day;
    popup.waiting_for_cache = None;

    // Spawn the popup (pass reference to stored options)
    let popup_entity = spawn_transfer_popup(
        &mut commands,
        &source_body.name,
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
    bodies: Query<&GlobalTransform, With<Body>>,
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

    // Get target body position (camera-relative via big_space)
    let Ok(target_transform) = bodies.get(target_entity) else {
        return;
    };

    // Get screen position
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };
    let Ok(screen_pos) = camera.world_to_viewport(camera_transform, target_transform.translation())
    else {
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
    lut: Res<TransferLut>,
    sim_time: Res<SimulationTime>,
    bodies: Query<(&Body, &ComputedBody, &GlobalTransform)>,
    player_query: Query<
        (
            Entity,
            &crate::ship::Fleet,
            &crate::ship::FleetLocation,
            &crate::ship::FlightPlan,
        ),
        With<crate::ship::Selected>,
    >,
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

    // Get source body orbital elements
    let Ok((source_body, _, _)) = bodies.get(source_entity) else {
        return;
    };

    // Get target body info
    let Ok((target_body, _target_computed, target_transform)) = bodies.get(target_entity) else {
        return;
    };

    // Get screen position for respawning popup (camera-relative via big_space)
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };
    let screen_pos =
        match camera.world_to_viewport(camera_transform, target_transform.translation()) {
            Ok(pos) => pos,
            Err(_) => return,
        };

    // Get available delta-v from player ship
    let available_dv = ship.delta_v_remaining;

    // Rebuild options
    let options = build_transfer_options(
        &lut,
        source_entity,
        target_entity,
        &source_body.orbital_elements,
        &target_body.orbital_elements,
        base_day,
    );

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
        &source_body.name,
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
    mut player_query: Query<
        (
            Entity,
            &crate::ship::Fleet,
            &crate::ship::FleetLocation,
            &mut crate::ship::FlightPlan,
        ),
        With<crate::ship::Selected>,
    >,
    sim_time: Res<SimulationTime>,
    interactions: Query<(&Interaction, &TransferOptionButton), Changed<Interaction>>,
    bodies: Query<&crate::orbital_data::Body>,
    lut: Res<TransferLut>,
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

        // Source is where we'd depart from after all current legs
        let next_leg_index = plan.legs.len();
        let source_entity = crate::ship::leg_source(location, &plan, next_leg_index);
        let base_day = crate::ship::leg_base_day(location, &plan, next_leg_index, current_day);

        let departure_day = base_day + option.departure_day;
        let tof_days = option.tof_days;

        // Calculate total delta-v needed (all existing legs + this new one)
        let existing_dv: f64 = plan
            .legs
            .iter()
            .enumerate()
            .filter_map(|(i, leg)| {
                let src = crate::ship::leg_source(location, &plan, i);
                let (Ok(src_body), Ok(tgt_body)) = (bodies.get(src), bodies.get(leg.target)) else {
                    return None;
                };
                lut.get_transfer(
                    src,
                    leg.target,
                    &src_body.orbital_elements,
                    &tgt_body.orbital_elements,
                    leg.departure_day,
                    leg.tof_days,
                )
                .map(|s| s.total_dv)
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

        let target_name = bodies
            .get(target_entity)
            .map(|b| b.name.as_str())
            .unwrap_or("???");
        let source_name = bodies
            .get(source_entity)
            .map(|b| b.name.as_str())
            .unwrap_or("???");

        info!(
            "Queueing leg {} -> {} (dep day {}, {} m/s)",
            source_name, target_name, departure_day, option.solution.total_dv as i32
        );

        plan.legs.push_back(crate::ship::PlannedLeg {
            target: target_entity,
            departure_day,
            tof_days,
        });

        // Close the popup
        despawn_transfer_popup(&mut commands, &mut popup);

        // Only handle one click per frame
        break;
    }
}

// ============================================================================
// Fleet Tabs UI
// ============================================================================

/// Marker for the fleet tabs container
#[derive(Component)]
pub struct FleetTabsContainer;

/// Marker for individual fleet tab with the fleet entity
#[derive(Component)]
pub struct FleetTab {
    pub fleet_entity: Entity,
}

/// Spawns the fleet tabs UI container at the bottom center of the screen.
pub fn spawn_fleet_tabs(commands: &mut Commands) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(16.0),
            left: Val::Percent(50.0),
            // Will be centered via transform or margin
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(8.0),
            padding: UiRect::all(Val::Px(8.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
        BorderRadius::all(Val::Px(8.0)),
        FleetTabsContainer,
    ));
}

/// Updates fleet tabs - rebuilds when fleet count changes.
pub fn update_fleet_tabs(
    mut commands: Commands,
    fleets: Query<(
        Entity,
        &crate::ship::Fleet,
        Option<&crate::ship::Selected>,
        &crate::ship::Faction,
    )>,
    children_query: Query<&Children>,
    logical_ships: Query<&crate::ship::LogicalShip>,
    container_query: Query<Entity, With<FleetTabsContainer>>,
    existing_tabs: Query<(Entity, &FleetTab)>,
) {
    let Ok(container) = container_query.single() else {
        return;
    };

    // Collect player fleet info sorted for consistent ordering
    let mut fleet_info: Vec<_> = fleets
        .iter()
        .filter(|(_, _, _, faction)| **faction == crate::ship::Faction::Player)
        .collect();
    fleet_info.sort_by(|a, b| a.1.name.cmp(&b.1.name));

    // Check if we need to rebuild
    let current_tab_count = existing_tabs.iter().count();
    let need_rebuild = current_tab_count != fleet_info.len();

    if need_rebuild {
        // Despawn existing tabs
        for (tab_entity, _) in existing_tabs.iter() {
            commands.entity(tab_entity).despawn();
        }

        // Spawn new tabs as children of container
        for (index, (fleet_entity, fleet, is_selected, _)) in fleet_info.iter().enumerate() {
            let is_selected = is_selected.is_some();
            let bg_color = if is_selected {
                Color::srgba(0.2, 0.4, 0.5, 0.9)
            } else {
                Color::srgba(0.1, 0.15, 0.2, 0.7)
            };

            let fleet_entity_copy = *fleet_entity;
            let fleet_name = fleet.name.clone();
            let ship_count =
                crate::ship::ship_count(*fleet_entity, &children_query, &logical_ships);
            let delta_v = fleet.delta_v_remaining;

            commands.entity(container).with_children(|parent| {
                let text_alpha = if is_selected { 1.0 } else { 0.6 };

                parent
                    .spawn((
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: Val::Px(6.0),
                            padding: UiRect {
                                left: Val::Px(10.0),
                                right: Val::Px(10.0),
                                top: Val::Px(6.0),
                                bottom: Val::Px(6.0),
                            },
                            ..default()
                        },
                        BackgroundColor(bg_color),
                        BorderRadius::all(Val::Px(4.0)),
                        FleetTab {
                            fleet_entity: fleet_entity_copy,
                        },
                    ))
                    .with_children(|tab| {
                        // Number key hint
                        tab.spawn((
                            Text::new(format!("{}", index + 1)),
                            TextFont {
                                font_size: 10.0,
                                ..default()
                            },
                            TextColor(Color::srgba(0.6, 0.7, 0.8, text_alpha * 0.7)),
                        ));

                        // Color indicator
                        tab.spawn((
                            Node {
                                width: Val::Px(8.0),
                                height: Val::Px(8.0),
                                ..default()
                            },
                            BackgroundColor(
                                crate::ship::FLEET_PLAYER_SELECTED.with_alpha(if is_selected {
                                    1.0
                                } else {
                                    0.5
                                }),
                            ),
                            BorderRadius::all(Val::Px(2.0)),
                        ));

                        // Fleet name
                        tab.spawn((
                            Text::new(&fleet_name),
                            TextFont {
                                font_size: 12.0,
                                ..default()
                            },
                            TextColor(Color::srgba(0.9, 0.95, 1.0, text_alpha)),
                        ));

                        // Ship count
                        tab.spawn((
                            Text::new(format!("{}s", ship_count)),
                            TextFont {
                                font_size: 10.0,
                                ..default()
                            },
                            TextColor(Color::srgba(0.7, 0.8, 0.9, text_alpha * 0.8)),
                        ));

                        // Delta-v
                        let dv_str = if delta_v >= 1000.0 {
                            format!("{:.0}k", delta_v / 1000.0)
                        } else {
                            format!("{:.0}", delta_v)
                        };
                        tab.spawn((
                            Text::new(dv_str),
                            TextFont {
                                font_size: 10.0,
                                ..default()
                            },
                            TextColor(Color::srgba(0.5, 0.9, 0.5, text_alpha * 0.8)),
                        ));
                    });
            });
        }
    } else {
        // Just update existing tabs' appearance
        for (tab_entity, tab) in existing_tabs.iter() {
            let Some((_, _fleet, is_selected, _)) = fleet_info
                .iter()
                .find(|(e, _, _, _)| *e == tab.fleet_entity)
            else {
                continue;
            };
            let is_selected = is_selected.is_some();

            // Update background color based on selection
            let bg_color = if is_selected {
                Color::srgba(0.2, 0.4, 0.5, 0.9)
            } else {
                Color::srgba(0.1, 0.15, 0.2, 0.7)
            };
            commands
                .entity(tab_entity)
                .insert(BackgroundColor(bg_color));

            // Update text content would require additional children queries
            // For simplicity, we'll rebuild on selection change - check below
        }
    }
}

/// System to handle number key presses for fleet selection.
/// Double-tap a number key to pan the camera to that fleet.
pub fn handle_fleet_number_keys(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut key_state: ResMut<FleetKeyState>,
    fleets: Query<(
        Entity,
        &crate::ship::Fleet,
        &crate::ship::FleetLocation,
        &crate::ship::Faction,
    )>,
    selected: Query<Entity, With<crate::ship::Selected>>,
    bodies: Query<&GlobalTransform, With<Body>>,
    sim_time: Res<SimulationTime>,
    mut camera_query: Query<&mut crate::camera::CameraTarget>,
) {
    // Map digit keys to indices
    let key_to_index = [
        (KeyCode::Digit1, 0),
        (KeyCode::Digit2, 1),
        (KeyCode::Digit3, 2),
        (KeyCode::Digit4, 3),
        (KeyCode::Digit5, 4),
        (KeyCode::Digit6, 5),
        (KeyCode::Digit7, 6),
        (KeyCode::Digit8, 7),
        (KeyCode::Digit9, 8),
    ];

    for (key, index) in key_to_index {
        if keyboard.just_pressed(key) {
            // Get player fleets sorted by name for consistent ordering
            let mut fleet_list: Vec<_> = fleets
                .iter()
                .filter(|(_, _, _, faction)| **faction == crate::ship::Faction::Player)
                .collect();
            fleet_list.sort_by(|a, b| a.1.name.cmp(&b.1.name));

            if let Some((fleet_entity, fleet, location, _)) = fleet_list.get(index) {
                let current_time = time.elapsed_secs_f64();
                let is_double_tap = key_state.last_key == Some(key)
                    && (current_time - key_state.last_press_time) < DOUBLE_TAP_THRESHOLD;

                if is_double_tap {
                    // Double-tap: zoom camera to fleet position
                    info!("Double-tap: zooming to fleet {}", fleet.name);

                    // Get fleet position
                    let fleet_pos = match location {
                        crate::ship::FleetLocation::AtBody(body_entity) => bodies
                            .get(*body_entity)
                            .map(|gt| gt.translation())
                            .unwrap_or(Vec3::ZERO),
                        crate::ship::FleetLocation::InTransit {
                            solution,
                            departure_time,
                            ..
                        } => {
                            let elapsed = sim_time.sim_time - departure_time;
                            if elapsed >= 0.0 {
                                if let Some((r_vec, _)) = propagate_kepler_full(
                                    solution.departure_pos,
                                    solution.departure_vel,
                                    MU_SUN,
                                    elapsed,
                                ) {
                                    phys_vec_to_vec3(r_vec)
                                } else {
                                    Vec3::ZERO
                                }
                            } else {
                                Vec3::ZERO
                            }
                        }
                    };

                    // Set camera target for smooth animation (just recenter, no zoom)
                    if let Ok(mut camera_target) = camera_query.single_mut() {
                        camera_target.pan_to(Vec2::new(fleet_pos.x, fleet_pos.y));
                    }

                    // Clear double-tap state
                    key_state.last_key = None;
                } else {
                    // Single tap: select fleet
                    info!("Selecting fleet {} via key {}", fleet.name, index + 1);

                    // Remove Selected from all
                    for old in selected.iter() {
                        commands.entity(old).remove::<crate::ship::Selected>();
                    }

                    // Add Selected to target
                    commands.entity(*fleet_entity).insert(crate::ship::Selected);

                    // Record key press for double-tap detection
                    key_state.last_key = Some(key);
                    key_state.last_press_time = current_time;
                }
            }
            break;
        }
    }
}

// ============================================================================
// Victory Overlay
// ============================================================================

/// Marker for the victory overlay
#[derive(Component)]
pub struct VictoryOverlay;

/// Spawns or updates the victory overlay when victory is achieved.
pub fn update_victory_overlay(
    mut commands: Commands,
    victory: Res<crate::ship::VictoryState>,
    existing: Query<Entity, With<VictoryOverlay>>,
    sim_time: Res<SimulationTime>,
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

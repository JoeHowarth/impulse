//! Strategic mode input handling.
//!
//! Input systems that post StrategicCommand events.

use bevy::ecs::message::MessageWriter;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::common::ComputedBody;
use crate::model::{
    Body, CombatState, ComputedFleetPosition, Faction, Fleet, FleetLocation, Selected,
};

use super::commands::StrategicCommand;
use super::ui::TransferPopup;

// ============================================================================
// Input Systems
// ============================================================================

/// Detects clicks on fleets or bodies (strategic mode only).
/// - Click: select fleet if one is at click location
/// - Shift+click: open transfer popup for selected fleet
pub fn handle_body_click(
    mouse_button: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    body_query: Query<(Entity, &Body, &ComputedBody, &GlobalTransform)>,
    fleet_query: Query<(Entity, &Fleet, &FleetLocation, &Faction)>,
    fleet_positions: Query<(Entity, &ComputedFleetPosition, &Faction)>,
    selected_query: Query<Entity, With<Selected>>,
    mut popup: ResMut<TransferPopup>,
    combat: Res<CombatState>,
    mut cmd_writer: MessageWriter<StrategicCommand>,
) {
    // Skip in tactical mode - picking::handle_tactical_click handles that
    if combat.active {
        return;
    }

    if !mouse_button.just_pressed(MouseButton::Left) {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };

    let shift_held = keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);

    if shift_held {
        // Shift+click: open transfer popup for body
        let selected_fleet = fleet_query
            .iter()
            .filter(|(_, _, _, faction)| **faction == Faction::Player)
            .find(|(e, _, _, _)| selected_query.get(*e).is_ok());
        let current_entity = selected_fleet.map(|(_, _, loc, _)| loc.effective_body());

        let mut best_match: Option<(Entity, f32)> = None;
        for (entity, _body, computed, global_transform) in body_query.iter() {
            if computed.visibility < 0.01 {
                continue;
            }
            if current_entity == Some(entity) {
                continue;
            }

            let Ok(screen_pos) =
                camera.world_to_viewport(camera_transform, global_transform.translation())
            else {
                continue;
            };

            let screen_dist = cursor_pos.distance(screen_pos);
            let click_radius = computed.display_size * 2.0 + 10.0;
            if screen_dist <= click_radius {
                match &best_match {
                    None => best_match = Some((entity, screen_dist)),
                    Some((_, best_dist)) if screen_dist < *best_dist => {
                        best_match = Some((entity, screen_dist))
                    }
                    _ => {}
                }
            }
        }

        if let Some((clicked_entity, _)) = best_match {
            let body_name = body_query
                .get(clicked_entity)
                .map(|(_, b, _, _)| b.name.clone())
                .unwrap_or_default();
            info!("Shift+clicked on body: {}", body_name);
            popup.target_entity = Some(clicked_entity);
        }
    } else {
        // Click: select fleet if there's one at click location
        let mut best_match: Option<(Entity, f32)> = None;
        for (fleet_entity, computed, faction) in fleet_positions.iter() {
            // Only consider player fleets for selection
            if *faction != Faction::Player {
                continue;
            }
            let Ok(screen_pos) = camera.world_to_viewport(camera_transform, computed.position)
            else {
                continue;
            };

            let screen_dist = cursor_pos.distance(screen_pos);
            if screen_dist <= 20.0 {
                match &best_match {
                    None => best_match = Some((fleet_entity, screen_dist)),
                    Some((_, best_dist)) if screen_dist < *best_dist => {
                        best_match = Some((fleet_entity, screen_dist))
                    }
                    _ => {}
                }
            }
        }

        if let Some((fleet_entity, _)) = best_match {
            let fleet_name = fleet_query
                .get(fleet_entity)
                .map(|(_, f, _, _)| f.name.clone())
                .unwrap_or_default();
            info!("Selected fleet: {}", fleet_name);
            // Post SelectFleet event
            cmd_writer.write(StrategicCommand::SelectFleet(fleet_entity));
        }
    }
}

/// Posts CommitPlan command when Enter is pressed.
pub fn commit_plan(
    keyboard: Res<ButtonInput<KeyCode>>,
    selected: Query<Entity, With<Selected>>,
    mut cmd_writer: MessageWriter<StrategicCommand>,
) {
    if !keyboard.just_pressed(KeyCode::Enter) {
        return;
    }

    for fleet_entity in &selected {
        cmd_writer.write(StrategicCommand::CommitPlan(fleet_entity));
    }
}

/// Posts CancelLeg command when N is pressed.
pub fn cancel_last_leg(
    keyboard: Res<ButtonInput<KeyCode>>,
    selected: Query<Entity, With<Selected>>,
    mut cmd_writer: MessageWriter<StrategicCommand>,
) {
    if !keyboard.just_pressed(KeyCode::KeyN) {
        return;
    }

    for fleet_entity in &selected {
        cmd_writer.write(StrategicCommand::CancelLeg(fleet_entity));
    }
}

/// Posts SplitFleet command when S is pressed.
pub fn split_fleet(
    keyboard: Res<ButtonInput<KeyCode>>,
    selected: Query<Entity, With<Selected>>,
    mut cmd_writer: MessageWriter<StrategicCommand>,
) {
    if !keyboard.just_pressed(KeyCode::KeyS) {
        return;
    }

    if let Ok(fleet_entity) = selected.single() {
        cmd_writer.write(StrategicCommand::SplitFleet(fleet_entity));
    }
}

/// Posts MergeFleets command when M is pressed.
pub fn merge_fleets(
    keyboard: Res<ButtonInput<KeyCode>>,
    selected: Query<Entity, With<Selected>>,
    mut cmd_writer: MessageWriter<StrategicCommand>,
) {
    if !keyboard.just_pressed(KeyCode::KeyM) {
        return;
    }

    if let Ok(fleet_entity) = selected.single() {
        cmd_writer.write(StrategicCommand::MergeFleets(fleet_entity));
    }
}

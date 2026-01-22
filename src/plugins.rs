use bevy::prelude::*;

use crate::app_sets::AppSet;
use crate::app_state::AppState;
use crate::{
    camera, handle_body_click, picking, ship, simulation, tactical, transfer_vis, ui,
    update_body_positions, update_body_shape_scale,
};

pub struct AppCameraPlugin;

impl Plugin for AppCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            camera::update_camera_scale.in_set(AppSet::Simulation),
        )
        .add_systems(Update, camera::animate_camera.in_set(AppSet::Render));
    }
}

pub struct PhysicsSyncPlugin;

impl Plugin for PhysicsSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::physics::TacticalPhysicsPlugin);
    }
}

pub struct StrategicPlugin;

impl Plugin for StrategicPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(AppState::Strategic),
            (ship::reset_combat_state, ui::show_transfer_ui),
        );

        // Input systems (strategic only)
        app.add_systems(
            Update,
            (
                handle_body_click,
                ship::commit_plan,
                ship::cancel_last_leg,
                ship::split_fleet,
                ship::merge_fleets,
                ui::handle_fleet_number_keys,
                ui::handle_popup_spawn,
                ui::handle_close_button,
                ui::handle_escape_key,
                ui::handle_option_hover,
                ui::handle_option_selection,
            )
                .chain()
                .in_set(AppSet::Input)
                .run_if(in_state(AppState::Strategic)),
        );

        // Simulation systems (shared for now)
        app.add_systems(
            Update,
            (
                simulation::handle_time_controls,
                update_body_positions,
                ship::update_fleet_positions,
                ship::execute_departure,
                ship::check_arrival,
                ship::check_objectives,
                ship::expire_stale_uncommitted_legs,
                ship::sync_transfer_entities,
                ship::detect_combat,
            )
                .chain()
                .in_set(AppSet::Simulation)
                .after(camera::update_camera_scale),
        );

        // Rendering systems (shared)
        app.add_systems(
            Update,
            (update_body_shape_scale,).chain().in_set(AppSet::Render),
        );

        // Rendering systems (strategic only)
        app.add_systems(
            Update,
            (
                ship::sync_fleet_shapes,
                ship::sync_objective_rings,
                ship::sync_plan_markers,
                transfer_vis::update_hovered_arc,
            )
                .chain()
                .in_set(AppSet::Render)
                .run_if(in_state(AppState::Strategic)),
        );

        // UI systems (strategic only)
        app.add_systems(
            Update,
            (
                ui::update_transfer_panel,
                ui::update_fleet_tabs,
                ui::update_victory_overlay,
                ui::update_popup_options,
                ui::update_popup_position,
            )
                .chain()
                .in_set(AppSet::Ui)
                .run_if(in_state(AppState::Strategic)),
        );
    }
}

pub struct TacticalPlugin;

impl Plugin for TacticalPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(AppState::Tactical),
            (ship::despawn_strategic_markers, ui::hide_transfer_ui),
        );

        // Input systems (tactical only)
        app.add_systems(
            Update,
            (
                picking::update_box_selection,
                picking::handle_tactical_click,
                picking::handle_tactical_move_order,
            )
                .chain()
                .in_set(AppSet::Input)
                .run_if(in_state(AppState::Tactical)),
        );

        // Simulation systems (tactical only)
        app.add_systems(
            Update,
            tactical::enter_tactical_mode
                .in_set(AppSet::Simulation)
                .run_if(in_state(AppState::Tactical)),
        );
        app.add_systems(
            Update,
            tactical::update_arena_position
                .in_set(AppSet::Simulation)
                .after(crate::update_body_positions)
                .run_if(in_state(AppState::Tactical)),
        );
        app.add_systems(
            Update,
            tactical::update_ship_movement
                .in_set(AppSet::Simulation)
                .run_if(in_state(AppState::Tactical)),
        );

        // Rendering systems (tactical only)
        app.add_systems(
            Update,
            (
                tactical::render_move_markers,
                tactical::update_ship_mesh_scale,
                picking::sync_box_selection,
            )
                .chain()
                .in_set(AppSet::Render)
                .run_if(in_state(AppState::Tactical)),
        );
    }
}

pub struct TransferPlugin;

impl Plugin for TransferPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            transfer_vis::check_transfer_expiration
                .in_set(AppSet::Simulation)
                .after(ship::sync_transfer_entities),
        );
    }
}

pub struct AppUiPlugin;

impl Plugin for AppUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (ui::update_time_ui, ui::update_labels).in_set(AppSet::Ui),
        );
    }
}

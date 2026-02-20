use bevy::prelude::*;

use crate::app_sets::AppSet;
use crate::app_state::AppState;
use crate::common::{update_body_positions, update_body_shape_scale};
use crate::{camera, common, spatial, strategic, tactical};

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
        // Register strategic command message
        app.add_message::<strategic::StrategicCommand>();

        app.add_systems(
            OnEnter(AppState::Strategic),
            (
                strategic::systems::reset_combat_state,
                strategic::ui::show_transfer_ui,
            ),
        );

        // Input systems (strategic only)
        app.add_systems(
            Update,
            (
                strategic::input::handle_body_click,
                strategic::input::commit_plan,
                strategic::input::cancel_last_leg,
                strategic::input::split_fleet,
                strategic::input::merge_fleets,
                strategic::ui::handle_fleet_number_keys,
                strategic::ui::handle_popup_spawn,
                strategic::ui::handle_close_button,
                strategic::ui::handle_escape_key,
                strategic::ui::handle_option_hover,
                strategic::ui::handle_option_selection,
            )
                .chain()
                .in_set(AppSet::Input)
                .run_if(in_state(AppState::Strategic)),
        );

        // Command consumers (process strategic command events)
        app.add_systems(
            Update,
            (
                strategic::systems::process_select_fleet,
                strategic::systems::process_plan_transfer,
                strategic::systems::process_commit_plan,
                strategic::systems::process_cancel_leg,
                strategic::systems::process_split_fleet,
                strategic::systems::process_merge_fleets,
            )
                .in_set(AppSet::Simulation)
                .run_if(in_state(AppState::Strategic)),
        );

        // Time control runs in all modes (pause/speed work in tactical too)
        app.add_systems(
            Update,
            strategic::systems::process_time_control.in_set(AppSet::Simulation),
        );

        // Simulation systems (shared for now)
        app.add_systems(
            Update,
            (
                common::handle_time_controls,
                update_body_positions,
                strategic::rendering::update_fleet_positions,
                strategic::systems::execute_departure,
                strategic::systems::check_arrival,
                strategic::systems::check_objectives,
                strategic::systems::expire_stale_uncommitted_legs,
                strategic::rendering::sync_transfer_entities,
                strategic::systems::detect_combat,
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
                strategic::rendering::sync_fleet_shapes,
                strategic::rendering::sync_objective_rings,
                strategic::rendering::sync_plan_markers,
                strategic::transfer_vis::update_hovered_arc,
            )
                .chain()
                .in_set(AppSet::Render)
                .run_if(in_state(AppState::Strategic)),
        );

        // UI systems (strategic only)
        app.add_systems(
            Update,
            (
                strategic::ui::update_transfer_panel,
                strategic::ui::update_fleet_tabs,
                strategic::ui::update_victory_overlay,
                strategic::ui::update_popup_options,
                strategic::ui::update_popup_position,
            )
                .chain()
                .in_set(AppSet::Ui)
                .run_if(in_state(AppState::Strategic)),
        );

        // Debug validation (strategic only) - panics if tactical entities leaked
        app.add_systems(
            Update,
            tactical::validate_no_tactical_leaks.run_if(in_state(AppState::Strategic)),
        );
    }
}

pub struct TacticalPlugin;

impl Plugin for TacticalPlugin {
    fn build(&self, app: &mut App) {
        // Register tactical messages and resources
        app.add_message::<tactical::TacticalCommand>();
        app.init_resource::<tactical::ShowExitDialog>();
        app.init_resource::<tactical::TacticalCameraState>();
        app.init_resource::<tactical::BoxSelection>();
        app.init_resource::<tactical::RightClickDrag>();
        app.init_resource::<tactical::Flagships>();

        app.add_systems(
            OnEnter(AppState::Tactical),
            (
                strategic::rendering::despawn_strategic_markers,
                strategic::ui::hide_transfer_ui,
                tactical::setup_tactical_arena,
            ),
        );

        app.add_systems(
            OnExit(AppState::Tactical),
            (
                tactical::cleanup_empty_fleets,
                tactical::teardown_tactical_arena,
                strategic::ui::show_transfer_ui,
            ),
        );

        // Input systems (tactical only) - unified input handler
        app.add_systems(
            Update,
            (
                tactical::handle_tactical_escape,
                tactical::handle_tactical_input,
            )
                .chain()
                .in_set(AppSet::Input)
                .run_if(in_state(AppState::Tactical)),
        );

        // Command handlers (selection, movement) - process input commands
        app.add_systems(
            Update,
            (
                tactical::apply_selection_commands,
                tactical::apply_move_commands,
                tactical::apply_acceleration_commands,
                tactical::apply_escort_position_commands,
            )
                .in_set(AppSet::Simulation)
                .run_if(in_state(AppState::Tactical)),
        );

        // Simulation systems (tactical only)
        app.add_systems(
            Update,
            tactical::update_arena_position
                .in_set(AppSet::Simulation)
                .after(update_body_positions)
                .run_if(in_state(AppState::Tactical)),
        );
        app.add_systems(
            Update,
            tactical::apply_attack_target
                .in_set(AppSet::Simulation)
                .run_if(in_state(AppState::Tactical)),
        );
        // Ship movement systems - legacy MoveOrder and new relational positioning
        app.add_systems(
            Update,
            (
                tactical::update_ship_movement,      // Legacy: go-to-destination
                tactical::update_flagship_movement,  // New: flagship AccelerationOrder
                tactical::update_escort_movement,    // New: escort RelativePosition (PD controller)
            )
                .in_set(AppSet::Simulation)
                .run_if(in_state(AppState::Tactical)),
        );
        app.add_systems(
            Update,
            (
                tactical::update_missile_firing,
                tactical::update_missile_guidance,
            )
                .chain()
                .in_set(AppSet::Simulation)
                .after(tactical::update_ship_movement)
                .run_if(in_state(AppState::Tactical)),
        );
        app.add_systems(
            Update,
            (
                tactical::handle_missile_collisions,
                tactical::clear_dead_targets,
                tactical::process_despawn_at,
                tactical::check_ship_bounds,
                tactical::detect_combat_end,
            )
                .chain()
                .in_set(AppSet::Simulation)
                .after(tactical::update_missile_guidance)
                .run_if(in_state(AppState::Tactical)),
        );

        // Rendering systems (tactical only)
        app.add_systems(
            Update,
            (
                tactical::render_move_markers,
                tactical::sync_target_rings,
                tactical::render_targeting_lines,
                tactical::render_threat_axis,
                tactical::update_ship_mesh_scale,
                tactical::update_missile_mesh_scale,
                tactical::sync_box_selection,
            )
                .chain()
                .in_set(AppSet::Render)
                .run_if(in_state(AppState::Tactical)),
        );

        // UI systems (tactical only)
        app.add_systems(
            Update,
            (
                tactical::sync_exit_dialog,
                tactical::handle_exit_dialog_buttons,
            )
                .chain()
                .in_set(AppSet::Ui)
                .run_if(in_state(AppState::Tactical)),
        );

        // PostUpdate: capture world positions for next frame's delta calculation
        app.add_systems(
            PostUpdate,
            spatial::sync_tracked_world_positions.run_if(in_state(AppState::Tactical)),
        );
    }
}

pub struct TransferPlugin;

impl Plugin for TransferPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            strategic::transfer_vis::check_transfer_expiration
                .in_set(AppSet::Simulation)
                .after(strategic::rendering::sync_transfer_entities),
        );
    }
}

pub struct AppUiPlugin;

impl Plugin for AppUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (strategic::ui::update_time_ui, strategic::ui::update_labels).in_set(AppSet::Ui),
        );
    }
}

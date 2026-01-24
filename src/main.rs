use std::{cell::LazyCell, time::Duration};

use astrora_core::core::{Vector3, elements::coe_to_rv};
use avian3d::prelude::Position;
use bevy::{
    asset::Assets,
    camera::visibility::NoFrustumCulling,
    diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin},
    ecs::message::MessageWriter,
    gizmos::{
        GizmoAsset,
        config::{GizmoConfigStore, GizmoLineJoint},
    },
    math::{DVec3, Vec3},
    platform::collections::HashMap,
    prelude::*,
    transform::TransformPlugin,
    window::PrimaryWindow,
};
use bevy_pancam::{PanCam, PanCamPlugin};
use bevy_vector_shapes::{prelude as shape_prelude, prelude::*};
use big_space::prelude::*;

mod app_sets;
mod app_state;
mod camera;
mod common;
mod model;
mod physics;
mod picking;
mod plugins;
mod ship;
mod spatial;
mod tactical;
mod transfer_lut;
mod transfer_vis;
mod ui;

use common::{
    BodyShape, ComputedBody, OrbitGizmo, SimulationTime,
    create_orbit_gizmo_asset, spawn_body_circles, update_body_positions, update_body_shape_scale,
};
use model::{
    Body, CombatState, ComputedFleetPosition, Faction, Fleet, FleetLocation, FlightPlan,
    LogicalShip, PlanetaryElements, Selected, VictoryState,
};

use crate::app_sets::AppSet;
use crate::app_state::AppState;
use crate::camera::spawn_camera;
use crate::plugins::{
    AppCameraPlugin, AppUiPlugin, PhysicsSyncPlugin, StrategicPlugin, TacticalPlugin,
    TransferPlugin,
};

// ============================================================================
// Type Aliases (for clarity when storing Entity references)
// ============================================================================

/// Entity reference to a Body entity
type BodyEntity = Entity;

fn main() {
    let start_day = common::parse_start_day();
    if start_day != 0 {
        eprintln!("Starting simulation at day {}", start_day);
    }

    App::new()
        // Disable Bevy's TransformPlugin - big_space replaces it
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(BigSpaceDefaultPlugins)
        .add_plugins(PanCamPlugin)
        .add_plugins(ShapePlugin::default())
        .add_plugins(PhysicsSyncPlugin)
        .add_plugins((
            // Performance diagnostics - logs to console every X seconds
            FrameTimeDiagnosticsPlugin::default(),
            LogDiagnosticsPlugin {
                wait_duration: Duration::from_secs(10),
                ..default()
            },
        ))
        .insert_resource(SimulationTime::from_start_day(start_day))
        .init_resource::<ui::TransferPopup>()
        .init_resource::<ui::FleetKeyState>()
        .init_resource::<VictoryState>()
        .init_resource::<CombatState>()
        .init_resource::<picking::BoxSelection>()
        .init_state::<AppState>()
        .add_systems(
            Startup,
            (
                spawn_camera,
                ApplyDeferred, // Ensure BigSpaceRoot resource is available
                setup,
                ApplyDeferred,
                spawn_body_circles,
                init_parent_entities,
                transfer_lut::init_transfer_lut,
                configure_gizmos,
            )
                .chain(),
        )
        .configure_sets(
            Update,
            (
                AppSet::Input,
                AppSet::Simulation,
                AppSet::Render,
                AppSet::Ui,
            )
                .chain(),
        )
        .add_plugins((
            AppCameraPlugin,
            StrategicPlugin,
            TacticalPlugin,
            TransferPlugin,
            AppUiPlugin,
        ))
        .run();
}

fn setup(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    big_space_root: Res<camera::BigSpaceRoot>,
) {
    // Load planetary data
    let bodies = PlanetaryElements::get_planetary_elements();
    commands.insert_resource(PlanetaryElements {
        bodies: bodies.clone(),
    });

    // First pass: spawn all bodies as children of BigSpace with GridNode
    // Bodies use GridNode because they can have tactical arenas as children
    let mut body_entities: HashMap<String, BodyEntity> = HashMap::new();
    for body in bodies.values() {
        // Spawn body as child of BigSpace root with spatial components
        // Initial CellCoord is origin - update_body_positions will compute correct position
        let entity = commands
            .spawn((
                body.clone(),
                Visibility::Visible,
                ComputedBody::default(),
                spatial::GridNode,
                spatial::GridNode::default_grid(), // 100m cells for children (arena, etc)
                Transform::default(),              // Will be updated by update_body_positions
                ChildOf(big_space_root.0),         // Parent to BigSpace root
            ))
            .id();
        body_entities.insert(body.name.clone(), entity);
    }

    // Player fleet at Venus (for testing f32 precision - closer to sun = smaller coordinates)
    if let Some(&venus) = body_entities.get("Venus") {
        commands
            .spawn((
                Fleet {
                    delta_v_remaining: 500_000.0,
                    name: "Alpha".to_string(),
                },
                FleetLocation::AtBody(venus),
                Faction::Player,
                Selected,
                FlightPlan::default(),
            ))
            .with_children(|builder| {
                for _ in 0..10 {
                    builder.spawn(LogicalShip);
                }
            });
    }

    // Enemy garrison at Mercury (for testing f32 precision)
    if let Some(&mercury) = body_entities.get("Mercury") {
        commands
            .spawn((
                Fleet {
                    delta_v_remaining: 500_000.0,
                    name: "Mercury Garrison".to_string(),
                },
                FleetLocation::AtBody(mercury),
                Faction::Enemy,
                FlightPlan::default(),
            ))
            .with_children(|builder| {
                for _ in 0..10 {
                    builder.spawn(LogicalShip);
                }
            });
    }

    // Second pass: spawn labels and orbit gizmos (now we can look up parent entities)
    for body in bodies.values() {
        let body_entity = body_entities[&body.name];
        ui::spawn_body_label(&mut commands, &body.name, body_entity);
        // Body -> Body shape
        // let mut config = shape_prelude::ShapeConfig::default_3d();
        // config.color = body.color.clone();

        // let mut shape_bundle = shape_prelude::ShapeBundle::circle(&config, 1E10 as f32);
        // shape_bundle.visibility = Visibility::Visible;
        // commands
        //     .entity(dbg!(body_entity))
        //     .with_child((BodyShape, shape_bundle, NoFrustumCulling));

        // Create orbit gizmo if body has a parent
        // Gizmos stay outside BigSpace - their Transform is set from parent's GlobalTransform
        let Some(parent_name) = &body.parent_name else {
            continue;
        };
        let Some(parent_entity) = body_entities.get(parent_name) else {
            continue;
        };
        let Some(parent_body) = bodies.get(parent_name) else {
            continue;
        };
        // Parent Body -> Orbit gizmo
        commands.entity(*parent_entity).with_child((
            Gizmo {
                handle: gizmo_assets.add(create_orbit_gizmo_asset(body, parent_body)),
                depth_bias: 0.1,
                ..default()
            },
            OrbitGizmo,
            Transform::default(),
        ));
    }

    // Spawn time control panel
    ui::spawn_time_panel(&mut commands);

    // Spawn transfer info panel
    ui::spawn_transfer_panel(&mut commands);

    // Spawn fleet tabs at bottom center
    ui::spawn_fleet_tabs(&mut commands);
}

/// Links parent entities in Body components after all bodies are spawned.
/// This must run after ApplyDeferred to ensure all entities exist.
/// TODO: Can we just do this in the setup function? We just need entity ids, they don't need to actually be spawned.
fn init_parent_entities(mut bodies: Query<(Entity, &mut Body)>) {
    // Build entity map by name
    let entity_map: HashMap<String, Entity> = bodies
        .iter()
        .map(|(entity, body)| (body.name.clone(), entity))
        .collect();

    // Link parent entities
    for (_, mut body) in bodies.iter_mut() {
        if let Some(parent_name) = &body.parent_name {
            body.parent_entity = entity_map.get(parent_name).copied();
        }
    }
}

/// Configure gizmo line settings for orbit rendering.
fn configure_gizmos(mut config_store: ResMut<GizmoConfigStore>) {
    let (config, _) = config_store.config_mut::<DefaultGizmoConfigGroup>();
    config.line.joints = GizmoLineJoint::None;
}

// Re-export phys_vec_to_vec3 for backward compatibility
pub use common::phys_vec_to_vec3;

// ============================================================================
// Body Click Detection
// ============================================================================

/// Detects clicks on fleets or bodies (strategic mode only).
/// - Click: select fleet if one is at click location
/// - Shift+click: open transfer popup for selected fleet
pub(crate) fn handle_body_click(
    mouse_button: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    body_query: Query<(Entity, &Body, &ComputedBody, &GlobalTransform)>,
    fleet_query: Query<(Entity, &Fleet, &FleetLocation, &Faction)>,
    fleet_positions: Query<(Entity, &ComputedFleetPosition, &Faction)>,
    selected_query: Query<Entity, With<Selected>>,
    mut popup: ResMut<ui::TransferPopup>,
    combat: Res<CombatState>,
    mut cmd_writer: MessageWriter<ship::StrategicCommand>,
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
            cmd_writer.write(ship::StrategicCommand::SelectFleet(fleet_entity));
        }
    }
}

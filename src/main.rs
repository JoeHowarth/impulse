use std::time::Duration;

use bevy::{
    asset::Assets,
    diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin},
    gizmos::{
        GizmoAsset,
        config::{GizmoConfigStore, GizmoLineJoint},
    },
    platform::collections::HashMap,
    prelude::*,
    transform::TransformPlugin,
};
use bevy_pancam::PanCamPlugin;
use bevy_vector_shapes::ShapePlugin;
use big_space::prelude::*;

mod app_sets;
mod app_state;
mod camera;
mod common;
mod model;
mod physics;
mod picking;
mod plugins;
mod spatial;
mod strategic;
mod tactical;

use common::{
    BodyShape, ComputedBody, OrbitGizmo, SimulationTime, create_orbit_gizmo_asset,
    spawn_body_circles, update_body_positions, update_body_shape_scale,
};
use model::{
    Body, CombatState, Faction, Fleet, FleetLocation, FlightPlan, LogicalShip, PlanetaryElements,
    Selected, VictoryState,
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
        .init_resource::<strategic::TransferPopup>()
        .init_resource::<strategic::FleetKeyState>()
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
                strategic::transfer_lut::init_transfer_lut,
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
        strategic::ui::spawn_body_label(&mut commands, &body.name, body_entity);
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
    strategic::ui::spawn_time_panel(&mut commands);

    // Spawn transfer info panel
    strategic::ui::spawn_transfer_panel(&mut commands);

    // Spawn fleet tabs at bottom center
    strategic::ui::spawn_fleet_tabs(&mut commands);
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

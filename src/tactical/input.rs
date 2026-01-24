//! Tactical mode input handling.
//!
//! Single unified input system that interprets mouse/keyboard and emits TacticalCommands.
//! Separate systems consume commands to modify world state.

use bevy::ecs::message::MessageWriter;
use bevy::gizmos::GizmoAsset;
use bevy::math::{DVec3, Isometry3d};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::model::{CombatState, Faction, Selected};

use super::commands::TacticalCommand;
use super::{TacticalArena, VisualShip};

/// Marker for the box selection rectangle gizmo entity.
#[derive(Component)]
pub struct BoxSelectionGizmo;

// ============================================================================
// Constants
// ============================================================================

/// Click radius in screen pixels for picking
const CLICK_RADIUS: f32 = 20.0;

/// Minimum drag distance to trigger box selection (pixels)
const BOX_SELECT_THRESHOLD: f32 = 5.0;

/// Box selection rectangle color
const BOX_SELECT_COLOR: Color = Color::srgba(0.5, 0.8, 1.0, 0.5);

// ============================================================================
// Resources
// ============================================================================

/// Tracks box selection drag state.
#[derive(Resource, Default)]
pub struct BoxSelection {
    /// Screen position where drag started
    pub start: Option<Vec2>,
    /// Current drag position
    pub current: Option<Vec2>,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Finds the closest entity within click radius.
/// Returns (Entity, distance) for the closest match, or None if nothing is close enough.
fn find_closest_pickable(
    cursor_pos: Vec2,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    candidates: impl Iterator<Item = (Entity, Vec3)>,
    click_radius: f32,
) -> Option<(Entity, f32)> {
    let mut best_match: Option<(Entity, f32)> = None;

    for (entity, world_pos) in candidates {
        let Ok(screen_pos) = camera.world_to_viewport(camera_transform, world_pos) else {
            continue;
        };

        let screen_dist = cursor_pos.distance(screen_pos);
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

    best_match
}

/// Convert screen coordinates to arena-local coordinates.
fn screen_to_arena_local(
    screen_pos: Vec2,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    arena_transform: &GlobalTransform,
) -> Option<DVec3> {
    let ray = camera
        .viewport_to_world(camera_transform, screen_pos)
        .ok()?;
    let arena_pos = arena_transform.translation();
    Some(DVec3::new(
        (ray.origin.x - arena_pos.x) as f64,
        (ray.origin.y - arena_pos.y) as f64,
        0.0,
    ))
}

/// Find ships inside a screen-space rectangle.
fn find_ships_in_rect(
    min: Vec2,
    max: Vec2,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    ships: impl Iterator<Item = (Entity, Vec3)>,
) -> Vec<Entity> {
    let mut result = Vec::new();
    for (entity, world_pos) in ships {
        let Ok(screen_pos) = camera.world_to_viewport(camera_transform, world_pos) else {
            continue;
        };
        if screen_pos.x >= min.x
            && screen_pos.x <= max.x
            && screen_pos.y >= min.y
            && screen_pos.y <= max.y
        {
            result.push(entity);
        }
    }
    result
}

// ============================================================================
// Unified Input System
// ============================================================================

/// Single input handler for tactical mode.
/// Interprets mouse/keyboard state and emits TacticalCommands.
pub fn handle_tactical_input(
    mut cmd_writer: MessageWriter<TacticalCommand>,
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    arena_query: Query<&GlobalTransform, With<TacticalArena>>,
    combat: Res<CombatState>,
    mut box_sel: ResMut<BoxSelection>,
    visual_ships: Query<(Entity, &GlobalTransform, &VisualShip)>,
    selected_ships: Query<Entity, (With<VisualShip>, With<Selected>)>,
    ui_interaction: Query<&Interaction>,
) {
    // Only in tactical mode
    if !combat.active {
        box_sel.start = None;
        box_sel.current = None;
        return;
    }

    // Get window and cursor
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };

    // Check if UI is blocking input (only at action start, not during drag)
    let ui_blocking = ui_interaction
        .iter()
        .any(|i| *i != Interaction::None);

    // Modifier keys
    let shift = keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);
    let e_held = keyboard.pressed(KeyCode::KeyE);

    // Collect ship data for picking
    let player_ships: Vec<_> = visual_ships
        .iter()
        .filter(|(_, _, vs)| vs.faction == Faction::Player)
        .map(|(e, gt, _)| (e, gt.translation()))
        .collect();

    let enemy_ships: Vec<_> = visual_ships
        .iter()
        .filter(|(_, _, vs)| vs.faction == Faction::Enemy)
        .map(|(e, gt, _)| (e, gt.translation()))
        .collect();

    // ========== LEFT CLICK PRESS: Start drag ==========
    if mouse.just_pressed(MouseButton::Left) {
        if !ui_blocking {
            box_sel.start = Some(cursor_pos);
            box_sel.current = Some(cursor_pos);
        }
    }

    // ========== LEFT CLICK HELD: Update drag position ==========
    if mouse.pressed(MouseButton::Left) && box_sel.start.is_some() {
        box_sel.current = Some(cursor_pos);
    }

    // ========== LEFT CLICK RELEASE: Determine action ==========
    if mouse.just_released(MouseButton::Left) {
        if let Some(start) = box_sel.start {
            let drag_dist = start.distance(cursor_pos);
            let is_box_select = drag_dist > BOX_SELECT_THRESHOLD;

            if e_held {
                // E+click: Attack targeting
                let selected: Vec<Entity> = selected_ships.iter().collect();
                if !selected.is_empty() {
                    if let Some((target, _)) = find_closest_pickable(
                        cursor_pos,
                        camera,
                        camera_transform,
                        enemy_ships.into_iter(),
                        CLICK_RADIUS,
                    ) {
                        cmd_writer.write(TacticalCommand::AttackTarget {
                            ships: selected,
                            target,
                        });
                    } else {
                        // E+click on empty = clear target
                        cmd_writer.write(TacticalCommand::ClearAttackTarget { ships: selected });
                    }
                }
            } else if is_box_select {
                // Box selection
                let min = Vec2::new(start.x.min(cursor_pos.x), start.y.min(cursor_pos.y));
                let max = Vec2::new(start.x.max(cursor_pos.x), start.y.max(cursor_pos.y));

                let ships_in_box = find_ships_in_rect(
                    min,
                    max,
                    camera,
                    camera_transform,
                    player_ships.into_iter(),
                );

                if shift {
                    // Shift+box: Add to selection
                    if !ships_in_box.is_empty() {
                        cmd_writer.write(TacticalCommand::AddToSelection(ships_in_box));
                    }
                } else {
                    // Box: Replace selection
                    cmd_writer.write(TacticalCommand::SelectShips(ships_in_box));
                }
            } else {
                // Single click
                if let Some((entity, _)) = find_closest_pickable(
                    cursor_pos,
                    camera,
                    camera_transform,
                    player_ships.into_iter(),
                    CLICK_RADIUS,
                ) {
                    if shift {
                        // Shift+click: Toggle (add single ship)
                        cmd_writer.write(TacticalCommand::AddToSelection(vec![entity]));
                    } else {
                        // Click: Select single
                        cmd_writer.write(TacticalCommand::SelectShips(vec![entity]));
                    }
                } else if !shift {
                    // Click on empty: Clear selection
                    cmd_writer.write(TacticalCommand::ClearSelection);
                }
            }
        }

        // Reset drag state
        box_sel.start = None;
        box_sel.current = None;
    }

    // ========== RIGHT CLICK: Move order ==========
    if mouse.just_pressed(MouseButton::Right) && !ui_blocking {
        let selected: Vec<Entity> = selected_ships.iter().collect();
        if !selected.is_empty() {
            if let Ok(arena_transform) = arena_query.single() {
                if let Some(destination) =
                    screen_to_arena_local(cursor_pos, camera, camera_transform, arena_transform)
                {
                    cmd_writer.write(TacticalCommand::MoveShips {
                        ships: selected,
                        destination,
                    });
                }
            }
        }
    }
}

// ============================================================================
// Command Handlers
// ============================================================================

/// Handles selection commands.
pub fn apply_selection_commands(
    mut commands: Commands,
    mut cmd_reader: bevy::ecs::message::MessageReader<TacticalCommand>,
    selected_query: Query<Entity, With<Selected>>,
) {
    for cmd in cmd_reader.read() {
        match cmd {
            TacticalCommand::SelectShips(ships) => {
                // Clear existing selection
                for entity in selected_query.iter() {
                    commands.entity(entity).remove::<Selected>();
                }
                // Select new ships
                for &entity in ships {
                    commands.entity(entity).insert(Selected);
                }
            }
            TacticalCommand::AddToSelection(ships) => {
                for &entity in ships {
                    commands.entity(entity).insert(Selected);
                }
            }
            TacticalCommand::ClearSelection => {
                for entity in selected_query.iter() {
                    commands.entity(entity).remove::<Selected>();
                }
            }
            _ => {} // Other commands handled elsewhere
        }
    }
}

/// Handles move commands.
pub fn apply_move_commands(
    mut commands: Commands,
    mut cmd_reader: bevy::ecs::message::MessageReader<TacticalCommand>,
) {
    for cmd in cmd_reader.read() {
        if let TacticalCommand::MoveShips { ships, destination } = cmd {
            for &entity in ships {
                commands.entity(entity).insert(super::MoveOrder {
                    destination: *destination,
                });
            }
        }
    }
}

// ============================================================================
// Rendering (Box Selection Gizmo)
// ============================================================================

/// Syncs the box selection rectangle gizmo with drag state.
pub fn sync_box_selection(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    box_sel: Res<BoxSelection>,
    combat: Res<CombatState>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    arena_query: Query<(Entity, &GlobalTransform), With<TacticalArena>>,
    existing_gizmo: Query<Entity, With<BoxSelectionGizmo>>,
    mut gizmo_transforms: Query<&mut Transform, With<BoxSelectionGizmo>>,
) {
    // Determine if we should show the box selection
    let should_show = combat.active
        && box_sel.start.is_some()
        && box_sel.current.is_some()
        && box_sel.start.unwrap().distance(box_sel.current.unwrap()) > BOX_SELECT_THRESHOLD;

    if !should_show {
        // Despawn any existing gizmo
        for entity in &existing_gizmo {
            commands.entity(entity).despawn();
        }
        return;
    }

    let start = box_sel.start.unwrap();
    let current = box_sel.current.unwrap();

    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };
    let Ok((arena_entity, arena_transform)) = arena_query.single() else {
        return;
    };

    // Convert screen coords to arena-local coords
    let Some(start_local) = screen_to_arena_local(start, camera, camera_transform, arena_transform)
    else {
        return;
    };
    let Some(end_local) = screen_to_arena_local(current, camera, camera_transform, arena_transform)
    else {
        return;
    };

    // Calculate center and size for the rectangle
    let center = Vec3::new(
        ((start_local.x + end_local.x) * 0.5) as f32,
        ((start_local.y + end_local.y) * 0.5) as f32,
        0.1, // Z offset for visibility
    );
    let size = Vec2::new(
        (end_local.x - start_local.x).abs() as f32,
        (end_local.y - start_local.y).abs() as f32,
    );

    if let Ok(mut transform) = gizmo_transforms.single_mut() {
        // Update existing gizmo's transform
        transform.translation = center;
        transform.scale = Vec3::new(size.x, size.y, 1.0);
    } else {
        // Spawn new gizmo with unit rect (1x1), scaled by Transform
        let mut gizmo = GizmoAsset::new();
        gizmo.rect(Isometry3d::IDENTITY, Vec2::ONE, BOX_SELECT_COLOR);

        commands.spawn((
            Gizmo {
                handle: gizmo_assets.add(gizmo),
                depth_bias: -0.1, // Draw on top
                ..default()
            },
            BoxSelectionGizmo,
            Transform::from_translation(center).with_scale(Vec3::new(size.x, size.y, 1.0)),
            ChildOf(arena_entity),
        ));
    }
}

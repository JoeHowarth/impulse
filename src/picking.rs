//! Unified picking/selection system for both strategic and tactical modes.
//!
//! Strategic mode: Single fleet selection via click
//! Tactical mode: Multi-ship selection via click, Shift+click, and box select

use bevy::gizmos::GizmoAsset;
use bevy::math::{DVec3, Isometry3d};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::model::{CombatState, Faction, Selected};

use crate::tactical::{MoveOrder, TacticalArena, VisualShip};

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

// ============================================================================
// Resources
// ============================================================================

/// Tracks box selection drag state (tactical mode only).
#[derive(Resource, Default)]
pub struct BoxSelection {
    /// Screen position where drag started
    pub start: Option<Vec2>,
    /// Current drag position
    pub current: Option<Vec2>,
    /// Set to true when a box select just occurred (so click handler skips)
    pub did_box_select: bool,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Finds the closest entity within click radius.
/// Returns (Entity, distance) for the closest match, or None if nothing is close enough.
pub fn find_closest_pickable(
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

// ============================================================================
// Tactical Mode Systems
// ============================================================================

/// Handles click and shift+click selection in tactical mode.
pub fn handle_tactical_click(
    mut commands: Commands,
    mouse_button: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    combat: Res<CombatState>,
    visual_ships: Query<(Entity, &GlobalTransform, &VisualShip)>,
    selected: Query<Entity, With<Selected>>,
    box_sel: Res<BoxSelection>,
) {
    // Only run in tactical mode
    if !combat.active {
        return;
    }

    // Don't process click if it was a box selection drag
    if box_sel.did_box_select {
        info!("Tactical click: skipping, was box select");
        return;
    }

    if !mouse_button.just_released(MouseButton::Left) {
        return;
    }

    let Ok(window) = windows.single() else {
        info!("Tactical click: no window");
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        info!("Tactical click: no cursor position");
        return;
    };
    let Ok((camera, camera_transform)) = camera_query.single() else {
        info!("Tactical click: no camera");
        return;
    };

    let shift = keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);

    // Count player ships for debug
    let player_ships: Vec<_> = visual_ships
        .iter()
        .filter(|(_, _, vs)| vs.faction == Faction::Player)
        .collect();

    info!(
        "Tactical click at {:?}, {} player ships in scene",
        cursor_pos,
        player_ships.len()
    );

    // Debug: show screen positions of ships
    for (entity, gt, _) in &player_ships {
        let world_pos = gt.translation();
        if let Ok(screen_pos) = camera.world_to_viewport(camera_transform, world_pos) {
            let dist = cursor_pos.distance(screen_pos);
            info!(
                "  Ship {:?}: world={:?}, screen={:?}, dist={:.1}px",
                entity, world_pos, screen_pos, dist
            );
        } else {
            info!("  Ship {:?}: world={:?}, NOT ON SCREEN", entity, world_pos);
        }
    }

    // Find clicked ship (player faction only)
    let candidates = player_ships.iter().map(|(e, gt, _)| (*e, gt.translation()));

    if let Some((entity, dist)) = find_closest_pickable(
        cursor_pos,
        camera,
        camera_transform,
        candidates,
        CLICK_RADIUS,
    ) {
        info!("Selected ship {:?} at distance {:.1}px", entity, dist);
        if shift {
            // Toggle selection
            if selected.get(entity).is_ok() {
                commands.entity(entity).remove::<Selected>();
            } else {
                commands.entity(entity).insert(Selected);
            }
        } else {
            // Single select (clear others)
            for old in selected.iter() {
                commands.entity(old).remove::<Selected>();
            }
            commands.entity(entity).insert(Selected);
        }
    } else {
        info!("No ship within {}px of click", CLICK_RADIUS);
        if !shift {
            // Clicked empty space - deselect all
            for old in selected.iter() {
                commands.entity(old).remove::<Selected>();
            }
        }
    }
}

/// Tracks box selection drag and selects ships on release.
/// Returns true if a box selection occurred (so click handler can skip).
pub fn update_box_selection(
    mut commands: Commands,
    mouse_button: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    combat: Res<CombatState>,
    mut box_sel: ResMut<BoxSelection>,
    visual_ships: Query<(Entity, &GlobalTransform, &VisualShip)>,
    selected: Query<Entity, With<Selected>>,
) {
    // Only run in tactical mode
    if !combat.active {
        box_sel.start = None;
        box_sel.current = None;
        box_sel.did_box_select = false;
        return;
    }

    // Reset the flag at start of each frame
    box_sel.did_box_select = false;

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };

    // Start drag
    if mouse_button.just_pressed(MouseButton::Left) {
        box_sel.start = Some(cursor_pos);
        box_sel.current = Some(cursor_pos);
        info!("Box select: drag started at {:?}", cursor_pos);
    }

    // Update drag position
    if mouse_button.pressed(MouseButton::Left) {
        box_sel.current = Some(cursor_pos);
    }

    // End drag - check if it was a box select
    if mouse_button.just_released(MouseButton::Left) {
        if let (Some(start), Some(end)) = (box_sel.start, box_sel.current) {
            let drag_dist = start.distance(end);
            info!(
                "Box select: drag ended, distance={:.1}px (threshold={})",
                drag_dist, BOX_SELECT_THRESHOLD
            );

            // Only trigger box select if dragged far enough
            if drag_dist > BOX_SELECT_THRESHOLD {
                box_sel.did_box_select = true;

                let Ok((camera, camera_transform)) = camera_query.single() else {
                    box_sel.start = None;
                    box_sel.current = None;
                    return;
                };

                let shift =
                    keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);

                // Clear selection if not shift-dragging
                if !shift {
                    for old in selected.iter() {
                        commands.entity(old).remove::<Selected>();
                    }
                }

                // Calculate selection rectangle
                let min_x = start.x.min(end.x);
                let max_x = start.x.max(end.x);
                let min_y = start.y.min(end.y);
                let max_y = start.y.max(end.y);

                info!(
                    "Box select: rect x=[{:.0}, {:.0}] y=[{:.0}, {:.0}]",
                    min_x, max_x, min_y, max_y
                );

                // Select all player ships inside the rectangle
                let mut selected_count = 0;
                for (entity, global_transform, visual_ship) in &visual_ships {
                    if visual_ship.faction != Faction::Player {
                        continue;
                    }

                    let Ok(screen_pos) =
                        camera.world_to_viewport(camera_transform, global_transform.translation())
                    else {
                        continue;
                    };

                    let inside = screen_pos.x >= min_x
                        && screen_pos.x <= max_x
                        && screen_pos.y >= min_y
                        && screen_pos.y <= max_y;

                    info!("  Ship at screen {:?}: inside={}", screen_pos, inside);

                    if inside {
                        commands.entity(entity).insert(Selected);
                        selected_count += 1;
                    }
                }

                info!("Box select: selected {} ships", selected_count);
            }
        }

        box_sel.start = None;
        box_sel.current = None;
    }
}

/// Box selection rectangle color
const BOX_SELECT_COLOR: Color = Color::srgba(0.5, 0.8, 1.0, 0.5);

/// Syncs the box selection rectangle gizmo with drag state.
/// Spawns gizmo when dragging starts, updates position/scale during drag, despawns when done.
pub fn sync_box_selection(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    box_sel: Res<BoxSelection>,
    combat: Res<CombatState>,
    windows: Query<&Window, With<PrimaryWindow>>,
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

    let Ok(window) = windows.single() else {
        return;
    };
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

/// Handles right-click to set movement orders for selected ships.
pub fn handle_tactical_move_order(
    mut commands: Commands,
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    combat: Res<CombatState>,
    arena_query: Query<&GlobalTransform, With<TacticalArena>>,
    selected_ships: Query<Entity, (With<VisualShip>, With<Selected>)>,
) {
    // Only in tactical mode
    if !combat.active {
        return;
    }

    // Right-click just pressed
    if !mouse_button.just_pressed(MouseButton::Right) {
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };
    let Ok(arena_transform) = arena_query.single() else {
        return;
    };

    // Convert screen → world → arena-local
    let Some(arena_local) =
        screen_to_arena_local(cursor_pos, camera, camera_transform, arena_transform)
    else {
        return;
    };

    // Count selected ships
    let count = selected_ships.iter().count();
    if count == 0 {
        return;
    }

    info!(
        "Move order: {} ship(s) to arena-local ({:.0} km, {:.0} km)",
        count,
        arena_local.x / 1000.0,
        arena_local.y / 1000.0
    );

    // Issue move order to all selected ships
    for entity in &selected_ships {
        commands.entity(entity).insert(MoveOrder {
            destination: arena_local,
        });
    }
}

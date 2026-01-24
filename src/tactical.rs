//! Tactical combat mode - individual ship battles at a body.
//!
//! When combat triggers, we spawn a TacticalArena at the body's position
//! and VisualShip entities for each LogicalShip in the involved fleets.

use avian3d::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::ecs::message::Message;
use bevy::math::primitives::Triangle3d;
use bevy::math::{DVec2, DVec3};
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy_vector_shapes::prelude::*;
use big_space::prelude::{BigSpace, CellCoord, Grid};
use std::collections::{HashMap, HashSet};

use crate::ComputedBody;
use crate::camera::{CameraScale, CameraTarget};
use crate::model::{CombatState, Faction, Fleet, LogicalShip, Selected, ship_count};
use crate::simulation::SimulationTime;
use crate::spatial::{BigSpaceHierarchy, GridLeaf, GridNode, TrackedWorldPosition};

// ============================================================================
// Constants
// ============================================================================

// Tactical scale uses real-world meters (no precision scaling hack).

/// Arena size in meters (400,000 km)
pub const ARENA_SIZE: f64 = 400_000_000.0;

/// Half arena size - retreat boundary (200,000 km)
pub const ARENA_HALF: f64 = 200_000_000.0;

/// Tactical time scale: 60 sim seconds per real second (1 min/s)
pub const TACTICAL_TIME_SCALE: f64 = 60.0;

/// Offset from arena center for fleet spawns (3 km in meters)
const FLEET_SPAWN_OFFSET: f64 = 3_000.0;

/// Horizontal spacing between ships (1 km)
const SHIP_SPACING: f64 = 1_000.0;

/// Physical ship size in meters (100 m)
const SHIP_PHYSICAL_SIZE: f32 = 100.0;

/// Target screen-size range for ships in pixels (LOD).
const SHIP_TARGET_PIXELS_SCALE: f32 = 5.0;
const SHIP_TARGET_PIXELS_MAX: f32 = 8.0;

/// Screen-size fade range for ships in pixels.
/// Fade starts at this size and reaches 0 below the end size.
const SHIP_FADE_START_PX: f32 = 1.0;
const SHIP_FADE_END_PX: f32 = 0.2;

/// Camera scale for tactical view
/// scale = world units per pixel. 10 km / 1000 pixels ≈ 10 m/px
const TACTICAL_CAMERA_SCALE: f32 = 10.0;

/// Arena center offset from body (150,000 km to put body on right side)
const ARENA_CENTER_OFFSET: f64 = 150_000_000.0;

/// Arrival distance threshold in meters (1 km)
const ARRIVAL_DISTANCE: f64 = 1_000.0;

/// Arrival velocity threshold in m/s (10 m/s)
const ARRIVAL_VELOCITY: f64 = 10.0;

// ============================================================================
// Components
// ============================================================================

/// The tactical combat arena - exists during combat only.
/// Children include VisualShips and missiles.
///
/// Uses GridNode to position in the body's grid, inheriting body motion automatically.
#[derive(Component)]
pub struct TacticalArena {
    /// The body where this battle is occurring
    pub body: Entity,
}

/// A visual representation of a LogicalShip during tactical combat.
/// Spawned as a child of TacticalArena. Transform is arena-relative in visual units.
#[derive(Component)]
pub struct VisualShip {
    /// Reference to the persistent LogicalShip entity
    pub logical: Entity,
    /// Reference to parent Fleet entity
    pub fleet: Entity,
    /// Faction for rendering
    pub faction: Faction,
}

/// Movement order for a VisualShip - stores arena-local destination.
#[derive(Component)]
pub struct MoveOrder {
    /// Arena-local destination (in meters, relative to TacticalArena center)
    pub destination: DVec3,
}

/// Marker for the ship mesh (child of VisualShip).
#[derive(Component)]
pub struct ShipMesh;

/// Marker for the selection ring gizmo (child of VisualShip, only when selected).
#[derive(Component)]
pub struct SelectionRingGizmo;

/// Marker for the move target gizmo (child of TacticalArena).
#[derive(Component)]
pub struct MoveMarker {
    pub ship: Entity,
}

/// Stats for ship movement and combat capabilities.
#[derive(Component)]
pub struct ShipStats {
    /// Maximum thrust acceleration in m/s²
    pub max_acceleration: f64,
    /// Maximum linear speed in m/s
    pub max_speed: f64,
}

impl Default for ShipStats {
    fn default() -> Self {
        Self {
            max_acceleration: 10.0, // ~1g
            max_speed: 50_000.0,    // 50 km/s
        }
    }
}

// ============================================================================
// Events
// ============================================================================

/// Commands posted by input systems, consumed by simulation systems.
/// Decouples input handling from game state mutation.
#[derive(Message, Debug, Clone)]
pub enum TacticalCommand {
    /// Move ships to destination (arena-local coordinates in meters)
    MoveShips {
        ships: Vec<Entity>,
        destination: DVec3,
    },
    /// Select ships (replace current selection)
    SelectShips(Vec<Entity>),
    /// Add ships to current selection
    AddToSelection(Vec<Entity>),
    /// Clear all selection
    ClearSelection,
    /// Request exit from tactical mode
    RequestExit { reason: ExitReason },
}

/// Reason for exiting tactical mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// All enemy ships destroyed or fled
    Victory,
    /// All player ships destroyed or fled
    Defeat,
    /// Player pressed escape (requires confirmation)
    Manual,
}

/// Resource controlling the exit confirmation dialog
#[derive(Resource, Default)]
pub struct ShowExitDialog(pub bool);

/// Saved camera state for restoration after tactical mode
#[derive(Resource, Default)]
pub struct TacticalCameraState {
    pub previous_position: DVec2,
    pub previous_scale: f32,
}

// ============================================================================
// Systems
// ============================================================================

/// Sets up tactical mode: spawns TacticalArena and VisualShips, animates camera, adjusts time scale.
/// Runs once on OnEnter(AppState::Tactical).
pub fn setup_tactical_arena(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut combat: ResMut<CombatState>,
    mut sim_time: ResMut<SimulationTime>,
    mut camera_query: Query<(&Transform, &CellCoord, &mut CameraTarget), With<Camera3d>>,
    mut camera_state: ResMut<TacticalCameraState>,
    cam_scale: Res<CameraScale>,
    bodies: Query<(&ComputedBody, &Grid)>,
    children_query: Query<&Children>,
    ships: Query<&LogicalShip>,
    root_grid: Query<&Grid, With<BigSpace>>,
) {
    let Some(body_entity) = combat.body else {
        warn!("Entering tactical but no body specified in CombatState");
        return;
    };

    let Ok((body_computed, body_grid)) = bodies.get(body_entity) else {
        warn!("Cannot get computed body or grid for combat");
        return;
    };

    let Ok(root_grid) = root_grid.single() else {
        warn!("No root grid found");
        return;
    };

    // Get current camera state and save it for restoration on exit
    let Ok((cam_transform, cam_cell, mut camera_target)) = camera_query.single_mut() else {
        warn!("No camera found");
        return;
    };
    // Save current camera state for restoration when exiting tactical
    let cam_world_pos = root_grid.grid_position_double(cam_cell, cam_transform);
    camera_state.previous_position = cam_world_pos.xy();
    camera_state.previous_scale = cam_scale.0;

    // Arena offset from body center (in meters, positioned in body's grid)
    let arena_offset = DVec3::new(-ARENA_CENTER_OFFSET, 0.0, 0.0);
    let arena_helio = body_computed.helio_pos + arena_offset;

    info!(
        "Entering tactical mode at body (offset: {:?}m), {} player fleets, {} enemy fleets",
        arena_offset,
        combat.player_fleets.len(),
        combat.enemy_fleets.len()
    );

    // Spawn TacticalArena with GridNode - positioned in body's grid
    // Arena inherits body's motion automatically via nested grid hierarchy
    let (arena_cell, arena_local) = body_grid.translation_to_grid(arena_offset);
    let arena_grid = Grid::new(
        crate::spatial::GRID_CELL_SIZE,
        crate::spatial::GRID_SWITCH_THRESHOLD,
    );
    let arena = commands
        .spawn((
            TacticalArena { body: body_entity },
            GridNode,
            arena_grid.clone(), // Explicit grid so we can use it for ship spawning
            arena_cell,
            Transform::from_translation(arena_local),
            ChildOf(body_entity),
            Visibility::default(),
            // Track world position for camera delta calculation
            TrackedWorldPosition {
                last_frame: arena_helio,
            },
        ))
        .id();

    // Count ships per side for horizontal positioning
    let player_ship_count: u32 = combat
        .player_fleets
        .iter()
        .map(|&e| ship_count(e, &children_query, &ships))
        .sum();
    let enemy_ship_count: u32 = combat
        .enemy_fleets
        .iter()
        .map(|&e| ship_count(e, &children_query, &ships))
        .sum();

    let ship_mesh = meshes.add(create_ship_triangle_mesh());
    let player_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.4, 1.0, 0.4),
        unlit: true,
        cull_mode: None,
        ..default()
    });
    let enemy_material = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.3, 0.3),
        unlit: true,
        cull_mode: None,
        ..default()
    });

    // Spawn player ships (bottom center, y = -3 km)
    let player_spawned = spawn_fleet_ships(
        &mut commands,
        arena,
        &arena_grid,
        &combat.player_fleets,
        Faction::Player,
        -FLEET_SPAWN_OFFSET,
        player_ship_count as usize,
        &children_query,
        &ships,
        &ship_mesh,
        &player_material,
    );

    // Spawn enemy ships (top center, y = +3 km)
    let enemy_spawned = spawn_fleet_ships(
        &mut commands,
        arena,
        &arena_grid,
        &combat.enemy_fleets,
        Faction::Enemy,
        FLEET_SPAWN_OFFSET,
        enemy_ship_count as usize,
        &children_query,
        &ships,
        &ship_mesh,
        &enemy_material,
    );

    info!(
        "Spawned {} player ships and {} enemy ships in arena",
        player_spawned, enemy_spawned
    );

    // Store arena entity in combat state
    combat.arena = Some(arena);

    // Animate camera to tactical view
    camera_target.move_to(arena_helio.xy(), TACTICAL_CAMERA_SCALE);

    // Set tactical time scale
    sim_time.time_scale = TACTICAL_TIME_SCALE;
}

/// Spawns VisualShips for all LogicalShips in the given fleets.
/// Ships use GridLeaf for high-precision positioning in the arena's grid.
/// Returns the number of ships spawned.
fn spawn_fleet_ships(
    commands: &mut Commands,
    arena: Entity,
    arena_grid: &Grid,
    fleets: &[Entity],
    faction: Faction,
    y_offset_meters: f64,
    total_ships: usize,
    children_query: &Query<&Children>,
    ships: &Query<&LogicalShip>,
    ship_mesh: &Handle<Mesh>,
    ship_material: &Handle<StandardMaterial>,
) -> usize {
    let mut index = 0;

    for &fleet_entity in fleets {
        let Ok(children) = children_query.get(fleet_entity) else {
            continue;
        };

        for child in children.iter() {
            if ships.contains(child) {
                let x_offset = compute_ship_x_offset(index, total_ships);
                let logical_ship = child;

                // Position ship in arena's grid (arena-local coordinates in meters)
                let ship_position = DVec3::new(x_offset, y_offset_meters, 0.1);
                let (leaf, cell, transform) = GridLeaf::at_position(ship_position, arena_grid);

                // Spawn VisualShip with GridLeaf for high-precision positioning
                let visual_ship = commands
                    .spawn((
                        VisualShip {
                            logical: logical_ship,
                            fleet: fleet_entity,
                            faction,
                        },
                        leaf,
                        cell,
                        transform,
                        Visibility::default(),
                        // Physics components
                        RigidBody::Dynamic,
                        Collider::sphere(50.0), // 50m radius collider
                        LinearVelocity::default(),
                        SweptCcd::default(), // Prevent tunneling at high speeds
                        // Movement stats
                        ShipStats::default(),
                        ChildOf(arena),
                    ))
                    .id();

                // Spawn triangle mesh as child of VisualShip (low-precision, just local offset)
                commands.spawn((
                    Mesh3d(ship_mesh.clone()),
                    MeshMaterial3d(ship_material.clone()),
                    ShipMesh,
                    Transform::from_scale(Vec3::splat(SHIP_PHYSICAL_SIZE)),
                    ChildOf(visual_ship),
                ));

                index += 1;
            }
        }
    }

    index
}

/// Computes horizontal X offset in meters for a ship given its index and total count.
/// Ships are centered horizontally with SHIP_SPACING between them.
fn compute_ship_x_offset(index: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let half_width = (total as f64 - 1.0) / 2.0 * SHIP_SPACING;
    -half_width + index as f64 * SHIP_SPACING
}

/// Updates the camera to track the tactical arena.
///
/// The camera needs to move with the body's orbital motion while also
/// animating toward the arena center. We handle these separately:
/// 1. Apply body motion delta directly to camera position
/// 2. Let animate_camera handle the lerp toward arena center
pub fn update_arena_position(
    hierarchy: BigSpaceHierarchy,
    arena_query: Query<(Entity, &TrackedWorldPosition)>,
    root_grid_query: Query<&Grid, With<BigSpace>>,
    mut camera_query: Query<
        (
            &mut Transform,
            &mut CellCoord,
            &mut crate::camera::CameraTarget,
        ),
        (With<Camera3d>, Without<TacticalArena>),
    >,
) {
    let Ok(root_grid) = root_grid_query.single() else {
        return;
    };

    let Ok((mut cam_transform, mut cam_cell, mut camera_target)) = camera_query.single_mut() else {
        return;
    };

    for (arena_entity, tracked) in &arena_query {
        // Get arena's current world position via hierarchy walk
        let Some(arena_world) = hierarchy.world_position(arena_entity) else {
            continue;
        };

        // Compute how much the arena moved since last frame (body orbital motion)
        let delta = arena_world - tracked.last_frame;

        // Apply delta to camera position - this keeps the camera moving with the body
        let cam_world = root_grid.grid_position_double(&cam_cell, &cam_transform);
        let new_cam_world = cam_world + delta;
        let (new_cell, new_local) = root_grid.translation_to_grid(new_cam_world);
        *cam_cell = new_cell;
        cam_transform.translation = new_local;

        // Update animation target to arena's current position
        // animate_camera will lerp from camera's new position toward this target
        if camera_target.position.is_some() {
            camera_target.position = Some(arena_world.xy());
        }
    }
}

/// Computes ship display size using LOD system similar to bodies.
/// Returns (display_size, visibility) where visibility is 0.0-1.0.
fn compute_ship_display(cam_scale: f32) -> (f32, f32) {
    // Use same log-based scaling as bodies:
    // log_scaled_size = ((log_radius - 4.0).max(1.0) * 1.5).min(8.0) * cam_scale
    // For 100m ship: log10(100) = 2, so (2 - 4).max(1) = 1, * 1.5 = 1.5 pixels target
    // But that's tiny, so we use a slightly different formula for ships

    let log_radius = SHIP_PHYSICAL_SIZE.log10();
    // Ships get more screen presence than bodies of same size
    // Target ~30-80 pixels when zoomed out
    let screen_pixels =
        ((log_radius - 1.0).max(1.0) * SHIP_TARGET_PIXELS_SCALE).min(SHIP_TARGET_PIXELS_MAX);
    let log_scaled_size = screen_pixels * cam_scale;

    // Take max of log-scaled size vs physical size (like bodies do)
    let display_size = log_scaled_size.max(SHIP_PHYSICAL_SIZE);

    // Compute what the screen size actually is
    let actual_screen_size = display_size / cam_scale;

    // Fade out only when ships are very small on screen.
    let visibility = if actual_screen_size < SHIP_FADE_START_PX {
        ((actual_screen_size - SHIP_FADE_END_PX) / (SHIP_FADE_START_PX - SHIP_FADE_END_PX))
            .clamp(0.0, 1.0)
    } else {
        1.0
    };

    (display_size, visibility)
}

fn create_ship_triangle_mesh() -> Mesh {
    Mesh::from(Triangle3d::new(
        Vec3::new(0.0, 0.5, 0.0),
        Vec3::new(-0.5, -0.5, 0.0),
        Vec3::new(0.5, -0.5, 0.0),
    ))
}

fn create_move_marker_mesh() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    let half_len = 0.5;
    let half_thickness = 0.08;

    let quad_for_line = |a: Vec2, b: Vec2, half_t: f32| -> [Vec3; 4] {
        let dir = (b - a).normalize_or_zero();
        let perp = Vec2::new(-dir.y, dir.x) * half_t;
        [
            (a + perp).extend(0.0),
            (a - perp).extend(0.0),
            (b - perp).extend(0.0),
            (b + perp).extend(0.0),
        ]
    };

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();

    let quads = [
        quad_for_line(
            Vec2::new(-half_len, -half_len),
            Vec2::new(half_len, half_len),
            half_thickness,
        ),
        quad_for_line(
            Vec2::new(-half_len, half_len),
            Vec2::new(half_len, -half_len),
            half_thickness,
        ),
    ];

    for quad in quads {
        let v0 = quad[0];
        let v1 = quad[1];
        let v2 = quad[2];
        let v3 = quad[3];
        for v in [v0, v1, v2, v0, v2, v3] {
            positions.push([v.x, v.y, v.z]);
            normals.push([0.0, 0.0, 1.0]);
        }
    }

    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh
}

/// Updates ship mesh scale based on camera zoom.
pub fn update_ship_mesh_scale(
    cam_scale: Res<CameraScale>,
    mut meshes: Query<(&mut Transform, &mut Visibility), With<ShipMesh>>,
) {
    let (display_size, visibility) = compute_ship_display(cam_scale.0);
    let visible = visibility > 0.01;

    for (mut transform, mut vis) in &mut meshes {
        transform.scale = Vec3::splat(display_size);
        *vis = if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// Renders VisualShips as triangles during tactical combat.
pub fn render_visual_ships(
    combat: Res<CombatState>,
    visual_ships: Query<(&VisualShip, &GlobalTransform, &Transform, Option<&Selected>)>,
    camera_query: Query<&GlobalTransform, With<Camera3d>>,
    cam_scale: Res<CameraScale>,
    mut painter: ShapePainter,
) {
    if !combat.active {
        return;
    }

    let (display_size, visibility) = compute_ship_display(cam_scale.0);

    // Skip rendering if too faded
    if visibility < 0.01 {
        return;
    }

    for (ship, global_transform, _local_transform, is_selected) in &visual_ships {
        let pos = global_transform.translation();

        let base_color = match ship.faction {
            Faction::Player => Color::srgb(0.4, 1.0, 0.4),
            Faction::Enemy => Color::srgb(1.0, 0.3, 0.3),
        };

        // Apply visibility fade
        let color = base_color.with_alpha(visibility);

        painter.set_translation(pos);
        painter.set_rotation(Quat::IDENTITY);
        painter.set_color(color);

        // Triangle dimensions
        let size = display_size;
        painter.thickness = size * 0.15;
        let half_base = size * 0.5;
        let height = size;

        painter.line(
            Vec3::new(0.0, height * 0.5, 0.0),
            Vec3::new(-half_base, -height * 0.5, 0.0),
        );
        painter.line(
            Vec3::new(-half_base, -height * 0.5, 0.0),
            Vec3::new(half_base, -height * 0.5, 0.0),
        );
        painter.line(
            Vec3::new(half_base, -height * 0.5, 0.0),
            Vec3::new(0.0, height * 0.5, 0.0),
        );

        // Selection indicator: white ring around selected ships
        if is_selected.is_some() {
            painter.set_color(Color::srgba(1.0, 1.0, 1.0, 0.8 * visibility));
            painter.hollow = true;
            painter.thickness = size * 0.1;
            painter.circle(size * 0.8);
            painter.hollow = false;
        }
    }
}

/// Updates ship velocities based on move orders using Newtonian physics.
/// Ships accelerate toward destination, then decelerate to stop.
pub fn update_ship_movement(
    mut commands: Commands,
    time: Res<Time>,
    mut ships: Query<
        (
            Entity,
            &MoveOrder,
            &ShipStats,
            &mut LinearVelocity,
            &Transform,
            &Position,
        ),
        With<VisualShip>,
    >,
) {
    // Use frame delta time - Avian will integrate the velocity
    let dt = time.delta_secs_f64();
    if dt <= 0.0 {
        return;
    }

    // Throttled logging
    static LAST_LOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let last = LAST_LOG.load(std::sync::atomic::Ordering::Relaxed);
    let should_log = now - last > 500;
    if should_log {
        LAST_LOG.store(now, std::sync::atomic::Ordering::Relaxed);
    }

    for (entity, order, stats, mut velocity, transform, avian_pos) in &mut ships {
        // Current position in arena-local coords (from Transform)
        let current_pos = DVec3::new(
            transform.translation.x as f64,
            transform.translation.y as f64,
            0.0,
        );

        // Vector to destination
        let to_target = order.destination - current_pos;
        let distance = to_target.length();

        // Current velocity state
        let current_speed = velocity.0.length();

        // Stopping distance: d = v² / (2a)
        let stopping_distance = if stats.max_acceleration > 0.0 {
            current_speed * current_speed / (2.0 * stats.max_acceleration)
        } else {
            0.0
        };

        if should_log {
            let mode = if distance <= ARRIVAL_DISTANCE && current_speed <= ARRIVAL_VELOCITY {
                "ARRIVED"
            } else if distance > stopping_distance + ARRIVAL_DISTANCE {
                "ACCEL"
            } else {
                "BRAKE"
            };
            let direction = to_target.normalize_or_zero();
            info!(
                "Ship movement: mode={}, transform=({:.0},{:.0})km, avian_pos=({:.0},{:.0})km, dest=({:.0},{:.0})km, dir=({:.2},{:.2}), vel=({:.0},{:.0})km/s",
                mode,
                transform.translation.x as f64 / 1000.0,
                transform.translation.y as f64 / 1000.0,
                avian_pos.x / 1000.0,
                avian_pos.y / 1000.0,
                order.destination.x / 1000.0,
                order.destination.y / 1000.0,
                direction.x,
                direction.y,
                velocity.0.x / 1000.0,
                velocity.0.y / 1000.0
            );
        }

        if distance <= ARRIVAL_DISTANCE && current_speed <= ARRIVAL_VELOCITY {
            // ARRIVED: clear order and stop
            commands.entity(entity).remove::<MoveOrder>();
            velocity.0 = DVec3::ZERO;
        } else if distance > stopping_distance + ARRIVAL_DISTANCE {
            // ACCELERATE: thrust toward target
            let direction = to_target.normalize_or_zero();
            let delta_v = direction * stats.max_acceleration * dt;
            velocity.0 += delta_v;

            // Clamp to max speed
            let new_speed = velocity.0.length();
            if new_speed > stats.max_speed {
                velocity.0 = velocity.0.normalize() * stats.max_speed;
            }
        } else {
            // DECELERATE: thrust opposite to velocity
            if current_speed > ARRIVAL_VELOCITY {
                let brake_dir = -velocity.0.normalize_or_zero();
                let delta_v = brake_dir * stats.max_acceleration * dt;
                let new_vel = velocity.0 + delta_v;

                // Check if we've overshot (velocity reversed direction)
                if new_vel.dot(velocity.0) < 0.0 {
                    // Overshot - just stop
                    velocity.0 = DVec3::ZERO;
                } else {
                    velocity.0 = new_vel;
                }
            } else {
                // Already slow enough, just stop
                velocity.0 = DVec3::ZERO;
            }
        }
    }
}

/// Renders destination markers for selected ships with move orders.
pub fn render_move_markers(
    mut commands: Commands,
    combat: Res<CombatState>,
    arena_query: Query<Entity, With<TacticalArena>>,
    ships: Query<(Entity, &MoveOrder), (With<VisualShip>, With<Selected>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut markers: Query<(Entity, &MoveMarker, &mut Transform)>,
    cam_scale: Res<CameraScale>,
    mut marker_mesh: Local<Option<Handle<Mesh>>>,
    mut marker_material: Local<Option<Handle<StandardMaterial>>>,
) {
    if !combat.active {
        for (entity, _, _) in &mut markers {
            commands.entity(entity).despawn();
        }
        return;
    }

    let Ok(arena_entity) = arena_query.single() else {
        return;
    };

    let mut desired_positions: HashMap<Entity, Vec3> = HashMap::new();
    for (ship_entity, order) in &ships {
        desired_positions.insert(
            ship_entity,
            Vec3::new(order.destination.x as f32, order.destination.y as f32, 0.1),
        );
    }

    let mut existing_ships: HashSet<Entity> = HashSet::new();
    let (display_size, _) = compute_ship_display(cam_scale.0);
    for (marker_entity, marker, mut transform) in &mut markers {
        if let Some(pos) = desired_positions.get(&marker.ship) {
            transform.translation = *pos;
            transform.scale = Vec3::splat(display_size);
            existing_ships.insert(marker.ship);
        } else {
            commands.entity(marker_entity).despawn();
        }
    }

    let mesh_handle = marker_mesh.get_or_insert_with(|| meshes.add(create_move_marker_mesh()));
    let material_handle = marker_material.get_or_insert_with(|| {
        materials.add(StandardMaterial {
            base_color: Color::srgb(0.5, 1.0, 0.5),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            cull_mode: None,
            ..default()
        })
    });

    for (ship_entity, pos) in desired_positions {
        if existing_ships.contains(&ship_entity) {
            continue;
        }
        commands.spawn((
            Mesh3d(mesh_handle.clone()),
            MeshMaterial3d(material_handle.clone()),
            MoveMarker { ship: ship_entity },
            Transform::from_translation(pos).with_scale(Vec3::splat(display_size)),
            ChildOf(arena_entity),
        ));
    }
}

// ============================================================================
// State Transition Systems
// ============================================================================

/// Cleans up tactical mode when exiting.
/// Despawns arena and all children, resets combat state, restores camera and time.
pub fn teardown_tactical_arena(
    mut commands: Commands,
    arena_query: Query<Entity, With<TacticalArena>>,
    exit_dialog: Query<Entity, With<ExitDialogRoot>>,
    mut combat: ResMut<CombatState>,
    mut sim_time: ResMut<SimulationTime>,
    mut show_dialog: ResMut<ShowExitDialog>,
    mut camera_query: Query<&mut CameraTarget>,
    camera_state: Res<TacticalCameraState>,
) {
    // Despawn arena - children are automatically despawned via ChildOf relationship
    for arena in &arena_query {
        commands.entity(arena).despawn();
    }

    // Despawn exit dialog if open
    for dialog in &exit_dialog {
        commands.entity(dialog).despawn();
    }
    show_dialog.0 = false;

    // Reset combat state
    combat.active = false;
    combat.arena = None;
    combat.player_fleets.clear();
    combat.enemy_fleets.clear();
    combat.body = None;

    // Restore time scale to strategic default
    sim_time.time_scale = 1.0;

    // Restore camera to previous position
    if let Ok(mut camera_target) = camera_query.single_mut() {
        camera_target.move_to(camera_state.previous_position, camera_state.previous_scale);
    }

    info!("Tactical arena cleaned up, returning to strategic mode");
}

/// Cleans up empty fleets after combat (fleets with no remaining LogicalShips).
pub fn cleanup_empty_fleets(
    mut commands: Commands,
    fleets: Query<(Entity, &Children), With<Fleet>>,
    ships: Query<&LogicalShip>,
) {
    for (fleet_entity, children) in &fleets {
        let has_ships = children.iter().any(|child| ships.contains(child));
        if !has_ships {
            info!(
                "Fleet {:?} has no remaining ships - despawning",
                fleet_entity
            );
            commands.entity(fleet_entity).despawn();
        }
    }
}

/// Checks if ships have left the arena bounds (400km x 400km).
/// Ships leaving the arena are considered to have fled - despawn both visual and logical.
pub fn check_ship_bounds(mut commands: Commands, ships: Query<(Entity, &VisualShip, &Transform)>) {
    for (entity, visual, transform) in &ships {
        let pos = transform.translation;

        // Check if outside arena bounds (±200,000 km = ±200,000,000 m)
        if pos.x.abs() as f64 > ARENA_HALF || pos.y.abs() as f64 > ARENA_HALF {
            info!(
                "Ship {:?} fled arena at ({:.0} km, {:.0} km) - despawning",
                entity,
                pos.x as f64 / 1000.0,
                pos.y as f64 / 1000.0
            );

            // Despawn visual ship (children despawn automatically via ChildOf)
            commands.entity(entity).despawn();

            // Despawn the logical ship from strategic layer
            commands.entity(visual.logical).despawn();
        }
    }
}

/// Handles escape key press in tactical mode - shows exit confirmation dialog.
pub fn handle_tactical_escape(
    input: Res<ButtonInput<KeyCode>>,
    mut show_dialog: ResMut<ShowExitDialog>,
) {
    if input.just_pressed(KeyCode::Escape) {
        // Toggle dialog (pressing escape again closes it)
        show_dialog.0 = !show_dialog.0;
        info!("Tactical escape pressed, show_dialog={}", show_dialog.0);
    }
}

/// Marker for the exit dialog root entity
#[derive(Component)]
pub struct ExitDialogRoot;

/// Marker for the "Yes" button
#[derive(Component)]
pub struct ExitDialogYesButton;

/// Marker for the "No" button
#[derive(Component)]
pub struct ExitDialogNoButton;

/// Spawns or despawns the exit dialog based on ShowExitDialog state.
pub fn sync_exit_dialog(
    mut commands: Commands,
    show_dialog: Res<ShowExitDialog>,
    existing_dialog: Query<Entity, With<ExitDialogRoot>>,
) {
    let dialog_exists = !existing_dialog.is_empty();

    if show_dialog.0 && !dialog_exists {
        // Spawn dialog
        commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
                ExitDialogRoot,
            ))
            .with_children(|parent| {
                // Dialog box
                parent
                    .spawn((
                        Node {
                            padding: UiRect::all(Val::Px(20.0)),
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Center,
                            row_gap: Val::Px(16.0),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.15, 0.15, 0.2, 0.95)),
                        BorderRadius::all(Val::Px(8.0)),
                    ))
                    .with_children(|dialog| {
                        // Title
                        dialog.spawn((
                            Text::new("Retreat from combat?"),
                            TextFont {
                                font_size: 18.0,
                                ..default()
                            },
                            TextColor(Color::srgba(1.0, 1.0, 1.0, 1.0)),
                        ));

                        // Button row
                        dialog
                            .spawn(Node {
                                flex_direction: FlexDirection::Row,
                                column_gap: Val::Px(12.0),
                                ..default()
                            })
                            .with_children(|row| {
                                // Yes button
                                row.spawn((
                                    Button,
                                    Node {
                                        padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                                        ..default()
                                    },
                                    BackgroundColor(Color::srgba(0.6, 0.2, 0.2, 1.0)),
                                    BorderRadius::all(Val::Px(4.0)),
                                    ExitDialogYesButton,
                                ))
                                .with_child((
                                    Text::new("Yes, retreat"),
                                    TextFont {
                                        font_size: 14.0,
                                        ..default()
                                    },
                                    TextColor(Color::WHITE),
                                ));

                                // No button
                                row.spawn((
                                    Button,
                                    Node {
                                        padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                                        ..default()
                                    },
                                    BackgroundColor(Color::srgba(0.3, 0.3, 0.4, 1.0)),
                                    BorderRadius::all(Val::Px(4.0)),
                                    ExitDialogNoButton,
                                ))
                                .with_child((
                                    Text::new("No, continue"),
                                    TextFont {
                                        font_size: 14.0,
                                        ..default()
                                    },
                                    TextColor(Color::WHITE),
                                ));
                            });
                    });
            });
    } else if !show_dialog.0 && dialog_exists {
        // Despawn dialog
        for entity in &existing_dialog {
            commands.entity(entity).despawn();
        }
    }
}

/// Handles button clicks on the exit dialog.
pub fn handle_exit_dialog_buttons(
    mut show_dialog: ResMut<ShowExitDialog>,
    mut next_state: ResMut<NextState<crate::app_state::AppState>>,
    yes_button: Query<&Interaction, (Changed<Interaction>, With<ExitDialogYesButton>)>,
    no_button: Query<&Interaction, (Changed<Interaction>, With<ExitDialogNoButton>)>,
) {
    for interaction in &yes_button {
        if *interaction == Interaction::Pressed {
            show_dialog.0 = false;
            next_state.set(crate::app_state::AppState::Strategic);
        }
    }

    for interaction in &no_button {
        if *interaction == Interaction::Pressed {
            show_dialog.0 = false;
        }
    }
}

/// Detects end of combat: victory (all enemies destroyed/fled) or defeat (all player ships destroyed/fled).
pub fn detect_combat_end(
    ships: Query<&VisualShip>,
    mut next_state: ResMut<NextState<crate::app_state::AppState>>,
) {
    let mut player_alive = 0;
    let mut enemy_alive = 0;

    for ship in &ships {
        match ship.faction {
            Faction::Player => player_alive += 1,
            Faction::Enemy => enemy_alive += 1,
        }
    }

    if enemy_alive == 0 && player_alive > 0 {
        info!("Victory! All enemy ships destroyed or fled");
        next_state.set(crate::app_state::AppState::Strategic);
    } else if player_alive == 0 {
        info!("Defeat! All player ships destroyed or fled");
        next_state.set(crate::app_state::AppState::Strategic);
    }
}

// ============================================================================
// Debug Validation
// ============================================================================

/// Debug-only system that validates no tactical entities leaked into strategic mode.
/// Panics in debug builds if VisualShip or TacticalArena entities exist during Strategic state.
#[cfg(debug_assertions)]
pub fn validate_no_tactical_leaks(
    arenas: Query<Entity, With<TacticalArena>>,
    ships: Query<Entity, With<VisualShip>>,
) {
    let arena_count = arenas.iter().count();
    let ship_count = ships.iter().count();

    if arena_count > 0 || ship_count > 0 {
        panic!(
            "TACTICAL LEAK DETECTED: {} TacticalArena(s), {} VisualShip(s) in Strategic mode!",
            arena_count, ship_count
        );
    }
}

/// No-op in release builds
#[cfg(not(debug_assertions))]
pub fn validate_no_tactical_leaks() {}

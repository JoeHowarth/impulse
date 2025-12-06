//! Tactical combat mode - individual ship battles at a body.
//!
//! When combat triggers, we spawn a TacticalArena at the body's position
//! and VisualShip entities for each LogicalShip in the involved fleets.

use avian3d::prelude::*;
use bevy::gizmos::GizmoAsset;
use bevy::math::{DVec2, DVec3, Isometry3d};
use bevy::prelude::*;
use bevy_vector_shapes::prelude::*;

use crate::ComputedBody;
use crate::camera::{CameraScale, CameraTarget};
use crate::ship::{CombatState, Faction, LogicalShip, Selected, ship_count};
use crate::simulation::SimulationTime;

// ============================================================================
// Constants
// ============================================================================

// TEMPORARY SCALING WORKAROUND (remove after big_space integration)
// =================================================================
// Ship movement exhibits f32 precision issues at planetary distances.
// Avian3D uses f64 internally but syncs Position → Transform (f32).
// At Mercury (~50 billion meters), f32 can only represent changes of ~5,000m.
// At Venus (~100 billion meters), precision drops to ~10,000m.
//
// Symptoms observed:
// - Transform.x stays constant while Avian Position.x changes correctly
// - Small velocity components get "eaten" by f32 precision loss
// - Ships appear to move only in the dominant direction
//
// Workaround: Scale ships/speeds 100,000x larger so movements exceed f32 precision.
// Real values → Test values:
// - Ship size: 100m → 10,000 km (SHIP_PHYSICAL_SIZE)
// - Ship spacing: 1km → 100,000 km (SHIP_SPACING)
// - Acceleration: 10 m/s² (1g) → 1,000,000 m/s² (SHIP_STATS)
// - Max speed: 50 km/s → 5,000,000 km/s (SHIP_STATS)
// - Arrival distance: 1km → 10,000 km (ARRIVAL_DISTANCE)
// - Arrival velocity: 10 m/s → 100 km/s (ARRIVAL_VELOCITY)
//
// Fix: Integrate big_space for camera-relative GlobalTransforms.
// See plans/big_space_migration.md for implementation details.
// =================================================================

/// Arena size in meters (400,000 km)
pub const ARENA_SIZE: f64 = 400_000_000.0;

/// Half arena size - retreat boundary (200,000 km)
pub const ARENA_HALF: f64 = 200_000_000.0;

/// Tactical time scale: 60 sim seconds per real second (1 min/s)
pub const TACTICAL_TIME_SCALE: f64 = 60.0;

/// Offset from arena center for fleet spawns (50,000 km in meters)
const FLEET_SPAWN_OFFSET: f64 = 50_000_000.0;

/// Horizontal spacing between ships (100,000 km - temporary 100,000x for testing without big_space)
const SHIP_SPACING: f64 = 100_000_000.0;

/// Physical ship size in meters (10,000 km - temporary 100,000x for testing without big_space)
const SHIP_PHYSICAL_SIZE: f32 = 10_000_000.0;

/// Minimum screen size for ships in pixels (below this they fade out)
const SHIP_FADE_SCREEN_SIZE: f32 = 2.0;

/// Camera scale for tactical view
/// scale = world units per pixel. 400,000 km / 1000 pixels ≈ 4e5
const TACTICAL_CAMERA_SCALE: f32 = 4.0e5;

/// Arena center offset from body (150,000 km to put body on right side)
const ARENA_CENTER_OFFSET: f64 = 150_000_000.0;

/// Arrival distance threshold in meters (10,000 km - scaled for testing)
const ARRIVAL_DISTANCE: f64 = 10_000_000.0;

/// Arrival velocity threshold in m/s (100 km/s - scaled for testing)
const ARRIVAL_VELOCITY: f64 = 100_000.0;

// ============================================================================
// Components
// ============================================================================

/// The tactical combat arena - exists during combat only.
/// Children include VisualShips and missiles.
#[derive(Component)]
pub struct TacticalArena {
    /// The body where this battle is occurring
    pub body: Entity,
    // /// Arena center in heliocentric coordinates (Vec3 visual units)
    // pub heliocentric_pos: Vec3,
    // /// Previous camera position for restoration
    // pub previous_camera_pos: Vec2,
    // /// Previous camera scale for restoration
    // pub previous_camera_scale: f32,
    // /// Previous time scale for restoration
    // pub previous_time_scale: f64,
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

/// Marker for the ship triangle gizmo (child of VisualShip).
#[derive(Component)]
pub struct ShipGizmo;

/// Marker for the selection ring gizmo (child of VisualShip, only when selected).
#[derive(Component)]
pub struct SelectionRingGizmo;

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
            max_acceleration: 1_000_000.0, // 100,000g - temporary 100,000x scale for testing
            max_speed: 5_000_000_000.0,    // 5,000,000 km/s - temporary 100,000x scale for testing
        }
    }
}

// ============================================================================
// Systems
// ============================================================================

/// Enters tactical mode when combat is triggered.
/// Spawns TacticalArena and VisualShips, animates camera, adjusts time scale.
pub fn enter_tactical_mode(
    mut commands: Commands,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    mut combat: ResMut<CombatState>,
    mut sim_time: ResMut<SimulationTime>,
    mut camera_query: Query<&mut CameraTarget>,
    bodies: Query<&ComputedBody>,
    children_query: Query<&Children>,
    ships: Query<&LogicalShip>,
) {
    // Only run when combat is active but arena not yet spawned
    if !combat.active || combat.arena.is_some() {
        return;
    }

    let Some(body_entity) = combat.body else {
        warn!("Combat active but no body specified");
        return;
    };

    let Ok(body_computed) = bodies.get(body_entity) else {
        warn!("Cannot get computed body for combat");
        return;
    };

    // Get current camera state for later restoration
    let mut camera_target = camera_query.single_mut().expect("No camera found");

    // Offset arena center so body appears on right side of view
    // Offset in negative X to put body on positive X side
    // TODO(Phase 5): Use CellCoord + Transform instead of f32 position
    let arena_relative = Vec3::new(-(ARENA_CENTER_OFFSET as f32), 0.0, 0.0);
    let arena_helio = body_computed.helio_pos + DVec3::from(arena_relative);

    info!(
        "Entering tactical mode at body (relative pos: {:?}), {} player fleets, {} enemy fleets",
        arena_relative,
        combat.player_fleets.len(),
        combat.enemy_fleets.len()
    );

    // Spawn TacticalArena
    let arena = commands
        .spawn((
            TacticalArena {
                body: body_entity,
                // heliocentric_pos: arena_pos,
                // previous_camera_pos: current_pos,
                // previous_camera_scale: current_scale,
                // previous_time_scale: sim_time.time_scale,
            },
            ChildOf(body_entity),
            Transform::from_translation(arena_relative),
            Visibility::default(),
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

    // Spawn player ships (bottom center, y = -50,000 km)
    let player_spawned = spawn_fleet_ships(
        &mut commands,
        &mut gizmo_assets,
        arena,
        &combat.player_fleets,
        Faction::Player,
        -FLEET_SPAWN_OFFSET,
        player_ship_count as usize,
        &children_query,
        &ships,
    );

    // Spawn enemy ships (top center, y = +50,000 km)
    let enemy_spawned = spawn_fleet_ships(
        &mut commands,
        &mut gizmo_assets,
        arena,
        &combat.enemy_fleets,
        Faction::Enemy,
        FLEET_SPAWN_OFFSET,
        enemy_ship_count as usize,
        &children_query,
        &ships,
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
/// Returns the number of ships spawned.
fn spawn_fleet_ships(
    commands: &mut Commands,
    gizmo_assets: &mut Assets<GizmoAsset>,
    arena: Entity,
    fleets: &[Entity],
    faction: Faction,
    y_offset_meters: f64,
    total_ships: usize,
    children_query: &Query<&Children>,
    ships: &Query<&LogicalShip>,
) -> usize {
    let mut index = 0;

    // Create triangle gizmo asset (unit size, will be scaled by Transform)
    let color = match faction {
        Faction::Player => Color::srgb(0.4, 1.0, 0.4),
        Faction::Enemy => Color::srgb(1.0, 0.3, 0.3),
    };
    let mut gizmo = GizmoAsset::new();
    // Draw unit triangle pointing up (will be scaled by parent)
    gizmo.linestrip(
        [
            Vec3::new(0.0, 0.5, 0.0),   // top
            Vec3::new(-0.5, -0.5, 0.0), // bottom left
            Vec3::new(0.5, -0.5, 0.0),  // bottom right
            Vec3::new(0.0, 0.5, 0.0),   // back to top
        ],
        color,
    );
    let gizmo_handle = gizmo_assets.add(gizmo);

    for &fleet_entity in fleets {
        let Ok(children) = children_query.get(fleet_entity) else {
            continue;
        };

        for child in children.iter() {
            if ships.contains(child) {
                let x_offset = compute_ship_x_offset(index, total_ships);
                let logical_ship = child;

                // Spawn VisualShip
                let visual_ship = commands
                    .spawn((
                        VisualShip {
                            logical: logical_ship,
                            fleet: fleet_entity,
                            faction,
                        },
                        Transform::from_translation(Vec3::new(
                            x_offset as f32,
                            y_offset_meters as f32,
                            0.1,
                        )),
                        Visibility::default(),
                        // Physics components
                        RigidBody::Dynamic,
                        Collider::sphere(500.0), // 500m radius collider
                        LinearVelocity::default(),
                        SweptCcd::default(), // Prevent tunneling at high speeds
                        // Movement stats
                        ShipStats::default(),
                        ChildOf(arena),
                    ))
                    .id();

                // Spawn triangle gizmo as child of VisualShip
                commands.spawn((
                    Gizmo {
                        handle: gizmo_handle.clone(),
                        depth_bias: 0.0,
                        ..default()
                    },
                    ShipGizmo,
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

/// Updates the arena position to follow the combat body.
/// This keeps tactical ships centered on the body as it orbits.
/// Also moves the camera by the same delta so it tracks the arena.
pub fn update_arena_position(
    mut arena_query: Query<(&TacticalArena, &mut Transform)>,
    bodies: Query<&ComputedBody>,
    mut camera_query: Query<
        (&mut Transform, &mut crate::camera::CameraTarget),
        Without<TacticalArena>,
    >,
) {
    // for (arena, mut arena_transform) in &mut arena_query {
    //     if let Ok(body) = bodies.get(arena.body) {
    //         // Apply same offset as spawn to keep body on right side
    //         // TODO(Phase 5): Use CellCoord + Transform instead of f32 position
    //         let arena_offset = Vec3::new(-(ARENA_CENTER_OFFSET as f32), 0.0, 0.0);
    //         let new_pos = body.helio_pos.as_vec3() + arena_offset;

    //         // Calculate how much the arena moved this frame
    //         let delta = new_pos - arena_transform.translation;

    //         // Update arena position
    //         arena_transform.translation = new_pos;

    //         // Move camera by the same delta to track the arena
    //         // Always apply delta to camera position so it moves with arena
    //         if let Ok((mut cam_transform, mut camera_target)) = camera_query.single_mut() {
    //             // Always move camera position with arena
    //             cam_transform.translation.x += delta.x;
    //             cam_transform.translation.y += delta.y;

    //             // If animating, also move the target so we animate toward the right place
    //             if let Some(ref mut target_pos) = camera_target.position {
    //                 target_pos.x += delta.x;
    //                 target_pos.y += delta.y;
    //             }
    //         }
    //     }
    // }
}

/// Computes ship display size using LOD system similar to bodies.
/// Returns (display_size, visibility) where visibility is 0.0-1.0.
fn compute_ship_display(cam_scale: f32) -> (f32, f32) {
    // Use same log-based scaling as bodies:
    // log_scaled_size = ((log_radius - 4.0).max(1.0) * 1.5).min(8.0) * cam_scale
    // For 100m ship: log10(100) = 2, so (2 - 4).max(1) = 1, * 1.5 = 1.5 pixels target
    // But that's tiny, so we use a slightly different formula for ships

    let log_radius = SHIP_PHYSICAL_SIZE.log10();
    // Ships get a bit more screen presence than bodies of same size
    // Target ~4-6 pixels when zoomed out
    let screen_pixels = ((log_radius - 1.0).max(1.0) * 3.0).min(8.0);
    let log_scaled_size = screen_pixels * cam_scale;

    // Take max of log-scaled size vs physical size (like bodies do)
    let display_size = log_scaled_size.max(SHIP_PHYSICAL_SIZE);

    // Compute what the screen size actually is
    let actual_screen_size = display_size / cam_scale;

    // Fade out when scaled marker would be less than 2 pixels
    let visibility = if actual_screen_size < SHIP_FADE_SCREEN_SIZE {
        (actual_screen_size / SHIP_FADE_SCREEN_SIZE).clamp(0.0, 1.0)
    } else {
        1.0
    };

    (display_size, visibility)
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
    combat: Res<CombatState>,
    ships: Query<&MoveOrder, (With<VisualShip>, With<Selected>)>,
    arena_query: Query<&GlobalTransform, With<TacticalArena>>,
    cam_scale: Res<CameraScale>,
    mut painter: ShapePainter,
) {
    if !combat.active {
        return;
    }

    let Ok(arena_transform) = arena_query.single() else {
        return;
    };
    let arena_pos = arena_transform.translation();

    for order in &ships {
        // Convert arena-local to world
        let world_pos = Vec3::new(
            arena_pos.x + order.destination.x as f32,
            arena_pos.y + order.destination.y as f32,
            0.1,
        );

        // Draw X marker
        painter.set_translation(world_pos);
        painter.set_color(Color::srgba(0.5, 1.0, 0.5, 0.6));
        painter.thickness = cam_scale.0 * 2.0;

        let size = cam_scale.0 * 15.0; // 15 pixels
        painter.line(Vec3::new(-size, -size, 0.0), Vec3::new(size, size, 0.0));
        painter.line(Vec3::new(-size, size, 0.0), Vec3::new(size, -size, 0.0));
    }
}

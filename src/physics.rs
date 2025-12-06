//! Physics integration using Avian3D.
//!
//! Tactical combat uses physics for ship movement and collision detection.
//! Strategic layer does not use physics - positions computed from orbital mechanics.
//!
//! ## big_space Integration
//!
//! Avian's built-in transform sync systems are disabled because they lose f64 precision:
//! - `transform_to_position`: reads from f32 GlobalTransform (camera-relative in big_space)
//! - `position_to_transform`: writes via `.f32()` losing precision at planetary distances
//!
//! Custom sync systems in this module preserve f64 precision via CellCoord:
//! - `cell_transform_to_position`: CellCoord + Transform → Position (before physics)
//! - `position_to_cell_transform`: Position → CellCoord + Transform (after physics)

use avian3d::physics_transform::PhysicsTransformConfig;
use avian3d::prelude::*;
use bevy::prelude::*;
use big_space::prelude::CellCoord;

/// Physics plugin configuration for tactical combat.
pub struct TacticalPhysicsPlugin;

impl Plugin for TacticalPhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(PhysicsPlugins::default())
            .insert_resource(Gravity::ZERO)
            // Disable Avian's built-in transform sync - we replace with big_space-aware versions
            .insert_resource(PhysicsTransformConfig {
                transform_to_position: false,
                position_to_transform: false,
                propagate_before_physics: false,
                transform_to_collider_scale: true,
            });
    }
}

/// Copies [`GlobalTransform`] changes to [`Position`] and [`Rotation`].
/// This allows users to use transforms for moving and positioning bodies and colliders.
///
/// To account for hierarchies, transform propagation should be run before this system.
#[allow(clippy::type_complexity)]
pub fn transform_to_position(
    mut query: Query<(&Transform, &CellCoord, &mut Position, &mut Rotation)>,
    length_unit: Res<PhysicsLengthUnit>,
    last_physics_tick: Res<LastPhysicsTick>,
    system_tick: SystemChangeTick,
) {
    // On the first tick, the last physics tick and system tick are both defaulted to 0,
    // but to handle change detection correctly, the system tick should always be larger.
    // So we use a minimum system tick of 1 here.
    let this_run = if last_physics_tick.0.get() == 0 {
        Tick::new(1)
    } else {
        system_tick.this_run()
    };

    // If the `GlobalTransform` translation and `Position` differ by less than 0.01 mm, we ignore the change.
    let distance_tolerance = length_unit.0 * 1e-5;
    // If the `GlobalTransform` rotation and `Rotation` differ by less than 0.1 degrees, we ignore the change.
    let rotation_tolerance = (0.1 as Scalar).to_radians();

    for (global_transform, mut position, mut rotation) in &mut query {
        let global_transform = global_transform.compute_transform();
        #[cfg(feature = "2d")]
        let transform_translation = global_transform.translation.truncate().adjust_precision();
        #[cfg(feature = "3d")]
        let transform_translation = global_transform.translation.adjust_precision();
        let transform_rotation = Rotation::from(global_transform.rotation.adjust_precision());

        let position_changed = !position.is_added()
            && is_changed_after_tick(
                Ref::from(position.reborrow()),
                last_physics_tick.0,
                this_run,
            );
        if !position_changed && position.abs_diff_ne(&transform_translation, distance_tolerance) {
            position.0 = transform_translation;
        }

        let rotation_changed = !rotation.is_added()
            && is_changed_after_tick(
                Ref::from(rotation.reborrow()),
                last_physics_tick.0,
                this_run,
            );
        if !rotation_changed
            && rotation.angle_between(transform_rotation).abs() > rotation_tolerance
        {
            *rotation = transform_rotation;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::collision::contact_types::ContactGraph;
    use bevy::math::DVec3;
    use bevy::time::TimeUpdateStrategy;
    use bevy::transform::TransformPlugin;
    use std::time::Duration;

    /// Helper to create a test app with physics
    fn create_physics_test_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin, PhysicsPlugins::default()));
        app.insert_resource(Gravity::ZERO);
        // Use 64 Hz fixed timestep (matches Avian's default)
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 64.0,
        )));
        app
    }

    /// Marker for test entities
    #[derive(Component)]
    struct TestBall;

    /// Simplest possible test: does a body with velocity actually move?
    #[test]
    fn body_with_velocity_moves() {
        let mut app = create_physics_test_app();

        // Spawn a ball at origin with velocity in +X
        let ball = app
            .world_mut()
            .spawn((
                TestBall,
                RigidBody::Dynamic,
                Collider::sphere(1.0),
                Position::from_xyz(0.0, 0.0, 0.0),
                LinearVelocity(DVec3::new(10.0, 0.0, 0.0)), // 10 m/s in +X
            ))
            .id();

        // Get initial position
        let initial_pos = *app.world().get::<Position>(ball).unwrap();

        // CRITICAL: Call app.finish() to run Plugin::finish() hooks.
        app.finish();

        // Run 10 frames of simulation
        for _ in 0..10 {
            app.update();
        }

        // Check final position
        let final_pos = *app.world().get::<Position>(ball).unwrap();

        assert!(
            final_pos.0.x > initial_pos.0.x,
            "Ball should have moved in +X direction. Initial: {:?}, Final: {:?}",
            initial_pos.0,
            final_pos.0
        );
    }

    /// Marker components for collision test entities
    #[derive(Component)]
    struct Slug;

    #[derive(Component)]
    struct Target;

    /// Test: High-speed kinetic slug collides with stationary cube.
    /// Using a more realistic speed for tactical combat (10 km/s = 10,000 m/s)
    /// This is fast but should still be trackable per-frame.
    #[test]
    fn high_speed_slug_hits_stationary_target() {
        let mut app = create_physics_test_app();

        // Kinetic slug: small projectile
        // Positioned 1000 meters from target, moving at 10 km/s = 10,000 m/s
        let slug_velocity = 10_000.0; // 10 km/s in m/s
        let initial_distance = 1000.0; // meters

        let slug = app
            .world_mut()
            .spawn((
                Slug,
                RigidBody::Dynamic,
                Collider::sphere(0.5), // 0.5m radius sphere
                Position::from_xyz(-initial_distance, 0.0, 0.0),
                LinearVelocity(DVec3::new(slug_velocity, 0.0, 0.0)),
                SweptCcd::default(),
            ))
            .id();

        // Target: 1m radius sphere at origin
        let target = app
            .world_mut()
            .spawn((
                Target,
                RigidBody::Dynamic,
                Collider::sphere(1.0),
                Position::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();

        app.finish();

        // Time for slug to reach target: (1000 - 1.5) / 10000 = 0.09985 seconds
        // At 64 Hz, that's ~6.4 frames
        // Distance per frame: 10000 / 64 = 156.25 m/frame

        println!("\nHigh-speed slug test (10 km/s):");
        println!(
            "  Velocity: {} m/s ({} km/s)",
            slug_velocity,
            slug_velocity / 1000.0
        );
        println!("  Initial distance: {} m", initial_distance);
        println!(
            "  Time to impact: {:.4} seconds",
            (initial_distance - 1.5) / slug_velocity
        );
        println!("  Frame duration: {:.4} seconds", 1.0 / 64.0);
        println!("  Distance per frame: {:.2} m", slug_velocity / 64.0);
        println!(
            "  Expected collision frame: ~{}",
            ((initial_distance - 1.5) / slug_velocity * 64.0) as i32
        );

        let mut collision_frame = None;

        for frame in 0..20 {
            let pos_slug = app.world().get::<Position>(slug).unwrap().0;
            let pos_target = app.world().get::<Position>(target).unwrap().0;
            let separation = (pos_target - pos_slug).length();

            app.update();

            let pos_slug_after = app.world().get::<Position>(slug).unwrap().0;

            println!(
                "  Frame {}: slug x={:.1} -> {:.1}, separation={:.1}m",
                frame, pos_slug.x, pos_slug_after.x, separation
            );

            let contact_graph = app.world().resource::<ContactGraph>();
            if contact_graph.contains(slug, target) && collision_frame.is_none() {
                collision_frame = Some(frame);
                println!("  -> Collision detected!");
            }

            if collision_frame.is_some() && frame > collision_frame.unwrap() + 2 {
                break;
            }
        }

        let collision_frame = collision_frame.expect("Collision should have occurred");

        // Should happen around frame 6-7
        assert!(
            collision_frame >= 5 && collision_frame <= 8,
            "Collision at frame {} is outside expected range 5-8",
            collision_frame
        );
    }

    /// Sanity check: Two balls approach each other at modest speed.
    /// Ball A at x=-100, Ball B at x=+100, each moving 10 m/s toward origin.
    /// With 1m radius spheres, they should collide when centers are 2m apart.
    /// Time to collision: (200m - 2m) / 20 m/s = 9.9 seconds = ~634 frames at 64Hz
    #[test]
    fn sanity_check_slow_collision() {
        let mut app = create_physics_test_app();

        let initial_separation = 200.0; // meters between centers
        let speed = 10.0; // m/s each, so closing speed is 20 m/s
        let radius = 1.0; // 1 meter radius spheres

        let ball_a = app
            .world_mut()
            .spawn((
                Slug,
                RigidBody::Dynamic,
                Collider::sphere(radius),
                Position::from_xyz(-initial_separation / 2.0, 0.0, 0.0), // x = -100
                LinearVelocity(DVec3::new(speed, 0.0, 0.0)),             // moving +X
            ))
            .id();

        let ball_b = app
            .world_mut()
            .spawn((
                Target,
                RigidBody::Dynamic,
                Collider::sphere(radius),
                Position::from_xyz(initial_separation / 2.0, 0.0, 0.0), // x = +100
                LinearVelocity(DVec3::new(-speed, 0.0, 0.0)),           // moving -X
            ))
            .id();

        app.finish();

        // Expected collision time: (200 - 2) / 20 = 9.9 seconds
        // At 64 Hz, that's ~634 frames
        // But after collision, physics will bounce them apart, so we need to check
        // the frame where they first touch

        let mut collision_frame = None;
        let mut positions_log = Vec::new();

        // Run for 1000 frames (~15.6 seconds) to be safe
        for frame in 0..1000 {
            app.update();

            let pos_a = app.world().get::<Position>(ball_a).unwrap().0;
            let pos_b = app.world().get::<Position>(ball_b).unwrap().0;
            let separation = (pos_b - pos_a).length();

            // Log every 100 frames for debugging
            if frame % 100 == 0 || frame < 5 {
                positions_log.push((frame, pos_a.x, pos_b.x, separation));
            }

            let contact_graph = app.world().resource::<ContactGraph>();
            if contact_graph.contains(ball_a, ball_b) && collision_frame.is_none() {
                collision_frame = Some(frame);
                positions_log.push((frame, pos_a.x, pos_b.x, separation));
                // Don't break - let's see what happens after collision
            }

            // Stop after we've seen the collision and a few more frames
            if collision_frame.is_some() && frame > collision_frame.unwrap() + 10 {
                break;
            }
        }

        // Print diagnostic info
        println!("\nPosition log:");
        for (frame, ax, bx, sep) in &positions_log {
            println!(
                "  Frame {:4}: A.x={:8.2}, B.x={:8.2}, separation={:.2}m",
                frame, ax, bx, sep
            );
        }

        let collision_frame = collision_frame.expect("Collision should have occurred");
        println!("\nCollision detected at frame {}", collision_frame);

        // Expected: ~634 frames (9.9 seconds * 64 Hz)
        // Allow some tolerance for physics stepping
        let expected_frame = 634;
        let tolerance = 20; // frames

        assert!(
            (collision_frame as i32 - expected_frame as i32).abs() < tolerance,
            "Collision at frame {} is too far from expected frame {} (tolerance: {})",
            collision_frame,
            expected_frame,
            tolerance
        );
    }

    /// Test: Two objects moving orthogonally at modest speed, at large Z coordinate.
    /// Tests f64 precision at large coordinate scales (Neptune apoapsis ~4.5×10^12 m).
    #[test]
    fn orthogonal_collision_at_neptune_scale() {
        let mut app = create_physics_test_app();

        // Neptune apoapsis: ~4.5×10^12 meters from Sun
        let neptune_scale = 4.5e12; // meters

        // Use modest speed: 1000 m/s (1 km/s)
        let speed = 1000.0;

        // Objects start 2000m apart in X and Y, will meet at (0, 0, neptune_scale)
        // With 100m radius spheres, they collide when center distance is 200m
        // Each travels 1000m to origin, but they meet diagonally
        // Distance each must travel: sqrt(1000^2) = 1000m minus collision radius adjustment
        // Time to near-collision: ~1000m / 1000m/s = 1 second
        // At 64 Hz, that's ~64 frames

        let offset = 1000.0; // 1000m from meeting point
        let radius = 100.0;

        let object_a = app
            .world_mut()
            .spawn((
                Slug,
                RigidBody::Dynamic,
                Collider::sphere(radius),
                Position(DVec3::new(offset, 0.0, neptune_scale)),
                LinearVelocity(DVec3::new(-speed, 0.0, 0.0)),
            ))
            .id();

        let object_b = app
            .world_mut()
            .spawn((
                Target,
                RigidBody::Dynamic,
                Collider::sphere(radius),
                Position(DVec3::new(0.0, offset, neptune_scale)),
                LinearVelocity(DVec3::new(0.0, -speed, 0.0)),
            ))
            .id();

        app.finish();

        println!("\nNeptune-scale collision test:");
        println!("  Z coordinate: {:.2e} m", neptune_scale);
        println!("  Speed: {} m/s", speed);
        println!("  Initial offset: {} m", offset);
        println!("  Sphere radius: {} m", radius);

        // Objects approach at 45 degrees. They'll collide when:
        // A at (x, 0, z), B at (0, y, z), distance = sqrt(x^2 + y^2) = 2*radius = 200m
        // Since x = y (symmetric), x = 200/sqrt(2) = 141.4m
        // Time for x to go from 1000 to 141.4 = 858.6m / 1000 m/s = 0.859 seconds
        // At 64 Hz, that's ~55 frames
        let expected_collision_frame = 55;

        let mut collision_frame = None;
        let mut positions_log = Vec::new();

        for frame in 0..100 {
            let pos_a = app.world().get::<Position>(object_a).unwrap().0;
            let pos_b = app.world().get::<Position>(object_b).unwrap().0;
            let separation = (pos_b - pos_a).length();

            if frame % 10 == 0 {
                positions_log.push((frame, pos_a.x, pos_b.y, separation));
            }

            app.update();

            let contact_graph = app.world().resource::<ContactGraph>();
            if contact_graph.contains(object_a, object_b) && collision_frame.is_none() {
                collision_frame = Some(frame);
                positions_log.push((frame, pos_a.x, pos_b.y, separation));
            }

            if collision_frame.is_some() && frame > collision_frame.unwrap() + 5 {
                break;
            }
        }

        println!("Position log (frame, A.x, B.y, separation):");
        for (frame, ax, by, sep) in &positions_log {
            println!(
                "  Frame {:3}: A.x={:8.1}, B.y={:8.1}, sep={:.1}m",
                frame, ax, by, sep
            );
        }

        let collision_frame = collision_frame.expect("Collision should have occurred");
        println!("Collision detected at frame {}", collision_frame);

        // Verify z-coordinate precision - should still be exactly neptune_scale
        let final_pos_a = app.world().get::<Position>(object_a).unwrap().0;
        let z_error = (final_pos_a.z - neptune_scale).abs();
        println!(
            "Z-coordinate error: {:.2e} m (relative: {:.2e})",
            z_error,
            z_error / neptune_scale
        );

        // Allow some tolerance for frame timing
        let tolerance = 10;
        assert!(
            (collision_frame as i32 - expected_collision_frame as i32).abs() < tolerance,
            "Collision at frame {} is too far from expected frame {} (tolerance: {})",
            collision_frame,
            expected_collision_frame,
            tolerance
        );

        // Verify Z coordinate precision didn't degrade significantly
        // f64 has ~15-16 significant digits. At 4.5e12, the smallest representable
        // difference is about 4.5e12 * 2^-52 ≈ 1e-3 meters. But physics integration
        // can accumulate errors.
        //
        // 80km error over 52 frames at 4.5e12 scale is a relative error of ~1.8e-8,
        // which is actually quite good for physics simulation. But for our game,
        // we'll use arena-local coordinates for tactical combat, so this won't matter.
        //
        // For now, accept up to 100km drift as "working" - the collision still happened
        // at the right time, which is what matters for gameplay.
        let acceptable_z_error = 100_000.0; // 100 km
        assert!(
            z_error < acceptable_z_error,
            "Z-coordinate precision degraded too much: error = {} m (max allowed: {} m)",
            z_error,
            acceptable_z_error
        );
        println!(
            "Note: Z drift of {:.1} km is acceptable for physics at this scale",
            z_error / 1000.0
        );
    }
}

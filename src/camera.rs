//! Camera setup, control, and animation systems.

use astrora_core::core::constants::AU;
use bevy::{
    camera::visibility::NoFrustumCulling,
    input::mouse::MouseButton,
    math::{DVec2, DVec3},
    prelude::*,
    window::PrimaryWindow,
};
use bevy_pancam::PanCam;
use bevy_vector_shapes::{
    prelude::ShapeConfig,
    shapes::{DiscBundle, ShapeBundle},
};
use big_space::prelude::*;

use crate::BodyShape;

// ============================================================================
// Constants
// ============================================================================

/// Camera animation speed (higher = faster lerp)
const CAMERA_LERP_SPEED: f32 = 8.0;

/// Default camera scale fallback (roughly solar system scale)
const DEFAULT_CAMERA_SCALE: f32 = 1.0e11;

/// Camera far plane (must encompass solar system)
// const CAMERA_FAR: f32 = 1.0e15;

/// Camera near plane
// const CAMERA_NEAR: f32 = 0.1;

const CAMERA_NEAR: f32 = -2e11; // Negative is valid for ortho - allows objects "behind" camera position
const CAMERA_FAR: f32 = 2e11;

/// Camera Z position (looking down at XY plane)
const CAMERA_Z: f32 = 1.0e7;

/// Grid cell edge length in meters.
/// 10km cells give sub-meter precision for tactical combat (100m ships, 0.1m projectiles).
const GRID_CELL_SIZE: f32 = 10_000.0;

/// Switching threshold - how far past cell edge before recentering (in meters).
const GRID_SWITCH_THRESHOLD: f32 = 100.0;

// ============================================================================
// Setup
// ============================================================================

/// Spawns the BigSpace root with Grid, and the camera with FloatingOrigin.
/// The camera is the floating origin, so GlobalTransforms are computed relative to it.
pub fn spawn_camera(mut commands: Commands, query: Query<&Window, With<PrimaryWindow>>) {
    let window = query.single().unwrap();
    let window_width = window.resolution.width();
    let initial_scale = AU as f32 * 3. / window_width;

    // Configure the heliocentric grid
    let grid = Grid::new(GRID_CELL_SIZE, GRID_SWITCH_THRESHOLD);

    // Track the root entity so other systems can spawn children
    let mut root_entity = Entity::PLACEHOLDER;

    // Spawn BigSpace with camera inside
    commands.spawn_big_space(grid, |root| {
        root_entity = root.id();
        root.spawn_spatial((
            Camera3d::default(),
            FloatingOrigin, // Camera is the floating origin for precise rendering
            Projection::from(OrthographicProjection {
                far: CAMERA_FAR,
                near: CAMERA_NEAR,
                scale: initial_scale,
                ..OrthographicProjection::default_3d()
            }),
            Transform::from_xyz(0.0, 0.0, 0.0).looking_at(Vec3::NEG_Z, Vec3::Y),
            PanCam {
                // Only use right/middle mouse for panning - left is for selection
                grab_buttons: vec![MouseButton::Right, MouseButton::Middle],
                ..default()
            },
            CameraTarget::default(),
        ));
    });
    commands.entity(root_entity).insert(Visibility::Visible);

    // Store the BigSpace root for other systems to use
    commands.insert_resource(BigSpaceRoot(root_entity));

    // Initialize the CameraScale resource with the starting scale
    commands.insert_resource(CameraScale(initial_scale));
}

// ============================================================================
// Resources
// ============================================================================

/// Current camera scale, updated each frame.
/// Use this for screen-space sizing: `cam_scale.0 * pixels` gives world units.
#[derive(Resource, Default)]
pub struct CameraScale(pub f32);

/// The root entity of the BigSpace hierarchy.
/// All spatial entities (bodies, fleets, ships) should be children of this entity.
#[derive(Resource)]
pub struct BigSpaceRoot(pub Entity);

/// Updates the CameraScale resource from the current camera projection.
/// Run this before any rendering systems that need screen-space sizing.
pub fn update_camera_scale(
    camera: Query<(&Projection, &Transform), With<Camera>>,
    mut scale: ResMut<CameraScale>,
) {
    let mut next_scale = DEFAULT_CAMERA_SCALE;
    for (projection, transform) in camera.iter() {
        if let Projection::Orthographic(ortho) = projection {
            dbg!(ortho.scale, transform.scale);
            next_scale = ortho.scale;
            break;
        }
    }
    dbg!(next_scale);
    scale.0 = next_scale;
}

/// Component attached to a camera to smoothly animate it toward a target.
/// When present, the camera will lerp toward the target position/scale each frame.
/// Fields are cleared when the target is reached.
#[derive(Component, Default)]
pub struct CameraTarget {
    /// Target position (x, y). Camera will smoothly pan to this position.
    pub position: Option<DVec2>,
    /// Target orthographic scale. Camera will smoothly zoom to this scale.
    pub scale: Option<f32>,
}

impl CameraTarget {
    /// Set target position for smooth pan.
    pub fn pan_to(&mut self, pos: DVec2) {
        self.position = Some(pos);
    }

    /// Set target scale for smooth zoom.
    pub fn zoom_to(&mut self, scale: f32) {
        self.scale = Some(scale);
    }

    /// Set both position and scale targets.
    pub fn move_to(&mut self, pos: DVec2, scale: f32) {
        self.position = Some(pos);
        self.scale = Some(scale);
    }

    /// Returns true if camera is currently animating.
    pub fn is_animating(&self) -> bool {
        self.position.is_some() || self.scale.is_some()
    }
}

/// System that smoothly animates cameras toward their target position and zoom.
/// Works on any camera with a `CameraTarget` component.
pub fn animate_camera(
    time: Res<Time>,
    mut camera_query: Query<(
        &mut Transform,
        &mut CellCoord,
        &mut Projection,
        &mut CameraTarget,
    )>,
    grid_query: Query<&Grid, With<BigSpace>>,
) {
    let dt = time.delta_secs_f64();
    let lerp_factor = (CAMERA_LERP_SPEED as f64 * dt).min(1.0);
    let grid = grid_query.single().unwrap();

    for (mut transform, mut cell, mut projection, mut target) in camera_query.iter_mut() {
        // Animate position
        if let Some(target_pos) = target.position {
            let current_3 = grid.grid_position_double(&cell, &transform);
            let current_2 = current_3.xy();

            let new_pos = current_2.lerp(target_pos, lerp_factor as f64);

            let (new_cell, new_transform) =
                grid.translation_to_grid(DVec3::new(new_pos.x, new_pos.y, current_3.z));

            *cell = new_cell;
            transform.translation = new_transform;

            // Clear target if close enough (0.1% of target magnitude, min 1.0)
            let threshold = (target_pos.length() * 0.001).max(1.0);
            if current_2.distance(target_pos) < threshold {
                // Snap to exact target position to eliminate any remaining error
                let (target_cell, target_transform) =
                    grid.translation_to_grid(DVec3::new(target_pos.x, target_pos.y, current_3.z));

                *cell = target_cell;
                transform.translation = target_transform;
                target.position = None;
            }
        }

        // Animate zoom
        if let Some(target_scale) = target.scale {
            if let Projection::Orthographic(ref mut ortho) = *projection {
                let new_scale = ortho.scale + (target_scale - ortho.scale) * lerp_factor as f32;
                ortho.scale = new_scale;

                // Clear target if close enough (1% of target scale)
                if (ortho.scale - target_scale).abs() < target_scale * 0.01 {
                    target.scale = None;
                }
            }
        }
    }
}

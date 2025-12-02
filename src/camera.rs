//! Camera setup, control, and animation systems.

use astrora_core::core::constants::AU;
use bevy::{prelude::*, window::PrimaryWindow};
use bevy_pancam::PanCam;

// ============================================================================
// Constants
// ============================================================================

/// Camera animation speed (higher = faster lerp)
const CAMERA_LERP_SPEED: f32 = 8.0;

/// Default camera scale fallback (roughly solar system scale)
const DEFAULT_CAMERA_SCALE: f32 = 1.0e11;

// /// Initial camera scale: shows ~2 AU vertically (inner solar system)
// /// Scale = half-height in meters. 1.5e11 / 1200 pixels ≈ 1.25e8 m/pixel
// const INITIAL_CAMERA_SCALE: f32 = 1.5e11 / 1900.0;

/// Camera far plane (must encompass solar system)
const CAMERA_FAR: f32 = 1.0e15;

/// Camera near plane
const CAMERA_NEAR: f32 = 0.1;

/// Camera Z position (looking down at XY plane)
const CAMERA_Z: f32 = 1.0e12;

// ============================================================================
// Resources
// ============================================================================

/// Current camera scale, updated each frame.
/// Use this for screen-space sizing: `cam_scale.0 * pixels` gives world units.
#[derive(Resource, Default)]
pub struct CameraScale(pub f32);

/// Updates the CameraScale resource from the current camera projection.
/// Run this before any rendering systems that need screen-space sizing.
pub fn update_camera_scale(
    camera: Query<&Projection, With<Camera>>,
    mut scale: ResMut<CameraScale>,
) {
    scale.0 = camera
        .iter()
        .find_map(|p| match p {
            Projection::Orthographic(o) => Some(o.scale),
            _ => None,
        })
        .unwrap_or(DEFAULT_CAMERA_SCALE);
}

/// Component attached to a camera to smoothly animate it toward a target.
/// When present, the camera will lerp toward the target position/scale each frame.
/// Fields are cleared when the target is reached.
#[derive(Component, Default)]
pub struct CameraTarget {
    /// Target position (x, y). Camera will smoothly pan to this position.
    pub position: Option<Vec2>,
    /// Target orthographic scale. Camera will smoothly zoom to this scale.
    pub scale: Option<f32>,
}

impl CameraTarget {
    /// Set target position for smooth pan.
    pub fn pan_to(&mut self, pos: Vec2) {
        self.position = Some(pos);
    }

    /// Set target scale for smooth zoom.
    pub fn zoom_to(&mut self, scale: f32) {
        self.scale = Some(scale);
    }

    /// Set both position and scale targets.
    pub fn move_to(&mut self, pos: Vec2, scale: f32) {
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
    mut camera_query: Query<(&mut Transform, &mut Projection, &mut CameraTarget)>,
) {
    let dt = time.delta_secs();
    let lerp_factor = (CAMERA_LERP_SPEED * dt).min(1.0);

    for (mut transform, mut projection, mut target) in camera_query.iter_mut() {
        // Animate position
        if let Some(target_pos) = target.position {
            let current = Vec2::new(transform.translation.x, transform.translation.y);
            let new_pos = current.lerp(target_pos, lerp_factor);
            transform.translation.x = new_pos.x;
            transform.translation.y = new_pos.y;

            // Clear target if close enough (0.1% of target magnitude, min 1.0)
            let threshold = (target_pos.length() * 0.001).max(1.0);
            if current.distance(target_pos) < threshold {
                target.position = None;
            }
        }

        // Animate zoom
        if let Some(target_scale) = target.scale {
            if let Projection::Orthographic(ref mut ortho) = *projection {
                let new_scale = ortho.scale + (target_scale - ortho.scale) * lerp_factor;
                ortho.scale = new_scale;

                // Clear target if close enough (1% of target scale)
                if (ortho.scale - target_scale).abs() < target_scale * 0.01 {
                    target.scale = None;
                }
            }
        }
    }
}

// ============================================================================
// Setup
// ============================================================================

/// Spawns the main orthographic camera with pan/zoom support.
/// Also initializes the CameraScale resource.
pub fn spawn_camera(mut commands: Commands, query: Query<&Window, With<PrimaryWindow>>) {
    let window = query.single().unwrap();
    let window_width = window.resolution.width();
    let initial_scale = AU as f32 * 3. / window_width;

    commands.spawn((
        Camera3d::default(),
        Projection::from(OrthographicProjection {
            far: CAMERA_FAR,
            near: CAMERA_NEAR,
            scale: initial_scale,
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 0.0, CAMERA_Z).looking_at(Vec3::ZERO, Vec3::Y),
        PanCam::default(),
        CameraTarget::default(),
    ));

    // Initialize the CameraScale resource with the starting scale
    commands.insert_resource(CameraScale(initial_scale));
}

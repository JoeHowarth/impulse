//! Camera control and animation systems.

use bevy::prelude::*;

/// Camera animation speed (higher = faster)
const CAMERA_LERP_SPEED: f32 = 8.0;

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

            // Clear target if close enough
            if current.distance(target_pos) < 0.1 {
                target.position = None;
            }
        }

        // Animate zoom
        if let Some(target_scale) = target.scale {
            if let Projection::Orthographic(ref mut ortho) = *projection {
                let new_scale = ortho.scale + (target_scale - ortho.scale) * lerp_factor;
                ortho.scale = new_scale;

                // Clear target if close enough
                if (ortho.scale - target_scale).abs() < 0.01 {
                    target.scale = None;
                }
            }
        }
    }
}

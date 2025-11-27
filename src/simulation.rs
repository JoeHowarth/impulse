//! Simulation time management.

use bevy::prelude::*;

/// Base rate: 1 real second = 10 days of simulation
pub const SIM_BASE_RATE: f64 = 60.0 * 60.0 * 24.0 * 10.0;

/// Simulation time state, decoupled from wall clock.
#[derive(Resource)]
pub struct SimulationTime {
    /// Accumulated simulation time in seconds
    pub sim_time: f64,
    /// Speed multiplier (1.0 = 10 days per real second)
    pub time_scale: f64,
    /// Whether simulation is paused
    pub paused: bool,
}

impl Default for SimulationTime {
    fn default() -> Self {
        Self {
            sim_time: 0.0,
            time_scale: 1.0,
            paused: false,
        }
    }
}

/// Handles keyboard input for time controls (pause, speed up/down).
pub fn handle_time_controls(
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut sim_time: ResMut<SimulationTime>,
) {
    // Toggle pause with P
    if keyboard.just_pressed(KeyCode::KeyP) {
        sim_time.paused = !sim_time.paused;
    }

    // Adjust time scale with +/- (also = for + without shift)
    if keyboard.just_pressed(KeyCode::Equal) || keyboard.just_pressed(KeyCode::NumpadAdd) {
        sim_time.time_scale *= 2.0;
        sim_time.time_scale = sim_time.time_scale.min(64.0); // Cap at 64x
    }
    if keyboard.just_pressed(KeyCode::Minus) || keyboard.just_pressed(KeyCode::NumpadSubtract) {
        sim_time.time_scale /= 2.0;
        sim_time.time_scale = sim_time.time_scale.max(0.125); // Min at 0.125x
    }

    // Advance simulation time if not paused
    if !sim_time.paused {
        let delta = time.delta_secs_f64();
        sim_time.sim_time += delta * SIM_BASE_RATE * sim_time.time_scale;
    }
}

//! Simulation time management.

use avian3d::schedule::{Physics, PhysicsTime};
use bevy::ecs::message::MessageWriter;
use bevy::prelude::*;

use crate::strategic::StrategicCommand;

/// Seconds per day
pub const SECONDS_PER_DAY: f64 = 60.0 * 60.0 * 24.0;

/// Default strategic time scale: 2.5 days per real second
pub const STRATEGIC_TIME_SCALE: f64 = SECONDS_PER_DAY * 2.5;

/// Simulation time state, decoupled from wall clock.
#[derive(Resource)]
pub struct SimulationTime {
    /// Accumulated simulation time in seconds
    pub sim_time: f64,
    /// Simulation seconds per real second (e.g., 864000 = 10 days/sec)
    pub time_scale: f64,
    /// Whether simulation is paused
    pub paused: bool,
}

impl Default for SimulationTime {
    fn default() -> Self {
        Self {
            sim_time: 0.0,
            time_scale: STRATEGIC_TIME_SCALE,
            paused: false,
        }
    }
}

impl SimulationTime {
    /// Create SimulationTime starting at a specific day
    pub fn from_start_day(day: i32) -> Self {
        Self {
            sim_time: day as f64 * SECONDS_PER_DAY,
            time_scale: STRATEGIC_TIME_SCALE,
            paused: false,
        }
    }
}

/// Parse CLI arguments and return the starting day (default 0)
pub fn parse_start_day() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--day" || args[i] == "-d" {
            if let Some(day_str) = args.get(i + 1) {
                if let Ok(day) = day_str.parse::<i32>() {
                    return day;
                }
            }
        }
    }
    0
}

/// Posts time control commands when keyboard input is detected.
/// Also advances simulation time if not paused, and syncs physics time.
pub fn handle_time_controls(
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut sim_time: ResMut<SimulationTime>,
    mut physics_time: ResMut<Time<Physics>>,
    mut cmd_writer: MessageWriter<StrategicCommand>,
) {
    // Toggle pause with P
    if keyboard.just_pressed(KeyCode::KeyP) {
        cmd_writer.write(StrategicCommand::SetPaused(!sim_time.paused));
    }

    // Adjust time scale with +/- (also = for + without shift)
    if keyboard.just_pressed(KeyCode::Equal) || keyboard.just_pressed(KeyCode::NumpadAdd) {
        let new_scale = (sim_time.time_scale * 2.0).min(STRATEGIC_TIME_SCALE * 64.0);
        cmd_writer.write(StrategicCommand::SetTimeScale(new_scale));
    }
    if keyboard.just_pressed(KeyCode::Minus) || keyboard.just_pressed(KeyCode::NumpadSubtract) {
        let new_scale = (sim_time.time_scale / 2.0).max(1.0);
        cmd_writer.write(StrategicCommand::SetTimeScale(new_scale));
    }

    // Advance simulation time if not paused
    if !sim_time.paused {
        let delta = time.delta_secs_f64();
        sim_time.sim_time += delta * sim_time.time_scale;
    }

    // Sync physics time with simulation time
    physics_time.set_relative_speed_f64(sim_time.time_scale);
    if sim_time.paused {
        physics_time.pause();
    } else {
        physics_time.unpause();
    }
}

/// Formats a time_scale value as a human-readable rate string.
/// e.g., 60 -> "1min/s", 86400 -> "1d/s", 864000 -> "10d/s"
pub fn format_time_scale(time_scale: f64) -> String {
    const SECS_PER_MIN: f64 = 60.0;
    const SECS_PER_HOUR: f64 = 3600.0;
    const SECS_PER_DAY: f64 = 86400.0;

    if time_scale >= SECS_PER_DAY {
        let days = time_scale / SECS_PER_DAY;
        if days == days.floor() {
            format!("{:.0} day/s", days)
        } else {
            format!("{:.1} day/s", days)
        }
    } else if time_scale >= SECS_PER_HOUR {
        let hours = time_scale / SECS_PER_HOUR;
        if hours == hours.floor() {
            format!("{:.0} hr/s", hours)
        } else {
            format!("{:.1} hr/s", hours)
        }
    } else if time_scale >= SECS_PER_MIN {
        let mins = time_scale / SECS_PER_MIN;
        if mins == mins.floor() {
            format!("{:.0} min/s", mins)
        } else {
            format!("{:.1} min/s", mins)
        }
    } else if time_scale >= 1.0 {
        format!("{:.0} sec/s", time_scale)
    } else {
        format!("{:.2} sec/s", time_scale)
    }
}

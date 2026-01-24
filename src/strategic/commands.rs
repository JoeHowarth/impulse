//! Strategic mode command types.
//!
//! Commands posted by input systems, consumed by simulation systems.
//! Decouples input handling from game state mutation for programmatic control.

use bevy::ecs::message::Message;
use bevy::prelude::*;

/// Commands posted by input systems, consumed by simulation systems.
/// Decouples input handling from game state mutation for programmatic control.
#[derive(Message, Debug, Clone)]
pub enum StrategicCommand {
    /// Select a fleet (sets SelectedFleet resource)
    SelectFleet(Entity),
    /// Deselect current fleet
    DeselectFleet,

    /// Plan a transfer leg for a fleet
    PlanTransfer {
        fleet: Entity,
        target: Entity,
        /// Absolute departure day
        departure_day: i32,
        /// Time of flight in days
        tof_days: i32,
    },
    /// Commit all uncommitted legs of a fleet's plan
    CommitPlan(Entity),
    /// Cancel the last leg of a fleet's plan
    CancelLeg(Entity),

    /// Split a fleet in half (fleet must be at a body with >1 ship)
    SplitFleet(Entity),
    /// Merge all other player fleets at same body into this fleet
    MergeFleets(Entity),

    /// Set simulation paused state
    SetPaused(bool),
    /// Set simulation time scale (sim seconds per real second)
    SetTimeScale(f64),
}

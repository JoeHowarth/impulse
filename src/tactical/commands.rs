//! Tactical mode command types.
//!
//! Commands posted by input systems, consumed by simulation systems.
//! Decouples input handling from game state mutation.

use bevy::ecs::message::Message;
use bevy::math::DVec3;
use bevy::prelude::*;

/// Commands posted by input systems, consumed by simulation systems.
/// Decouples input handling from game state mutation.
#[derive(Message, Debug, Clone)]
pub enum TacticalCommand {
    /// Move ships to destination (arena-local coordinates in meters)
    MoveShips {
        ships: Vec<Entity>,
        destination: DVec3,
    },
    /// Set attack target for ships (auto-fire when in range)
    AttackTarget { ships: Vec<Entity>, target: Entity },
    /// Clear attack target from ships
    ClearAttackTarget { ships: Vec<Entity> },
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

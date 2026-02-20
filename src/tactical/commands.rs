//! Tactical mode command types.
//!
//! Commands posted by input systems, consumed by simulation systems.
//! Decouples input handling from game state mutation.

use bevy::ecs::message::Message;
use bevy::math::DVec3;
use bevy::prelude::*;

use super::Urgency;

/// Commands posted by input systems, consumed by simulation systems.
/// Decouples input handling from game state mutation.
#[derive(Message, Debug, Clone)]
pub enum TacticalCommand {
    /// Move ships to destination (arena-local coordinates in meters)
    /// Legacy command - still works for ships without RelativePosition
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

    // === New relational positioning commands ===

    /// Set flagship acceleration vector (direct thrust control).
    /// Flagship maintains this acceleration until changed or cleared.
    SetFlagshipAcceleration {
        flagship: Entity,
        /// Thrust direction (unit vector, arena-local)
        direction: DVec3,
        /// Thrust magnitude in m/s²
        magnitude: f64,
    },
    /// Clear flagship acceleration (stop thrusting, coast on current velocity)
    ClearAcceleration { flagship: Entity },
    /// Set escort's relative position to flagship.
    /// Position is polar: angle from threat axis + radius from flagship.
    SetEscortPosition {
        ship: Entity,
        /// Angle in radians from threat axis (0 = toward enemy)
        angle: f64,
        /// Distance from flagship in meters
        radius: f64,
        /// How aggressively to maintain position
        urgency: Urgency,
    },
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

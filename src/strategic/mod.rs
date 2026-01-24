//! Strategic mode module.
//!
//! Contains all systems, UI, and rendering for strategic (solar system) mode.

pub mod commands;
pub mod input;
pub mod rendering;
pub mod systems;
pub mod transfer_lut;
pub mod transfer_vis;
pub mod ui;

// Re-export commonly used types
pub use commands::StrategicCommand;
pub use rendering::{
    FLEET_ENEMY_SELECTED, FLEET_ENEMY_UNSELECTED, FLEET_PLAYER_SELECTED, FLEET_PLAYER_UNSELECTED,
    FleetShape, ObjectiveRing, PlanMarker,
};
pub use transfer_lut::TransferLut;
pub use transfer_vis::{HoveredTransferArc, Transfer, TransferArcType};
pub use ui::{FleetKeyState, TransferPopup};

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
pub use ui::{FleetKeyState, TransferPopup};

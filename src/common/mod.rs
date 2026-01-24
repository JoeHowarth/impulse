//! Common systems that run in both strategic and tactical modes.
//!
//! This module contains:
//! - Simulation time management
//! - Body position and visibility updates
//! - Shared UI (HUD, labels, victory overlay)

pub mod rendering;
pub mod simulation;
pub mod ui;

pub use rendering::*;
pub use simulation::*;

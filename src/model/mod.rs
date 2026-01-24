//! Pure data types and computation.
//!
//! This module contains types and functions that don't depend on Bevy systems.
//! Types may have Bevy derives (Component, Resource) but no system logic.

pub mod fleet;
pub mod orbital;
pub mod transfer;

pub use fleet::*;
pub use orbital::*;
pub use transfer::*;

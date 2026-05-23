//! Shared GameEase domain logic and backend contracts.

pub mod config;
pub mod gamepad;
pub mod traits;

pub use traits::{InputBackend, OverlayBackend, SystemBackend, VolumeSnapshot, WindowEntry};

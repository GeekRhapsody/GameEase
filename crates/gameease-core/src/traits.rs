use anyhow::{bail, Result};

use crate::config::AppConfig;
use crate::gamepad::{KeyCode, MouseButton};

/// Snapshot of the system output volume.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct VolumeSnapshot {
    /// Volume percentage in the inclusive range 0-100.
    pub volume: u8,
    /// Whether the default output device is muted.
    pub muted: bool,
}

/// A top-level application window that can be focused or terminated.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WindowEntry {
    /// Backend-specific stable window identifier.
    pub id: String,
    /// User-visible window title.
    pub title: String,
    /// Process identifier that owns the window when available.
    pub process_id: Option<u32>,
}

/// Platform overlay operations needed by GameEase core flows.
pub trait OverlayBackend {
    /// Returns a stable backend name for diagnostics and capability checks.
    fn name(&self) -> &'static str;

    /// Returns whether this backend can present above normal application windows.
    fn supports_overlay(&self) -> bool;
}

/// Platform input injection operations used by GameEase.
pub trait InputBackend: Send {
    /// Sends a key press event.
    fn press_key(&self, key: KeyCode) -> Result<()>;

    /// Sends a key release event.
    fn release_key(&self, key: KeyCode) -> Result<()>;

    /// Sends a short key tap.
    fn tap_key(&self, key: KeyCode) -> Result<()>;

    /// Moves the pointer by a relative delta.
    fn move_pointer_relative(&self, dx: i32, dy: i32) -> Result<()>;

    /// Sends a mouse wheel delta.
    fn scroll(&self, dy: i32) -> Result<()>;

    /// Holds a mouse button down.
    fn button_down(&self, button: MouseButton) -> Result<()>;

    /// Releases a mouse button.
    fn button_up(&self, button: MouseButton) -> Result<()>;

    /// Sends a short mouse button click.
    fn click(&self, button: MouseButton) -> Result<()>;
}

/// Platform system services used by GameEase.
pub trait SystemBackend {
    /// Loads GameEase configuration.
    fn load_config(&self) -> AppConfig;

    /// Saves GameEase configuration.
    fn save_config(&self, config: &AppConfig) -> Result<()>;

    /// Reads the default output volume.
    fn get_volume(&self) -> Result<VolumeSnapshot> {
        bail!("volume control is not supported by this backend")
    }

    /// Sets the default output volume in the inclusive range 0-100.
    fn set_volume(&self, _pct: u8) -> Result<()> {
        bail!("volume control is not supported by this backend")
    }

    /// Toggles mute for the default output device.
    fn toggle_mute(&self) -> Result<()> {
        bail!("volume control is not supported by this backend")
    }

    /// Lists top-level application windows.
    fn list_windows(&self) -> Result<Vec<WindowEntry>> {
        bail!("window enumeration is not supported by this backend")
    }

    /// Requests focus for a top-level application window.
    fn focus_window(&self, _window_id: &str) -> Result<()> {
        bail!("window focusing is not supported by this backend")
    }

    /// Terminates the process that owns a top-level application window.
    fn terminate_window(&self, _window_id: &str) -> Result<()> {
        bail!("window termination is not supported by this backend")
    }
}

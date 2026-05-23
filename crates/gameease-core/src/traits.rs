use anyhow::Result;
use evdev::Key;

use crate::config::AppConfig;
use crate::gamepad::MouseButton;

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
    fn press_key(&self, key: Key) -> Result<()>;

    /// Sends a key release event.
    fn release_key(&self, key: Key) -> Result<()>;

    /// Sends a short key tap.
    fn tap_key(&self, key: Key) -> Result<()>;

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
}

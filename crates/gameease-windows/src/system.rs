use anyhow::Result;
use gameease_core::config::{self, AppConfig};
use gameease_core::{SystemBackend, VolumeSnapshot, WindowEntry};

use crate::audio::WindowsAudioBackend;
use crate::windows::WindowsTaskBackend;

/// Windows system backend for configuration, volume, and task operations.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsSystemBackend {
    audio: WindowsAudioBackend,
    tasks: WindowsTaskBackend,
}

impl WindowsSystemBackend {
    /// Creates a Windows system backend.
    pub fn new() -> Self {
        Self::default()
    }
}

impl SystemBackend for WindowsSystemBackend {
    fn load_config(&self) -> AppConfig {
        config::load_config_or_default()
    }

    fn save_config(&self, config: &AppConfig) -> Result<()> {
        config::save_config(config)
    }

    fn get_volume(&self) -> Result<VolumeSnapshot> {
        self.audio.get_volume()
    }

    fn set_volume(&self, pct: u8) -> Result<()> {
        self.audio.set_volume(pct)
    }

    fn toggle_mute(&self) -> Result<()> {
        self.audio.toggle_mute()
    }

    fn list_windows(&self) -> Result<Vec<WindowEntry>> {
        self.tasks.list_windows()
    }

    fn focus_window(&self, window_id: &str) -> Result<()> {
        self.tasks.focus_window(window_id)
    }

    fn terminate_window(&self, window_id: &str) -> Result<()> {
        self.tasks.terminate_window(window_id)
    }
}

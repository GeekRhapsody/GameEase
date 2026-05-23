use anyhow::Result;
use gameease_core::config::{self, AppConfig};
use gameease_core::SystemBackend;

/// Linux system service adapter for configuration persistence.
pub struct LinuxSystemBackend;

impl SystemBackend for LinuxSystemBackend {
    fn load_config(&self) -> AppConfig {
        config::load_config_or_default()
    }

    fn save_config(&self, config: &AppConfig) -> Result<()> {
        config::save_config(config)
    }
}

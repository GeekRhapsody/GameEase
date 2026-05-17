use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Default Desktop Mode mouse sensitivity multiplier.
pub const DEFAULT_DESKTOP_MOUSE_SENSITIVITY: f32 = 1.0;
/// Minimum Desktop Mode mouse sensitivity multiplier.
pub const MIN_DESKTOP_MOUSE_SENSITIVITY: f32 = 0.25;
/// Maximum Desktop Mode mouse sensitivity multiplier.
pub const MAX_DESKTOP_MOUSE_SENSITIVITY: f32 = 3.0;
/// Desktop Mode mouse sensitivity slider increment.
pub const DESKTOP_MOUSE_SENSITIVITY_STEP: f32 = 0.05;

/// Persisted GameEase configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    /// Desktop Mode settings.
    pub desktop_mode: DesktopModeConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            desktop_mode: DesktopModeConfig::default(),
        }
    }
}

/// Persisted Desktop Mode configuration.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct DesktopModeConfig {
    /// Mouse movement multiplier applied to right-stick pointer movement.
    pub mouse_sensitivity: f32,
}

impl Default for DesktopModeConfig {
    fn default() -> Self {
        Self {
            mouse_sensitivity: DEFAULT_DESKTOP_MOUSE_SENSITIVITY,
        }
    }
}

/// Loads the persisted GameEase configuration, returning defaults if it cannot be read.
pub fn load_config_or_default() -> AppConfig {
    match load_config() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Failed to load GameEase configuration: {error:#}");
            let config = AppConfig::default().sanitized();
            if let Err(save_error) = save_config(&config) {
                eprintln!("Failed to write default GameEase configuration: {save_error:#}");
            }
            config
        }
    }
}

/// Loads the persisted GameEase configuration from `~/.gameease/config.json`.
pub fn load_config() -> Result<AppConfig> {
    let path = config_path()?;
    if !path.exists() {
        let config = AppConfig::default().sanitized();
        save_config(&config)?;
        return Ok(config);
    }

    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let config = serde_json::from_str::<AppConfig>(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?
        .sanitized();
    Ok(config)
}

/// Saves the GameEase configuration to `~/.gameease/config.json`.
pub fn save_config(config: &AppConfig) -> Result<()> {
    let path = config_path()?;
    let Some(parent) = path.parent() else {
        anyhow::bail!("configuration path has no parent directory");
    };
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;

    let content = serde_json::to_string_pretty(&config.clone().sanitized())
        .context("failed to serialize GameEase configuration")?;
    fs::write(&path, format!("{content}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Returns the GameEase configuration file path.
pub fn config_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".gameease").join("config.json"))
}

impl AppConfig {
    fn sanitized(mut self) -> Self {
        self.desktop_mode.mouse_sensitivity =
            sanitize_desktop_mouse_sensitivity(self.desktop_mode.mouse_sensitivity);
        self
    }
}

/// Clamps a Desktop Mode mouse sensitivity value to the supported range.
pub fn sanitize_desktop_mouse_sensitivity(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_DESKTOP_MOUSE_SENSITIVITY, MAX_DESKTOP_MOUSE_SENSITIVITY)
    } else {
        DEFAULT_DESKTOP_MOUSE_SENSITIVITY
    }
}

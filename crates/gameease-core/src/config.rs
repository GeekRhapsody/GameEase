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
/// Default non-OSK UI scale.
pub const DEFAULT_UI_SCALE: f32 = 1.0;
/// Minimum non-OSK UI scale.
pub const MIN_UI_SCALE: f32 = 0.75;
/// Maximum non-OSK UI scale.
pub const MAX_UI_SCALE: f32 = 1.5;
/// UI scale slider increment.
pub const UI_SCALE_STEP: f32 = 0.05;
/// Default on-screen keyboard UI scale.
pub const DEFAULT_OSK_SCALE: f32 = 1.0;
/// Minimum on-screen keyboard UI scale.
pub const MIN_OSK_SCALE: f32 = 0.7;
/// Maximum on-screen keyboard UI scale.
pub const MAX_OSK_SCALE: f32 = 1.4;
/// On-screen keyboard scale slider increment.
pub const OSK_SCALE_STEP: f32 = 0.05;

/// Persisted GameEase configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    /// General UI settings.
    #[serde(default)]
    pub general: GeneralConfig,
    /// On-screen keyboard settings.
    #[serde(default)]
    pub on_screen_keyboard: OnScreenKeyboardConfig,
    /// Desktop Mode settings.
    #[serde(default)]
    pub desktop_mode: DesktopModeConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            on_screen_keyboard: OnScreenKeyboardConfig::default(),
            desktop_mode: DesktopModeConfig::default(),
        }
    }
}

/// Persisted general UI configuration.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct GeneralConfig {
    /// Scale multiplier applied to GameEase UI outside the OSK.
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            ui_scale: DEFAULT_UI_SCALE,
        }
    }
}

/// Persisted on-screen keyboard configuration.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct OnScreenKeyboardConfig {
    /// Scale multiplier applied to the OSK only.
    #[serde(default = "default_osk_scale")]
    pub scale: f32,
}

impl Default for OnScreenKeyboardConfig {
    fn default() -> Self {
        Self {
            scale: DEFAULT_OSK_SCALE,
        }
    }
}

/// Persisted Desktop Mode configuration.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct DesktopModeConfig {
    /// Mouse movement multiplier applied to right-stick pointer movement.
    #[serde(default = "default_desktop_mouse_sensitivity")]
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
        self.general.ui_scale = sanitize_ui_scale(self.general.ui_scale);
        self.on_screen_keyboard.scale = sanitize_osk_scale(self.on_screen_keyboard.scale);
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

/// Clamps a non-OSK UI scale value to the supported range.
pub fn sanitize_ui_scale(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_UI_SCALE, MAX_UI_SCALE)
    } else {
        DEFAULT_UI_SCALE
    }
}

/// Clamps an on-screen keyboard scale value to the supported range.
pub fn sanitize_osk_scale(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_OSK_SCALE, MAX_OSK_SCALE)
    } else {
        DEFAULT_OSK_SCALE
    }
}

fn default_ui_scale() -> f32 {
    DEFAULT_UI_SCALE
}

fn default_osk_scale() -> f32 {
    DEFAULT_OSK_SCALE
}

fn default_desktop_mouse_sensitivity() -> f32 {
    DEFAULT_DESKTOP_MOUSE_SENSITIVITY
}

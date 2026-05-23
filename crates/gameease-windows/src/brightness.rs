use anyhow::Result;

/// Windows display brightness backend placeholder.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsBrightnessBackend;

impl WindowsBrightnessBackend {
    /// Creates the Windows brightness backend.
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

use anyhow::Result;

/// Windows Wi-Fi backend placeholder for future Native Wi-Fi API integration.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsWifiBackend;

impl WindowsWifiBackend {
    /// Creates the Windows Wi-Fi backend.
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

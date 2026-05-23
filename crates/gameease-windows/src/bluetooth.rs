use anyhow::Result;

/// Windows Bluetooth backend placeholder for future Bluetooth API integration.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsBluetoothBackend;

impl WindowsBluetoothBackend {
    /// Creates the Windows Bluetooth backend.
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

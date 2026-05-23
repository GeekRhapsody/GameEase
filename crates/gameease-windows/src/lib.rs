//! Windows platform backend implementations for GameEase.

mod audio;
mod bluetooth;
mod brightness;
mod gamepad;
mod input;
mod overlay;
mod system;
mod wifi;
mod windows;

pub use audio::WindowsAudioBackend;
pub use bluetooth::WindowsBluetoothBackend;
pub use brightness::WindowsBrightnessBackend;
pub use gamepad::WindowsGamepadBackend;
pub use input::WindowsInputBackend;
pub use overlay::WindowsOverlayBackend;
pub use system::WindowsSystemBackend;
pub use wifi::WindowsWifiBackend;
pub use windows::WindowsTaskBackend;

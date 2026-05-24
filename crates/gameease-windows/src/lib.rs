//! Windows platform backend implementations for GameEase.

mod audio;
mod bluetooth;
mod brightness;
mod gamepad;
mod input;
mod overlay;
mod sidemenu;
mod system;
mod tray;
mod wifi;
mod windows;

pub use audio::WindowsAudioBackend;
pub use bluetooth::WindowsBluetoothBackend;
pub use brightness::WindowsBrightnessBackend;
pub use gamepad::WindowsGamepadBackend;
pub use input::WindowsInputBackend;
pub use overlay::WindowsOverlayBackend;
pub use sidemenu::{WindowsSideMenu, WindowsSideMenuHandle};
pub use system::WindowsSystemBackend;
pub use tray::{WindowsTrayIcon, WindowsTrayNotifier};
pub use wifi::WindowsWifiBackend;
pub use windows::WindowsTaskBackend;

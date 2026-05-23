//! Windows platform backend implementations for GameEase.

mod audio;
mod input;
mod overlay;
mod system;
mod windows;

pub use audio::WindowsAudioBackend;
pub use input::WindowsInputBackend;
pub use overlay::WindowsOverlayBackend;
pub use system::WindowsSystemBackend;
pub use windows::WindowsTaskBackend;

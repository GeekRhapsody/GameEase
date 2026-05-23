use anyhow::Result;
use gameease_core::VolumeSnapshot;

/// Windows system volume backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsAudioBackend;

#[cfg(windows)]
mod imp {
    use super::*;
    use anyhow::Context;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };

    impl WindowsAudioBackend {
        /// Reads the default output endpoint volume.
        pub fn get_volume(&self) -> Result<VolumeSnapshot> {
            let endpoint = default_endpoint_volume()?;
            let scalar = unsafe { endpoint.GetMasterVolumeLevelScalar() }
                .context("failed to read master volume")?;
            let muted = unsafe { endpoint.GetMute() }.context("failed to read mute state")?;

            Ok(VolumeSnapshot {
                volume: (scalar * 100.0).round().clamp(0.0, 100.0) as u8,
                muted: muted.as_bool(),
            })
        }

        /// Sets the default output endpoint volume, clamped to 0-100.
        pub fn set_volume(&self, pct: u8) -> Result<()> {
            let endpoint = default_endpoint_volume()?;
            let scalar = f32::from(pct.min(100)) / 100.0;
            unsafe { endpoint.SetMasterVolumeLevelScalar(scalar, std::ptr::null()) }
                .context("failed to set master volume")?;
            Ok(())
        }

        /// Toggles mute on the default output endpoint.
        pub fn toggle_mute(&self) -> Result<()> {
            let endpoint = default_endpoint_volume()?;
            let muted = unsafe { endpoint.GetMute() }.context("failed to read mute state")?;
            unsafe { endpoint.SetMute(!muted.as_bool(), std::ptr::null()) }
                .context("failed to set mute state")?;
            Ok(())
        }
    }

    fn default_endpoint_volume() -> Result<IAudioEndpointVolume> {
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .context("failed to create MMDeviceEnumerator")?;
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .context("failed to get default output endpoint")?;
        let endpoint = unsafe { device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) }
            .context("failed to activate IAudioEndpointVolume")?;
        Ok(endpoint)
    }
}

#[cfg(not(windows))]
impl WindowsAudioBackend {
    /// Reads the default output endpoint volume.
    pub fn get_volume(&self) -> Result<VolumeSnapshot> {
        anyhow::bail!("Windows audio backend is only available on Windows")
    }

    /// Sets the default output endpoint volume.
    pub fn set_volume(&self, _pct: u8) -> Result<()> {
        anyhow::bail!("Windows audio backend is only available on Windows")
    }

    /// Toggles mute on the default output endpoint.
    pub fn toggle_mute(&self) -> Result<()> {
        anyhow::bail!("Windows audio backend is only available on Windows")
    }
}

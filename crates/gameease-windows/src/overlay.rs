use anyhow::Result;
use gameease_core::OverlayBackend;

/// Windows topmost layered overlay backend.
#[derive(Debug, Clone, Copy)]
pub struct WindowsOverlayBackend {
    #[cfg(windows)]
    hwnd: windows::Win32::Foundation::HWND,
}

#[cfg(windows)]
mod imp {
    use super::*;
    use anyhow::{anyhow, Context};
    use std::ptr::null_mut;
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::HBRUSH;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, RegisterClassW, SetLayeredWindowAttributes, ShowWindow,
        CW_USEDEFAULT, LWA_ALPHA, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WNDCLASSW, WS_EX_LAYERED,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    impl WindowsOverlayBackend {
        /// Creates a borderless topmost layered overlay window.
        pub fn new(width: i32, height: i32) -> Result<Self> {
            let hmodule =
                unsafe { GetModuleHandleW(None) }.context("failed to get module handle")?;
            let hinstance = HINSTANCE(hmodule.0);
            let class_name = w!("GameEaseOverlayWindow");
            let window_class = WNDCLASSW {
                hInstance: hinstance,
                lpszClassName: class_name,
                lpfnWndProc: Some(window_proc),
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };

            let atom = unsafe { RegisterClassW(&window_class) };
            if atom == 0 {
                return Err(anyhow!("failed to register GameEase overlay window class"));
            }

            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0),
                    class_name,
                    w!("GameEase"),
                    WS_POPUP,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    width,
                    height,
                    None,
                    None,
                    Some(hinstance),
                    Some(null_mut()),
                )
            }
            .context("failed to create GameEase overlay window")?;

            if hwnd.0.is_null() {
                return Err(anyhow!("failed to create GameEase overlay window"));
            }

            unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA) }
                .context("failed to configure layered overlay opacity")?;
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };

            Ok(Self { hwnd })
        }

        /// Returns the native Win32 window handle.
        pub fn hwnd(&self) -> HWND {
            self.hwnd
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }
}

#[cfg(not(windows))]
impl WindowsOverlayBackend {
    /// Creates a Windows overlay backend.
    pub fn new(_width: i32, _height: i32) -> Result<Self> {
        anyhow::bail!("Windows overlay backend is only available on Windows")
    }
}

impl OverlayBackend for WindowsOverlayBackend {
    fn name(&self) -> &'static str {
        "win32-layered-window"
    }

    fn supports_overlay(&self) -> bool {
        cfg!(windows)
    }
}

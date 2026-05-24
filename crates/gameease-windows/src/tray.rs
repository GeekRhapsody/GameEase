use anyhow::Result;

/// Windows notification-area icon for the lifetime of the process.
#[derive(Debug)]
pub struct WindowsTrayIcon {
    #[cfg(windows)]
    hwnd: windows::Win32::Foundation::HWND,
    #[cfg(windows)]
    id: u32,
}

/// Lightweight handle that can show tray notifications from worker threads.
#[derive(Debug, Clone, Copy)]
pub struct WindowsTrayNotifier {
    hwnd: isize,
    id: u32,
}

#[cfg(windows)]
mod imp {
    use super::*;
    use anyhow::{anyhow, Context};
    use std::ptr::null_mut;
    use windows::core::w;
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD,
        NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
        DispatchMessageW, GetCursorPos, LoadIconW, PostQuitMessage, RegisterClassW,
        SetForegroundWindow, TrackPopupMenu, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
        CW_USEDEFAULT, IDI_APPLICATION, MF_DISABLED, MF_SEPARATOR, MF_STRING, MSG, TPM_LEFTALIGN,
        TPM_RIGHTBUTTON, TPM_TOPALIGN, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_CONTEXTMENU,
        WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP, WM_USER, WNDCLASSW,
    };

    const TRAY_ID: u32 = 1;
    const TRAY_CALLBACK: u32 = WM_USER + 1;
    const MENU_QUIT: usize = 1001;

    impl WindowsTrayIcon {
        /// Creates the hidden window and adds a GameEase icon to the Windows tray.
        pub fn new() -> Result<Self> {
            let hwnd = create_hidden_window()?;
            let mut data = notify_icon_data(hwnd, TRAY_ID);
            data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
            data.uCallbackMessage = TRAY_CALLBACK;
            data.hIcon = unsafe { LoadIconW(None, IDI_APPLICATION) }
                .context("failed to load default tray icon")?;
            copy_utf16("GameEase is running", &mut data.szTip);

            if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
                let _ = unsafe { DestroyWindow(hwnd) };
                return Err(anyhow!("failed to add GameEase tray icon"));
            }

            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) };

            Ok(Self { hwnd, id: TRAY_ID })
        }

        /// Returns a thread-friendly notification handle for this tray icon.
        pub fn notifier(&self) -> WindowsTrayNotifier {
            WindowsTrayNotifier {
                hwnd: self.hwnd.0 as isize,
                id: self.id,
            }
        }
    }

    impl Drop for WindowsTrayIcon {
        fn drop(&mut self) {
            let data = notify_icon_data(self.hwnd, self.id);
            let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }

    impl WindowsTrayNotifier {
        /// Shows a short notification balloon from the GameEase tray icon.
        pub fn notify(&self, title: &str, message: &str) -> Result<()> {
            let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
            let mut data = notify_icon_data(hwnd, self.id);
            data.uFlags = NIF_INFO;
            copy_utf16(title, &mut data.szInfoTitle);
            copy_utf16(message, &mut data.szInfo);

            if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
                Ok(())
            } else {
                Err(anyhow!("failed to update GameEase tray notification"))
            }
        }
    }

    fn create_hidden_window() -> Result<HWND> {
        let hmodule = unsafe { GetModuleHandleW(None) }.context("failed to get module handle")?;
        let hinstance = HINSTANCE(hmodule.0);
        let class_name = w!("GameEaseTrayWindow");
        let window_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            hInstance: hinstance,
            lpszClassName: class_name,
            lpfnWndProc: Some(window_proc),
            ..Default::default()
        };

        let atom = unsafe { RegisterClassW(&window_class) };
        if atom == 0 {
            return Err(anyhow!("failed to register GameEase tray window class"));
        }

        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                w!("GameEase"),
                WINDOW_STYLE::default(),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                0,
                0,
                None,
                None,
                Some(hinstance),
                Some(null_mut()),
            )
        }
        .context("failed to create GameEase tray window")
    }

    fn notify_icon_data(hwnd: HWND, id: u32) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: id,
            ..Default::default()
        }
    }

    fn copy_utf16(source: &str, destination: &mut [u16]) {
        let mut encoded = source.encode_utf16();
        let max_units = destination.len().saturating_sub(1);
        for slot in destination.iter_mut().take(max_units) {
            match encoded.next() {
                Some(unit) => *slot = unit,
                None => break,
            }
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            TRAY_CALLBACK => {
                let event = lparam.0 as u32;
                if matches!(event, WM_CONTEXTMENU | WM_RBUTTONUP | WM_LBUTTONUP) {
                    show_context_menu(hwnd);
                    return LRESULT(0);
                }
            }
            WM_COMMAND => {
                if (wparam.0 & 0xffff) == MENU_QUIT {
                    unsafe { PostQuitMessage(0) };
                    return LRESULT(0);
                }
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                return LRESULT(0);
            }
            _ => {}
        }

        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    fn show_context_menu(hwnd: HWND) {
        let menu = match unsafe { CreatePopupMenu() } {
            Ok(menu) => menu,
            Err(_) => return,
        };

        let _ = unsafe { AppendMenuW(menu, MF_STRING | MF_DISABLED, 0, w!("GameEase is running")) };
        let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, None) };
        let _ = unsafe { AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit")) };

        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_err() {
            point.x = 0;
            point.y = 0;
        }

        let _ = unsafe { SetForegroundWindow(hwnd) };
        let _ = unsafe {
            TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                None,
                hwnd,
                None,
            )
        };
        let _ = unsafe { DestroyMenu(menu) };

        let mut message = MSG::default();
        while unsafe {
            windows::Win32::UI::WindowsAndMessaging::PeekMessageW(
                &mut message,
                Some(hwnd),
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::PM_REMOVE,
            )
        }
        .as_bool()
        {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

#[cfg(not(windows))]
impl WindowsTrayIcon {
    /// Creates the Windows tray icon.
    pub fn new() -> Result<Self> {
        anyhow::bail!("Windows tray icon is only available on Windows")
    }

    /// Returns a thread-friendly notification handle for this tray icon.
    pub fn notifier(&self) -> WindowsTrayNotifier {
        WindowsTrayNotifier { hwnd: 0, id: 0 }
    }
}

#[cfg(not(windows))]
impl WindowsTrayNotifier {
    /// Shows a short notification balloon from the GameEase tray icon.
    pub fn notify(&self, _title: &str, _message: &str) -> Result<()> {
        anyhow::bail!("Windows tray notifications are only available on Windows")
    }
}

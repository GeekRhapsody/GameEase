use anyhow::Result;
use gameease_core::WindowEntry;

/// Windows task/window backend based on `EnumWindows`.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsTaskBackend;

#[cfg(windows)]
mod imp {
    use super::*;
    use anyhow::{anyhow, Context};
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        IsWindowVisible, SetForegroundWindow,
    };

    impl WindowsTaskBackend {
        /// Lists visible top-level application windows.
        pub fn list_windows(&self) -> Result<Vec<WindowEntry>> {
            let mut windows = Vec::<WindowEntry>::new();
            unsafe {
                EnumWindows(
                    Some(enum_window_proc),
                    LPARAM(&mut windows as *mut _ as isize),
                )
            }
            .context("EnumWindows failed")?;
            Ok(windows)
        }

        /// Requests foreground focus for the given window id.
        pub fn focus_window(&self, window_id: &str) -> Result<()> {
            let hwnd = parse_hwnd(window_id)?;
            if !unsafe { SetForegroundWindow(hwnd) }.as_bool() {
                return Err(anyhow!("failed to focus window"));
            }
            Ok(())
        }

        /// Terminates the process that owns the given window id.
        pub fn terminate_window(&self, window_id: &str) -> Result<()> {
            let hwnd = parse_hwnd(window_id)?;
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };

            if pid == 0 {
                return Err(anyhow!("window has no owning process id"));
            }

            let process = unsafe {
                OpenProcess(
                    PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                    false,
                    pid,
                )
            }
            .with_context(|| format!("failed to open process {pid}"))?;
            unsafe { TerminateProcess(process, 1) }
                .with_context(|| format!("failed to terminate process {pid}"))?;
            unsafe { CloseHandle(process) }.ok();
            Ok(())
        }
    }

    unsafe extern "system" fn enum_window_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let windows = &mut *(lparam.0 as *mut Vec<WindowEntry>);

        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return true.into();
        }

        let title = match window_title(hwnd) {
            Some(title) if !title.trim().is_empty() => title,
            _ => return true.into(),
        };

        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        windows.push(WindowEntry {
            id: format!("{:#x}", hwnd.0 as usize),
            title,
            process_id: (pid != 0).then_some(pid),
        });

        true.into()
    }

    fn window_title(hwnd: HWND) -> Option<String> {
        let len = unsafe { GetWindowTextLengthW(hwnd) };
        if len <= 0 {
            return None;
        }

        let mut buffer = vec![0u16; len as usize + 1];
        let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
        if copied <= 0 {
            return None;
        }

        Some(
            OsString::from_wide(&buffer[..copied as usize])
                .to_string_lossy()
                .into_owned(),
        )
    }

    fn parse_hwnd(window_id: &str) -> Result<HWND> {
        let trimmed = window_id.trim_start_matches("0x");
        let value = usize::from_str_radix(trimmed, 16)
            .with_context(|| format!("invalid Win32 window id {window_id:?}"))?;
        Ok(HWND(value as *mut core::ffi::c_void))
    }
}

#[cfg(not(windows))]
impl WindowsTaskBackend {
    /// Lists visible top-level application windows.
    pub fn list_windows(&self) -> Result<Vec<WindowEntry>> {
        anyhow::bail!("Windows task backend is only available on Windows")
    }

    /// Requests foreground focus for the given window id.
    pub fn focus_window(&self, _window_id: &str) -> Result<()> {
        anyhow::bail!("Windows task backend is only available on Windows")
    }

    /// Terminates the process that owns the given window id.
    pub fn terminate_window(&self, _window_id: &str) -> Result<()> {
        anyhow::bail!("Windows task backend is only available on Windows")
    }
}

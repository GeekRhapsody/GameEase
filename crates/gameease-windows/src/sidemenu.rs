use anyhow::Result;
use gameease_core::gamepad::{GamepadGrabCommand, KeyboardDirection, SideMenuCommand};
use std::sync::mpsc::Sender;

/// Native Windows side-menu window.
#[derive(Debug)]
pub struct WindowsSideMenu {
    #[cfg(windows)]
    hwnd: windows::Win32::Foundation::HWND,
}

/// Thread-friendly handle for posting gamepad side-menu commands to the UI thread.
#[derive(Debug, Clone, Copy)]
pub struct WindowsSideMenuHandle {
    hwnd: isize,
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::WindowsSystemBackend;
    use anyhow::{anyhow, Context};
    use gameease_core::config::{
        self, AppConfig, DESKTOP_MOUSE_SENSITIVITY_STEP, MAX_DESKTOP_MOUSE_SENSITIVITY,
        MAX_OSK_SCALE, MAX_UI_SCALE, MIN_DESKTOP_MOUSE_SENSITIVITY, MIN_OSK_SCALE, MIN_UI_SCALE,
        OSK_SCALE_STEP, UI_SCALE_STEP,
    };
    use gameease_core::{SystemBackend, VolumeSnapshot, WindowEntry};
    use std::ptr::null_mut;
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, InvalidateRect,
        SetBkMode, SetTextColor, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
        HBRUSH, HGDIOBJ, PAINTSTRUCT, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, GetWindowLongPtrW,
        PostMessageW, PostQuitMessage, RegisterClassW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, HWND_TOPMOST,
        SM_CYSCREEN, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, WINDOW_EX_STYLE, WM_APP,
        WM_ERASEBKGND, WM_LBUTTONDOWN, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WNDCLASSW,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    const SIDE_WIDTH: i32 = 320;
    const PANEL_WIDTH: i32 = 380;
    const PADDING: i32 = 18;
    const HEADER_HEIGHT: i32 = 54;
    const ROW_HEIGHT: i32 = 52;
    const CONFIG_ROW_HEIGHT: i32 = 58;
    const TASK_ROW_HEIGHT: i32 = 58;
    const MAX_VOLUME: u8 = 100;
    const VOLUME_STEP: u8 = 5;

    const WM_SIDE_MENU_COMMAND: u32 = WM_APP + 20;
    const CMD_TOGGLE: usize = 1;
    const CMD_CLOSE: usize = 2;
    const CMD_MOVE: usize = 3;
    const CMD_ACTIVATE: usize = 4;
    const CMD_SCAN: usize = 5;
    const CMD_TERMINATE: usize = 6;

    impl WindowsSideMenu {
        /// Creates the hidden side-menu window.
        pub fn new(config_sender: Sender<GamepadGrabCommand>) -> Result<Self> {
            let state = Box::new(SideMenuState::new(config_sender));
            let hwnd = create_window(Box::into_raw(state))?;
            Ok(Self { hwnd })
        }

        /// Returns a thread-friendly handle for the side menu.
        pub fn handle(&self) -> WindowsSideMenuHandle {
            WindowsSideMenuHandle {
                hwnd: self.hwnd.0 as isize,
            }
        }
    }

    impl Drop for WindowsSideMenu {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }

    impl WindowsSideMenuHandle {
        /// Posts a side-menu command to the UI thread.
        pub fn post(&self, command: SideMenuCommand) -> Result<()> {
            let (code, direction) = encode_command(command);
            let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
            unsafe {
                PostMessageW(
                    Some(hwnd),
                    WM_SIDE_MENU_COMMAND,
                    WPARAM(code),
                    LPARAM(direction),
                )
            }
            .context("failed to post side-menu command")
        }
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum SideRow {
        Volume,
        Wifi,
        Tasks,
        Bluetooth,
        Configuration,
        Quit,
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Panel {
        None,
        Wifi,
        Tasks,
        Bluetooth,
        Configuration,
    }

    struct SideMenuState {
        hwnd: HWND,
        system: WindowsSystemBackend,
        config_sender: Sender<GamepadGrabCommand>,
        visible: bool,
        selected: usize,
        panel: Panel,
        volume: Option<VolumeSnapshot>,
        status: Option<String>,
        tasks: Vec<WindowEntry>,
        task_selected: usize,
        config: AppConfig,
        config_selected: usize,
    }

    impl SideMenuState {
        fn new(config_sender: Sender<GamepadGrabCommand>) -> Self {
            let system = WindowsSystemBackend::new();
            let config = system.load_config();
            let _ = config_sender.send(GamepadGrabCommand::SetDesktopMouseSensitivity(
                config.desktop_mode.mouse_sensitivity,
            ));
            Self {
                hwnd: HWND(null_mut()),
                system,
                config_sender,
                visible: false,
                selected: 0,
                panel: Panel::None,
                volume: None,
                status: None,
                tasks: Vec::new(),
                task_selected: 0,
                config,
                config_selected: 0,
            }
        }

        fn handle_command(&mut self, code: usize, direction: isize) {
            match code {
                CMD_TOGGLE => self.toggle(),
                CMD_CLOSE => self.close_or_cancel(),
                CMD_MOVE => self.move_selection(decode_direction(direction)),
                CMD_ACTIVATE => self.activate_selected(),
                CMD_SCAN => self.toggle_scan(),
                CMD_TERMINATE => self.terminate_selection(),
                _ => {}
            }
            self.invalidate();
        }

        fn toggle(&mut self) {
            if self.visible {
                self.hide();
            } else {
                self.show();
            }
        }

        fn show(&mut self) {
            self.visible = true;
            self.refresh_volume();
            self.status = None;
            self.resize_and_show();
        }

        fn hide(&mut self) {
            self.visible = false;
            self.panel = Panel::None;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }

        fn close_or_cancel(&mut self) {
            if !self.visible {
                return;
            }
            if self.panel != Panel::None {
                self.panel = Panel::None;
                self.resize_and_show();
            } else {
                self.hide();
            }
        }

        fn move_selection(&mut self, direction: KeyboardDirection) {
            if !self.visible {
                return;
            }

            match self.panel {
                Panel::Tasks => self.move_task_selection(direction),
                Panel::Configuration => self.move_config_selection(direction),
                Panel::None | Panel::Wifi | Panel::Bluetooth => match direction {
                    KeyboardDirection::Up => self.selected = self.selected.saturating_sub(1),
                    KeyboardDirection::Down => {
                        self.selected = (self.selected + 1).min(rows().len() - 1)
                    }
                    KeyboardDirection::Left | KeyboardDirection::Right => {
                        if self.selected_row() == SideRow::Volume {
                            self.adjust_volume(direction);
                        }
                    }
                },
            }
        }

        fn activate_selected(&mut self) {
            if !self.visible {
                return;
            }

            match self.panel {
                Panel::Tasks => {
                    if self.tasks.is_empty() {
                        self.refresh_tasks();
                        return;
                    }
                    let index = self.task_selected.min(self.tasks.len().saturating_sub(1));
                    let task = self.tasks[index].clone();
                    match self.system.focus_window(&task.id) {
                        Ok(()) => self.hide(),
                        Err(error) => self.status = Some(error.to_string()),
                    }
                }
                Panel::Configuration => {}
                Panel::Wifi | Panel::Bluetooth => {
                    self.panel = Panel::None;
                    self.resize_and_show();
                }
                Panel::None => match self.selected_row() {
                    SideRow::Volume => self.toggle_mute(),
                    SideRow::Wifi => {
                        self.panel = Panel::Wifi;
                        self.status =
                            Some("Wi-Fi control is not implemented on Windows yet.".into());
                        self.resize_and_show();
                    }
                    SideRow::Tasks => {
                        self.panel = Panel::Tasks;
                        self.refresh_tasks();
                        self.resize_and_show();
                    }
                    SideRow::Bluetooth => {
                        self.panel = Panel::Bluetooth;
                        self.status =
                            Some("Bluetooth control is not implemented on Windows yet.".into());
                        self.resize_and_show();
                    }
                    SideRow::Configuration => {
                        self.panel = Panel::Configuration;
                        self.resize_and_show();
                    }
                    SideRow::Quit => unsafe { PostQuitMessage(0) },
                },
            }
        }

        fn toggle_scan(&mut self) {
            if matches!(self.panel, Panel::Wifi | Panel::Bluetooth) {
                self.status = Some("Scanning is not implemented on Windows yet.".into());
            }
        }

        fn terminate_selection(&mut self) {
            if self.panel != Panel::Tasks || self.tasks.is_empty() {
                return;
            }

            let index = self.task_selected.min(self.tasks.len().saturating_sub(1));
            let task = self.tasks[index].clone();
            match self.system.terminate_window(&task.id) {
                Ok(()) => {
                    self.tasks.remove(index);
                    self.task_selected = self.task_selected.min(self.tasks.len().saturating_sub(1));
                    self.status = None;
                }
                Err(error) => self.status = Some(error.to_string()),
            }
        }

        fn selected_row(&self) -> SideRow {
            rows()[self.selected]
        }

        fn refresh_volume(&mut self) {
            match self.system.get_volume() {
                Ok(volume) => self.volume = Some(volume),
                Err(error) => self.status = Some(error.to_string()),
            }
        }

        fn adjust_volume(&mut self, direction: KeyboardDirection) {
            let Some(snapshot) = self.volume else {
                self.refresh_volume();
                return;
            };
            let next = match direction {
                KeyboardDirection::Left => snapshot.volume.saturating_sub(VOLUME_STEP),
                KeyboardDirection::Right => {
                    snapshot.volume.saturating_add(VOLUME_STEP).min(MAX_VOLUME)
                }
                KeyboardDirection::Up | KeyboardDirection::Down => snapshot.volume,
            };

            match self.system.set_volume(next) {
                Ok(()) => self.refresh_volume(),
                Err(error) => self.status = Some(error.to_string()),
            }
        }

        fn toggle_mute(&mut self) {
            match self.system.toggle_mute() {
                Ok(()) => self.refresh_volume(),
                Err(error) => self.status = Some(error.to_string()),
            }
        }

        fn refresh_tasks(&mut self) {
            match self.system.list_windows() {
                Ok(tasks) => {
                    self.tasks = tasks
                        .into_iter()
                        .filter(|task| {
                            let title = task.title.trim();
                            !title.is_empty() && !title.starts_with("GameEase")
                        })
                        .collect();
                    self.task_selected = self.task_selected.min(self.tasks.len().saturating_sub(1));
                    self.status = None;
                }
                Err(error) => {
                    self.tasks.clear();
                    self.status = Some(error.to_string());
                }
            }
        }

        fn move_task_selection(&mut self, direction: KeyboardDirection) {
            match direction {
                KeyboardDirection::Up => self.task_selected = self.task_selected.saturating_sub(1),
                KeyboardDirection::Down => {
                    self.task_selected =
                        (self.task_selected + 1).min(self.tasks.len().saturating_sub(1));
                }
                KeyboardDirection::Left | KeyboardDirection::Right => {}
            }
        }

        fn move_config_selection(&mut self, direction: KeyboardDirection) {
            match direction {
                KeyboardDirection::Up => {
                    self.config_selected = self.config_selected.saturating_sub(1)
                }
                KeyboardDirection::Down => self.config_selected = (self.config_selected + 1).min(2),
                KeyboardDirection::Left | KeyboardDirection::Right => self.adjust_config(direction),
            }
        }

        fn adjust_config(&mut self, direction: KeyboardDirection) {
            let sign = match direction {
                KeyboardDirection::Left => -1.0,
                KeyboardDirection::Right => 1.0,
                KeyboardDirection::Up | KeyboardDirection::Down => 0.0,
            };

            match self.config_selected {
                0 => {
                    let next = (self.config.desktop_mode.mouse_sensitivity
                        + DESKTOP_MOUSE_SENSITIVITY_STEP * sign)
                        .clamp(MIN_DESKTOP_MOUSE_SENSITIVITY, MAX_DESKTOP_MOUSE_SENSITIVITY);
                    self.config.desktop_mode.mouse_sensitivity = next;
                    let _ = self
                        .config_sender
                        .send(GamepadGrabCommand::SetDesktopMouseSensitivity(next));
                }
                1 => {
                    self.config.general.ui_scale = (self.config.general.ui_scale
                        + UI_SCALE_STEP * sign)
                        .clamp(MIN_UI_SCALE, MAX_UI_SCALE);
                }
                2 => {
                    self.config.on_screen_keyboard.scale = (self.config.on_screen_keyboard.scale
                        + OSK_SCALE_STEP * sign)
                        .clamp(MIN_OSK_SCALE, MAX_OSK_SCALE);
                }
                _ => {}
            }

            if let Err(error) = self.system.save_config(&self.config) {
                self.status = Some(error.to_string());
            }
            self.resize_and_show();
        }

        fn resize_and_show(&self) {
            let height = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(720);
            let width = self.active_width();
            let _ = unsafe {
                SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    width,
                    height,
                    SWP_SHOWWINDOW | SWP_NOACTIVATE,
                )
            };
        }

        fn active_width(&self) -> i32 {
            let scale = config::sanitize_ui_scale(self.config.general.ui_scale);
            let width = if self.panel == Panel::None {
                SIDE_WIDTH
            } else {
                SIDE_WIDTH + PANEL_WIDTH
            };
            ((width as f32) * scale).round() as i32
        }

        fn scale(&self) -> f32 {
            config::sanitize_ui_scale(self.config.general.ui_scale)
        }

        fn invalidate(&self) {
            let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
        }
    }

    fn rows() -> &'static [SideRow] {
        &[
            SideRow::Volume,
            SideRow::Wifi,
            SideRow::Tasks,
            SideRow::Bluetooth,
            SideRow::Configuration,
            SideRow::Quit,
        ]
    }

    fn create_window(state: *mut SideMenuState) -> Result<HWND> {
        let hmodule = unsafe { GetModuleHandleW(None) }.context("failed to get module handle")?;
        let hinstance = HINSTANCE(hmodule.0);
        let class_name = w!("GameEaseSideMenuWindow");
        let window_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            hInstance: hinstance,
            lpszClassName: class_name,
            lpfnWndProc: Some(window_proc),
            ..Default::default()
        };

        let atom = unsafe { RegisterClassW(&window_class) };
        if atom == 0 {
            unsafe {
                drop(Box::from_raw(state));
            }
            return Err(anyhow!(
                "failed to register GameEase side-menu window class"
            ));
        }

        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0),
                class_name,
                w!("GameEase Side Menu"),
                WS_POPUP,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                0,
                0,
                None,
                None,
                Some(hinstance),
                Some(state.cast()),
            )
        }
        .context("failed to create GameEase side-menu window")
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_NCCREATE {
            let create = lparam.0 as *const CREATESTRUCTW;
            let state = unsafe { (*create).lpCreateParams as *mut SideMenuState };
            unsafe {
                (*state).hwnd = hwnd;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            }
            return LRESULT(1);
        }

        let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SideMenuState };
        if !state.is_null() {
            match message {
                WM_SIDE_MENU_COMMAND => {
                    unsafe { (*state).handle_command(wparam.0, lparam.0) };
                    return LRESULT(0);
                }
                WM_LBUTTONDOWN => {
                    unsafe { handle_click(&mut *state, lparam.0) };
                    return LRESULT(0);
                }
                WM_PAINT => {
                    unsafe { paint(hwnd, &*state) };
                    return LRESULT(0);
                }
                WM_ERASEBKGND => return LRESULT(1),
                WM_NCDESTROY => {
                    unsafe {
                        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                        drop(Box::from_raw(state));
                    }
                    return LRESULT(0);
                }
                _ => {}
            }
        }

        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    fn handle_click(state: &mut SideMenuState, lparam: isize) {
        let scale = state.scale();
        let x = low_word(lparam) as i32;
        let y = high_word(lparam) as i32;
        let side_width = scaled(SIDE_WIDTH, scale);
        if x < side_width {
            let first_y = scaled(HEADER_HEIGHT + PADDING, scale);
            let row_height = scaled(ROW_HEIGHT, scale);
            let index = (y - first_y) / row_height;
            if index >= 0 && (index as usize) < rows().len() {
                state.selected = index as usize;
                state.activate_selected();
            }
        } else if state.panel == Panel::Tasks {
            let first_y = scaled(HEADER_HEIGHT + PADDING, scale);
            let row_height = scaled(TASK_ROW_HEIGHT, scale);
            let index = (y - first_y) / row_height;
            if index >= 0 && (index as usize) < state.tasks.len() {
                state.task_selected = index as usize;
                state.activate_selected();
            }
        } else if state.panel == Panel::Configuration {
            let first_y = scaled(HEADER_HEIGHT + PADDING, scale);
            let row_height = scaled(CONFIG_ROW_HEIGHT, scale);
            let index = (y - first_y) / row_height;
            if (0..=2).contains(&index) {
                state.config_selected = index as usize;
            }
        }
        state.invalidate();
    }

    unsafe fn paint(hwnd: HWND, state: &SideMenuState) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
        unsafe {
            SetBkMode(hdc, TRANSPARENT);
        }

        let scale = state.scale();
        let width = state.active_width();
        let height = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(720);
        fill(hdc, rect(0, 0, width, height), rgb(18, 18, 20));
        fill(
            hdc,
            rect(
                scaled(SIDE_WIDTH, scale) - 1,
                0,
                scaled(SIDE_WIDTH, scale),
                height,
            ),
            rgb(54, 58, 62),
        );

        draw_text(
            hdc,
            "GameEase",
            rect(
                scaled(PADDING, scale),
                scaled(16, scale),
                scaled(SIDE_WIDTH - PADDING, scale),
                scaled(46, scale),
            ),
            rgb(245, 245, 245),
        );
        draw_rows(hdc, state);
        draw_panel(hdc, state);

        unsafe {
            let _ = EndPaint(hwnd, &ps);
        }
    }

    fn draw_rows(hdc: windows::Win32::Graphics::Gdi::HDC, state: &SideMenuState) {
        let scale = state.scale();
        let left = scaled(PADDING, scale);
        let right = scaled(SIDE_WIDTH - PADDING, scale);
        let mut y = scaled(HEADER_HEIGHT + PADDING, scale);
        for (index, row) in rows().iter().enumerate() {
            let row_rect = rect(left, y, right, y + scaled(ROW_HEIGHT - 6, scale));
            if index == state.selected {
                fill(hdc, row_rect, rgb(36, 82, 96));
            }
            let label = match row {
                SideRow::Volume => "Volume",
                SideRow::Wifi => "Wi-Fi",
                SideRow::Tasks => "Task Switcher",
                SideRow::Bluetooth => "Bluetooth",
                SideRow::Configuration => "Configuration",
                SideRow::Quit => "Quit",
            };
            draw_text(
                hdc,
                label,
                inset(row_rect, scaled(12, scale), 0),
                rgb(242, 244, 244),
            );

            if *row == SideRow::Volume {
                draw_volume(hdc, state, row_rect);
            }
            y += scaled(ROW_HEIGHT, scale);
        }

        if let Some(status) = state.status.as_deref() {
            draw_text(
                hdc,
                status,
                rect(left, y + scaled(16, scale), right, y + scaled(88, scale)),
                rgb(255, 186, 174),
            );
        }
    }

    fn draw_volume(hdc: windows::Win32::Graphics::Gdi::HDC, state: &SideMenuState, row_rect: RECT) {
        let Some(volume) = state.volume else {
            return;
        };
        let scale = state.scale();
        let bar_left = row_rect.left + scaled(92, scale);
        let bar_right = row_rect.right - scaled(44, scale);
        let bar_top = row_rect.top + scaled(32, scale);
        let bar_bottom = bar_top + scaled(5, scale);
        fill(
            hdc,
            rect(bar_left, bar_top, bar_right, bar_bottom),
            rgb(74, 78, 82),
        );
        let filled = bar_left + ((bar_right - bar_left) * i32::from(volume.volume) / 100);
        fill(
            hdc,
            rect(bar_left, bar_top, filled, bar_bottom),
            rgb(116, 205, 217),
        );
        let value = if volume.muted {
            "Muted".to_string()
        } else {
            format!("{}%", volume.volume)
        };
        draw_text(
            hdc,
            &value,
            rect(
                bar_right + scaled(6, scale),
                row_rect.top,
                row_rect.right,
                row_rect.bottom,
            ),
            rgb(206, 211, 214),
        );
    }

    fn draw_panel(hdc: windows::Win32::Graphics::Gdi::HDC, state: &SideMenuState) {
        match state.panel {
            Panel::None => {}
            Panel::Wifi => draw_placeholder_panel(
                hdc,
                state,
                "Wi-Fi",
                "Windows Wi-Fi control is not wired up yet.",
            ),
            Panel::Bluetooth => draw_placeholder_panel(
                hdc,
                state,
                "Bluetooth",
                "Windows Bluetooth control is not wired up yet.",
            ),
            Panel::Tasks => draw_task_panel(hdc, state),
            Panel::Configuration => draw_config_panel(hdc, state),
        }
    }

    fn draw_placeholder_panel(
        hdc: windows::Win32::Graphics::Gdi::HDC,
        state: &SideMenuState,
        title: &str,
        message: &str,
    ) {
        let scale = state.scale();
        let left = scaled(SIDE_WIDTH + PADDING, scale);
        draw_text(
            hdc,
            title,
            rect(
                left,
                scaled(16, scale),
                state.active_width() - scaled(PADDING, scale),
                scaled(46, scale),
            ),
            rgb(245, 245, 245),
        );
        draw_text(
            hdc,
            message,
            rect(
                left,
                scaled(86, scale),
                state.active_width() - scaled(PADDING, scale),
                scaled(160, scale),
            ),
            rgb(206, 211, 214),
        );
    }

    fn draw_task_panel(hdc: windows::Win32::Graphics::Gdi::HDC, state: &SideMenuState) {
        let scale = state.scale();
        let left = scaled(SIDE_WIDTH + PADDING, scale);
        let right = state.active_width() - scaled(PADDING, scale);
        draw_text(
            hdc,
            "Task Switcher",
            rect(left, scaled(16, scale), right, scaled(46, scale)),
            rgb(245, 245, 245),
        );

        if state.tasks.is_empty() {
            draw_text(
                hdc,
                "No visible app windows found.",
                rect(left, scaled(86, scale), right, scaled(136, scale)),
                rgb(206, 211, 214),
            );
            return;
        }

        let mut y = scaled(HEADER_HEIGHT + PADDING, scale);
        let max_y = unsafe { GetSystemMetrics(SM_CYSCREEN) } - scaled(PADDING, scale);
        for (index, task) in state.tasks.iter().enumerate() {
            if y + scaled(TASK_ROW_HEIGHT, scale) > max_y {
                break;
            }
            let row_rect = rect(left, y, right, y + scaled(TASK_ROW_HEIGHT - 6, scale));
            if index == state.task_selected {
                fill(hdc, row_rect, rgb(46, 50, 54));
            }
            draw_text(
                hdc,
                &task.title,
                inset(row_rect, scaled(10, scale), 0),
                rgb(242, 244, 244),
            );
            y += scaled(TASK_ROW_HEIGHT, scale);
        }
    }

    fn draw_config_panel(hdc: windows::Win32::Graphics::Gdi::HDC, state: &SideMenuState) {
        let scale = state.scale();
        let left = scaled(SIDE_WIDTH + PADDING, scale);
        let right = state.active_width() - scaled(PADDING, scale);
        draw_text(
            hdc,
            "Configuration",
            rect(left, scaled(16, scale), right, scaled(46, scale)),
            rgb(245, 245, 245),
        );

        let rows = [
            (
                "Desktop sensitivity",
                format!("{:.2}x", state.config.desktop_mode.mouse_sensitivity),
            ),
            ("UI scale", format!("{:.2}x", state.config.general.ui_scale)),
            (
                "OSK scale",
                format!("{:.2}x", state.config.on_screen_keyboard.scale),
            ),
        ];
        let mut y = scaled(HEADER_HEIGHT + PADDING, scale);
        for (index, (label, value)) in rows.iter().enumerate() {
            let row_rect = rect(left, y, right, y + scaled(CONFIG_ROW_HEIGHT - 8, scale));
            if index == state.config_selected {
                fill(hdc, row_rect, rgb(46, 50, 54));
            }
            draw_text(
                hdc,
                label,
                inset(row_rect, scaled(10, scale), 0),
                rgb(242, 244, 244),
            );
            draw_text(
                hdc,
                value,
                rect(
                    row_rect.right - scaled(90, scale),
                    row_rect.top,
                    row_rect.right,
                    row_rect.bottom,
                ),
                rgb(116, 205, 217),
            );
            y += scaled(CONFIG_ROW_HEIGHT, scale);
        }
    }

    fn encode_command(command: SideMenuCommand) -> (usize, isize) {
        match command {
            SideMenuCommand::ToggleSideMenu => (CMD_TOGGLE, 0),
            SideMenuCommand::CloseSideMenu => (CMD_CLOSE, 0),
            SideMenuCommand::MoveSelection(direction) => (CMD_MOVE, encode_direction(direction)),
            SideMenuCommand::ActivateSelection => (CMD_ACTIVATE, 0),
            SideMenuCommand::ToggleScan => (CMD_SCAN, 0),
            SideMenuCommand::TerminateSelection => (CMD_TERMINATE, 0),
        }
    }

    fn encode_direction(direction: KeyboardDirection) -> isize {
        match direction {
            KeyboardDirection::Up => 1,
            KeyboardDirection::Down => 2,
            KeyboardDirection::Left => 3,
            KeyboardDirection::Right => 4,
        }
    }

    fn decode_direction(value: isize) -> KeyboardDirection {
        match value {
            1 => KeyboardDirection::Up,
            2 => KeyboardDirection::Down,
            3 => KeyboardDirection::Left,
            4 => KeyboardDirection::Right,
            _ => KeyboardDirection::Down,
        }
    }

    fn draw_text(
        hdc: windows::Win32::Graphics::Gdi::HDC,
        text: &str,
        mut area: RECT,
        color: COLORREF,
    ) {
        let mut text: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            SetTextColor(hdc, color);
            DrawTextW(
                hdc,
                &mut text,
                &mut area,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
        }
    }

    fn fill(hdc: windows::Win32::Graphics::Gdi::HDC, area: RECT, color: COLORREF) {
        let brush = unsafe { CreateSolidBrush(color) };
        unsafe {
            FillRect(hdc, &area, brush);
            delete_brush(brush);
        }
    }

    unsafe fn delete_brush(brush: HBRUSH) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    fn inset(area: RECT, x: i32, y: i32) -> RECT {
        RECT {
            left: area.left + x,
            top: area.top + y,
            right: area.right - x,
            bottom: area.bottom - y,
        }
    }

    fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
        COLORREF(u32::from(red) | (u32::from(green) << 8) | (u32::from(blue) << 16))
    }

    fn scaled(value: i32, scale: f32) -> i32 {
        ((value as f32) * scale).round().max(1.0) as i32
    }

    fn low_word(value: isize) -> i16 {
        (value as u16) as i16
    }

    fn high_word(value: isize) -> i16 {
        ((value >> 16) as u16) as i16
    }
}

#[cfg(not(windows))]
impl WindowsSideMenu {
    /// Creates the hidden side-menu window.
    pub fn new(_config_sender: Sender<GamepadGrabCommand>) -> Result<Self> {
        anyhow::bail!("Windows side menu is only available on Windows")
    }

    /// Returns a thread-friendly handle for the side menu.
    pub fn handle(&self) -> WindowsSideMenuHandle {
        WindowsSideMenuHandle { hwnd: 0 }
    }
}

#[cfg(not(windows))]
impl WindowsSideMenuHandle {
    /// Posts a side-menu command to the UI thread.
    pub fn post(&self, _command: SideMenuCommand) -> Result<()> {
        anyhow::bail!("Windows side menu is only available on Windows")
    }
}

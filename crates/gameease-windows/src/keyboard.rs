use anyhow::Result;
use gameease_core::gamepad::{GamepadCommand, KeyCode, KeyboardDirection};
use gameease_core::InputBackend;

/// Native Windows on-screen keyboard.
#[derive(Debug)]
pub struct WindowsOnScreenKeyboard {
    #[cfg(windows)]
    hwnd: windows::Win32::Foundation::HWND,
}

/// Thread-friendly handle for posting gamepad OSK commands to the UI thread.
#[derive(Debug, Clone, Copy)]
pub struct WindowsOnScreenKeyboardHandle {
    hwnd: isize,
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::WindowsInputBackend;
    use anyhow::{anyhow, Context};
    use gameease_core::config;
    use std::ptr::null_mut;
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
        GetStockObject, InvalidateRect, Rectangle, SelectObject, SetBkMode, SetTextColor,
        DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, HGDIOBJ, NULL_BRUSH,
        PAINTSTRUCT, PS_SOLID, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, GetWindowLongPtrW,
        PostMessageW, RegisterClassW, SetWindowLongPtrW, SetWindowPos, ShowWindow, CREATESTRUCTW,
        CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, HWND_TOPMOST, SM_CXSCREEN,
        SM_CYSCREEN, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, WINDOW_EX_STYLE, WM_APP,
        WM_ERASEBKGND, WM_LBUTTONDOWN, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WNDCLASSW,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    const INITIAL_ROW: usize = 2;
    const INITIAL_COLUMN: usize = 6;
    const KEY_UNIT_SIZE: i32 = 58;
    const GRID_SPACING: i32 = 6;
    const PANEL_PADDING: i32 = 5;
    const OSK_EDGE_GAP: i32 = 50;

    const WM_OSK_COMMAND: u32 = WM_APP + 40;
    const CMD_TOGGLE: usize = 1;
    const CMD_CLOSE: usize = 2;
    const CMD_TOGGLE_POSITION: usize = 3;
    const CMD_MOVE: usize = 4;
    const CMD_ACTIVATE: usize = 5;
    const CMD_SPACE: usize = 6;
    const CMD_BACKSPACE: usize = 7;
    const CMD_ENTER: usize = 8;
    const CMD_SHIFT_HELD: usize = 9;
    const CMD_CAPS: usize = 10;

    impl WindowsOnScreenKeyboard {
        /// Creates the hidden OSK window.
        pub fn new() -> Result<Self> {
            let state = Box::new(OskState::new());
            let hwnd = create_window(Box::into_raw(state))?;
            Ok(Self { hwnd })
        }

        /// Returns a thread-friendly handle for the OSK.
        pub fn handle(&self) -> WindowsOnScreenKeyboardHandle {
            WindowsOnScreenKeyboardHandle {
                hwnd: self.hwnd.0 as isize,
            }
        }
    }

    impl Drop for WindowsOnScreenKeyboard {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }

    impl WindowsOnScreenKeyboardHandle {
        /// Posts a gamepad OSK command to the UI thread.
        pub fn post(&self, command: GamepadCommand) -> Result<()> {
            let Some((code, value)) = encode_command(command) else {
                return Ok(());
            };
            let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
            unsafe { PostMessageW(Some(hwnd), WM_OSK_COMMAND, WPARAM(code), LPARAM(value)) }
                .context("failed to post OSK command")
        }
    }

    #[derive(Clone, Copy)]
    struct KeySpec {
        label: &'static str,
        shifted_label: &'static str,
        caps_label: &'static str,
        shift_caps_label: &'static str,
        hint: Option<&'static str>,
        action: KeyAction,
        width: i32,
    }

    impl KeySpec {
        const fn tap(label: &'static str, key: KeyCode, width: i32) -> Self {
            Self {
                label,
                shifted_label: label,
                caps_label: label,
                shift_caps_label: label,
                hint: None,
                action: KeyAction::Tap(key),
                width,
            }
        }

        const fn tap_hint(
            label: &'static str,
            key: KeyCode,
            width: i32,
            hint: &'static str,
        ) -> Self {
            Self {
                label,
                shifted_label: label,
                caps_label: label,
                shift_caps_label: label,
                hint: Some(hint),
                action: KeyAction::Tap(key),
                width,
            }
        }

        const fn tap_letter(
            label: &'static str,
            shifted_label: &'static str,
            key: KeyCode,
            width: i32,
        ) -> Self {
            Self {
                label,
                shifted_label,
                caps_label: shifted_label,
                shift_caps_label: label,
                hint: None,
                action: KeyAction::Tap(key),
                width,
            }
        }

        const fn tap_shift(
            label: &'static str,
            shifted_label: &'static str,
            key: KeyCode,
            width: i32,
        ) -> Self {
            Self {
                label,
                shifted_label,
                caps_label: label,
                shift_caps_label: shifted_label,
                hint: None,
                action: KeyAction::Tap(key),
                width,
            }
        }

        const fn toggle(label: &'static str, key: KeyCode, width: i32) -> Self {
            Self {
                label,
                shifted_label: label,
                caps_label: label,
                shift_caps_label: label,
                hint: None,
                action: KeyAction::Toggle(key),
                width,
            }
        }

        const fn toggle_hint(
            label: &'static str,
            key: KeyCode,
            width: i32,
            hint: &'static str,
        ) -> Self {
            Self {
                label,
                shifted_label: label,
                caps_label: label,
                shift_caps_label: label,
                hint: Some(hint),
                action: KeyAction::Toggle(key),
                width,
            }
        }

        const fn caps_lock(label: &'static str, width: i32) -> Self {
            Self {
                label,
                shifted_label: label,
                caps_label: label,
                shift_caps_label: label,
                hint: Some("L3"),
                action: KeyAction::CapsLock,
                width,
            }
        }

        const fn move_keyboard(width: i32) -> Self {
            Self {
                label: "",
                shifted_label: "",
                caps_label: "",
                shift_caps_label: "",
                hint: Some("View"),
                action: KeyAction::MoveKeyboard,
                width,
            }
        }

        fn display_label(self, shift: bool, caps: bool) -> &'static str {
            if shift && caps {
                self.shift_caps_label
            } else if shift {
                self.shifted_label
            } else if caps {
                self.caps_label
            } else {
                self.label
            }
        }
    }

    #[derive(Clone, Copy)]
    enum KeyAction {
        Tap(KeyCode),
        Toggle(KeyCode),
        CapsLock,
        MoveKeyboard,
    }

    const KEY_ROWS: &[&[KeySpec]] = &[
        &[
            KeySpec::tap_shift("`", "~", KeyCode::Grave, 1),
            KeySpec::tap_shift("1", "!", KeyCode::Num1, 1),
            KeySpec::tap_shift("2", "@", KeyCode::Num2, 1),
            KeySpec::tap_shift("3", "#", KeyCode::Num3, 1),
            KeySpec::tap_shift("4", "$", KeyCode::Num4, 1),
            KeySpec::tap_shift("5", "%", KeyCode::Num5, 1),
            KeySpec::tap_shift("6", "^", KeyCode::Num6, 1),
            KeySpec::tap_shift("7", "&", KeyCode::Num7, 1),
            KeySpec::tap_shift("8", "*", KeyCode::Num8, 1),
            KeySpec::tap_shift("9", "(", KeyCode::Num9, 1),
            KeySpec::tap_shift("0", ")", KeyCode::Num0, 1),
            KeySpec::tap_shift("-", "_", KeyCode::Minus, 1),
            KeySpec::tap_shift("=", "+", KeyCode::Equal, 1),
            KeySpec::tap_hint("Back", KeyCode::Backspace, 2, "Left"),
        ],
        &[
            KeySpec::tap("Tab", KeyCode::Tab, 2),
            KeySpec::tap_letter("q", "Q", KeyCode::Q, 1),
            KeySpec::tap_letter("w", "W", KeyCode::W, 1),
            KeySpec::tap_letter("e", "E", KeyCode::E, 1),
            KeySpec::tap_letter("r", "R", KeyCode::R, 1),
            KeySpec::tap_letter("t", "T", KeyCode::T, 1),
            KeySpec::tap_letter("y", "Y", KeyCode::Y, 1),
            KeySpec::tap_letter("u", "U", KeyCode::U, 1),
            KeySpec::tap_letter("i", "I", KeyCode::I, 1),
            KeySpec::tap_letter("o", "O", KeyCode::O, 1),
            KeySpec::tap_letter("p", "P", KeyCode::P, 1),
            KeySpec::tap_shift("[", "{", KeyCode::LeftBrace, 1),
            KeySpec::tap_shift("]", "}", KeyCode::RightBrace, 1),
            KeySpec::tap_shift("\\", "|", KeyCode::Backslash, 1),
        ],
        &[
            KeySpec::caps_lock("Caps", 2),
            KeySpec::tap_letter("a", "A", KeyCode::A, 1),
            KeySpec::tap_letter("s", "S", KeyCode::S, 1),
            KeySpec::tap_letter("d", "D", KeyCode::D, 1),
            KeySpec::tap_letter("f", "F", KeyCode::F, 1),
            KeySpec::tap_letter("g", "G", KeyCode::G, 1),
            KeySpec::tap_letter("h", "H", KeyCode::H, 1),
            KeySpec::tap_letter("j", "J", KeyCode::J, 1),
            KeySpec::tap_letter("k", "K", KeyCode::K, 1),
            KeySpec::tap_letter("l", "L", KeyCode::L, 1),
            KeySpec::tap_shift(";", ":", KeyCode::Semicolon, 1),
            KeySpec::tap_shift("'", "\"", KeyCode::Apostrophe, 1),
            KeySpec::tap_hint("Enter", KeyCode::Enter, 2, "RT"),
        ],
        &[
            KeySpec::toggle_hint("Shift", KeyCode::LeftShift, 2, "LT"),
            KeySpec::tap_letter("z", "Z", KeyCode::Z, 1),
            KeySpec::tap_letter("x", "X", KeyCode::X, 1),
            KeySpec::tap_letter("c", "C", KeyCode::C, 1),
            KeySpec::tap_letter("v", "V", KeyCode::V, 1),
            KeySpec::tap_letter("b", "B", KeyCode::B, 1),
            KeySpec::tap_letter("n", "N", KeyCode::N, 1),
            KeySpec::tap_letter("m", "M", KeyCode::M, 1),
            KeySpec::tap_shift(",", "<", KeyCode::Comma, 1),
            KeySpec::tap_shift(".", ">", KeyCode::Dot, 1),
            KeySpec::tap_shift("/", "?", KeyCode::Slash, 1),
            KeySpec::tap("Up", KeyCode::Up, 1),
            KeySpec::toggle_hint("Shift", KeyCode::RightShift, 2, "LT"),
        ],
        &[
            KeySpec::toggle("Ctrl", KeyCode::LeftControl, 1),
            KeySpec::toggle("Meta", KeyCode::LeftMeta, 1),
            KeySpec::toggle("Alt", KeyCode::LeftAlt, 1),
            KeySpec::tap_hint("Space", KeyCode::Space, 8, "Up"),
            KeySpec::tap("Left", KeyCode::Left, 1),
            KeySpec::tap("Down", KeyCode::Down, 1),
            KeySpec::tap("Right", KeyCode::Right, 1),
            KeySpec::move_keyboard(1),
        ],
    ];

    struct OskState {
        hwnd: HWND,
        input: WindowsInputBackend,
        visible: bool,
        placement: KeyboardPlacement,
        selected_row: usize,
        selected_column: usize,
        shift_toggle: bool,
        shift_held: bool,
        caps: bool,
        ctrl: bool,
        meta: bool,
        alt: bool,
    }

    #[derive(Clone, Copy)]
    enum KeyboardPlacement {
        Bottom,
        Top,
    }

    impl OskState {
        fn new() -> Self {
            Self {
                hwnd: HWND(null_mut()),
                input: WindowsInputBackend,
                visible: false,
                placement: KeyboardPlacement::Bottom,
                selected_row: INITIAL_ROW,
                selected_column: INITIAL_COLUMN,
                shift_toggle: false,
                shift_held: false,
                caps: false,
                ctrl: false,
                meta: false,
                alt: false,
            }
        }

        fn handle_command(&mut self, code: usize, value: isize) {
            match code {
                CMD_TOGGLE => self.toggle(),
                CMD_CLOSE => self.hide(),
                CMD_TOGGLE_POSITION => self.toggle_position(),
                CMD_MOVE if self.visible => self.move_selection(decode_direction(value)),
                CMD_ACTIVATE if self.visible => self.activate_selected(),
                CMD_SPACE if self.visible => self.tap_key(KeyCode::Space),
                CMD_BACKSPACE if self.visible => self.tap_key(KeyCode::Backspace),
                CMD_ENTER if self.visible => self.tap_key(KeyCode::Enter),
                CMD_SHIFT_HELD => self.set_shift_held(value != 0),
                CMD_CAPS if self.visible => self.toggle_caps(),
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
            self.resize_and_show();
        }

        fn hide(&mut self) {
            self.visible = false;
            if self.shift_held || self.shift_toggle {
                self.shift_held = false;
                self.shift_toggle = false;
                let _ = self.input.release_key(KeyCode::LeftShift);
            }
            if self.ctrl {
                self.ctrl = false;
                let _ = self.input.release_key(KeyCode::LeftControl);
            }
            if self.meta {
                self.meta = false;
                let _ = self.input.release_key(KeyCode::LeftMeta);
            }
            if self.alt {
                self.alt = false;
                let _ = self.input.release_key(KeyCode::LeftAlt);
            }
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }

        fn toggle_position(&mut self) {
            if !self.visible {
                return;
            }
            self.placement = match self.placement {
                KeyboardPlacement::Bottom => KeyboardPlacement::Top,
                KeyboardPlacement::Top => KeyboardPlacement::Bottom,
            };
            self.resize_and_show();
        }

        fn move_selection(&mut self, direction: KeyboardDirection) {
            match direction {
                KeyboardDirection::Left => {
                    self.selected_column = self.selected_column.saturating_sub(1)
                }
                KeyboardDirection::Right => {
                    self.selected_column =
                        (self.selected_column + 1).min(KEY_ROWS[self.selected_row].len() - 1)
                }
                KeyboardDirection::Up => {
                    let row = self.selected_row.saturating_sub(1);
                    self.selected_column = self.closest_column(row);
                    self.selected_row = row;
                }
                KeyboardDirection::Down => {
                    let row = (self.selected_row + 1).min(KEY_ROWS.len() - 1);
                    self.selected_column = self.closest_column(row);
                    self.selected_row = row;
                }
            }
        }

        fn activate_selected(&mut self) {
            self.activate(KEY_ROWS[self.selected_row][self.selected_column]);
        }

        fn activate(&mut self, spec: KeySpec) {
            match spec.action {
                KeyAction::Tap(key) => self.tap_key(key),
                KeyAction::Toggle(key) if is_shift_key(key) => self.toggle_shift(),
                KeyAction::Toggle(key) => self.toggle_modifier(key),
                KeyAction::CapsLock => self.toggle_caps(),
                KeyAction::MoveKeyboard => self.toggle_position(),
            }
        }

        fn tap_key(&mut self, key: KeyCode) {
            if let Err(error) = self.input.tap_key(key) {
                eprintln!("Failed to tap Windows OSK key: {error:#}");
            }
        }

        fn toggle_modifier(&mut self, key: KeyCode) {
            let active = match key {
                KeyCode::LeftControl => &mut self.ctrl,
                KeyCode::LeftMeta => &mut self.meta,
                KeyCode::LeftAlt => &mut self.alt,
                _ => return,
            };
            let result = if *active {
                self.input.release_key(key)
            } else {
                self.input.press_key(key)
            };
            if result.is_ok() {
                *active = !*active;
            }
        }

        fn toggle_shift(&mut self) {
            self.shift_toggle = !self.shift_toggle;
            self.sync_shift();
        }

        fn set_shift_held(&mut self, active: bool) {
            self.shift_held = active;
            self.sync_shift();
        }

        fn toggle_caps(&mut self) {
            if let Err(error) = self.input.tap_key(KeyCode::CapsLock) {
                eprintln!("Failed to toggle Windows OSK caps lock: {error:#}");
            }
            self.caps = !self.caps;
        }

        fn sync_shift(&self) {
            let result = if self.shift_active() {
                self.input.press_key(KeyCode::LeftShift)
            } else {
                self.input.release_key(KeyCode::LeftShift)
            };
            if let Err(error) = result {
                eprintln!("Failed to sync Windows OSK shift: {error:#}");
            }
        }

        fn shift_active(&self) -> bool {
            self.shift_toggle || self.shift_held
        }

        fn closest_column(&self, row: usize) -> usize {
            let current_center = key_center(self.selected_row, self.selected_column);
            let mut best = 0;
            let mut best_distance = i32::MAX;
            for column in 0..KEY_ROWS[row].len() {
                let distance = (key_center(row, column) - current_center).abs();
                if distance < best_distance {
                    best = column;
                    best_distance = distance;
                }
            }
            best
        }

        fn layout(&self) -> Layout {
            let scale = config::sanitize_osk_scale(
                config::load_config_or_default().on_screen_keyboard.scale,
            );
            let width = scaled(total_units(), scale) + scaled(PANEL_PADDING * 2, scale);
            let height = scaled(KEY_ROWS.len() as i32 * KEY_UNIT_SIZE, scale)
                + scaled((KEY_ROWS.len() as i32 - 1) * GRID_SPACING, scale)
                + scaled(PANEL_PADDING * 2, scale);
            let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
            let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
            let x = ((screen_width - width) / 2).max(0);
            let y = match self.placement {
                KeyboardPlacement::Bottom => {
                    (screen_height - height - scaled(OSK_EDGE_GAP, scale)).max(0)
                }
                KeyboardPlacement::Top => scaled(OSK_EDGE_GAP, scale),
            };
            Layout {
                scale,
                x,
                y,
                width,
                height,
            }
        }

        fn resize_and_show(&self) {
            let layout = self.layout();
            let _ = unsafe {
                SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    layout.x,
                    layout.y,
                    layout.width,
                    layout.height,
                    SWP_SHOWWINDOW | SWP_NOACTIVATE,
                )
            };
        }

        fn invalidate(&self) {
            let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
        }
    }

    struct Layout {
        scale: f32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    }

    fn create_window(state: *mut OskState) -> Result<HWND> {
        let hmodule = unsafe { GetModuleHandleW(None) }.context("failed to get module handle")?;
        let hinstance = HINSTANCE(hmodule.0);
        let class_name = w!("GameEaseOskWindow");
        let window_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            hInstance: hinstance,
            lpszClassName: class_name,
            lpfnWndProc: Some(window_proc),
            ..Default::default()
        };

        let atom = unsafe { RegisterClassW(&window_class) };
        if atom == 0 {
            unsafe { drop(Box::from_raw(state)) };
            return Err(anyhow!("failed to register GameEase OSK window class"));
        }

        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0),
                class_name,
                w!("GameEase Keyboard"),
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
        .context("failed to create GameEase OSK window")
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_NCCREATE {
            let create = lparam.0 as *const CREATESTRUCTW;
            let state = unsafe { (*create).lpCreateParams as *mut OskState };
            unsafe {
                (*state).hwnd = hwnd;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            }
            return LRESULT(1);
        }

        let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OskState };
        if !state.is_null() {
            match message {
                WM_OSK_COMMAND => {
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

    fn handle_click(state: &mut OskState, lparam: isize) {
        let x = low_word(lparam) as i32;
        let y = high_word(lparam) as i32;
        let layout = state.layout();
        if let Some((row, column)) = hit_test(x, y, layout.scale) {
            state.selected_row = row;
            state.selected_column = column;
            state.activate_selected();
            state.invalidate();
        }
    }

    fn hit_test(x: i32, y: i32, scale: f32) -> Option<(usize, usize)> {
        let padding = scaled(PANEL_PADDING, scale);
        let spacing = scaled(GRID_SPACING, scale);
        let key_h = scaled(KEY_UNIT_SIZE, scale);
        let mut top = padding;
        for (row_index, row) in KEY_ROWS.iter().enumerate() {
            let mut left = padding;
            for (column_index, key) in row.iter().enumerate() {
                let width = scaled(key.width * KEY_UNIT_SIZE, scale);
                if x >= left && x < left + width && y >= top && y < top + key_h {
                    return Some((row_index, column_index));
                }
                left += width + spacing;
            }
            top += key_h + spacing;
        }
        None
    }

    unsafe fn paint(hwnd: HWND, state: &OskState) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
        unsafe {
            SetBkMode(hdc, TRANSPARENT);
        }

        let layout = state.layout();
        fill(
            hdc,
            rect(0, 0, layout.width, layout.height),
            rgb(36, 36, 36),
        );
        draw_keys(hdc, state, layout.scale);

        unsafe {
            let _ = EndPaint(hwnd, &ps);
        }
    }

    fn draw_keys(hdc: windows::Win32::Graphics::Gdi::HDC, state: &OskState, scale: f32) {
        let padding = scaled(PANEL_PADDING, scale);
        let spacing = scaled(GRID_SPACING, scale);
        let key_h = scaled(KEY_UNIT_SIZE, scale);
        let mut top = padding;
        for (row_index, row) in KEY_ROWS.iter().enumerate() {
            let mut left = padding;
            for (column_index, spec) in row.iter().enumerate() {
                let width = scaled(spec.width * KEY_UNIT_SIZE, scale);
                let key_rect = rect(left, top, left + width, top + key_h);
                let active = is_active_modifier(*spec, state);
                let selected =
                    row_index == state.selected_row && column_index == state.selected_column;
                draw_key(
                    hdc,
                    key_rect,
                    *spec,
                    state.shift_active(),
                    state.caps,
                    active,
                    selected,
                );
                left += width + spacing;
            }
            top += key_h + spacing;
        }
    }

    fn draw_key(
        hdc: windows::Win32::Graphics::Gdi::HDC,
        area: RECT,
        spec: KeySpec,
        shift: bool,
        caps: bool,
        active: bool,
        selected: bool,
    ) {
        fill(
            hdc,
            area,
            if active {
                rgb(48, 86, 95)
            } else {
                rgb(51, 51, 51)
            },
        );
        if selected {
            outline(hdc, area, rgb(114, 199, 216), 3);
        }
        let mut text = match spec.hint {
            Some(hint) if spec.label.is_empty() => hint.to_string(),
            Some(hint) => format!("{hint}  {}", spec.display_label(shift, caps)),
            None => spec.display_label(shift, caps).to_string(),
        };
        if matches!(spec.action, KeyAction::Tap(KeyCode::Up)) {
            text = "^".into();
        } else if matches!(spec.action, KeyAction::Tap(KeyCode::Left)) {
            text = "<".into();
        } else if matches!(spec.action, KeyAction::Tap(KeyCode::Down)) {
            text = "v".into();
        } else if matches!(spec.action, KeyAction::Tap(KeyCode::Right)) {
            text = ">".into();
        }
        draw_text(hdc, &text, inset(area, 4, 4), rgb(240, 240, 240));
    }

    fn encode_command(command: GamepadCommand) -> Option<(usize, isize)> {
        Some(match command {
            GamepadCommand::ToggleKeyboard => (CMD_TOGGLE, 0),
            GamepadCommand::CloseKeyboard => (CMD_CLOSE, 0),
            GamepadCommand::ToggleKeyboardPosition => (CMD_TOGGLE_POSITION, 0),
            GamepadCommand::MoveSelection(direction) => (CMD_MOVE, encode_direction(direction)),
            GamepadCommand::ActivateSelection => (CMD_ACTIVATE, 0),
            GamepadCommand::ActivateSpace => (CMD_SPACE, 0),
            GamepadCommand::ActivateBackspace => (CMD_BACKSPACE, 0),
            GamepadCommand::ActivateEnter => (CMD_ENTER, 0),
            GamepadCommand::SetShiftHeld(active) => (CMD_SHIFT_HELD, if active { 1 } else { 0 }),
            GamepadCommand::ToggleCapsLock => (CMD_CAPS, 0),
            GamepadCommand::DesktopModeChanged(_) => return None,
        })
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

    fn is_shift_key(key: KeyCode) -> bool {
        matches!(key, KeyCode::LeftShift | KeyCode::RightShift)
    }

    fn is_active_modifier(spec: KeySpec, state: &OskState) -> bool {
        match spec.action {
            KeyAction::Toggle(key) if is_shift_key(key) => state.shift_active(),
            KeyAction::Toggle(KeyCode::LeftControl) => state.ctrl,
            KeyAction::Toggle(KeyCode::LeftMeta) => state.meta,
            KeyAction::Toggle(KeyCode::LeftAlt) => state.alt,
            KeyAction::CapsLock => state.caps,
            _ => false,
        }
    }

    fn key_center(row: usize, column: usize) -> i32 {
        let mut left = 0;
        for key in KEY_ROWS[row].iter().take(column) {
            left += key.width;
        }
        left + KEY_ROWS[row][column].width / 2
    }

    fn total_units() -> i32 {
        KEY_ROWS
            .iter()
            .map(|row| row.iter().map(|key| key.width).sum())
            .max()
            .unwrap_or(0)
            * KEY_UNIT_SIZE
            + (KEY_ROWS[0].len() as i32 - 1) * GRID_SPACING
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
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
        }
    }

    fn fill(hdc: windows::Win32::Graphics::Gdi::HDC, area: RECT, color: COLORREF) {
        let brush = unsafe { CreateSolidBrush(color) };
        unsafe {
            FillRect(hdc, &area, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }

    fn outline(hdc: windows::Win32::Graphics::Gdi::HDC, area: RECT, color: COLORREF, width: i32) {
        let pen = unsafe { CreatePen(PS_SOLID, width, color) };
        let brush = unsafe { GetStockObject(NULL_BRUSH) };
        unsafe {
            let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
            let old_brush = SelectObject(hdc, brush);
            let _ = Rectangle(hdc, area.left, area.top, area.right, area.bottom);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(HGDIOBJ(pen.0));
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
impl WindowsOnScreenKeyboard {
    /// Creates the hidden OSK window.
    pub fn new() -> Result<Self> {
        anyhow::bail!("Windows OSK is only available on Windows")
    }

    /// Returns a thread-friendly handle for the OSK.
    pub fn handle(&self) -> WindowsOnScreenKeyboardHandle {
        WindowsOnScreenKeyboardHandle { hwnd: 0 }
    }
}

#[cfg(not(windows))]
impl WindowsOnScreenKeyboardHandle {
    /// Posts a gamepad OSK command to the UI thread.
    pub fn post(&self, _command: GamepadCommand) -> Result<()> {
        anyhow::bail!("Windows OSK is only available on Windows")
    }
}

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::config;
use crate::traits::InputBackend;

const POINTER_DEADZONE: f32 = 0.12;
const POINTER_MAX_SPEED: f32 = 18.0;
const SCROLL_MAX_SPEED: f32 = 3.0;
const DPAD_REPEAT_INITIAL_DELAY: Duration = Duration::from_millis(400);
const DPAD_REPEAT_INTERVAL: Duration = Duration::from_millis(120);

/// Commands emitted by the gamepad input thread.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GamepadCommand {
    /// Toggle visibility of the on-screen keyboard.
    ToggleKeyboard,
    /// Close the on-screen keyboard if it is visible.
    CloseKeyboard,
    /// Move the on-screen keyboard between bottom and top positions.
    ToggleKeyboardPosition,
    /// Move the selected OSK key.
    MoveSelection(KeyboardDirection),
    /// Activate the selected OSK key.
    ActivateSelection,
    /// Activate the Space key without changing OSK selection.
    ActivateSpace,
    /// Activate the Backspace key without changing OSK selection.
    ActivateBackspace,
    /// Activate the Enter key without changing OSK selection.
    ActivateEnter,
    /// Hold or release the OSK Shift modifier from a physical gamepad trigger.
    SetShiftHeld(bool),
    /// Toggle Caps Lock from the OSK/gamepad layer.
    ToggleCapsLock,
    /// Show that Desktop Mode was enabled or disabled.
    DesktopModeChanged(bool),
}

/// Direction to move the selected OSK key.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum KeyboardDirection {
    /// Move selection up one row.
    Up,
    /// Move selection down one row.
    Down,
    /// Move selection left one key.
    Left,
    /// Move selection right one key.
    Right,
}

/// Commands emitted by the gamepad input thread for the side menu.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SideMenuCommand {
    /// Toggle visibility of the side menu.
    ToggleSideMenu,
    /// Close the side menu if it is visible.
    CloseSideMenu,
    /// Move the selected side menu row.
    MoveSelection(KeyboardDirection),
    /// Activate the selected side menu row.
    ActivateSelection,
    /// Toggle scanning in the active side menu panel.
    ToggleScan,
    /// Terminate the selected side menu item when supported.
    TerminateSelection,
}

/// Side menu row currently focused by gamepad navigation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FocusedRow {
    /// Volume controls are focused.
    Volume,
    /// Wi-Fi controls are focused.
    Wifi,
    /// Task switcher controls are focused.
    TaskSwitcher,
    /// Bluetooth controls are focused.
    Bluetooth,
    /// Configuration controls are focused.
    Configuration,
    /// No feature row is focused.
    None,
}

/// Commands sent from the GTK thread to control gamepad-thread state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GamepadGrabCommand {
    /// Enable or disable exclusive evdev grabs for connected gamepads.
    SetExclusive(bool),
    /// Set the Desktop Mode mouse sensitivity multiplier.
    SetDesktopMouseSensitivity(f32),
}

/// Mouse buttons emitted by the GameEase virtual mouse.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MouseButton {
    /// Primary mouse button.
    Left,
    /// Secondary mouse button.
    Right,
    /// Middle mouse button.
    Middle,
}

/// Keyboard keys emitted by platform input backends.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum KeyCode {
    /// Escape.
    Escape,
    /// Grave/backtick key.
    Grave,
    /// Number row 1.
    Num1,
    /// Number row 2.
    Num2,
    /// Number row 3.
    Num3,
    /// Number row 4.
    Num4,
    /// Number row 5.
    Num5,
    /// Number row 6.
    Num6,
    /// Number row 7.
    Num7,
    /// Number row 8.
    Num8,
    /// Number row 9.
    Num9,
    /// Number row 0.
    Num0,
    /// Minus.
    Minus,
    /// Equal.
    Equal,
    /// A-Z letter key.
    A,
    /// A-Z letter key.
    B,
    /// A-Z letter key.
    C,
    /// A-Z letter key.
    D,
    /// A-Z letter key.
    E,
    /// A-Z letter key.
    F,
    /// A-Z letter key.
    G,
    /// A-Z letter key.
    H,
    /// A-Z letter key.
    I,
    /// A-Z letter key.
    J,
    /// A-Z letter key.
    K,
    /// A-Z letter key.
    L,
    /// A-Z letter key.
    M,
    /// A-Z letter key.
    N,
    /// A-Z letter key.
    O,
    /// A-Z letter key.
    P,
    /// A-Z letter key.
    Q,
    /// A-Z letter key.
    R,
    /// A-Z letter key.
    S,
    /// A-Z letter key.
    T,
    /// A-Z letter key.
    U,
    /// A-Z letter key.
    V,
    /// A-Z letter key.
    W,
    /// A-Z letter key.
    X,
    /// A-Z letter key.
    Y,
    /// A-Z letter key.
    Z,
    /// Left bracket.
    LeftBrace,
    /// Right bracket.
    RightBrace,
    /// Backslash.
    Backslash,
    /// Tab.
    Tab,
    /// Caps Lock.
    CapsLock,
    /// Semicolon.
    Semicolon,
    /// Apostrophe.
    Apostrophe,
    /// Comma.
    Comma,
    /// Dot/period.
    Dot,
    /// Slash.
    Slash,
    /// Space.
    Space,
    /// Enter.
    Enter,
    /// Backspace.
    Backspace,
    /// Left Shift.
    LeftShift,
    /// Right Shift.
    RightShift,
    /// Left Control.
    LeftControl,
    /// Left Super/Windows key.
    LeftMeta,
    /// Left Alt.
    LeftAlt,
    /// Right Alt.
    RightAlt,
    /// ISO 102nd key.
    Iso102nd,
    /// Left arrow.
    Left,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Right arrow.
    Right,
}

/// Gamepad buttons understood by the core gamepad state machine.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GamepadButton {
    /// South face button.
    South,
    /// East face button.
    East,
    /// North face button.
    North,
    /// West face button.
    West,
    /// Left stick click.
    LeftThumb,
    /// Right stick click.
    RightThumb,
    /// Left trigger button edge.
    LeftTrigger2,
    /// Right trigger button edge.
    RightTrigger2,
    /// Select/View button.
    Select,
    /// Start/Menu button.
    Start,
    /// D-pad up.
    DPadUp,
    /// D-pad down.
    DPadDown,
    /// D-pad left.
    DPadLeft,
    /// D-pad right.
    DPadRight,
}

/// Gamepad axes understood by the core gamepad state machine.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GamepadAxis {
    /// Left stick vertical axis.
    LeftStickY,
    /// Right stick horizontal axis.
    RightStickX,
    /// Right stick vertical axis.
    RightStickY,
    /// Left trigger analog axis.
    LeftZ,
    /// Right trigger analog axis.
    RightZ,
}

/// Normalized gamepad event consumed by the core gamepad state machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawGamepadEvent {
    /// A button was pressed.
    Press(GamepadButton),
    /// A button was released.
    Release(GamepadButton),
    /// An axis changed to a normalized value.
    Axis(GamepadAxis, f32),
    /// A stick edge should move the UI selection.
    MoveSelection(KeyboardDirection),
}

/// Core gamepad mode state machine and command router.
pub struct GamepadState {
    osk_sender: Sender<GamepadCommand>,
    sidemenu_sender: Sender<SideMenuCommand>,
    south_pressed: bool,
    south_consumed: bool,
    north_pressed: bool,
    north_consumed: bool,
    select_pressed: bool,
    select_consumed: bool,
    start_pressed: bool,
    l2_pressed: bool,
    osk_combo_armed: bool,
    sidemenu_combo_armed: bool,
    desktop_combo_armed: bool,
    sidemenu_open: bool,
    focused_row_index: usize,
    focused_row: FocusedRow,
    desktop_mode: DesktopModeState,
}

impl GamepadState {
    /// Creates a gamepad FSM that emits UI commands and uses `input` for Desktop Mode.
    pub fn new(
        osk_sender: Sender<GamepadCommand>,
        sidemenu_sender: Sender<SideMenuCommand>,
        input: Box<dyn InputBackend>,
    ) -> Self {
        Self {
            osk_sender,
            sidemenu_sender,
            south_pressed: false,
            south_consumed: false,
            north_pressed: false,
            north_consumed: false,
            select_pressed: false,
            select_consumed: false,
            start_pressed: false,
            l2_pressed: false,
            osk_combo_armed: false,
            sidemenu_combo_armed: false,
            desktop_combo_armed: false,
            sidemenu_open: false,
            focused_row_index: 0,
            focused_row: FocusedRow::None,
            desktop_mode: DesktopModeState::new(input),
        }
    }

    /// Handles one normalized gamepad event.
    pub fn handle_event(&mut self, event: RawGamepadEvent) -> Result<()> {
        match event {
            RawGamepadEvent::Press(GamepadButton::Select) => self.select_pressed = true,
            RawGamepadEvent::Release(GamepadButton::Select) => {
                if !self.desktop_mode.is_active()
                    && self.select_pressed
                    && !self.select_consumed
                    && !self.south_pressed
                    && !self.north_pressed
                    && !self.start_pressed
                {
                    self.osk_sender
                        .send(GamepadCommand::ToggleKeyboardPosition)
                        .context("failed to send OSK position command")?;
                }

                self.select_pressed = false;
                self.select_consumed = false;
            }
            RawGamepadEvent::Press(GamepadButton::Start) => self.start_pressed = true,
            RawGamepadEvent::Release(GamepadButton::Start) => self.start_pressed = false,
            _ if self.desktop_mode.is_active() => self.desktop_mode.handle_event(event),
            RawGamepadEvent::Press(GamepadButton::South) => {
                self.south_pressed = true;
            }
            RawGamepadEvent::Release(GamepadButton::South) => {
                if self.south_pressed && !self.south_consumed && !self.select_pressed {
                    self.osk_sender
                        .send(GamepadCommand::ActivateSelection)
                        .context("failed to send OSK activation command")?;
                    self.sidemenu_sender
                        .send(SideMenuCommand::ActivateSelection)
                        .context("failed to send side menu activation command")?;
                }

                self.south_pressed = false;
                self.south_consumed = false;
            }
            RawGamepadEvent::Press(GamepadButton::North) => self.north_pressed = true,
            RawGamepadEvent::Release(GamepadButton::North) => {
                if self.north_pressed && !self.north_consumed && !self.select_pressed {
                    self.osk_sender
                        .send(GamepadCommand::ActivateSpace)
                        .context("failed to send OSK space command")?;
                    self.sidemenu_sender
                        .send(SideMenuCommand::ToggleScan)
                        .context("failed to send side menu scan command")?;
                }

                self.north_pressed = false;
                self.north_consumed = false;
            }
            RawGamepadEvent::Release(GamepadButton::RightTrigger2) => {
                self.osk_sender
                    .send(GamepadCommand::ActivateEnter)
                    .context("failed to send OSK enter command")?;
            }
            RawGamepadEvent::Press(GamepadButton::LeftThumb) => {
                self.osk_sender
                    .send(GamepadCommand::ToggleCapsLock)
                    .context("failed to send OSK caps lock command")?;
            }
            RawGamepadEvent::Press(GamepadButton::LeftTrigger2) => {
                if !self.l2_pressed {
                    self.l2_pressed = true;
                    self.osk_sender
                        .send(GamepadCommand::SetShiftHeld(true))
                        .context("failed to send OSK shift hold command")?;
                }
            }
            RawGamepadEvent::Release(GamepadButton::LeftTrigger2) => {
                if self.l2_pressed {
                    self.l2_pressed = false;
                    self.osk_sender
                        .send(GamepadCommand::SetShiftHeld(false))
                        .context("failed to send OSK shift release command")?;
                }
            }
            RawGamepadEvent::Release(GamepadButton::East) => {
                self.osk_sender
                    .send(GamepadCommand::CloseKeyboard)
                    .context("failed to send OSK close command")?;
                self.sidemenu_sender
                    .send(SideMenuCommand::CloseSideMenu)
                    .context("failed to send side menu close command")?;
                self.sidemenu_open = false;
                self.focused_row_index = 0;
                self.focused_row = FocusedRow::None;
            }
            RawGamepadEvent::Release(GamepadButton::West) => {
                self.osk_sender
                    .send(GamepadCommand::ActivateBackspace)
                    .context("failed to send OSK backspace command")?;
                self.sidemenu_sender
                    .send(SideMenuCommand::TerminateSelection)
                    .context("failed to send side menu terminate command")?;
            }
            RawGamepadEvent::Press(GamepadButton::DPadUp) => self
                .update_focused_row(KeyboardDirection::Up)
                .and_then(|_| self.send_selection_move(KeyboardDirection::Up))?,
            RawGamepadEvent::Press(GamepadButton::DPadDown) => self
                .update_focused_row(KeyboardDirection::Down)
                .and_then(|_| self.send_selection_move(KeyboardDirection::Down))?,
            RawGamepadEvent::Press(GamepadButton::DPadLeft) => {
                self.send_selection_move(KeyboardDirection::Left)?
            }
            RawGamepadEvent::Press(GamepadButton::DPadRight) => {
                self.send_selection_move(KeyboardDirection::Right)?
            }
            RawGamepadEvent::MoveSelection(direction) => self.send_selection_move(direction)?,
            RawGamepadEvent::Axis(_, _) => {}
            _ => {}
        }

        self.detect_combos()
    }

    /// Ticks repeat and analog Desktop Mode behavior.
    pub fn tick(&mut self) {
        self.desktop_mode.tick();
    }

    /// Returns whether Desktop Mode is active.
    pub fn desktop_mode_active(&self) -> bool {
        self.desktop_mode.is_active()
    }

    /// Applies a sampled Desktop Mode axis value.
    pub fn sample_desktop_axis(&mut self, axis: GamepadAxis, value: f32) {
        self.desktop_mode
            .handle_event(RawGamepadEvent::Axis(axis, value));
    }

    /// Updates the Desktop Mode mouse sensitivity.
    pub fn set_desktop_mouse_sensitivity(&mut self, sensitivity: f32) {
        self.desktop_mode.set_mouse_sensitivity(sensitivity);
    }

    /// Resets transient button and Desktop Mode input state after grab changes.
    pub fn reset_inputs(&mut self) {
        if self.l2_pressed {
            let _ = self.osk_sender.send(GamepadCommand::SetShiftHeld(false));
        }

        self.south_pressed = false;
        self.south_consumed = false;
        self.north_pressed = false;
        self.north_consumed = false;
        self.select_pressed = false;
        self.select_consumed = false;
        self.start_pressed = false;
        self.l2_pressed = false;
        self.osk_combo_armed = false;
        self.sidemenu_combo_armed = false;
        self.desktop_combo_armed = false;
        self.desktop_mode.reset_inputs();
    }

    fn detect_combos(&mut self) -> Result<()> {
        if self.select_pressed && self.start_pressed && !self.desktop_combo_armed {
            self.desktop_combo_armed = true;
            self.select_consumed = true;
            let enabled = !self.desktop_mode.is_active();
            self.desktop_mode.set_active(enabled);
            self.osk_sender
                .send(GamepadCommand::DesktopModeChanged(enabled))
                .context("failed to send desktop mode notification command")?;
        }

        if !self.select_pressed || !self.start_pressed {
            self.desktop_combo_armed = false;
        }

        if self.south_pressed && self.select_pressed && !self.osk_combo_armed {
            self.osk_combo_armed = true;
            self.south_consumed = true;
            self.select_consumed = true;
            self.osk_sender
                .send(GamepadCommand::ToggleKeyboard)
                .context("failed to send gamepad toggle command")?;
        }

        if !self.south_pressed || !self.select_pressed {
            self.osk_combo_armed = false;
        }

        if self.select_pressed && self.north_pressed && !self.sidemenu_combo_armed {
            self.sidemenu_combo_armed = true;
            self.select_consumed = true;
            self.north_consumed = true;
            self.sidemenu_open = !self.sidemenu_open;
            self.focused_row_index = 0;
            self.focused_row = if self.sidemenu_open {
                FocusedRow::Volume
            } else {
                FocusedRow::None
            };
            self.sidemenu_sender
                .send(SideMenuCommand::ToggleSideMenu)
                .context("failed to send side menu toggle command")?;
        }

        if !self.select_pressed || !self.north_pressed {
            self.sidemenu_combo_armed = false;
        }

        Ok(())
    }

    fn update_focused_row(&mut self, direction: KeyboardDirection) -> Result<()> {
        if !self.sidemenu_open {
            return Ok(());
        }

        match direction {
            KeyboardDirection::Up => {
                self.focused_row_index = self.focused_row_index.saturating_sub(1)
            }
            KeyboardDirection::Down => self.focused_row_index = (self.focused_row_index + 1).min(5),
            KeyboardDirection::Left | KeyboardDirection::Right => {}
        }

        self.focused_row = match self.focused_row_index {
            0 => FocusedRow::Volume,
            1 => FocusedRow::Wifi,
            2 => FocusedRow::TaskSwitcher,
            3 => FocusedRow::Bluetooth,
            4 => FocusedRow::Configuration,
            _ => FocusedRow::None,
        };

        Ok(())
    }

    fn send_selection_move(&self, direction: KeyboardDirection) -> Result<()> {
        self.osk_sender
            .send(GamepadCommand::MoveSelection(direction))
            .context("failed to send OSK selection command")?;
        self.sidemenu_sender
            .send(SideMenuCommand::MoveSelection(direction))
            .context("failed to send side menu selection command")?;

        Ok(())
    }
}

struct DesktopModeState {
    active: bool,
    input: Box<dyn InputBackend>,
    mouse_sensitivity: f32,
    right_stick_x: f32,
    right_stick_y: f32,
    left_stick_y: f32,
    left_button_down: bool,
    right_button_down: bool,
    dpad_up: DpadRepeat,
    dpad_down: DpadRepeat,
    dpad_left: DpadRepeat,
    dpad_right: DpadRepeat,
}

impl DesktopModeState {
    fn new(input: Box<dyn InputBackend>) -> Self {
        Self {
            active: false,
            input,
            mouse_sensitivity: config::DEFAULT_DESKTOP_MOUSE_SENSITIVITY,
            right_stick_x: 0.0,
            right_stick_y: 0.0,
            left_stick_y: 0.0,
            left_button_down: false,
            right_button_down: false,
            dpad_up: DpadRepeat::default(),
            dpad_down: DpadRepeat::default(),
            dpad_left: DpadRepeat::default(),
            dpad_right: DpadRepeat::default(),
        }
    }

    fn is_active(&self) -> bool {
        self.active
    }

    fn set_mouse_sensitivity(&mut self, sensitivity: f32) {
        self.mouse_sensitivity = config::sanitize_desktop_mouse_sensitivity(sensitivity);
    }

    fn set_active(&mut self, active: bool) {
        if self.active == active {
            return;
        }

        if !active {
            self.release_pointer_buttons();
        }

        self.active = active;
        self.reset_inputs();

        eprintln!(
            "Desktop Mode {}",
            if self.active { "enabled" } else { "disabled" }
        );
    }

    fn reset_inputs(&mut self) {
        self.right_stick_x = 0.0;
        self.right_stick_y = 0.0;
        self.left_stick_y = 0.0;
        self.dpad_up.reset();
        self.dpad_down.reset();
        self.dpad_left.reset();
        self.dpad_right.reset();
    }

    fn handle_event(&mut self, event: RawGamepadEvent) {
        match event {
            RawGamepadEvent::Axis(GamepadAxis::RightStickX, value) => self.right_stick_x = value,
            RawGamepadEvent::Axis(GamepadAxis::RightStickY, value) => self.right_stick_y = value,
            RawGamepadEvent::Axis(GamepadAxis::LeftStickY, value) => self.left_stick_y = value,
            RawGamepadEvent::Axis(GamepadAxis::RightZ, value) => {
                self.set_mouse_button(MouseButton::Left, value > 0.5)
            }
            RawGamepadEvent::Axis(GamepadAxis::LeftZ, value) => {
                self.set_mouse_button(MouseButton::Right, value > 0.5)
            }
            RawGamepadEvent::Press(GamepadButton::RightThumb) => {
                self.click_mouse(MouseButton::Middle)
            }
            RawGamepadEvent::Press(GamepadButton::DPadUp) => self.press_dpad(KeyCode::Up),
            RawGamepadEvent::Release(GamepadButton::DPadUp) => self.dpad_up.reset(),
            RawGamepadEvent::Press(GamepadButton::DPadDown) => self.press_dpad(KeyCode::Down),
            RawGamepadEvent::Release(GamepadButton::DPadDown) => self.dpad_down.reset(),
            RawGamepadEvent::Press(GamepadButton::DPadLeft) => self.press_dpad(KeyCode::Left),
            RawGamepadEvent::Release(GamepadButton::DPadLeft) => self.dpad_left.reset(),
            RawGamepadEvent::Press(GamepadButton::DPadRight) => self.press_dpad(KeyCode::Right),
            RawGamepadEvent::Release(GamepadButton::DPadRight) => self.dpad_right.reset(),
            RawGamepadEvent::Press(GamepadButton::LeftTrigger2) => {
                self.set_mouse_button(MouseButton::Right, true)
            }
            RawGamepadEvent::Release(GamepadButton::LeftTrigger2) => {
                self.set_mouse_button(MouseButton::Right, false)
            }
            RawGamepadEvent::Press(GamepadButton::RightTrigger2) => {
                self.set_mouse_button(MouseButton::Left, true)
            }
            RawGamepadEvent::Release(GamepadButton::RightTrigger2) => {
                self.set_mouse_button(MouseButton::Left, false)
            }
            _ => {}
        }
    }

    fn tick(&mut self) {
        if !self.active {
            return;
        }

        self.move_pointer();
        self.scroll();
        self.repeat_dpad();
    }

    fn move_pointer(&self) {
        let max_speed = POINTER_MAX_SPEED * self.mouse_sensitivity;
        let dx = accelerated_axis_delta(self.right_stick_x, 1.6, max_speed);
        let dy = accelerated_axis_delta(self.right_stick_y, 1.6, max_speed);

        if dx == 0 && dy == 0 {
            return;
        }

        if let Err(error) = self.input.move_pointer_relative(dx, dy) {
            eprintln!("Failed to move desktop pointer: {error:#}");
        }
    }

    fn scroll(&self) {
        let dy = accelerated_axis_delta(self.left_stick_y, 1.4, SCROLL_MAX_SPEED);

        if dy == 0 {
            return;
        }

        if let Err(error) = self.input.scroll(dy) {
            eprintln!("Failed to scroll desktop pointer: {error:#}");
        }
    }

    fn repeat_dpad(&mut self) {
        let now = Instant::now();
        Self::repeat_dpad_key(&*self.input, &mut self.dpad_up, KeyCode::Up, now);
        Self::repeat_dpad_key(&*self.input, &mut self.dpad_down, KeyCode::Down, now);
        Self::repeat_dpad_key(&*self.input, &mut self.dpad_left, KeyCode::Left, now);
        Self::repeat_dpad_key(&*self.input, &mut self.dpad_right, KeyCode::Right, now);
    }

    fn repeat_dpad_key(
        input: &dyn InputBackend,
        repeat: &mut DpadRepeat,
        key: KeyCode,
        now: Instant,
    ) {
        if !repeat.should_repeat(now) {
            return;
        }

        if let Err(error) = input.tap_key(key) {
            eprintln!("Failed to tap desktop arrow key: {error:#}");
        }
    }

    fn press_dpad(&mut self, key: KeyCode) {
        let repeat = match key {
            KeyCode::Up => &mut self.dpad_up,
            KeyCode::Down => &mut self.dpad_down,
            KeyCode::Left => &mut self.dpad_left,
            KeyCode::Right => &mut self.dpad_right,
            _ => return,
        };

        if repeat.press(Instant::now()) {
            if let Err(error) = self.input.tap_key(key) {
                eprintln!("Failed to tap desktop arrow key: {error:#}");
            }
        }
    }

    fn set_mouse_button(&mut self, button: MouseButton, pressed: bool) {
        let state = match button {
            MouseButton::Left => &mut self.left_button_down,
            MouseButton::Right => &mut self.right_button_down,
            MouseButton::Middle => return,
        };

        if *state == pressed {
            return;
        }

        let result = if pressed {
            self.input.button_down(button)
        } else {
            self.input.button_up(button)
        };

        match result {
            Ok(()) => *state = pressed,
            Err(error) => eprintln!("Failed to update desktop mouse button: {error:#}"),
        }
    }

    fn click_mouse(&self, button: MouseButton) {
        if let Err(error) = self.input.click(button) {
            eprintln!("Failed to click desktop mouse button: {error:#}");
        }
    }

    fn release_pointer_buttons(&mut self) {
        self.set_mouse_button(MouseButton::Left, false);
        self.set_mouse_button(MouseButton::Right, false);
    }
}

#[derive(Default)]
struct DpadRepeat {
    held: bool,
    next_repeat: Option<Instant>,
}

impl DpadRepeat {
    fn press(&mut self, now: Instant) -> bool {
        if self.held {
            return false;
        }

        self.held = true;
        self.next_repeat = Some(now + DPAD_REPEAT_INITIAL_DELAY);
        true
    }

    fn reset(&mut self) {
        self.held = false;
        self.next_repeat = None;
    }

    fn should_repeat(&mut self, now: Instant) -> bool {
        let Some(next_repeat) = self.next_repeat else {
            return false;
        };

        if !self.held || now < next_repeat {
            return false;
        }

        self.next_repeat = Some(now + DPAD_REPEAT_INTERVAL);
        true
    }
}

fn accelerated_axis_delta(value: f32, exponent: f32, max_speed: f32) -> i32 {
    if value.abs() < POINTER_DEADZONE {
        return 0;
    }

    (value.abs().powf(exponent) * max_speed * value.signum()) as i32
}

use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use evdev::{AbsoluteAxisType, Device, InputEvent, InputEventKind, Key};
use gilrs::{Axis, Button, Event, EventType as GilrsEventType, Gamepad, Gilrs, LinuxGamepadExt};

use crate::config;
use crate::uinput::{MouseButton, SharedVirtualKeyboard, SharedVirtualMouse};

const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
const O_NONBLOCK: i32 = 0o0004000;
const POINTER_DEADZONE: f32 = 0.12;
const POINTER_MAX_SPEED: f32 = 18.0;
const SCROLL_MAX_SPEED: f32 = 3.0;
const DPAD_REPEAT_INITIAL_DELAY: Duration = Duration::from_millis(400);
const DPAD_REPEAT_INTERVAL: Duration = Duration::from_millis(120);

extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

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
#[allow(dead_code)]
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

/// Spawns the focus-independent gamepad event loop on a background thread.
pub fn spawn_gamepad_thread(
    osk_sender: Sender<GamepadCommand>,
    sidemenu_sender: Sender<SideMenuCommand>,
    grab_receiver: Receiver<GamepadGrabCommand>,
    virtual_keyboard: SharedVirtualKeyboard,
    virtual_mouse: SharedVirtualMouse,
) -> Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("gameease-gamepad".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("Failed to create gamepad runtime: {error:#}");
                    return;
                }
            };

            if let Err(error) = runtime.block_on(run_event_loop(
                osk_sender,
                sidemenu_sender,
                grab_receiver,
                virtual_keyboard,
                virtual_mouse,
            )) {
                eprintln!("Gamepad event loop stopped: {error:#}");
            }
        })
        .context("failed to spawn gamepad thread")
}

async fn run_event_loop(
    osk_sender: Sender<GamepadCommand>,
    sidemenu_sender: Sender<SideMenuCommand>,
    grab_receiver: Receiver<GamepadGrabCommand>,
    virtual_keyboard: SharedVirtualKeyboard,
    virtual_mouse: SharedVirtualMouse,
) -> Result<()> {
    let mut gilrs =
        Gilrs::new().map_err(|error| anyhow!("failed to initialise gilrs: {error:?}"))?;
    let mut grab_manager = GamepadGrabManager::default();
    let mut exclusive_requested = false;
    let mut exclusive_desired = false;
    let mut exclusive_active = false;
    let mut south_pressed = false;
    let mut south_consumed = false;
    let mut north_pressed = false;
    let mut north_consumed = false;
    let mut select_pressed = false;
    let mut select_consumed = false;
    let mut start_pressed = false;
    let mut l2_pressed = false;
    let mut osk_combo_armed = false;
    let mut sidemenu_combo_armed = false;
    let mut desktop_combo_armed = false;
    let mut sidemenu_open = false;
    let mut focused_row_index = 0usize;
    let mut focused_row = FocusedRow::None;
    let mut desktop_mode = DesktopModeState::new(virtual_keyboard, virtual_mouse);

    loop {
        while let Ok(command) = grab_receiver.try_recv() {
            match command {
                GamepadGrabCommand::SetExclusive(enabled) if enabled != exclusive_requested => {
                    exclusive_requested = enabled;
                }
                GamepadGrabCommand::SetExclusive(_) => {}
                GamepadGrabCommand::SetDesktopMouseSensitivity(sensitivity) => {
                    desktop_mode.set_mouse_sensitivity(sensitivity);
                }
            }
        }

        if sync_gamepad_grab(
            &mut grab_manager,
            &gilrs,
            exclusive_requested || desktop_mode.is_active(),
            &mut exclusive_desired,
            &mut exclusive_active,
        ) {
            reset_button_state(
                &mut south_pressed,
                &mut south_consumed,
                &mut north_pressed,
                &mut north_consumed,
                &mut select_pressed,
                &mut select_consumed,
                &mut start_pressed,
                &mut l2_pressed,
                &mut osk_combo_armed,
                &mut sidemenu_combo_armed,
                &mut desktop_combo_armed,
                &osk_sender,
            );
            desktop_mode.reset_inputs();
        }

        let events = if exclusive_active {
            grab_manager.poll_events()
        } else {
            let mut events = Vec::new();

            while let Some(Event { event, .. }) = gilrs.next_event() {
                push_raw_events_from_gilrs(event, &mut events);
            }

            events
        };

        for event in events {
            update_button_state(
                event,
                &mut south_pressed,
                &mut south_consumed,
                &mut north_pressed,
                &mut north_consumed,
                &mut select_pressed,
                &mut select_consumed,
                &mut start_pressed,
                &mut l2_pressed,
                &mut osk_combo_armed,
                &mut sidemenu_combo_armed,
                &mut desktop_combo_armed,
                &mut sidemenu_open,
                &mut focused_row_index,
                &mut focused_row,
                &mut desktop_mode,
                &osk_sender,
                &sidemenu_sender,
            )?;
        }

        if desktop_mode.is_active() && !exclusive_active {
            sample_desktop_axes_from_gilrs(&gilrs, &mut desktop_mode);
        }

        desktop_mode.tick();

        if sync_gamepad_grab(
            &mut grab_manager,
            &gilrs,
            exclusive_requested || desktop_mode.is_active(),
            &mut exclusive_desired,
            &mut exclusive_active,
        ) {
            reset_button_state(
                &mut south_pressed,
                &mut south_consumed,
                &mut north_pressed,
                &mut north_consumed,
                &mut select_pressed,
                &mut select_consumed,
                &mut start_pressed,
                &mut l2_pressed,
                &mut osk_combo_armed,
                &mut sidemenu_combo_armed,
                &mut desktop_combo_armed,
                &osk_sender,
            );
            desktop_mode.reset_inputs();
        }

        tokio::time::sleep(Duration::from_millis(16)).await;
    }
}

fn reset_button_state(
    south_pressed: &mut bool,
    south_consumed: &mut bool,
    north_pressed: &mut bool,
    north_consumed: &mut bool,
    select_pressed: &mut bool,
    select_consumed: &mut bool,
    start_pressed: &mut bool,
    l2_pressed: &mut bool,
    osk_combo_armed: &mut bool,
    sidemenu_combo_armed: &mut bool,
    desktop_combo_armed: &mut bool,
    osk_sender: &Sender<GamepadCommand>,
) {
    if *l2_pressed {
        let _ = osk_sender.send(GamepadCommand::SetShiftHeld(false));
    }

    *south_pressed = false;
    *south_consumed = false;
    *north_pressed = false;
    *north_consumed = false;
    *select_pressed = false;
    *select_consumed = false;
    *start_pressed = false;
    *l2_pressed = false;
    *osk_combo_armed = false;
    *sidemenu_combo_armed = false;
    *desktop_combo_armed = false;
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RawGamepadEvent {
    Press(Button),
    Release(Button),
    Axis(Axis, f32),
    MoveSelection(KeyboardDirection),
}

fn update_button_state(
    event: RawGamepadEvent,
    south_pressed: &mut bool,
    south_consumed: &mut bool,
    north_pressed: &mut bool,
    north_consumed: &mut bool,
    select_pressed: &mut bool,
    select_consumed: &mut bool,
    start_pressed: &mut bool,
    l2_pressed: &mut bool,
    osk_combo_armed: &mut bool,
    sidemenu_combo_armed: &mut bool,
    desktop_combo_armed: &mut bool,
    sidemenu_open: &mut bool,
    focused_row_index: &mut usize,
    focused_row: &mut FocusedRow,
    desktop_mode: &mut DesktopModeState,
    osk_sender: &Sender<GamepadCommand>,
    sidemenu_sender: &Sender<SideMenuCommand>,
) -> Result<()> {
    match event {
        RawGamepadEvent::Press(Button::Select) => *select_pressed = true,
        RawGamepadEvent::Release(Button::Select) => {
            if !desktop_mode.is_active()
                && *select_pressed
                && !*select_consumed
                && !*south_pressed
                && !*north_pressed
                && !*start_pressed
            {
                osk_sender
                    .send(GamepadCommand::ToggleKeyboardPosition)
                    .context("failed to send OSK position command")?;
            }

            *select_pressed = false;
            *select_consumed = false;
        }
        RawGamepadEvent::Press(Button::Start) => *start_pressed = true,
        RawGamepadEvent::Release(Button::Start) => *start_pressed = false,
        _ if desktop_mode.is_active() => desktop_mode.handle_event(event),
        RawGamepadEvent::Press(Button::South) => {
            *south_pressed = true;
        }
        RawGamepadEvent::Release(Button::South) => {
            if *south_pressed && !*south_consumed && !*select_pressed {
                osk_sender
                    .send(GamepadCommand::ActivateSelection)
                    .context("failed to send OSK activation command")?;
                sidemenu_sender
                    .send(SideMenuCommand::ActivateSelection)
                    .context("failed to send side menu activation command")?;
            }

            *south_pressed = false;
            *south_consumed = false;
        }
        RawGamepadEvent::Press(Button::North) => *north_pressed = true,
        RawGamepadEvent::Release(Button::North) => {
            if *north_pressed && !*north_consumed && !*select_pressed {
                osk_sender
                    .send(GamepadCommand::ActivateSpace)
                    .context("failed to send OSK space command")?;
                sidemenu_sender
                    .send(SideMenuCommand::ToggleScan)
                    .context("failed to send side menu scan command")?;
            }

            *north_pressed = false;
            *north_consumed = false;
        }
        RawGamepadEvent::Release(Button::RightTrigger2) => {
            osk_sender
                .send(GamepadCommand::ActivateEnter)
                .context("failed to send OSK enter command")?;
        }
        RawGamepadEvent::Press(Button::LeftThumb) => {
            osk_sender
                .send(GamepadCommand::ToggleCapsLock)
                .context("failed to send OSK caps lock command")?;
        }
        RawGamepadEvent::Press(Button::LeftTrigger2) => {
            if !*l2_pressed {
                *l2_pressed = true;
                osk_sender
                    .send(GamepadCommand::SetShiftHeld(true))
                    .context("failed to send OSK shift hold command")?;
            }
        }
        RawGamepadEvent::Release(Button::LeftTrigger2) => {
            if *l2_pressed {
                *l2_pressed = false;
                osk_sender
                    .send(GamepadCommand::SetShiftHeld(false))
                    .context("failed to send OSK shift release command")?;
            }
        }
        RawGamepadEvent::Release(Button::East) => {
            osk_sender
                .send(GamepadCommand::CloseKeyboard)
                .context("failed to send OSK close command")?;
            sidemenu_sender
                .send(SideMenuCommand::CloseSideMenu)
                .context("failed to send side menu close command")?;
            *sidemenu_open = false;
            *focused_row_index = 0;
            *focused_row = FocusedRow::None;
        }
        RawGamepadEvent::Release(Button::West) => {
            osk_sender
                .send(GamepadCommand::ActivateBackspace)
                .context("failed to send OSK backspace command")?;
            sidemenu_sender
                .send(SideMenuCommand::TerminateSelection)
                .context("failed to send side menu terminate command")?;
        }
        RawGamepadEvent::Press(Button::DPadUp) => update_focused_row(
            KeyboardDirection::Up,
            sidemenu_open,
            focused_row_index,
            focused_row,
        )
        .and_then(|_| send_selection_move(osk_sender, sidemenu_sender, KeyboardDirection::Up))?,
        RawGamepadEvent::Press(Button::DPadDown) => update_focused_row(
            KeyboardDirection::Down,
            sidemenu_open,
            focused_row_index,
            focused_row,
        )
        .and_then(|_| send_selection_move(osk_sender, sidemenu_sender, KeyboardDirection::Down))?,
        RawGamepadEvent::Press(Button::DPadLeft) => {
            send_selection_move(osk_sender, sidemenu_sender, KeyboardDirection::Left)?
        }
        RawGamepadEvent::Press(Button::DPadRight) => {
            send_selection_move(osk_sender, sidemenu_sender, KeyboardDirection::Right)?
        }
        RawGamepadEvent::MoveSelection(direction) => {
            send_selection_move(osk_sender, sidemenu_sender, direction)?
        }
        RawGamepadEvent::Axis(_, _) => {}
        _ => {}
    }

    if *select_pressed && *start_pressed && !*desktop_combo_armed {
        *desktop_combo_armed = true;
        *select_consumed = true;
        let enabled = !desktop_mode.is_active();
        desktop_mode.set_active(enabled);
        osk_sender
            .send(GamepadCommand::DesktopModeChanged(enabled))
            .context("failed to send desktop mode notification command")?;
    }

    if !*select_pressed || !*start_pressed {
        *desktop_combo_armed = false;
    }

    if *south_pressed && *select_pressed && !*osk_combo_armed {
        *osk_combo_armed = true;
        *south_consumed = true;
        *select_consumed = true;
        osk_sender
            .send(GamepadCommand::ToggleKeyboard)
            .context("failed to send gamepad toggle command")?;
    }

    if !*south_pressed || !*select_pressed {
        *osk_combo_armed = false;
    }

    if *select_pressed && *north_pressed && !*sidemenu_combo_armed {
        *sidemenu_combo_armed = true;
        *select_consumed = true;
        *north_consumed = true;
        *sidemenu_open = !*sidemenu_open;
        *focused_row_index = 0;
        *focused_row = if *sidemenu_open {
            FocusedRow::Volume
        } else {
            FocusedRow::None
        };
        sidemenu_sender
            .send(SideMenuCommand::ToggleSideMenu)
            .context("failed to send side menu toggle command")?;
    }

    if !*select_pressed || !*north_pressed {
        *sidemenu_combo_armed = false;
    }

    Ok(())
}

fn update_focused_row(
    direction: KeyboardDirection,
    sidemenu_open: &bool,
    focused_row_index: &mut usize,
    focused_row: &mut FocusedRow,
) -> Result<()> {
    if !*sidemenu_open {
        return Ok(());
    }

    match direction {
        KeyboardDirection::Up => *focused_row_index = focused_row_index.saturating_sub(1),
        KeyboardDirection::Down => *focused_row_index = (*focused_row_index + 1).min(5),
        KeyboardDirection::Left | KeyboardDirection::Right => {}
    }

    *focused_row = match *focused_row_index {
        0 => FocusedRow::Volume,
        1 => FocusedRow::Wifi,
        2 => FocusedRow::TaskSwitcher,
        3 => FocusedRow::Bluetooth,
        4 => FocusedRow::Configuration,
        _ => FocusedRow::None,
    };

    Ok(())
}

struct DesktopModeState {
    active: bool,
    virtual_keyboard: SharedVirtualKeyboard,
    virtual_mouse: SharedVirtualMouse,
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
    fn new(virtual_keyboard: SharedVirtualKeyboard, virtual_mouse: SharedVirtualMouse) -> Self {
        Self {
            active: false,
            virtual_keyboard,
            virtual_mouse,
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
            RawGamepadEvent::Axis(Axis::RightStickX, value) => self.right_stick_x = value,
            RawGamepadEvent::Axis(Axis::RightStickY, value) => self.right_stick_y = value,
            RawGamepadEvent::Axis(Axis::LeftStickY, value) => self.left_stick_y = value,
            RawGamepadEvent::Axis(Axis::RightZ, value) => {
                self.set_mouse_button(MouseButton::Left, value > 0.5)
            }
            RawGamepadEvent::Axis(Axis::LeftZ, value) => {
                self.set_mouse_button(MouseButton::Right, value > 0.5)
            }
            RawGamepadEvent::Press(Button::RightThumb) => self.click_mouse(MouseButton::Middle),
            RawGamepadEvent::Press(Button::DPadUp) => self.press_dpad(Key::KEY_UP),
            RawGamepadEvent::Release(Button::DPadUp) => self.dpad_up.reset(),
            RawGamepadEvent::Press(Button::DPadDown) => self.press_dpad(Key::KEY_DOWN),
            RawGamepadEvent::Release(Button::DPadDown) => self.dpad_down.reset(),
            RawGamepadEvent::Press(Button::DPadLeft) => self.press_dpad(Key::KEY_LEFT),
            RawGamepadEvent::Release(Button::DPadLeft) => self.dpad_left.reset(),
            RawGamepadEvent::Press(Button::DPadRight) => self.press_dpad(Key::KEY_RIGHT),
            RawGamepadEvent::Release(Button::DPadRight) => self.dpad_right.reset(),
            RawGamepadEvent::Press(Button::LeftTrigger2) => {
                self.set_mouse_button(MouseButton::Right, true)
            }
            RawGamepadEvent::Release(Button::LeftTrigger2) => {
                self.set_mouse_button(MouseButton::Right, false)
            }
            RawGamepadEvent::Press(Button::RightTrigger2) => {
                self.set_mouse_button(MouseButton::Left, true)
            }
            RawGamepadEvent::Release(Button::RightTrigger2) => {
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

    fn move_pointer(&mut self) {
        let max_speed = POINTER_MAX_SPEED * self.mouse_sensitivity;
        let dx = accelerated_axis_delta(self.right_stick_x, 1.6, max_speed);
        let dy = accelerated_axis_delta(self.right_stick_y, 1.6, max_speed);

        if dx == 0 && dy == 0 {
            return;
        }

        let result = self
            .virtual_mouse
            .lock()
            .map_err(|error| anyhow!("virtual mouse lock poisoned: {error}"))
            .and_then(|mut mouse| mouse.move_relative(dx, dy));

        if let Err(error) = result {
            eprintln!("Failed to move desktop pointer: {error:#}");
        }
    }

    fn scroll(&mut self) {
        let dy = accelerated_axis_delta(self.left_stick_y, 1.4, SCROLL_MAX_SPEED);

        if dy == 0 {
            return;
        }

        let result = self
            .virtual_mouse
            .lock()
            .map_err(|error| anyhow!("virtual mouse lock poisoned: {error}"))
            .and_then(|mut mouse| mouse.scroll(dy));

        if let Err(error) = result {
            eprintln!("Failed to scroll desktop pointer: {error:#}");
        }
    }

    fn repeat_dpad(&mut self) {
        let now = Instant::now();
        Self::repeat_dpad_key(&self.virtual_keyboard, &mut self.dpad_up, Key::KEY_UP, now);
        Self::repeat_dpad_key(
            &self.virtual_keyboard,
            &mut self.dpad_down,
            Key::KEY_DOWN,
            now,
        );
        Self::repeat_dpad_key(
            &self.virtual_keyboard,
            &mut self.dpad_left,
            Key::KEY_LEFT,
            now,
        );
        Self::repeat_dpad_key(
            &self.virtual_keyboard,
            &mut self.dpad_right,
            Key::KEY_RIGHT,
            now,
        );
    }

    fn repeat_dpad_key(
        virtual_keyboard: &SharedVirtualKeyboard,
        repeat: &mut DpadRepeat,
        key: Key,
        now: Instant,
    ) {
        if !repeat.should_repeat(now) {
            return;
        }

        tap_virtual_key(virtual_keyboard, key);
    }

    fn press_dpad(&mut self, key: Key) {
        let repeat = match key {
            Key::KEY_UP => &mut self.dpad_up,
            Key::KEY_DOWN => &mut self.dpad_down,
            Key::KEY_LEFT => &mut self.dpad_left,
            Key::KEY_RIGHT => &mut self.dpad_right,
            _ => return,
        };

        if repeat.press(Instant::now()) {
            tap_virtual_key(&self.virtual_keyboard, key);
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

        let result = self
            .virtual_mouse
            .lock()
            .map_err(|error| anyhow!("virtual mouse lock poisoned: {error}"))
            .and_then(|mut mouse| {
                if pressed {
                    mouse.button_down(button)
                } else {
                    mouse.button_up(button)
                }
            });

        match result {
            Ok(()) => *state = pressed,
            Err(error) => eprintln!("Failed to update desktop mouse button: {error:#}"),
        }
    }

    fn click_mouse(&mut self, button: MouseButton) {
        let result = self
            .virtual_mouse
            .lock()
            .map_err(|error| anyhow!("virtual mouse lock poisoned: {error}"))
            .and_then(|mut mouse| mouse.click(button));

        if let Err(error) = result {
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

fn tap_virtual_key(virtual_keyboard: &SharedVirtualKeyboard, key: Key) {
    let result = virtual_keyboard
        .lock()
        .map_err(|error| anyhow!("virtual keyboard lock poisoned: {error}"))
        .and_then(|mut keyboard| keyboard.tap(key));

    if let Err(error) = result {
        eprintln!("Failed to tap desktop arrow key: {error:#}");
    }
}

fn sync_gamepad_grab(
    grab_manager: &mut GamepadGrabManager,
    gilrs: &Gilrs,
    desired_active: bool,
    exclusive_desired: &mut bool,
    exclusive_active: &mut bool,
) -> bool {
    if desired_active == *exclusive_desired {
        return false;
    }

    *exclusive_desired = desired_active;

    if desired_active {
        grab_manager.enable(gilrs);
        *exclusive_active = grab_manager.is_active();
    } else {
        grab_manager.disable();
        *exclusive_active = false;
    }

    true
}

fn sample_desktop_axes_from_gilrs(gilrs: &Gilrs, desktop_mode: &mut DesktopModeState) {
    let Some((_, gamepad)) = gilrs.gamepads().next() else {
        return;
    };

    for axis in [
        Axis::RightStickX,
        Axis::RightStickY,
        Axis::LeftStickY,
        Axis::LeftZ,
        Axis::RightZ,
    ] {
        desktop_mode.handle_event(RawGamepadEvent::Axis(axis, gamepad.value(axis)));
    }
}

#[derive(Default)]
struct GamepadGrabManager {
    devices: Vec<GrabbedGamepad>,
}

struct GrabbedGamepad {
    path: PathBuf,
    device: Device,
    mapped_buttons: Vec<(u32, Button)>,
    mapped_axes: Vec<(u32, Axis)>,
    axis_ranges: Vec<(AbsoluteAxisType, AxisRange)>,
    dpad_x: i32,
    dpad_y: i32,
    left_trigger_z: i32,
    right_trigger_z: i32,
    right_stick_x: i32,
}

impl GamepadGrabManager {
    fn enable(&mut self, gilrs: &Gilrs) {
        self.disable();

        for (_, gamepad) in gilrs.gamepads() {
            let path = gamepad.devpath().to_path_buf();

            match open_grabbed_device(&path) {
                Ok(device) => {
                    let axis_ranges = axis_ranges_for_device(&device);
                    self.devices.push(GrabbedGamepad {
                        path,
                        device,
                        mapped_buttons: mapped_gamepad_buttons(&gamepad),
                        mapped_axes: mapped_gamepad_axes(&gamepad),
                        axis_ranges,
                        dpad_x: 0,
                        dpad_y: 0,
                        left_trigger_z: 0,
                        right_trigger_z: 0,
                        right_stick_x: 0,
                    });
                }
                Err(error) => {
                    eprintln!("Failed to grab gamepad device {}: {error}", path.display());
                }
            }
        }

        if self.devices.is_empty() {
            eprintln!("Exclusive gamepad mode requested, but no gamepads could be grabbed");
        }
    }

    fn disable(&mut self) {
        for grabbed in &mut self.devices {
            if let Err(error) = grabbed.device.ungrab() {
                eprintln!(
                    "Failed to ungrab gamepad device {}: {error}",
                    grabbed.path.display()
                );
            }
        }

        self.devices.clear();
    }

    fn is_active(&self) -> bool {
        !self.devices.is_empty()
    }

    fn poll_events(&mut self) -> Vec<RawGamepadEvent> {
        let mut raw_events = Vec::new();

        for grabbed in &mut self.devices {
            let input_events = match grabbed.device.fetch_events() {
                Ok(events) => events.collect::<Vec<InputEvent>>(),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => Vec::new(),
                Err(error) => {
                    eprintln!(
                        "Failed to read grabbed gamepad device {}: {error}",
                        grabbed.path.display()
                    );
                    Vec::new()
                }
            };

            for event in input_events {
                translate_evdev_event(
                    event,
                    &grabbed.mapped_buttons,
                    &grabbed.mapped_axes,
                    &grabbed.axis_ranges,
                    &mut grabbed.dpad_x,
                    &mut grabbed.dpad_y,
                    &mut grabbed.left_trigger_z,
                    &mut grabbed.right_trigger_z,
                    &mut grabbed.right_stick_x,
                    &mut raw_events,
                );
            }
        }

        raw_events
    }
}

fn open_grabbed_device(path: &Path) -> io::Result<Device> {
    let mut device = Device::open(path)?;
    set_nonblocking(&device)?;
    device.grab()?;

    Ok(device)
}

fn set_nonblocking(device: &Device) -> io::Result<()> {
    let fd = device.as_raw_fd();
    let flags = unsafe { fcntl(fd, F_GETFL) };

    if flags < 0 {
        return Err(io::Error::last_os_error());
    }

    let result = unsafe { fcntl(fd, F_SETFL, flags | O_NONBLOCK) };

    if result < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

fn push_raw_events_from_gilrs(event: GilrsEventType, raw_events: &mut Vec<RawGamepadEvent>) {
    match event {
        GilrsEventType::ButtonPressed(button, _) => raw_events.push(RawGamepadEvent::Press(button)),
        GilrsEventType::ButtonReleased(button, _) => {
            raw_events.push(RawGamepadEvent::Release(button))
        }
        GilrsEventType::AxisChanged(axis, value, _)
            if matches!(
                axis,
                Axis::RightStickX
                    | Axis::RightStickY
                    | Axis::LeftStickY
                    | Axis::LeftZ
                    | Axis::RightZ
            ) =>
        {
            raw_events.push(RawGamepadEvent::Axis(axis, value));

            match axis {
                Axis::RightStickX if value >= 0.65 => {
                    raw_events.push(RawGamepadEvent::MoveSelection(KeyboardDirection::Right));
                }
                Axis::RightStickX if value <= -0.65 => {
                    raw_events.push(RawGamepadEvent::MoveSelection(KeyboardDirection::Left));
                }
                Axis::LeftZ if value >= 0.5 => {
                    raw_events.push(RawGamepadEvent::Press(Button::LeftTrigger2));
                }
                Axis::LeftZ if value <= 0.2 => {
                    raw_events.push(RawGamepadEvent::Release(Button::LeftTrigger2));
                }
                Axis::RightZ if value >= 0.5 => {
                    raw_events.push(RawGamepadEvent::Press(Button::RightTrigger2));
                }
                Axis::RightZ if value <= 0.2 => {
                    raw_events.push(RawGamepadEvent::Release(Button::RightTrigger2));
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn translate_evdev_event(
    event: InputEvent,
    mapped_buttons: &[(u32, Button)],
    mapped_axes: &[(u32, Axis)],
    axis_ranges: &[(AbsoluteAxisType, AxisRange)],
    dpad_x: &mut i32,
    dpad_y: &mut i32,
    left_trigger_z: &mut i32,
    right_trigger_z: &mut i32,
    right_stick_x: &mut i32,
    raw_events: &mut Vec<RawGamepadEvent>,
) {
    match event.kind() {
        InputEventKind::Key(key) => translate_evdev_key(event, key, mapped_buttons, raw_events),
        InputEventKind::AbsAxis(axis)
            if mapped_axis_from_event(event, mapped_axes) == Some(Axis::LeftZ)
                || axis == AbsoluteAxisType::ABS_Z =>
        {
            translate_left_trigger(
                event.value(),
                axis_range(axis_ranges, axis),
                left_trigger_z,
                raw_events,
            )
        }
        InputEventKind::AbsAxis(axis)
            if mapped_axis_from_event(event, mapped_axes) == Some(Axis::RightZ)
                || axis == AbsoluteAxisType::ABS_RZ =>
        {
            translate_right_trigger(
                event.value(),
                axis_range(axis_ranges, axis),
                right_trigger_z,
                raw_events,
            )
        }
        InputEventKind::AbsAxis(AbsoluteAxisType::ABS_HAT0X) => translate_hat_axis(
            event.value(),
            dpad_x,
            Button::DPadLeft,
            Button::DPadRight,
            raw_events,
        ),
        InputEventKind::AbsAxis(AbsoluteAxisType::ABS_HAT0Y) => translate_hat_axis(
            event.value(),
            dpad_y,
            Button::DPadUp,
            Button::DPadDown,
            raw_events,
        ),
        InputEventKind::AbsAxis(axis)
            if mapped_axis_from_event(event, mapped_axes) == Some(Axis::LeftStickY)
                || axis == AbsoluteAxisType::ABS_Y =>
        {
            translate_left_stick_y(event.value(), axis_range(axis_ranges, axis), raw_events)
        }
        InputEventKind::AbsAxis(axis)
            if mapped_axis_from_event(event, mapped_axes) == Some(Axis::RightStickX)
                || axis == AbsoluteAxisType::ABS_RX =>
        {
            translate_right_stick_x(
                event.value(),
                axis_range(axis_ranges, axis),
                right_stick_x,
                raw_events,
            )
        }
        InputEventKind::AbsAxis(axis)
            if mapped_axis_from_event(event, mapped_axes) == Some(Axis::RightStickY)
                || axis == AbsoluteAxisType::ABS_RY =>
        {
            translate_right_stick_y(event.value(), axis_range(axis_ranges, axis), raw_events)
        }
        _ => {}
    }
}

fn translate_evdev_key(
    event: InputEvent,
    key: Key,
    mapped_buttons: &[(u32, Button)],
    raw_events: &mut Vec<RawGamepadEvent>,
) {
    let Some(button) =
        mapped_button_from_event(event, mapped_buttons).or_else(|| button_from_evdev_key(key))
    else {
        return;
    };

    match event.value() {
        0 => raw_events.push(RawGamepadEvent::Release(button)),
        1 => raw_events.push(RawGamepadEvent::Press(button)),
        _ => {}
    }
}

fn button_from_evdev_key(key: Key) -> Option<Button> {
    match key {
        Key::BTN_SOUTH => Some(Button::South),
        Key::BTN_EAST => Some(Button::East),
        Key::BTN_NORTH => Some(Button::North),
        Key::BTN_WEST => Some(Button::West),
        Key::BTN_THUMBL => Some(Button::LeftThumb),
        Key::BTN_THUMBR => Some(Button::RightThumb),
        Key::BTN_TL2 => Some(Button::LeftTrigger2),
        Key::BTN_TR2 => Some(Button::RightTrigger2),
        Key::BTN_BACK => Some(Button::Select),
        Key::BTN_SELECT => Some(Button::Select),
        Key::BTN_START => Some(Button::Start),
        Key::BTN_DPAD_UP => Some(Button::DPadUp),
        Key::BTN_DPAD_DOWN => Some(Button::DPadDown),
        Key::BTN_DPAD_LEFT => Some(Button::DPadLeft),
        Key::BTN_DPAD_RIGHT => Some(Button::DPadRight),
        _ => None,
    }
}

fn mapped_button_from_event(event: InputEvent, mapped_buttons: &[(u32, Button)]) -> Option<Button> {
    let event_code = packed_evdev_code(event);

    mapped_buttons
        .iter()
        .find_map(|(code, button)| (*code == event_code).then_some(*button))
}

fn mapped_axis_from_event(event: InputEvent, mapped_axes: &[(u32, Axis)]) -> Option<Axis> {
    let event_code = packed_evdev_code(event);

    mapped_axes
        .iter()
        .find_map(|(code, axis)| (*code == event_code).then_some(*axis))
}

#[derive(Debug, Clone, Copy)]
struct AxisRange {
    minimum: i32,
    maximum: i32,
    flat: i32,
}

impl AxisRange {
    fn normalize_stick(self, value: i32) -> f32 {
        let half_range = ((self.maximum - self.minimum) as f32 / 2.0).max(1.0);
        let center = self.minimum as f32 + half_range;

        if (value as f32 - center).abs() <= self.flat.max(0) as f32 {
            return 0.0;
        }

        ((value as f32 - center) / half_range).clamp(-1.0, 1.0)
    }

    fn normalize_trigger(self, value: i32) -> f32 {
        let range = (self.maximum - self.minimum) as f32;
        if range <= 0.0 {
            return 0.0;
        }

        ((value - self.minimum) as f32 / range).clamp(0.0, 1.0)
    }
}

fn axis_ranges_for_device(device: &Device) -> Vec<(AbsoluteAxisType, AxisRange)> {
    let Ok(abs_state) = device.get_abs_state() else {
        return Vec::new();
    };

    [
        AbsoluteAxisType::ABS_X,
        AbsoluteAxisType::ABS_Y,
        AbsoluteAxisType::ABS_RX,
        AbsoluteAxisType::ABS_RY,
        AbsoluteAxisType::ABS_Z,
        AbsoluteAxisType::ABS_RZ,
    ]
    .into_iter()
    .filter_map(|axis| {
        let info = abs_state[axis.0 as usize];
        (info.maximum != info.minimum).then_some((
            axis,
            AxisRange {
                minimum: info.minimum,
                maximum: info.maximum,
                flat: info.flat,
            },
        ))
    })
    .collect()
}

fn axis_range(
    axis_ranges: &[(AbsoluteAxisType, AxisRange)],
    axis: AbsoluteAxisType,
) -> Option<AxisRange> {
    axis_ranges
        .iter()
        .find_map(|(candidate, range)| (*candidate == axis).then_some(*range))
}

fn mapped_gamepad_buttons(gamepad: &Gamepad<'_>) -> Vec<(u32, Button)> {
    [
        Button::South,
        Button::East,
        Button::North,
        Button::West,
        Button::LeftThumb,
        Button::RightThumb,
        Button::LeftTrigger2,
        Button::RightTrigger2,
        Button::Select,
        Button::Start,
        Button::DPadUp,
        Button::DPadDown,
        Button::DPadLeft,
        Button::DPadRight,
    ]
    .into_iter()
    .filter_map(|button| {
        gamepad
            .button_code(button)
            .map(|code| (code.into_u32(), button))
    })
    .collect()
}

fn mapped_gamepad_axes(gamepad: &Gamepad<'_>) -> Vec<(u32, Axis)> {
    [
        Axis::LeftStickY,
        Axis::RightStickX,
        Axis::RightStickY,
        Axis::LeftZ,
        Axis::RightZ,
    ]
    .into_iter()
    .filter_map(|axis| gamepad.axis_code(axis).map(|code| (code.into_u32(), axis)))
    .collect()
}

fn packed_evdev_code(event: InputEvent) -> u32 {
    (u32::from(event.event_type().0) << 16) | u32::from(event.code())
}

fn translate_right_stick_x(
    value: i32,
    range: Option<AxisRange>,
    previous_zone: &mut i32,
    raw_events: &mut Vec<RawGamepadEvent>,
) {
    raw_events.push(RawGamepadEvent::Axis(
        Axis::RightStickX,
        normalize_stick_axis(value, range),
    ));

    let next_zone = axis_zone(value);

    if next_zone == *previous_zone {
        return;
    }

    match next_zone {
        zone if zone < 0 => {
            raw_events.push(RawGamepadEvent::MoveSelection(KeyboardDirection::Left))
        }
        zone if zone > 0 => {
            raw_events.push(RawGamepadEvent::MoveSelection(KeyboardDirection::Right))
        }
        _ => {}
    }

    *previous_zone = next_zone;
}

fn translate_right_stick_y(
    value: i32,
    range: Option<AxisRange>,
    raw_events: &mut Vec<RawGamepadEvent>,
) {
    raw_events.push(RawGamepadEvent::Axis(
        Axis::RightStickY,
        normalize_stick_axis(value, range),
    ));
}

fn translate_left_stick_y(
    value: i32,
    range: Option<AxisRange>,
    raw_events: &mut Vec<RawGamepadEvent>,
) {
    raw_events.push(RawGamepadEvent::Axis(
        Axis::LeftStickY,
        -normalize_stick_axis(value, range),
    ));
}

fn translate_left_trigger(
    value: i32,
    range: Option<AxisRange>,
    previous_zone: &mut i32,
    raw_events: &mut Vec<RawGamepadEvent>,
) {
    raw_events.push(RawGamepadEvent::Axis(
        Axis::LeftZ,
        normalize_trigger_axis(value, range),
    ));

    let next_zone = trigger_zone(value);

    if next_zone == *previous_zone {
        return;
    }

    if next_zone > 0 {
        raw_events.push(RawGamepadEvent::Press(Button::LeftTrigger2));
    } else {
        raw_events.push(RawGamepadEvent::Release(Button::LeftTrigger2));
    }

    *previous_zone = next_zone;
}

fn translate_right_trigger(
    value: i32,
    range: Option<AxisRange>,
    previous_zone: &mut i32,
    raw_events: &mut Vec<RawGamepadEvent>,
) {
    raw_events.push(RawGamepadEvent::Axis(
        Axis::RightZ,
        normalize_trigger_axis(value, range),
    ));

    let next_zone = trigger_zone(value);

    if next_zone == *previous_zone {
        return;
    }

    if next_zone > 0 {
        raw_events.push(RawGamepadEvent::Press(Button::RightTrigger2));
    } else {
        raw_events.push(RawGamepadEvent::Release(Button::RightTrigger2));
    }

    *previous_zone = next_zone;
}

fn normalize_stick_axis(value: i32, range: Option<AxisRange>) -> f32 {
    if let Some(range) = range {
        return range.normalize_stick(value);
    }

    if value == 0 {
        return 0.0;
    }

    if (0..=255).contains(&value) {
        ((value as f32 - 128.0) / 127.0).clamp(-1.0, 1.0)
    } else {
        (value as f32 / 32_767.0).clamp(-1.0, 1.0)
    }
}

fn normalize_trigger_axis(value: i32, range: Option<AxisRange>) -> f32 {
    if let Some(range) = range {
        return range.normalize_trigger(value);
    }

    if value <= 255 {
        (value as f32 / 255.0).clamp(0.0, 1.0)
    } else {
        (value as f32 / 32_767.0).clamp(0.0, 1.0)
    }
}

fn trigger_zone(value: i32) -> i32 {
    if value >= 16_000 || value >= 128 {
        1
    } else {
        0
    }
}

fn axis_zone(value: i32) -> i32 {
    if value <= -16_000 || (1..=64).contains(&value) {
        -1
    } else if value >= 16_000 || (192..=255).contains(&value) {
        1
    } else {
        0
    }
}

fn translate_hat_axis(
    value: i32,
    previous: &mut i32,
    negative_button: Button,
    positive_button: Button,
    raw_events: &mut Vec<RawGamepadEvent>,
) {
    let next = value.signum();

    if *previous == next {
        return;
    }

    match *previous {
        value if value < 0 => raw_events.push(RawGamepadEvent::Release(negative_button)),
        value if value > 0 => raw_events.push(RawGamepadEvent::Release(positive_button)),
        _ => {}
    }

    match next {
        value if value < 0 => raw_events.push(RawGamepadEvent::Press(negative_button)),
        value if value > 0 => raw_events.push(RawGamepadEvent::Press(positive_button)),
        _ => {}
    }

    *previous = next;
}

fn send_selection_move(
    osk_sender: &Sender<GamepadCommand>,
    sidemenu_sender: &Sender<SideMenuCommand>,
    direction: KeyboardDirection,
) -> Result<()> {
    osk_sender
        .send(GamepadCommand::MoveSelection(direction))
        .context("failed to send OSK selection command")?;
    sidemenu_sender
        .send(SideMenuCommand::MoveSelection(direction))
        .context("failed to send side menu selection command")?;

    Ok(())
}

use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use evdev::{AbsoluteAxisType, Device, InputEvent, InputEventKind, Key};
use gilrs::{Axis, Button, Event, EventType as GilrsEventType, Gamepad, Gilrs, LinuxGamepadExt};

const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
const O_NONBLOCK: i32 = 0o0004000;

extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

/// Commands emitted by the gamepad input thread.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GamepadCommand {
    /// Toggle visibility of the on-screen keyboard.
    ToggleKeyboard,
    /// Move the on-screen keyboard between bottom and top positions.
    ToggleKeyboardPosition,
    /// Move the selected OSK key.
    MoveSelection(KeyboardDirection),
    /// Activate the selected OSK key.
    ActivateSelection,
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
    /// Move the selected side menu row.
    MoveSelection(KeyboardDirection),
    /// Activate the selected side menu row.
    ActivateSelection,
}

/// Side menu row currently focused by gamepad navigation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[allow(dead_code)]
pub enum FocusedRow {
    /// Volume controls are focused.
    Volume,
    /// Brightness controls are focused.
    Brightness,
    /// Wi-Fi controls are focused.
    Wifi,
    /// Bluetooth controls are focused.
    Bluetooth,
    /// No feature row is focused.
    None,
}

/// Commands sent from the GTK thread to control exclusive gamepad access.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GamepadGrabCommand {
    /// Enable or disable exclusive evdev grabs for connected gamepads.
    SetExclusive(bool),
}

/// Spawns the focus-independent gamepad event loop on a background thread.
pub fn spawn_gamepad_thread(
    osk_sender: Sender<GamepadCommand>,
    sidemenu_sender: Sender<SideMenuCommand>,
    grab_receiver: Receiver<GamepadGrabCommand>,
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

            if let Err(error) =
                runtime.block_on(run_event_loop(osk_sender, sidemenu_sender, grab_receiver))
            {
                eprintln!("Gamepad event loop stopped: {error:#}");
            }
        })
        .context("failed to spawn gamepad thread")
}

async fn run_event_loop(
    osk_sender: Sender<GamepadCommand>,
    sidemenu_sender: Sender<SideMenuCommand>,
    grab_receiver: Receiver<GamepadGrabCommand>,
) -> Result<()> {
    let mut gilrs =
        Gilrs::new().map_err(|error| anyhow!("failed to initialise gilrs: {error:?}"))?;
    let mut grab_manager = GamepadGrabManager::default();
    let mut exclusive_requested = false;
    let mut exclusive_active = false;
    let mut south_pressed = false;
    let mut south_consumed = false;
    let mut select_pressed = false;
    let mut select_consumed = false;
    let mut start_pressed = false;
    let mut osk_combo_armed = false;
    let mut sidemenu_combo_armed = false;
    let mut sidemenu_open = false;
    let mut focused_row_index = 0usize;
    let mut focused_row = FocusedRow::None;

    loop {
        while let Ok(command) = grab_receiver.try_recv() {
            match command {
                GamepadGrabCommand::SetExclusive(enabled) if enabled != exclusive_requested => {
                    exclusive_requested = enabled;

                    if exclusive_requested {
                        grab_manager.enable(&gilrs);
                        exclusive_active = grab_manager.is_active();
                        reset_button_state(
                            &mut south_pressed,
                            &mut south_consumed,
                            &mut select_pressed,
                            &mut select_consumed,
                            &mut start_pressed,
                            &mut osk_combo_armed,
                            &mut sidemenu_combo_armed,
                        );
                    } else {
                        if exclusive_active {
                            grab_manager.disable();
                        }

                        exclusive_active = false;
                        reset_button_state(
                            &mut south_pressed,
                            &mut south_consumed,
                            &mut select_pressed,
                            &mut select_consumed,
                            &mut start_pressed,
                            &mut osk_combo_armed,
                            &mut sidemenu_combo_armed,
                        );
                    }
                }
                GamepadGrabCommand::SetExclusive(_) => {}
            }
        }

        let events = if exclusive_active {
            grab_manager.poll_events()
        } else {
            let mut events = Vec::new();

            while let Some(Event { event, .. }) = gilrs.next_event() {
                if let Some(event) = raw_event_from_gilrs(event) {
                    events.push(event);
                }
            }

            events
        };

        for event in events {
            update_button_state(
                event,
                &mut south_pressed,
                &mut south_consumed,
                &mut select_pressed,
                &mut select_consumed,
                &mut start_pressed,
                &mut osk_combo_armed,
                &mut sidemenu_combo_armed,
                &mut sidemenu_open,
                &mut focused_row_index,
                &mut focused_row,
                &osk_sender,
                &sidemenu_sender,
            )?;
        }

        tokio::time::sleep(Duration::from_millis(16)).await;
    }
}

fn reset_button_state(
    south_pressed: &mut bool,
    south_consumed: &mut bool,
    select_pressed: &mut bool,
    select_consumed: &mut bool,
    start_pressed: &mut bool,
    osk_combo_armed: &mut bool,
    sidemenu_combo_armed: &mut bool,
) {
    *south_pressed = false;
    *south_consumed = false;
    *select_pressed = false;
    *select_consumed = false;
    *start_pressed = false;
    *osk_combo_armed = false;
    *sidemenu_combo_armed = false;
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum RawGamepadEvent {
    Press(Button),
    Release(Button),
    MoveSelection(KeyboardDirection),
}

fn update_button_state(
    event: RawGamepadEvent,
    south_pressed: &mut bool,
    south_consumed: &mut bool,
    select_pressed: &mut bool,
    select_consumed: &mut bool,
    start_pressed: &mut bool,
    osk_combo_armed: &mut bool,
    sidemenu_combo_armed: &mut bool,
    sidemenu_open: &mut bool,
    focused_row_index: &mut usize,
    focused_row: &mut FocusedRow,
    osk_sender: &Sender<GamepadCommand>,
    sidemenu_sender: &Sender<SideMenuCommand>,
) -> Result<()> {
    match event {
        RawGamepadEvent::Press(Button::South) => *south_pressed = true,
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
        RawGamepadEvent::Press(Button::Select) => *select_pressed = true,
        RawGamepadEvent::Release(Button::Select) => {
            if *select_pressed && !*select_consumed && !*south_pressed && !*start_pressed {
                osk_sender
                    .send(GamepadCommand::ToggleKeyboardPosition)
                    .context("failed to send OSK position command")?;
            }

            *select_pressed = false;
            *select_consumed = false;
        }
        RawGamepadEvent::Press(Button::Start) => *start_pressed = true,
        RawGamepadEvent::Release(Button::Start) => *start_pressed = false,
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
        _ => {}
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

    if *start_pressed && *select_pressed && !*sidemenu_combo_armed {
        *sidemenu_combo_armed = true;
        *select_consumed = true;
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

    if !*start_pressed || !*select_pressed {
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
        KeyboardDirection::Down => *focused_row_index = (*focused_row_index + 1).min(3),
        KeyboardDirection::Left | KeyboardDirection::Right => {}
    }

    *focused_row = match *focused_row_index {
        0 => FocusedRow::Volume,
        _ => FocusedRow::None,
    };

    Ok(())
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
    dpad_x: i32,
    dpad_y: i32,
    right_stick_x: i32,
}

impl GamepadGrabManager {
    fn enable(&mut self, gilrs: &Gilrs) {
        self.disable();

        for (_, gamepad) in gilrs.gamepads() {
            let path = gamepad.devpath().to_path_buf();

            match open_grabbed_device(&path) {
                Ok(device) => self.devices.push(GrabbedGamepad {
                    path,
                    device,
                    mapped_buttons: mapped_gamepad_buttons(&gamepad),
                    mapped_axes: mapped_gamepad_axes(&gamepad),
                    dpad_x: 0,
                    dpad_y: 0,
                    right_stick_x: 0,
                }),
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
                    &mut grabbed.dpad_x,
                    &mut grabbed.dpad_y,
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

fn raw_event_from_gilrs(event: GilrsEventType) -> Option<RawGamepadEvent> {
    match event {
        GilrsEventType::ButtonPressed(button, _) => Some(RawGamepadEvent::Press(button)),
        GilrsEventType::ButtonReleased(button, _) => Some(RawGamepadEvent::Release(button)),
        GilrsEventType::AxisChanged(Axis::RightStickX, value, _) if value >= 0.65 => {
            Some(RawGamepadEvent::MoveSelection(KeyboardDirection::Right))
        }
        GilrsEventType::AxisChanged(Axis::RightStickX, value, _) if value <= -0.65 => {
            Some(RawGamepadEvent::MoveSelection(KeyboardDirection::Left))
        }
        _ => None,
    }
}

fn translate_evdev_event(
    event: InputEvent,
    mapped_buttons: &[(u32, Button)],
    mapped_axes: &[(u32, Axis)],
    dpad_x: &mut i32,
    dpad_y: &mut i32,
    right_stick_x: &mut i32,
    raw_events: &mut Vec<RawGamepadEvent>,
) {
    match event.kind() {
        InputEventKind::Key(key) => translate_evdev_key(event, key, mapped_buttons, raw_events),
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
            if mapped_axis_from_event(event, mapped_axes) == Some(Axis::RightStickX)
                || axis == AbsoluteAxisType::ABS_RX =>
        {
            translate_right_stick_x(event.value(), right_stick_x, raw_events)
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

fn mapped_gamepad_buttons(gamepad: &Gamepad<'_>) -> Vec<(u32, Button)> {
    [
        Button::South,
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
    [Axis::RightStickX]
        .into_iter()
        .filter_map(|axis| gamepad.axis_code(axis).map(|code| (code.into_u32(), axis)))
        .collect()
}

fn packed_evdev_code(event: InputEvent) -> u32 {
    (u32::from(event.event_type().0) << 16) | u32::from(event.code())
}

fn translate_right_stick_x(
    value: i32,
    previous_zone: &mut i32,
    raw_events: &mut Vec<RawGamepadEvent>,
) {
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

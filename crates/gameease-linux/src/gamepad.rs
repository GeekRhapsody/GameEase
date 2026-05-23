use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use evdev::{AbsoluteAxisType, Device, InputEvent, InputEventKind, Key};
use gameease_core::gamepad::{GamepadAxis, GamepadButton, GamepadState, RawGamepadEvent};
pub use gameease_core::gamepad::{
    GamepadCommand, GamepadGrabCommand, KeyboardDirection, SideMenuCommand,
};
use gilrs::{Axis, Button, Event, EventType as GilrsEventType, Gamepad, Gilrs, LinuxGamepadExt};

use crate::uinput::{LinuxInputBackend, SharedVirtualKeyboard, SharedVirtualMouse};

const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
const O_NONBLOCK: i32 = 0o0004000;

extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
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
    let input_backend = Box::new(LinuxInputBackend::new(virtual_keyboard, virtual_mouse));
    let mut gamepad_state = GamepadState::new(osk_sender, sidemenu_sender, input_backend);

    loop {
        while let Ok(command) = grab_receiver.try_recv() {
            match command {
                GamepadGrabCommand::SetExclusive(enabled) if enabled != exclusive_requested => {
                    exclusive_requested = enabled;
                }
                GamepadGrabCommand::SetExclusive(_) => {}
                GamepadGrabCommand::SetDesktopMouseSensitivity(sensitivity) => {
                    gamepad_state.set_desktop_mouse_sensitivity(sensitivity);
                }
            }
        }

        if sync_gamepad_grab(
            &mut grab_manager,
            &gilrs,
            exclusive_requested || gamepad_state.desktop_mode_active(),
            &mut exclusive_desired,
            &mut exclusive_active,
        ) {
            gamepad_state.reset_inputs();
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
            gamepad_state.handle_event(event)?;
        }

        if gamepad_state.desktop_mode_active() && !exclusive_active {
            sample_desktop_axes_from_gilrs(&gilrs, &mut gamepad_state);
        }

        gamepad_state.tick();

        if sync_gamepad_grab(
            &mut grab_manager,
            &gilrs,
            exclusive_requested || gamepad_state.desktop_mode_active(),
            &mut exclusive_desired,
            &mut exclusive_active,
        ) {
            gamepad_state.reset_inputs();
        }

        tokio::time::sleep(Duration::from_millis(16)).await;
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

fn sample_desktop_axes_from_gilrs(gilrs: &Gilrs, gamepad_state: &mut GamepadState) {
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
        if let Some(core_axis) = core_axis(axis) {
            gamepad_state.sample_desktop_axis(core_axis, gamepad.value(axis));
        }
    }
}

fn core_button(button: Button) -> Option<GamepadButton> {
    match button {
        Button::South => Some(GamepadButton::South),
        Button::East => Some(GamepadButton::East),
        Button::North => Some(GamepadButton::North),
        Button::West => Some(GamepadButton::West),
        Button::LeftThumb => Some(GamepadButton::LeftThumb),
        Button::RightThumb => Some(GamepadButton::RightThumb),
        Button::LeftTrigger2 => Some(GamepadButton::LeftTrigger2),
        Button::RightTrigger2 => Some(GamepadButton::RightTrigger2),
        Button::Select => Some(GamepadButton::Select),
        Button::Start => Some(GamepadButton::Start),
        Button::DPadUp => Some(GamepadButton::DPadUp),
        Button::DPadDown => Some(GamepadButton::DPadDown),
        Button::DPadLeft => Some(GamepadButton::DPadLeft),
        Button::DPadRight => Some(GamepadButton::DPadRight),
        _ => None,
    }
}

fn core_axis(axis: Axis) -> Option<GamepadAxis> {
    match axis {
        Axis::LeftStickY => Some(GamepadAxis::LeftStickY),
        Axis::RightStickX => Some(GamepadAxis::RightStickX),
        Axis::RightStickY => Some(GamepadAxis::RightStickY),
        Axis::LeftZ => Some(GamepadAxis::LeftZ),
        Axis::RightZ => Some(GamepadAxis::RightZ),
        _ => None,
    }
}

#[derive(Default)]
struct GamepadGrabManager {
    devices: Vec<GrabbedGamepad>,
}

struct GrabbedGamepad {
    path: PathBuf,
    device: Device,
    mapped_buttons: Vec<(u32, GamepadButton)>,
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
        GilrsEventType::ButtonPressed(button, _) => {
            if let Some(button) = core_button(button) {
                raw_events.push(RawGamepadEvent::Press(button));
            }
        }
        GilrsEventType::ButtonReleased(button, _) => {
            if let Some(button) = core_button(button) {
                raw_events.push(RawGamepadEvent::Release(button));
            }
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
            if let Some(core_axis) = core_axis(axis) {
                raw_events.push(RawGamepadEvent::Axis(core_axis, value));
            }

            match axis {
                Axis::RightStickX if value >= 0.65 => {
                    raw_events.push(RawGamepadEvent::MoveSelection(KeyboardDirection::Right));
                }
                Axis::RightStickX if value <= -0.65 => {
                    raw_events.push(RawGamepadEvent::MoveSelection(KeyboardDirection::Left));
                }
                Axis::LeftZ if value >= 0.5 => {
                    raw_events.push(RawGamepadEvent::Press(GamepadButton::LeftTrigger2));
                }
                Axis::LeftZ if value <= 0.2 => {
                    raw_events.push(RawGamepadEvent::Release(GamepadButton::LeftTrigger2));
                }
                Axis::RightZ if value >= 0.5 => {
                    raw_events.push(RawGamepadEvent::Press(GamepadButton::RightTrigger2));
                }
                Axis::RightZ if value <= 0.2 => {
                    raw_events.push(RawGamepadEvent::Release(GamepadButton::RightTrigger2));
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn translate_evdev_event(
    event: InputEvent,
    mapped_buttons: &[(u32, GamepadButton)],
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
            GamepadButton::DPadLeft,
            GamepadButton::DPadRight,
            raw_events,
        ),
        InputEventKind::AbsAxis(AbsoluteAxisType::ABS_HAT0Y) => translate_hat_axis(
            event.value(),
            dpad_y,
            GamepadButton::DPadUp,
            GamepadButton::DPadDown,
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
    mapped_buttons: &[(u32, GamepadButton)],
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

fn button_from_evdev_key(key: Key) -> Option<GamepadButton> {
    match key {
        Key::BTN_SOUTH => Some(GamepadButton::South),
        Key::BTN_EAST => Some(GamepadButton::East),
        Key::BTN_NORTH => Some(GamepadButton::North),
        Key::BTN_WEST => Some(GamepadButton::West),
        Key::BTN_THUMBL => Some(GamepadButton::LeftThumb),
        Key::BTN_THUMBR => Some(GamepadButton::RightThumb),
        Key::BTN_TL2 => Some(GamepadButton::LeftTrigger2),
        Key::BTN_TR2 => Some(GamepadButton::RightTrigger2),
        Key::BTN_BACK => Some(GamepadButton::Select),
        Key::BTN_SELECT => Some(GamepadButton::Select),
        Key::BTN_START => Some(GamepadButton::Start),
        Key::BTN_DPAD_UP => Some(GamepadButton::DPadUp),
        Key::BTN_DPAD_DOWN => Some(GamepadButton::DPadDown),
        Key::BTN_DPAD_LEFT => Some(GamepadButton::DPadLeft),
        Key::BTN_DPAD_RIGHT => Some(GamepadButton::DPadRight),
        _ => None,
    }
}

fn mapped_button_from_event(
    event: InputEvent,
    mapped_buttons: &[(u32, GamepadButton)],
) -> Option<GamepadButton> {
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

fn mapped_gamepad_buttons(gamepad: &Gamepad<'_>) -> Vec<(u32, GamepadButton)> {
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
            .and_then(|code| core_button(button).map(|button| (code.into_u32(), button)))
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
        GamepadAxis::RightStickX,
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
        GamepadAxis::RightStickY,
        normalize_stick_axis(value, range),
    ));
}

fn translate_left_stick_y(
    value: i32,
    range: Option<AxisRange>,
    raw_events: &mut Vec<RawGamepadEvent>,
) {
    raw_events.push(RawGamepadEvent::Axis(
        GamepadAxis::LeftStickY,
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
        GamepadAxis::LeftZ,
        normalize_trigger_axis(value, range),
    ));

    let next_zone = trigger_zone(value);

    if next_zone == *previous_zone {
        return;
    }

    if next_zone > 0 {
        raw_events.push(RawGamepadEvent::Press(GamepadButton::LeftTrigger2));
    } else {
        raw_events.push(RawGamepadEvent::Release(GamepadButton::LeftTrigger2));
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
        GamepadAxis::RightZ,
        normalize_trigger_axis(value, range),
    ));

    let next_zone = trigger_zone(value);

    if next_zone == *previous_zone {
        return;
    }

    if next_zone > 0 {
        raw_events.push(RawGamepadEvent::Press(GamepadButton::RightTrigger2));
    } else {
        raw_events.push(RawGamepadEvent::Release(GamepadButton::RightTrigger2));
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
    negative_button: GamepadButton,
    positive_button: GamepadButton,
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

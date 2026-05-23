use std::thread;

use anyhow::Result;
use gameease_core::gamepad::GamepadState;

/// Windows gamepad backend using XInput through `windows-rs`.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsGamepadBackend;

impl WindowsGamepadBackend {
    /// Creates the Windows gamepad backend.
    pub fn new() -> Self {
        Self
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use anyhow::Context;
    use gameease_core::gamepad::{GamepadAxis, GamepadButton, KeyboardDirection, RawGamepadEvent};
    use std::time::Duration;
    use windows::Win32::UI::Input::XboxController::{
        XInputGetState, XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_BACK,
        XINPUT_GAMEPAD_DPAD_DOWN, XINPUT_GAMEPAD_DPAD_LEFT, XINPUT_GAMEPAD_DPAD_RIGHT,
        XINPUT_GAMEPAD_DPAD_UP, XINPUT_GAMEPAD_LEFT_SHOULDER, XINPUT_GAMEPAD_LEFT_THUMB,
        XINPUT_GAMEPAD_RIGHT_SHOULDER, XINPUT_GAMEPAD_RIGHT_THUMB, XINPUT_GAMEPAD_START,
        XINPUT_GAMEPAD_X, XINPUT_GAMEPAD_Y, XINPUT_STATE,
    };

    impl WindowsGamepadBackend {
        /// Starts the background XInput polling loop.
        pub fn spawn_polling_loop(
            &self,
            mut state: GamepadState,
        ) -> Result<thread::JoinHandle<()>> {
            thread::Builder::new()
                .name("gameease-windows-gamepad".to_string())
                .spawn(move || {
                    let mut previous = [ControllerState::default(); 4];

                    loop {
                        for index in 0..4u32 {
                            poll_controller(index, &mut previous[index as usize], &mut state);
                        }

                        state.tick();
                        thread::sleep(Duration::from_millis(16));
                    }
                })
                .context("failed to spawn Windows gamepad thread")
        }
    }

    #[derive(Debug, Default, Clone, Copy)]
    struct ControllerState {
        buttons: u16,
        left_trigger_pressed: bool,
        right_trigger_pressed: bool,
        right_x_zone: i8,
    }

    fn poll_controller(index: u32, previous: &mut ControllerState, state: &mut GamepadState) {
        let mut xinput_state = XINPUT_STATE::default();
        let result = unsafe { XInputGetState(index, &mut xinput_state) };

        if result != 0 {
            *previous = ControllerState::default();
            return;
        }

        let gamepad = xinput_state.Gamepad;
        emit_button_edges(previous.buttons, gamepad.wButtons.0, state);
        previous.buttons = gamepad.wButtons.0;

        let left_trigger_pressed = gamepad.bLeftTrigger >= 128;
        emit_trigger_edge(
            previous.left_trigger_pressed,
            left_trigger_pressed,
            GamepadButton::LeftTrigger2,
            state,
        );
        previous.left_trigger_pressed = left_trigger_pressed;

        let right_trigger_pressed = gamepad.bRightTrigger >= 128;
        emit_trigger_edge(
            previous.right_trigger_pressed,
            right_trigger_pressed,
            GamepadButton::RightTrigger2,
            state,
        );
        previous.right_trigger_pressed = right_trigger_pressed;

        let left_trigger = f32::from(gamepad.bLeftTrigger) / 255.0;
        let right_trigger = f32::from(gamepad.bRightTrigger) / 255.0;
        let right_x = normalize_thumb(gamepad.sThumbRX);
        let right_y = -normalize_thumb(gamepad.sThumbRY);
        let left_y = normalize_thumb(gamepad.sThumbLY);

        let _ = state.handle_event(RawGamepadEvent::Axis(GamepadAxis::LeftZ, left_trigger));
        let _ = state.handle_event(RawGamepadEvent::Axis(GamepadAxis::RightZ, right_trigger));
        let _ = state.handle_event(RawGamepadEvent::Axis(GamepadAxis::RightStickX, right_x));
        let _ = state.handle_event(RawGamepadEvent::Axis(GamepadAxis::RightStickY, right_y));
        let _ = state.handle_event(RawGamepadEvent::Axis(GamepadAxis::LeftStickY, left_y));

        let next_zone = axis_zone(right_x);
        if next_zone != previous.right_x_zone {
            match next_zone {
                zone if zone < 0 => {
                    let _ =
                        state.handle_event(RawGamepadEvent::MoveSelection(KeyboardDirection::Left));
                }
                zone if zone > 0 => {
                    let _ = state
                        .handle_event(RawGamepadEvent::MoveSelection(KeyboardDirection::Right));
                }
                _ => {}
            }
            previous.right_x_zone = next_zone;
        }
    }

    fn emit_button_edges(previous: u16, current: u16, state: &mut GamepadState) {
        for (mask, button) in button_map() {
            let was_pressed = previous & mask != 0;
            let is_pressed = current & mask != 0;

            if was_pressed == is_pressed {
                continue;
            }

            let event = if is_pressed {
                RawGamepadEvent::Press(button)
            } else {
                RawGamepadEvent::Release(button)
            };
            let _ = state.handle_event(event);
        }
    }

    fn emit_trigger_edge(
        was_pressed: bool,
        is_pressed: bool,
        button: GamepadButton,
        state: &mut GamepadState,
    ) {
        if was_pressed == is_pressed {
            return;
        }

        let event = if is_pressed {
            RawGamepadEvent::Press(button)
        } else {
            RawGamepadEvent::Release(button)
        };
        let _ = state.handle_event(event);
    }

    fn button_map() -> [(u16, GamepadButton); 14] {
        [
            (XINPUT_GAMEPAD_A, GamepadButton::South),
            (XINPUT_GAMEPAD_B, GamepadButton::East),
            (XINPUT_GAMEPAD_Y, GamepadButton::North),
            (XINPUT_GAMEPAD_X, GamepadButton::West),
            (XINPUT_GAMEPAD_LEFT_THUMB, GamepadButton::LeftThumb),
            (XINPUT_GAMEPAD_RIGHT_THUMB, GamepadButton::RightThumb),
            (XINPUT_GAMEPAD_BACK, GamepadButton::Select),
            (XINPUT_GAMEPAD_START, GamepadButton::Start),
            (XINPUT_GAMEPAD_DPAD_UP, GamepadButton::DPadUp),
            (XINPUT_GAMEPAD_DPAD_DOWN, GamepadButton::DPadDown),
            (XINPUT_GAMEPAD_DPAD_LEFT, GamepadButton::DPadLeft),
            (XINPUT_GAMEPAD_DPAD_RIGHT, GamepadButton::DPadRight),
            (XINPUT_GAMEPAD_LEFT_SHOULDER, GamepadButton::LeftTrigger2),
            (XINPUT_GAMEPAD_RIGHT_SHOULDER, GamepadButton::RightTrigger2),
        ]
        .map(|(mask, button)| (mask.0, button))
    }

    fn normalize_thumb(value: i16) -> f32 {
        if value == 0 {
            return 0.0;
        }

        if value < 0 {
            (f32::from(value) / 32768.0).clamp(-1.0, 0.0)
        } else {
            (f32::from(value) / 32767.0).clamp(0.0, 1.0)
        }
    }

    fn axis_zone(value: f32) -> i8 {
        if value <= -0.65 {
            -1
        } else if value >= 0.65 {
            1
        } else {
            0
        }
    }
}

#[cfg(not(windows))]
impl WindowsGamepadBackend {
    /// Starts the background gamepad polling loop.
    pub fn spawn_polling_loop(&self, _state: GamepadState) -> Result<thread::JoinHandle<()>> {
        anyhow::bail!("Windows gamepad backend is only available on Windows")
    }
}

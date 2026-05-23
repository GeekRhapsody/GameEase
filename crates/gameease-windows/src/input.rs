use anyhow::Result;
use gameease_core::gamepad::{KeyCode, MouseButton};
use gameease_core::InputBackend;

/// Windows input injection backend using `SendInput`.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsInputBackend;

#[cfg(windows)]
mod imp {
    use super::*;
    use anyhow::anyhow;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
        MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
        MOUSEINPUT, VIRTUAL_KEY, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_LEFT,
        VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
    };

    impl InputBackend for WindowsInputBackend {
        fn press_key(&self, key: KeyCode) -> Result<()> {
            send_keyboard(key, false)
        }

        fn release_key(&self, key: KeyCode) -> Result<()> {
            send_keyboard(key, true)
        }

        fn tap_key(&self, key: KeyCode) -> Result<()> {
            self.press_key(key)?;
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.release_key(key)
        }

        fn move_pointer_relative(&self, dx: i32, dy: i32) -> Result<()> {
            send_mouse(MOUSEEVENTF_MOVE, dx, dy, 0)
        }

        fn scroll(&self, dy: i32) -> Result<()> {
            send_mouse(MOUSEEVENTF_WHEEL, 0, 0, dy.saturating_mul(120))
        }

        fn button_down(&self, button: MouseButton) -> Result<()> {
            send_mouse(mouse_down_flag(button), 0, 0, 0)
        }

        fn button_up(&self, button: MouseButton) -> Result<()> {
            send_mouse(mouse_up_flag(button), 0, 0, 0)
        }

        fn click(&self, button: MouseButton) -> Result<()> {
            self.button_down(button)?;
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.button_up(button)
        }
    }

    fn send_keyboard(key: KeyCode, release: bool) -> Result<()> {
        let vk = virtual_key(key)?;
        let flags = if release {
            KEYEVENTF_KEYUP
        } else {
            Default::default()
        };
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        send_input(&[input])
    }

    fn send_mouse(
        flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
        dx: i32,
        dy: i32,
        data: i32,
    ) -> Result<()> {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data as u32,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        send_input(&[input])
    }

    fn send_input(inputs: &[INPUT]) -> Result<()> {
        let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent == inputs.len() as u32 {
            Ok(())
        } else {
            Err(anyhow!("SendInput sent {sent} of {} events", inputs.len()))
        }
    }

    fn mouse_down_flag(
        button: MouseButton,
    ) -> windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS {
        match button {
            MouseButton::Left => MOUSEEVENTF_LEFTDOWN,
            MouseButton::Right => MOUSEEVENTF_RIGHTDOWN,
            MouseButton::Middle => MOUSEEVENTF_MIDDLEDOWN,
        }
    }

    fn mouse_up_flag(
        button: MouseButton,
    ) -> windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS {
        match button {
            MouseButton::Left => MOUSEEVENTF_LEFTUP,
            MouseButton::Right => MOUSEEVENTF_RIGHTUP,
            MouseButton::Middle => MOUSEEVENTF_MIDDLEUP,
        }
    }

    fn virtual_key(key: KeyCode) -> Result<VIRTUAL_KEY> {
        let vk = match key {
            KeyCode::Escape => VK_ESCAPE,
            KeyCode::Tab => VK_TAB,
            KeyCode::CapsLock => VK_CAPITAL,
            KeyCode::Space => VK_SPACE,
            KeyCode::Enter => VK_RETURN,
            KeyCode::Backspace => VK_BACK,
            KeyCode::LeftShift | KeyCode::RightShift => VK_LSHIFT,
            KeyCode::LeftControl => VK_CONTROL,
            KeyCode::LeftAlt | KeyCode::RightAlt => VK_LMENU,
            KeyCode::LeftMeta => VK_LWIN,
            KeyCode::Left => VK_LEFT,
            KeyCode::Right => VK_RIGHT,
            KeyCode::Up => VK_UP,
            KeyCode::Down => VK_DOWN,
            KeyCode::A => VIRTUAL_KEY(0x41),
            KeyCode::B => VIRTUAL_KEY(0x42),
            KeyCode::C => VIRTUAL_KEY(0x43),
            KeyCode::D => VIRTUAL_KEY(0x44),
            KeyCode::E => VIRTUAL_KEY(0x45),
            KeyCode::F => VIRTUAL_KEY(0x46),
            KeyCode::G => VIRTUAL_KEY(0x47),
            KeyCode::H => VIRTUAL_KEY(0x48),
            KeyCode::I => VIRTUAL_KEY(0x49),
            KeyCode::J => VIRTUAL_KEY(0x4A),
            KeyCode::K => VIRTUAL_KEY(0x4B),
            KeyCode::L => VIRTUAL_KEY(0x4C),
            KeyCode::M => VIRTUAL_KEY(0x4D),
            KeyCode::N => VIRTUAL_KEY(0x4E),
            KeyCode::O => VIRTUAL_KEY(0x4F),
            KeyCode::P => VIRTUAL_KEY(0x50),
            KeyCode::Q => VIRTUAL_KEY(0x51),
            KeyCode::R => VIRTUAL_KEY(0x52),
            KeyCode::S => VIRTUAL_KEY(0x53),
            KeyCode::T => VIRTUAL_KEY(0x54),
            KeyCode::U => VIRTUAL_KEY(0x55),
            KeyCode::V => VIRTUAL_KEY(0x56),
            KeyCode::W => VIRTUAL_KEY(0x57),
            KeyCode::X => VIRTUAL_KEY(0x58),
            KeyCode::Y => VIRTUAL_KEY(0x59),
            KeyCode::Z => VIRTUAL_KEY(0x5A),
            KeyCode::Num0 => VIRTUAL_KEY(0x30),
            KeyCode::Num1 => VIRTUAL_KEY(0x31),
            KeyCode::Num2 => VIRTUAL_KEY(0x32),
            KeyCode::Num3 => VIRTUAL_KEY(0x33),
            KeyCode::Num4 => VIRTUAL_KEY(0x34),
            KeyCode::Num5 => VIRTUAL_KEY(0x35),
            KeyCode::Num6 => VIRTUAL_KEY(0x36),
            KeyCode::Num7 => VIRTUAL_KEY(0x37),
            KeyCode::Num8 => VIRTUAL_KEY(0x38),
            KeyCode::Num9 => VIRTUAL_KEY(0x39),
            KeyCode::Grave => VIRTUAL_KEY(0xC0),
            KeyCode::Minus => VIRTUAL_KEY(0xBD),
            KeyCode::Equal => VIRTUAL_KEY(0xBB),
            KeyCode::LeftBrace => VIRTUAL_KEY(0xDB),
            KeyCode::RightBrace => VIRTUAL_KEY(0xDD),
            KeyCode::Backslash => VIRTUAL_KEY(0xDC),
            KeyCode::Semicolon => VIRTUAL_KEY(0xBA),
            KeyCode::Apostrophe => VIRTUAL_KEY(0xDE),
            KeyCode::Comma => VIRTUAL_KEY(0xBC),
            KeyCode::Dot => VIRTUAL_KEY(0xBE),
            KeyCode::Slash => VIRTUAL_KEY(0xBF),
            unsupported => anyhow::bail!("unsupported Windows virtual key: {unsupported:?}"),
        };

        Ok(vk)
    }
}

#[cfg(not(windows))]
impl InputBackend for WindowsInputBackend {
    fn press_key(&self, _key: KeyCode) -> Result<()> {
        anyhow::bail!("Windows input backend is only available on Windows")
    }

    fn release_key(&self, _key: KeyCode) -> Result<()> {
        anyhow::bail!("Windows input backend is only available on Windows")
    }

    fn tap_key(&self, _key: KeyCode) -> Result<()> {
        anyhow::bail!("Windows input backend is only available on Windows")
    }

    fn move_pointer_relative(&self, _dx: i32, _dy: i32) -> Result<()> {
        anyhow::bail!("Windows input backend is only available on Windows")
    }

    fn scroll(&self, _dy: i32) -> Result<()> {
        anyhow::bail!("Windows input backend is only available on Windows")
    }

    fn button_down(&self, _button: MouseButton) -> Result<()> {
        anyhow::bail!("Windows input backend is only available on Windows")
    }

    fn button_up(&self, _button: MouseButton) -> Result<()> {
        anyhow::bail!("Windows input backend is only available on Windows")
    }

    fn click(&self, _button: MouseButton) -> Result<()> {
        anyhow::bail!("Windows input backend is only available on Windows")
    }
}

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use evdev::Key;
use uinput::event::keyboard::{self, Keyboard};

/// Thread-shareable virtual keyboard handle.
pub type SharedVirtualKeyboard = Arc<Mutex<VirtualKeyboard>>;

/// Virtual keyboard backed by Linux uinput.
pub struct VirtualKeyboard {
    device: uinput::Device,
}

impl VirtualKeyboard {
    /// Opens `/dev/uinput` and registers every key emitted by the OSK.
    pub fn new() -> Result<Self> {
        let mut builder = uinput::open("/dev/uinput")
            .context("failed to open /dev/uinput")?
            .name("GameEase Virtual Keyboard")
            .context("failed to name virtual keyboard")?
            .bus(0x03)
            .vendor(0x1209)
            .product(0x0001)
            .version(1);

        for key in supported_uinput_keys() {
            builder = builder
                .event(*key)
                .with_context(|| format!("failed to register key {key:?}"))?;
        }

        let device = builder
            .create()
            .context("failed to create virtual keyboard")?;

        thread::sleep(Duration::from_millis(100));

        Ok(Self { device })
    }

    /// Opens `/dev/uinput` and returns the virtual keyboard behind `Arc<Mutex<_>>`.
    pub fn new_shared() -> Result<SharedVirtualKeyboard> {
        Ok(Arc::new(Mutex::new(Self::new()?)))
    }

    /// Sends a key press event.
    pub fn press(&mut self, key: Key) -> Result<()> {
        let key = to_uinput_key(key)?;
        self.device
            .press(&key)
            .with_context(|| format!("failed to press key {key:?}"))?;
        self.device
            .synchronize()
            .context("failed to synchronize key press")?;

        Ok(())
    }

    /// Sends a key release event.
    pub fn release(&mut self, key: Key) -> Result<()> {
        let key = to_uinput_key(key)?;
        self.device
            .release(&key)
            .with_context(|| format!("failed to release key {key:?}"))?;
        self.device
            .synchronize()
            .context("failed to synchronize key release")?;

        Ok(())
    }

    /// Sends a key press followed by a release after a short delay.
    pub fn tap(&mut self, key: Key) -> Result<()> {
        self.press(key)?;
        thread::sleep(Duration::from_millis(20));
        self.release(key)
    }
}

fn supported_uinput_keys() -> &'static [Keyboard] {
    &[
        Keyboard::Key(keyboard::Key::Esc),
        Keyboard::Key(keyboard::Key::_1),
        Keyboard::Key(keyboard::Key::_2),
        Keyboard::Key(keyboard::Key::_3),
        Keyboard::Key(keyboard::Key::_4),
        Keyboard::Key(keyboard::Key::_5),
        Keyboard::Key(keyboard::Key::_6),
        Keyboard::Key(keyboard::Key::_7),
        Keyboard::Key(keyboard::Key::_8),
        Keyboard::Key(keyboard::Key::_9),
        Keyboard::Key(keyboard::Key::_0),
        Keyboard::Key(keyboard::Key::Q),
        Keyboard::Key(keyboard::Key::W),
        Keyboard::Key(keyboard::Key::E),
        Keyboard::Key(keyboard::Key::R),
        Keyboard::Key(keyboard::Key::T),
        Keyboard::Key(keyboard::Key::Y),
        Keyboard::Key(keyboard::Key::U),
        Keyboard::Key(keyboard::Key::I),
        Keyboard::Key(keyboard::Key::O),
        Keyboard::Key(keyboard::Key::P),
        Keyboard::Key(keyboard::Key::Tab),
        Keyboard::Key(keyboard::Key::A),
        Keyboard::Key(keyboard::Key::S),
        Keyboard::Key(keyboard::Key::D),
        Keyboard::Key(keyboard::Key::F),
        Keyboard::Key(keyboard::Key::G),
        Keyboard::Key(keyboard::Key::H),
        Keyboard::Key(keyboard::Key::J),
        Keyboard::Key(keyboard::Key::K),
        Keyboard::Key(keyboard::Key::L),
        Keyboard::Key(keyboard::Key::SemiColon),
        Keyboard::Key(keyboard::Key::Apostrophe),
        Keyboard::Key(keyboard::Key::Z),
        Keyboard::Key(keyboard::Key::X),
        Keyboard::Key(keyboard::Key::C),
        Keyboard::Key(keyboard::Key::V),
        Keyboard::Key(keyboard::Key::B),
        Keyboard::Key(keyboard::Key::N),
        Keyboard::Key(keyboard::Key::M),
        Keyboard::Key(keyboard::Key::Comma),
        Keyboard::Key(keyboard::Key::Dot),
        Keyboard::Key(keyboard::Key::Slash),
        Keyboard::Key(keyboard::Key::Space),
        Keyboard::Key(keyboard::Key::Enter),
        Keyboard::Key(keyboard::Key::BackSpace),
        Keyboard::Key(keyboard::Key::LeftShift),
        Keyboard::Key(keyboard::Key::RightShift),
        Keyboard::Key(keyboard::Key::LeftControl),
        Keyboard::Key(keyboard::Key::LeftMeta),
        Keyboard::Key(keyboard::Key::LeftAlt),
        Keyboard::Key(keyboard::Key::Left),
        Keyboard::Key(keyboard::Key::Right),
        Keyboard::Misc(keyboard::Misc::MicMute),
    ]
}

fn to_uinput_key(key: Key) -> Result<Keyboard> {
    match key {
        Key::KEY_ESC => Ok(Keyboard::Key(keyboard::Key::Esc)),
        Key::KEY_1 => Ok(Keyboard::Key(keyboard::Key::_1)),
        Key::KEY_2 => Ok(Keyboard::Key(keyboard::Key::_2)),
        Key::KEY_3 => Ok(Keyboard::Key(keyboard::Key::_3)),
        Key::KEY_4 => Ok(Keyboard::Key(keyboard::Key::_4)),
        Key::KEY_5 => Ok(Keyboard::Key(keyboard::Key::_5)),
        Key::KEY_6 => Ok(Keyboard::Key(keyboard::Key::_6)),
        Key::KEY_7 => Ok(Keyboard::Key(keyboard::Key::_7)),
        Key::KEY_8 => Ok(Keyboard::Key(keyboard::Key::_8)),
        Key::KEY_9 => Ok(Keyboard::Key(keyboard::Key::_9)),
        Key::KEY_0 => Ok(Keyboard::Key(keyboard::Key::_0)),
        Key::KEY_Q => Ok(Keyboard::Key(keyboard::Key::Q)),
        Key::KEY_W => Ok(Keyboard::Key(keyboard::Key::W)),
        Key::KEY_E => Ok(Keyboard::Key(keyboard::Key::E)),
        Key::KEY_R => Ok(Keyboard::Key(keyboard::Key::R)),
        Key::KEY_T => Ok(Keyboard::Key(keyboard::Key::T)),
        Key::KEY_Y => Ok(Keyboard::Key(keyboard::Key::Y)),
        Key::KEY_U => Ok(Keyboard::Key(keyboard::Key::U)),
        Key::KEY_I => Ok(Keyboard::Key(keyboard::Key::I)),
        Key::KEY_O => Ok(Keyboard::Key(keyboard::Key::O)),
        Key::KEY_P => Ok(Keyboard::Key(keyboard::Key::P)),
        Key::KEY_TAB => Ok(Keyboard::Key(keyboard::Key::Tab)),
        Key::KEY_A => Ok(Keyboard::Key(keyboard::Key::A)),
        Key::KEY_S => Ok(Keyboard::Key(keyboard::Key::S)),
        Key::KEY_D => Ok(Keyboard::Key(keyboard::Key::D)),
        Key::KEY_F => Ok(Keyboard::Key(keyboard::Key::F)),
        Key::KEY_G => Ok(Keyboard::Key(keyboard::Key::G)),
        Key::KEY_H => Ok(Keyboard::Key(keyboard::Key::H)),
        Key::KEY_J => Ok(Keyboard::Key(keyboard::Key::J)),
        Key::KEY_K => Ok(Keyboard::Key(keyboard::Key::K)),
        Key::KEY_L => Ok(Keyboard::Key(keyboard::Key::L)),
        Key::KEY_SEMICOLON => Ok(Keyboard::Key(keyboard::Key::SemiColon)),
        Key::KEY_APOSTROPHE => Ok(Keyboard::Key(keyboard::Key::Apostrophe)),
        Key::KEY_Z => Ok(Keyboard::Key(keyboard::Key::Z)),
        Key::KEY_X => Ok(Keyboard::Key(keyboard::Key::X)),
        Key::KEY_C => Ok(Keyboard::Key(keyboard::Key::C)),
        Key::KEY_V => Ok(Keyboard::Key(keyboard::Key::V)),
        Key::KEY_B => Ok(Keyboard::Key(keyboard::Key::B)),
        Key::KEY_N => Ok(Keyboard::Key(keyboard::Key::N)),
        Key::KEY_M => Ok(Keyboard::Key(keyboard::Key::M)),
        Key::KEY_COMMA => Ok(Keyboard::Key(keyboard::Key::Comma)),
        Key::KEY_DOT => Ok(Keyboard::Key(keyboard::Key::Dot)),
        Key::KEY_SLASH => Ok(Keyboard::Key(keyboard::Key::Slash)),
        Key::KEY_SPACE => Ok(Keyboard::Key(keyboard::Key::Space)),
        Key::KEY_ENTER => Ok(Keyboard::Key(keyboard::Key::Enter)),
        Key::KEY_BACKSPACE => Ok(Keyboard::Key(keyboard::Key::BackSpace)),
        Key::KEY_LEFTSHIFT => Ok(Keyboard::Key(keyboard::Key::LeftShift)),
        Key::KEY_RIGHTSHIFT => Ok(Keyboard::Key(keyboard::Key::RightShift)),
        Key::KEY_LEFTCTRL => Ok(Keyboard::Key(keyboard::Key::LeftControl)),
        Key::KEY_LEFTMETA => Ok(Keyboard::Key(keyboard::Key::LeftMeta)),
        Key::KEY_LEFTALT => Ok(Keyboard::Key(keyboard::Key::LeftAlt)),
        Key::KEY_LEFT => Ok(Keyboard::Key(keyboard::Key::Left)),
        Key::KEY_RIGHT => Ok(Keyboard::Key(keyboard::Key::Right)),
        Key::KEY_MICMUTE => Ok(Keyboard::Misc(keyboard::Misc::MicMute)),
        _ => Err(anyhow!("unsupported OSK key code {}", key.code())),
    }
}

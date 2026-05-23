#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };

    let result = run();

    unsafe { CoUninitialize() };

    result
}

#[cfg(windows)]
fn run() -> anyhow::Result<()> {
    use std::sync::mpsc;

    use gameease_core::gamepad::{GamepadCommand, GamepadState, SideMenuCommand};
    use gameease_windows::{
        WindowsAudioBackend, WindowsBluetoothBackend, WindowsBrightnessBackend,
        WindowsGamepadBackend, WindowsInputBackend, WindowsOverlayBackend, WindowsTaskBackend,
        WindowsWifiBackend,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
    };

    let _overlay = WindowsOverlayBackend::new(1280, 720)?;
    let input = WindowsInputBackend;
    let _audio = WindowsAudioBackend;
    let _wifi = WindowsWifiBackend::new()?;
    let _bluetooth = WindowsBluetoothBackend::new()?;
    let _brightness = WindowsBrightnessBackend::new()?;
    let _window_manager = WindowsTaskBackend;

    let (osk_sender, _osk_receiver) = mpsc::channel::<GamepadCommand>();
    let (sidemenu_sender, _sidemenu_receiver) = mpsc::channel::<SideMenuCommand>();
    let gamepad_state = GamepadState::new(osk_sender, sidemenu_sender, Box::new(input));
    let gamepad = WindowsGamepadBackend::new();
    let _gamepad_thread = gamepad.spawn_polling_loop(gamepad_state)?;

    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("gameease-windows binary can only run on Windows");
}

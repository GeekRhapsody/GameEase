#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
use std::{
    sync::mpsc::{self, Receiver},
    thread,
};

#[cfg(windows)]
use gameease_core::gamepad::{GamepadCommand, GamepadGrabCommand, GamepadState, SideMenuCommand};
#[cfg(windows)]
use gameease_windows::{
    WindowsAudioBackend, WindowsBluetoothBackend, WindowsBrightnessBackend, WindowsGamepadBackend,
    WindowsInputBackend, WindowsSideMenu, WindowsSideMenuHandle, WindowsTaskBackend,
    WindowsTrayIcon, WindowsTrayNotifier, WindowsWifiBackend,
};

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use windows::core::{w, HSTRING};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    if let Err(error) = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() } {
        let message = HSTRING::from(format!("Failed to initialise GameEase:\n{error:#}"));
        unsafe {
            MessageBoxW(None, &message, w!("GameEase"), MB_OK | MB_ICONERROR);
        }
        return std::process::ExitCode::FAILURE;
    }

    let result = run();

    unsafe { CoUninitialize() };

    if let Err(error) = result {
        let message = HSTRING::from(format!("GameEase stopped:\n{error:#}"));
        unsafe {
            MessageBoxW(None, &message, w!("GameEase"), MB_OK | MB_ICONERROR);
        }
        return std::process::ExitCode::FAILURE;
    }

    std::process::ExitCode::SUCCESS
}

#[cfg(windows)]
fn run() -> anyhow::Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
    };

    let tray = WindowsTrayIcon::new()?;
    let notifier = tray.notifier();
    let (config_sender, config_receiver) = mpsc::channel::<GamepadGrabCommand>();
    let side_menu = WindowsSideMenu::new(config_sender)?;
    let side_menu_handle = side_menu.handle();
    let input = WindowsInputBackend;
    let _audio = WindowsAudioBackend;
    let _wifi = WindowsWifiBackend::new()?;
    let _bluetooth = WindowsBluetoothBackend::new()?;
    let _brightness = WindowsBrightnessBackend::new()?;
    let _window_manager = WindowsTaskBackend;

    let (osk_sender, osk_receiver) = mpsc::channel::<GamepadCommand>();
    let (sidemenu_sender, sidemenu_receiver) = mpsc::channel::<SideMenuCommand>();
    let _command_thread = spawn_command_notification_loop(
        osk_receiver,
        sidemenu_receiver,
        notifier,
        side_menu_handle,
    )?;
    let gamepad_state = GamepadState::new(osk_sender, sidemenu_sender, Box::new(input));
    let gamepad = WindowsGamepadBackend::new();
    let _gamepad_thread =
        gamepad.spawn_polling_loop_with_commands(gamepad_state, Some(config_receiver))?;

    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    Ok(())
}

#[cfg(windows)]
fn spawn_command_notification_loop(
    osk_receiver: Receiver<GamepadCommand>,
    sidemenu_receiver: Receiver<SideMenuCommand>,
    notifier: WindowsTrayNotifier,
    side_menu: WindowsSideMenuHandle,
) -> anyhow::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("gameease-windows-command-notifications".to_string())
        .spawn(move || loop {
            match osk_receiver.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(GamepadCommand::DesktopModeChanged(enabled)) => {
                    let state = if enabled { "enabled" } else { "disabled" };
                    let _ =
                        notifier.notify("GameEase Desktop Mode", &format!("Desktop Mode {state}"));
                }
                Ok(GamepadCommand::ToggleKeyboard) => {
                    let _ = notifier.notify(
                        "GameEase",
                        "The Windows OSK overlay is not available in this build yet.",
                    );
                }
                Ok(GamepadCommand::ToggleKeyboardPosition) => {
                    let _ = notifier.notify(
                        "GameEase",
                        "The Windows OSK overlay is not available in this build yet.",
                    );
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            while let Ok(command) = sidemenu_receiver.try_recv() {
                let _ = side_menu.post(command);
            }
        })
        .map_err(Into::into)
}

#[cfg(not(windows))]
fn main() {
    eprintln!("gameease-windows binary can only run on Windows");
}

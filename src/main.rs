mod audio;
mod gamepad;
mod keyboard;
mod overlay;
mod sidemenu;
mod tray;
mod uinput;
mod wifi;

use std::sync::mpsc;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::gamepad::{GamepadCommand, GamepadGrabCommand, SideMenuCommand};
use crate::uinput::VirtualKeyboard;

fn main() -> gtk::glib::ExitCode {
    let application = gtk::Application::builder()
        .application_id("dev.gameease.GameEase")
        .build();

    application.connect_activate(|app| {
        if let Err(error) = tray::spawn_tray_icon() {
            eprintln!("Failed to start tray icon: {error:#}");
        }

        let (toggle_sender, toggle_receiver) = mpsc::channel::<GamepadCommand>();
        let (sidemenu_sender, sidemenu_receiver) = mpsc::channel::<SideMenuCommand>();
        let (grab_sender, grab_receiver) = mpsc::channel::<GamepadGrabCommand>();
        let virtual_keyboard = match VirtualKeyboard::new_shared() {
            Ok(virtual_keyboard) => virtual_keyboard,
            Err(error) => {
                eprintln!("Failed to initialise virtual keyboard: {error:#}");
                app.quit();
                return;
            }
        };

        if let Err(error) =
            gamepad::spawn_gamepad_thread(toggle_sender, sidemenu_sender, grab_receiver)
        {
            eprintln!("Failed to start gamepad thread: {error:#}");
        }

        if let Err(error) = overlay::build_window(
            app,
            toggle_receiver,
            sidemenu_receiver,
            grab_sender,
            virtual_keyboard,
        ) {
            eprintln!("Failed to build overlay window: {error:#}");
            app.quit();
        }
    });

    application.run()
}

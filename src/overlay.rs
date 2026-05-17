use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};

use anyhow::Result;
use gtk::cairo;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::gamepad::{GamepadCommand, GamepadGrabCommand, SideMenuCommand};
use crate::keyboard::{self, OnScreenKeyboard};
use crate::sidemenu::{self, SideMenu, SideMenuAction};
use crate::uinput::SharedVirtualKeyboard;

const OSK_EDGE_GAP: i32 = 50;
const NOTIFICATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone, Copy, Eq, PartialEq)]
enum KeyboardPlacement {
    Bottom,
    Top,
}

#[derive(Clone)]
struct DesktopModeNotification {
    revealer: gtk::Revealer,
    label: gtk::Label,
    generation: Rc<Cell<u64>>,
}

impl DesktopModeNotification {
    fn revealer(&self) -> &gtk::Revealer {
        &self.revealer
    }
}

/// Builds and presents the layer-shell overlay window.
pub fn build_window(
    application: &gtk::Application,
    gamepad_receiver: Receiver<GamepadCommand>,
    sidemenu_receiver: Receiver<SideMenuCommand>,
    grab_sender: Sender<GamepadGrabCommand>,
    virtual_keyboard: SharedVirtualKeyboard,
) -> Result<gtk::ApplicationWindow> {
    install_overlay_css();

    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("GameEase")
        .can_focus(false)
        .decorated(false)
        .default_height(screen_height())
        .default_width(screen_width())
        .focus_on_click(false)
        .focusable(false)
        .resizable(false)
        .build();
    window.add_css_class("gameease-window");

    window.init_layer_shell();
    window.set_namespace(Some("gameease"));
    window.set_layer(Layer::Overlay);
    window.set_anchor(Edge::Bottom, true);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Right, true);
    window.set_anchor(Edge::Top, true);
    window.set_exclusive_zone(0);
    window.set_keyboard_mode(KeyboardMode::None);

    let sidemenu = sidemenu::build_sidemenu();
    let keyboard_placement = Rc::new(Cell::new(KeyboardPlacement::Bottom));
    let keyboard_widget = Rc::new(RefCell::new(None::<gtk::Grid>));
    let move_keyboard = {
        let window = window.clone();
        let sidemenu_revealer = sidemenu.revealer().clone();
        let keyboard_placement = keyboard_placement.clone();
        let keyboard_widget = keyboard_widget.clone();

        Rc::new(move || {
            let Some(keyboard) = keyboard_widget.borrow().as_ref().cloned() else {
                return;
            };

            if keyboard.get_visible() {
                toggle_keyboard_placement(
                    &window,
                    &keyboard,
                    &sidemenu_revealer,
                    &keyboard_placement,
                );
            }
        })
    };

    let keyboard = keyboard::build_keyboard(virtual_keyboard, move_keyboard);
    keyboard_widget.replace(Some(keyboard.widget().clone()));
    keyboard.widget().set_visible(false);
    apply_keyboard_placement(keyboard.widget(), KeyboardPlacement::Bottom);

    let main_child = gtk::DrawingArea::builder()
        .can_target(false)
        .hexpand(true)
        .opacity(0.0)
        .vexpand(true)
        .build();
    main_child.add_css_class("gameease-transparent");

    let root = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
    root.add_css_class("gameease-root");
    root.set_child(Some(&main_child));
    let desktop_notification = build_desktop_mode_notification();
    root.add_overlay(keyboard.widget());
    root.add_overlay(sidemenu.revealer());
    root.add_overlay(desktop_notification.revealer());

    window.set_child(Some(&root));
    install_gamepad_toggle(
        &window,
        &keyboard,
        &sidemenu,
        &desktop_notification,
        gamepad_receiver,
        grab_sender.clone(),
        keyboard_placement,
    );
    install_keyboard_entry_focus(
        &window,
        keyboard.widget(),
        &sidemenu,
        desktop_notification.revealer(),
        grab_sender.clone(),
    );
    install_sidemenu_toggle(
        &window,
        keyboard.widget(),
        &sidemenu,
        desktop_notification.revealer(),
        sidemenu_receiver,
        grab_sender,
    );
    install_input_region_updates(&window, keyboard.widget(), sidemenu.revealer());

    Ok(window)
}

fn screen_width() -> i32 {
    screen_size().0
}

fn screen_height() -> i32 {
    screen_size().1
}

fn screen_size() -> (i32, i32) {
    let Some(display) = gtk::gdk::Display::default() else {
        return (1920, 1080);
    };
    let monitors = display.monitors();
    let Some(first_monitor) = monitors
        .item(0)
        .and_then(|item| item.downcast::<gtk::gdk::Monitor>().ok())
    else {
        return (1920, 1080);
    };
    let geometry = first_monitor.geometry();

    (geometry.width(), geometry.height())
}

fn build_desktop_mode_notification() -> DesktopModeNotification {
    let revealer = gtk::Revealer::builder()
        .can_target(false)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Start)
        .margin_top(64)
        .reveal_child(false)
        .transition_duration(120)
        .transition_type(gtk::RevealerTransitionType::Crossfade)
        .build();

    let label = gtk::Label::builder().can_target(false).build();
    label.add_css_class("desktop-mode-notification-label");

    let panel = gtk::Box::builder()
        .can_target(false)
        .halign(gtk::Align::Center)
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .valign(gtk::Align::Center)
        .build();
    panel.add_css_class("desktop-mode-notification");
    panel.append(&label);
    revealer.set_child(Some(&panel));

    DesktopModeNotification {
        revealer,
        label,
        generation: Rc::new(Cell::new(0)),
    }
}

fn install_gamepad_toggle(
    window: &gtk::ApplicationWindow,
    keyboard: &OnScreenKeyboard,
    sidemenu: &SideMenu,
    notification: &DesktopModeNotification,
    gamepad_receiver: Receiver<GamepadCommand>,
    grab_sender: Sender<GamepadGrabCommand>,
    keyboard_placement: Rc<Cell<KeyboardPlacement>>,
) -> glib::SourceId {
    let window = window.clone();
    let keyboard = keyboard.clone();
    let sidemenu = sidemenu.clone();
    let notification = notification.clone();

    glib::idle_add_local(move || {
        while let Ok(command) = gamepad_receiver.try_recv() {
            match command {
                GamepadCommand::ToggleKeyboard => {
                    let next_visible = !keyboard.widget().get_visible();
                    if !next_visible {
                        keyboard.set_shift_held(false);
                    }

                    keyboard.widget().set_visible(next_visible);
                    update_overlay_visibility(
                        &window,
                        keyboard.widget(),
                        sidemenu.revealer(),
                        notification.revealer(),
                        &grab_sender,
                    );
                    schedule_input_region_update(&window, keyboard.widget(), sidemenu.revealer());
                }
                GamepadCommand::CloseKeyboard => {
                    if keyboard.widget().get_visible() {
                        keyboard.set_shift_held(false);
                        keyboard.widget().set_visible(false);
                        update_overlay_visibility(
                            &window,
                            keyboard.widget(),
                            sidemenu.revealer(),
                            notification.revealer(),
                            &grab_sender,
                        );
                        schedule_input_region_update(
                            &window,
                            keyboard.widget(),
                            sidemenu.revealer(),
                        );
                    }
                }
                GamepadCommand::ToggleKeyboardPosition => {
                    if keyboard.widget().get_visible() {
                        toggle_keyboard_placement(
                            &window,
                            keyboard.widget(),
                            sidemenu.revealer(),
                            &keyboard_placement,
                        );
                    }
                }
                GamepadCommand::MoveSelection(direction) => {
                    let keyboard_accepts_gamepad = keyboard.widget().get_visible()
                        && (!sidemenu.revealer().reveals_child()
                            || sidemenu.is_keyboard_entry_active());
                    if keyboard_accepts_gamepad {
                        keyboard.move_selection(direction);
                    }
                }
                GamepadCommand::ActivateSelection => {
                    let keyboard_accepts_gamepad = keyboard.widget().get_visible()
                        && (!sidemenu.revealer().reveals_child()
                            || sidemenu.is_keyboard_entry_active());
                    if keyboard_accepts_gamepad {
                        keyboard.activate_selected();
                    }
                }
                GamepadCommand::ActivateSpace => {
                    let keyboard_accepts_gamepad = keyboard.widget().get_visible()
                        && (!sidemenu.revealer().reveals_child()
                            || sidemenu.is_keyboard_entry_active());
                    if keyboard_accepts_gamepad {
                        keyboard.activate_space();
                    }
                }
                GamepadCommand::ActivateBackspace => {
                    let keyboard_accepts_gamepad = keyboard.widget().get_visible()
                        && (!sidemenu.revealer().reveals_child()
                            || sidemenu.is_keyboard_entry_active());
                    if keyboard_accepts_gamepad {
                        keyboard.activate_backspace();
                    }
                }
                GamepadCommand::ActivateEnter => {
                    let keyboard_accepts_gamepad = keyboard.widget().get_visible()
                        && (!sidemenu.revealer().reveals_child()
                            || sidemenu.is_keyboard_entry_active());
                    if keyboard_accepts_gamepad {
                        keyboard.activate_enter();
                    }
                }
                GamepadCommand::SetShiftHeld(active) => {
                    if keyboard.widget().get_visible() || !active {
                        keyboard.set_shift_held(active);
                    }
                }
                GamepadCommand::ToggleCapsLock => {
                    if keyboard.widget().get_visible() {
                        keyboard.toggle_caps_lock();
                    }
                }
                GamepadCommand::DesktopModeChanged(enabled) => {
                    show_desktop_mode_notification(
                        &window,
                        keyboard.widget(),
                        sidemenu.revealer(),
                        &notification,
                        &grab_sender,
                        enabled,
                    );
                }
            }
        }

        glib::ControlFlow::Continue
    })
}

fn install_keyboard_entry_focus(
    window: &gtk::ApplicationWindow,
    keyboard: &gtk::Grid,
    sidemenu: &SideMenu,
    notification: &gtk::Revealer,
    grab_sender: Sender<GamepadGrabCommand>,
) {
    let window = window.clone();
    let keyboard = keyboard.clone();
    let sidemenu = sidemenu.clone();
    let notification = notification.clone();

    let window_for_notify = window.clone();
    let keyboard_for_notify = keyboard.clone();
    let sidemenu_for_notify = sidemenu.clone();
    let notification_for_notify = notification.clone();
    let grab_sender_for_notify = grab_sender.clone();
    sidemenu.connect_keyboard_entry_active_notify(move || {
        update_keyboard_entry_focus(
            &window_for_notify,
            &keyboard_for_notify,
            &sidemenu_for_notify,
            &notification_for_notify,
            &grab_sender_for_notify,
        );
    });

    update_keyboard_entry_focus(&window, &keyboard, &sidemenu, &notification, &grab_sender);
}

fn update_keyboard_entry_focus(
    window: &gtk::ApplicationWindow,
    keyboard: &gtk::Grid,
    sidemenu: &SideMenu,
    notification: &gtk::Revealer,
    grab_sender: &Sender<GamepadGrabCommand>,
) {
    if sidemenu.is_keyboard_entry_active() {
        window.set_can_focus(true);
        window.set_focusable(true);
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        if !keyboard.get_visible() {
            keyboard.set_visible(true);
        }

        update_overlay_visibility(
            window,
            keyboard,
            sidemenu.revealer(),
            notification,
            grab_sender,
        );
        schedule_input_region_update(window, keyboard, sidemenu.revealer());

        let sidemenu_for_idle = sidemenu.clone();
        glib::idle_add_local_once(move || {
            sidemenu_for_idle.focus_keyboard_entry();
        });

        let sidemenu_for_timeout = sidemenu.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
            sidemenu_for_timeout.focus_keyboard_entry();
        });
    } else {
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_focusable(false);
        window.set_can_focus(false);
    }
}

fn apply_keyboard_placement(keyboard: &gtk::Grid, placement: KeyboardPlacement) {
    keyboard.set_halign(gtk::Align::Center);

    match placement {
        KeyboardPlacement::Bottom => {
            keyboard.set_valign(gtk::Align::End);
            keyboard.set_margin_top(0);
            keyboard.set_margin_bottom(OSK_EDGE_GAP);
        }
        KeyboardPlacement::Top => {
            keyboard.set_valign(gtk::Align::Start);
            keyboard.set_margin_top(OSK_EDGE_GAP);
            keyboard.set_margin_bottom(0);
        }
    }
}

fn toggle_keyboard_placement(
    window: &gtk::ApplicationWindow,
    keyboard: &gtk::Grid,
    sidemenu: &gtk::Revealer,
    keyboard_placement: &Cell<KeyboardPlacement>,
) {
    let next_placement = match keyboard_placement.get() {
        KeyboardPlacement::Bottom => KeyboardPlacement::Top,
        KeyboardPlacement::Top => KeyboardPlacement::Bottom,
    };
    keyboard_placement.set(next_placement);
    apply_keyboard_placement(keyboard, next_placement);
    schedule_input_region_update(window, keyboard, sidemenu);
}

fn install_sidemenu_toggle(
    window: &gtk::ApplicationWindow,
    keyboard: &gtk::Grid,
    sidemenu: &SideMenu,
    notification: &gtk::Revealer,
    sidemenu_receiver: Receiver<SideMenuCommand>,
    grab_sender: Sender<GamepadGrabCommand>,
) -> glib::SourceId {
    let window = window.clone();
    let keyboard = keyboard.clone();
    let sidemenu = sidemenu.clone();
    let notification = notification.clone();

    glib::idle_add_local(move || {
        while let Ok(command) = sidemenu_receiver.try_recv() {
            match command {
                SideMenuCommand::ToggleSideMenu => {
                    let reveal = !sidemenu.revealer().reveals_child();
                    if !reveal {
                        sidemenu.close_subpanels();
                    }

                    sidemenu.revealer().set_reveal_child(reveal);
                    update_overlay_visibility(
                        &window,
                        &keyboard,
                        sidemenu.revealer(),
                        &notification,
                        &grab_sender,
                    );
                    schedule_input_region_update(&window, &keyboard, sidemenu.revealer());
                    schedule_delayed_input_region_update(&window, &keyboard, sidemenu.revealer());
                    schedule_delayed_overlay_visibility_update(
                        &window,
                        &keyboard,
                        sidemenu.revealer(),
                        &notification,
                        &grab_sender,
                    );
                }
                SideMenuCommand::CloseSideMenu => {
                    if sidemenu.revealer().reveals_child()
                        || sidemenu.revealer().is_child_revealed()
                    {
                        if sidemenu.cancel_active_panel() {
                            schedule_input_region_update(&window, &keyboard, sidemenu.revealer());
                            schedule_delayed_input_region_update(
                                &window,
                                &keyboard,
                                sidemenu.revealer(),
                            );
                            continue;
                        }

                        sidemenu.close_subpanels();
                        sidemenu.revealer().set_reveal_child(false);
                        update_overlay_visibility(
                            &window,
                            &keyboard,
                            sidemenu.revealer(),
                            &notification,
                            &grab_sender,
                        );
                        schedule_input_region_update(&window, &keyboard, sidemenu.revealer());
                        schedule_delayed_input_region_update(
                            &window,
                            &keyboard,
                            sidemenu.revealer(),
                        );
                        schedule_delayed_overlay_visibility_update(
                            &window,
                            &keyboard,
                            sidemenu.revealer(),
                            &notification,
                            &grab_sender,
                        );
                    }
                }
                SideMenuCommand::MoveSelection(direction) => {
                    if sidemenu.revealer().reveals_child()
                        && !(keyboard.get_visible() && sidemenu.is_keyboard_entry_active())
                    {
                        sidemenu.move_selection(direction);
                    }
                }
                SideMenuCommand::ActivateSelection => {
                    if sidemenu.revealer().reveals_child()
                        && !(keyboard.get_visible() && sidemenu.is_keyboard_entry_active())
                    {
                        match sidemenu.activate_selected() {
                            SideMenuAction::None => {}
                            SideMenuAction::CloseMenu => {
                                sidemenu.close_subpanels();
                                sidemenu.revealer().set_reveal_child(false);
                                update_overlay_visibility(
                                    &window,
                                    &keyboard,
                                    sidemenu.revealer(),
                                    &notification,
                                    &grab_sender,
                                );
                                schedule_input_region_update(
                                    &window,
                                    &keyboard,
                                    sidemenu.revealer(),
                                );
                                schedule_delayed_input_region_update(
                                    &window,
                                    &keyboard,
                                    sidemenu.revealer(),
                                );
                                schedule_delayed_overlay_visibility_update(
                                    &window,
                                    &keyboard,
                                    sidemenu.revealer(),
                                    &notification,
                                    &grab_sender,
                                );
                            }
                            SideMenuAction::Quit => {
                                let _ = grab_sender.send(GamepadGrabCommand::SetExclusive(false));
                                if let Some(application) = window.application() {
                                    application.quit();
                                } else {
                                    window.close();
                                }
                            }
                        }
                        schedule_input_region_update(&window, &keyboard, sidemenu.revealer());
                        schedule_delayed_input_region_update(
                            &window,
                            &keyboard,
                            sidemenu.revealer(),
                        );
                    }
                }
                SideMenuCommand::ToggleScan => {
                    if sidemenu.revealer().reveals_child()
                        && !(keyboard.get_visible() && sidemenu.is_keyboard_entry_active())
                    {
                        sidemenu.toggle_scan();
                    }
                }
                SideMenuCommand::TerminateSelection => {
                    if sidemenu.revealer().reveals_child()
                        && !(keyboard.get_visible() && sidemenu.is_keyboard_entry_active())
                    {
                        sidemenu.terminate_selected();
                    }
                }
            }
        }

        glib::ControlFlow::Continue
    })
}

fn show_desktop_mode_notification(
    window: &gtk::ApplicationWindow,
    keyboard: &gtk::Grid,
    sidemenu: &gtk::Revealer,
    notification: &DesktopModeNotification,
    grab_sender: &Sender<GamepadGrabCommand>,
    enabled: bool,
) {
    let generation = notification.generation.get().wrapping_add(1);
    notification.generation.set(generation);
    notification.label.set_label(if enabled {
        "Desktop Mode enabled"
    } else {
        "Desktop Mode disabled"
    });
    notification.revealer.set_reveal_child(true);
    update_overlay_visibility(
        window,
        keyboard,
        sidemenu,
        notification.revealer(),
        grab_sender,
    );
    schedule_input_region_update(window, keyboard, sidemenu);

    let window = window.clone();
    let keyboard = keyboard.clone();
    let sidemenu = sidemenu.clone();
    let revealer = notification.revealer.clone();
    let notification_generation = notification.generation.clone();
    let grab_sender = grab_sender.clone();

    glib::timeout_add_local_once(NOTIFICATION_TIMEOUT, move || {
        if notification_generation.get() != generation {
            return;
        }

        revealer.set_reveal_child(false);
        schedule_delayed_overlay_visibility_update(
            &window,
            &keyboard,
            &sidemenu,
            &revealer,
            &grab_sender,
        );
    });
}

fn update_overlay_visibility(
    window: &gtk::ApplicationWindow,
    keyboard: &gtk::Grid,
    sidemenu: &gtk::Revealer,
    notification: &gtk::Revealer,
    grab_sender: &Sender<GamepadGrabCommand>,
) {
    let interactive_visible =
        keyboard.get_visible() || sidemenu.reveals_child() || sidemenu.is_child_revealed();
    let overlay_visible =
        interactive_visible || notification.reveals_child() || notification.is_child_revealed();

    let _ = grab_sender.send(GamepadGrabCommand::SetExclusive(interactive_visible));

    if overlay_visible {
        window.present();
        schedule_input_region_update(window, keyboard, sidemenu);
    } else {
        window.hide();
    }
}

fn schedule_delayed_overlay_visibility_update(
    window: &gtk::ApplicationWindow,
    keyboard: &gtk::Grid,
    sidemenu: &gtk::Revealer,
    notification: &gtk::Revealer,
    grab_sender: &Sender<GamepadGrabCommand>,
) {
    let window = window.clone();
    let keyboard = keyboard.clone();
    let sidemenu = sidemenu.clone();
    let notification = notification.clone();
    let grab_sender = grab_sender.clone();

    glib::timeout_add_local_once(std::time::Duration::from_millis(220), move || {
        update_overlay_visibility(&window, &keyboard, &sidemenu, &notification, &grab_sender);
    });
}

fn install_overlay_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };

    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "
        .gameease-window,
        window.gameease-window,
        window.background.gameease-window,
        .gameease-root,
        .gameease-transparent {
            background: transparent;
            background-color: transparent;
        }

        .desktop-mode-notification {
            background: rgba(20, 20, 20, 0.92);
            border: 1px solid rgba(255, 255, 255, 0.18);
            border-radius: 8px;
            color: #f4f4f4;
            padding: 12px 18px;
        }

        .desktop-mode-notification-label {
            color: #f4f4f4;
            font-weight: 700;
        }
        ",
    );

    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn install_input_region_updates(
    window: &gtk::ApplicationWindow,
    keyboard: &gtk::Grid,
    sidemenu: &gtk::Revealer,
) {
    let keyboard_for_map = keyboard.clone();
    let sidemenu_for_map = sidemenu.clone();
    window.connect_map(move |window| {
        schedule_input_region_update(window, &keyboard_for_map, &sidemenu_for_map);
    });

    schedule_input_region_update(window, keyboard, sidemenu);
}

fn schedule_input_region_update(
    window: &gtk::ApplicationWindow,
    keyboard: &gtk::Grid,
    sidemenu: &gtk::Revealer,
) {
    let window = window.clone();
    let keyboard = keyboard.clone();
    let sidemenu = sidemenu.clone();

    glib::idle_add_local_once(move || {
        update_input_region(&window, &keyboard, &sidemenu);
    });
}

fn schedule_delayed_input_region_update(
    window: &gtk::ApplicationWindow,
    keyboard: &gtk::Grid,
    sidemenu: &gtk::Revealer,
) {
    let window = window.clone();
    let keyboard = keyboard.clone();
    let sidemenu = sidemenu.clone();

    glib::timeout_add_local_once(std::time::Duration::from_millis(220), move || {
        update_input_region(&window, &keyboard, &sidemenu);
    });
}

fn update_input_region(
    window: &gtk::ApplicationWindow,
    keyboard: &gtk::Grid,
    sidemenu: &gtk::Revealer,
) {
    let Some(surface) = window.surface() else {
        return;
    };

    let region = cairo::Region::create();

    if keyboard.get_visible() {
        add_widget_region(&region, keyboard, window);
    }

    if sidemenu.reveals_child() {
        add_widget_region(&region, sidemenu, window);
    }

    // The layer-shell window spans the full screen so the side menu can start at
    // the left edge, but it must not intercept clicks in empty space. GDK input
    // regions define the parts of the surface that receive pointer events; any
    // pixel outside this cairo::Region is click-through to the focused app below.
    surface.set_input_region(Some(&region));
}

fn add_widget_region(
    region: &cairo::Region,
    widget: &impl IsA<gtk::Widget>,
    window: &gtk::ApplicationWindow,
) {
    let Some(bounds) = widget.compute_bounds(window) else {
        return;
    };

    let x = bounds.x().floor() as i32;
    let y = bounds.y().floor() as i32;
    let width = bounds.width().ceil() as i32;
    let height = bounds.height().ceil() as i32;

    if width <= 0 || height <= 0 {
        return;
    }

    if let Err(error) = region.union_rectangle(&cairo::RectangleInt::new(x, y, width, height)) {
        eprintln!("Failed to update overlay input region: {error}");
    }
}

use std::cell::{Cell, RefCell};
use std::ffi::CString;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};

use anyhow::{Context, Result};
use gameease_core::OverlayBackend as CoreOverlayBackend;
use gtk::cairo;
use gtk::glib;
use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use gtk4 as gtk;
use libloading::Library;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as X11ProtoConnectionExt,
    EventMask, PropMode, StackMode, Window,
};
use x11rb::wrapper::ConnectionExt as X11WrapperConnectionExt;

use crate::gamepad::{GamepadCommand, GamepadGrabCommand, SideMenuCommand};
use crate::keyboard::{self, OnScreenKeyboard};
use crate::sidemenu::{self, SideMenu, SideMenuAction};
use crate::system::LinuxSystemBackend;
use crate::uinput::SharedVirtualKeyboard;
use gameease_core::SystemBackend;

const OSK_EDGE_GAP: i32 = 50;
const NOTIFICATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const GTK_LAYER_SHELL_EDGE_LEFT: i32 = 0;
const GTK_LAYER_SHELL_EDGE_RIGHT: i32 = 1;
const GTK_LAYER_SHELL_EDGE_TOP: i32 = 2;
const GTK_LAYER_SHELL_EDGE_BOTTOM: i32 = 3;
const GTK_LAYER_SHELL_KEYBOARD_MODE_NONE: i32 = 0;
const GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND: i32 = 2;
const GTK_LAYER_SHELL_LAYER_OVERLAY: i32 = 3;

#[derive(Clone, Copy, Eq, PartialEq)]
enum KeyboardPlacement {
    Bottom,
    Top,
}

#[derive(Clone)]
enum PlatformOverlayBackend {
    WaylandLayerShell(Rc<LayerShellApi>),
    X11,
    Unsupported,
}

#[derive(Clone)]
struct DesktopModeNotification {
    revealer: gtk::Revealer,
    label: gtk::Label,
    generation: Rc<Cell<u64>>,
}

type GtkLayerInitForWindow = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow);
type GtkLayerSetNamespace = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, *const std::ffi::c_char);
type GtkLayerSetLayer = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, i32);
type GtkLayerSetAnchor =
    unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, i32, gtk::glib::ffi::gboolean);
type GtkLayerSetExclusiveZone = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, i32);
type GtkLayerSetKeyboardMode = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, i32);

struct LayerShellApi {
    _library: Library,
    init_for_window: GtkLayerInitForWindow,
    set_namespace: GtkLayerSetNamespace,
    set_layer: GtkLayerSetLayer,
    set_anchor: GtkLayerSetAnchor,
    set_exclusive_zone: GtkLayerSetExclusiveZone,
    set_keyboard_mode: GtkLayerSetKeyboardMode,
}

impl LayerShellApi {
    fn load() -> Result<Self> {
        let library = unsafe { Library::new("libgtk4-layer-shell.so.0") }
            .context("failed to load libgtk4-layer-shell.so.0")?;

        let init_for_window =
            unsafe { *library.get::<GtkLayerInitForWindow>(b"gtk_layer_init_for_window\0")? };
        let set_namespace =
            unsafe { *library.get::<GtkLayerSetNamespace>(b"gtk_layer_set_namespace\0")? };
        let set_layer = unsafe { *library.get::<GtkLayerSetLayer>(b"gtk_layer_set_layer\0")? };
        let set_anchor = unsafe { *library.get::<GtkLayerSetAnchor>(b"gtk_layer_set_anchor\0")? };
        let set_exclusive_zone =
            unsafe { *library.get::<GtkLayerSetExclusiveZone>(b"gtk_layer_set_exclusive_zone\0")? };
        let set_keyboard_mode =
            unsafe { *library.get::<GtkLayerSetKeyboardMode>(b"gtk_layer_set_keyboard_mode\0")? };

        Ok(Self {
            _library: library,
            init_for_window,
            set_namespace,
            set_layer,
            set_anchor,
            set_exclusive_zone,
            set_keyboard_mode,
        })
    }

    fn init_overlay_window(&self, window: &gtk::ApplicationWindow) -> Result<()> {
        let namespace = CString::new("gameease").context("invalid layer-shell namespace")?;
        unsafe {
            let window_ptr = gtk_window_ptr(window);
            (self.init_for_window)(window_ptr);
            (self.set_namespace)(window_ptr, namespace.as_ptr());
            (self.set_layer)(window_ptr, GTK_LAYER_SHELL_LAYER_OVERLAY);
            (self.set_anchor)(
                window_ptr,
                GTK_LAYER_SHELL_EDGE_BOTTOM,
                gtk::glib::ffi::GTRUE,
            );
            (self.set_anchor)(window_ptr, GTK_LAYER_SHELL_EDGE_LEFT, gtk::glib::ffi::GTRUE);
            (self.set_anchor)(
                window_ptr,
                GTK_LAYER_SHELL_EDGE_RIGHT,
                gtk::glib::ffi::GTRUE,
            );
            (self.set_anchor)(window_ptr, GTK_LAYER_SHELL_EDGE_TOP, gtk::glib::ffi::GTRUE);
            (self.set_exclusive_zone)(window_ptr, 0);
            (self.set_keyboard_mode)(window_ptr, GTK_LAYER_SHELL_KEYBOARD_MODE_NONE);
        }

        Ok(())
    }

    fn set_keyboard_mode(&self, window: &gtk::ApplicationWindow, mode: i32) {
        unsafe {
            (self.set_keyboard_mode)(gtk_window_ptr(window), mode);
        }
    }
}

fn gtk_window_ptr(window: &gtk::ApplicationWindow) -> *mut gtk::ffi::GtkWindow {
    let window: &gtk::Window = window.upcast_ref();
    window.to_glib_none().0
}

impl DesktopModeNotification {
    fn revealer(&self) -> &gtk::Revealer {
        &self.revealer
    }
}

impl CoreOverlayBackend for PlatformOverlayBackend {
    fn name(&self) -> &'static str {
        match self {
            Self::WaylandLayerShell(_) => "wayland-layer-shell",
            Self::X11 => "x11",
            Self::Unsupported => "unsupported",
        }
    }

    fn supports_overlay(&self) -> bool {
        matches!(self, Self::WaylandLayerShell(_) | Self::X11)
    }
}

/// Builds the overlay window for the active GTK backend.
pub fn build_window(
    application: &gtk::Application,
    gamepad_receiver: Receiver<GamepadCommand>,
    sidemenu_receiver: Receiver<SideMenuCommand>,
    grab_sender: Sender<GamepadGrabCommand>,
    virtual_keyboard: SharedVirtualKeyboard,
) -> Result<gtk::ApplicationWindow> {
    install_overlay_css();
    let system_backend = LinuxSystemBackend;
    let app_config = system_backend.load_config();

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

    let _backend = configure_overlay_backend(&window);

    let keyboard_controller = Rc::new(RefCell::new(None::<OnScreenKeyboard>));
    let keyboard_widget = Rc::new(RefCell::new(None::<gtk::Grid>));
    let sidemenu_revealer = Rc::new(RefCell::new(None::<gtk::Revealer>));

    let layout_changed = {
        let window = window.clone();
        let keyboard_widget = keyboard_widget.clone();
        let sidemenu_revealer = sidemenu_revealer.clone();
        Rc::new(move || {
            let Some(keyboard) = keyboard_widget.borrow().as_ref().cloned() else {
                return;
            };
            let Some(sidemenu) = sidemenu_revealer.borrow().as_ref().cloned() else {
                return;
            };
            schedule_input_region_update(&window, &keyboard, &sidemenu);
            schedule_delayed_input_region_update(&window, &keyboard, &sidemenu);
        })
    };
    let osk_scale_changed = {
        let keyboard_controller = keyboard_controller.clone();
        let layout_changed = layout_changed.clone();
        Rc::new(move |scale| {
            if let Some(keyboard) = keyboard_controller.borrow().as_ref().cloned() {
                keyboard.set_scale(scale);
                layout_changed();
            }
        })
    };
    let sidemenu = sidemenu::build_sidemenu(
        app_config.clone(),
        grab_sender.clone(),
        osk_scale_changed,
        layout_changed,
    );
    sidemenu_revealer.replace(Some(sidemenu.revealer().clone()));
    let keyboard_placement = Rc::new(Cell::new(KeyboardPlacement::Bottom));
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
    keyboard.set_scale(app_config.on_screen_keyboard.scale);
    keyboard_controller.replace(Some(keyboard.clone()));
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

fn configure_overlay_backend(window: &gtk::ApplicationWindow) -> PlatformOverlayBackend {
    let backend = detect_overlay_backend();

    match &backend {
        PlatformOverlayBackend::WaylandLayerShell(api) => {
            if let Err(error) = api.init_overlay_window(window) {
                eprintln!("Failed to initialise layer-shell overlay: {error:#}");
            }
        }
        PlatformOverlayBackend::X11 => install_x11_overlay_hooks(window),
        PlatformOverlayBackend::Unsupported => {
            eprintln!(
                "GameEase is running on an unsupported GTK backend; overlay behavior may be limited"
            );
        }
    }

    backend
}

fn detect_overlay_backend() -> PlatformOverlayBackend {
    let Some(display) = gtk::gdk::Display::default() else {
        return PlatformOverlayBackend::Unsupported;
    };
    let backend = display.backend();

    if backend.is_wayland() {
        match LayerShellApi::load() {
            Ok(api) => PlatformOverlayBackend::WaylandLayerShell(Rc::new(api)),
            Err(error) => {
                eprintln!(
                    "Wayland layer-shell backend is unavailable because gtk4-layer-shell could not be loaded: {error:#}"
                );
                PlatformOverlayBackend::Unsupported
            }
        }
    } else if backend.is_x11() {
        PlatformOverlayBackend::X11
    } else {
        PlatformOverlayBackend::Unsupported
    }
}

fn set_window_keyboard_mode(window: &gtk::ApplicationWindow, mode: i32) {
    if let PlatformOverlayBackend::WaylandLayerShell(api) = detect_overlay_backend() {
        api.set_keyboard_mode(window, mode);
    }
}

fn is_x11_display_backend() -> bool {
    gtk::gdk::Display::default()
        .map(|display| display.backend().is_x11())
        .unwrap_or(false)
}

fn install_x11_overlay_hooks(window: &gtk::ApplicationWindow) {
    let window_for_realize = window.clone();
    window.connect_realize(move |_| {
        apply_x11_overlay_hints(&window_for_realize);
    });

    let window_for_map = window.clone();
    window.connect_map(move |_| {
        window_for_map.fullscreen();
        apply_x11_overlay_hints(&window_for_map);
    });
}

fn apply_x11_overlay_hints(window: &gtk::ApplicationWindow) {
    let Some(surface) = window.surface() else {
        return;
    };
    let Ok(x11_surface) = surface.downcast::<gdk4_x11::X11Surface>() else {
        return;
    };
    let xid = x11_surface.xid();
    let Ok(window_id) = u32::try_from(xid) else {
        eprintln!("X11 overlay window id is out of range: {xid}");
        return;
    };

    x11_surface.set_skip_taskbar_hint(true);
    x11_surface.set_skip_pager_hint(true);
    x11_surface.set_utf8_property("WM_WINDOW_ROLE", Some("gameease-overlay"));

    if let Err(error) = apply_x11_ewmh_hints(window_id) {
        eprintln!("Failed to apply X11 overlay hints: {error:#}");
    }
}

fn apply_x11_ewmh_hints(window: Window) -> anyhow::Result<()> {
    let (connection, screen_num) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen_num].root;
    let atoms = X11Atoms::new(&connection)?;

    connection.change_property32(
        PropMode::REPLACE,
        window,
        atoms.net_wm_state,
        AtomEnum::ATOM,
        &[
            atoms.net_wm_state_above,
            atoms.net_wm_state_sticky,
            atoms.net_wm_state_skip_taskbar,
            atoms.net_wm_state_skip_pager,
            atoms.net_wm_state_fullscreen,
        ],
    )?;
    connection.change_property32(
        PropMode::REPLACE,
        window,
        atoms.net_wm_window_type,
        AtomEnum::ATOM,
        &[atoms.net_wm_window_type_dock],
    )?;

    send_x11_state_request(
        &connection,
        root,
        window,
        atoms.net_wm_state,
        atoms.net_wm_state_above,
        atoms.net_wm_state_sticky,
    )?;
    connection.configure_window(
        window,
        &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
    )?;
    connection.flush()?;

    Ok(())
}

fn send_x11_state_request<C: Connection>(
    connection: &C,
    root: Window,
    window: Window,
    net_wm_state: Atom,
    first_atom: Atom,
    second_atom: Atom,
) -> anyhow::Result<()> {
    let event =
        ClientMessageEvent::new(32, window, net_wm_state, [1, first_atom, second_atom, 1, 0]);
    connection.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )?;
    Ok(())
}

struct X11Atoms {
    net_wm_state: Atom,
    net_wm_state_above: Atom,
    net_wm_state_fullscreen: Atom,
    net_wm_state_skip_pager: Atom,
    net_wm_state_skip_taskbar: Atom,
    net_wm_state_sticky: Atom,
    net_wm_window_type: Atom,
    net_wm_window_type_dock: Atom,
}

impl X11Atoms {
    fn new<C: Connection>(connection: &C) -> anyhow::Result<Self> {
        Ok(Self {
            net_wm_state: intern_x11_atom(connection, "_NET_WM_STATE")?,
            net_wm_state_above: intern_x11_atom(connection, "_NET_WM_STATE_ABOVE")?,
            net_wm_state_fullscreen: intern_x11_atom(connection, "_NET_WM_STATE_FULLSCREEN")?,
            net_wm_state_skip_pager: intern_x11_atom(connection, "_NET_WM_STATE_SKIP_PAGER")?,
            net_wm_state_skip_taskbar: intern_x11_atom(connection, "_NET_WM_STATE_SKIP_TASKBAR")?,
            net_wm_state_sticky: intern_x11_atom(connection, "_NET_WM_STATE_STICKY")?,
            net_wm_window_type: intern_x11_atom(connection, "_NET_WM_WINDOW_TYPE")?,
            net_wm_window_type_dock: intern_x11_atom(connection, "_NET_WM_WINDOW_TYPE_DOCK")?,
        })
    }
}

fn intern_x11_atom<C: Connection>(connection: &C, name: &str) -> anyhow::Result<Atom> {
    Ok(connection
        .intern_atom(false, name.as_bytes())?
        .reply()?
        .atom)
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
        set_window_keyboard_mode(window, GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND);
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
        set_window_keyboard_mode(window, GTK_LAYER_SHELL_KEYBOARD_MODE_NONE);
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
        if is_x11_display_backend() {
            window.fullscreen();
            apply_x11_overlay_hints(window);
        }
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

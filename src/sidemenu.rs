use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::audio::{AudioController, AudioSnapshot, SharedAudioController};
use crate::bluetooth::{BluetoothDevice, BluetoothEvent, BluetoothManager, BluetoothWorker};
use crate::gamepad::KeyboardDirection;
use crate::tasks::{TaskEntry, TaskManager};
use crate::wifi::{WifiEvent, WifiManager, WifiNetwork, WifiWorker};

const SIDE_MENU_WIDTH: i32 = 280;
const WIFI_PANEL_WIDTH: i32 = 360;
const TASK_PANEL_WIDTH: i32 = 380;
const BLUETOOTH_PANEL_WIDTH: i32 = 380;
const CONTROLLER_PANEL_WIDTH: i32 = 320;
const VOLUME_STEP: f64 = 5.0;
const MAX_VOLUME: f64 = 100.0;

/// Action produced by activating a side menu row.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum SideMenuAction {
    /// The selected row does not need overlay-level handling.
    None,
    /// Close the side menu after completing an action.
    CloseMenu,
    /// Quit GameEase.
    Quit,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SideMenuRowKind {
    Volume,
    Brightness,
    Wifi,
    TaskSwitcher,
    Bluetooth,
    Quit,
}

struct SideMenuState {
    revealer: gtk::Revealer,
    root_panel: gtk::Box,
    wifi_panel: gtk::Box,
    task_panel: gtk::Box,
    bluetooth_panel: gtk::Box,
    controller_panel: gtk::Box,
    rows: Vec<gtk::ListBoxRow>,
    row_kinds: Vec<SideMenuRowKind>,
    selected_index: Cell<usize>,
    audio: Option<SharedAudioController>,
    volume_scale: gtk::Scale,
    mute_image: gtk::Image,
    audio_result_sender: Sender<AudioUiMessage>,
    updating_audio_widgets: Cell<bool>,
    audio_snapshot_pending: Cell<bool>,
    wifi_worker: Option<WifiWorker>,
    wifi_stack: gtk::Stack,
    wifi_spinner: gtk::Spinner,
    wifi_scan_button: gtk::Button,
    wifi_network_list: gtk::ListBox,
    wifi_error_label: gtk::Label,
    wifi_password_title: gtk::Label,
    wifi_password_entry: gtk::Entry,
    wifi_password_error: gtk::Label,
    wifi_networks: RefCell<Vec<WifiNetwork>>,
    wifi_network_rows: RefCell<Vec<gtk::ListBoxRow>>,
    wifi_selected_index: Cell<usize>,
    wifi_pending_ssid: RefCell<Option<String>>,
    wifi_connecting_ssid: RefCell<Option<String>>,
    task_list: gtk::ListBox,
    task_error_label: gtk::Label,
    tasks: RefCell<Vec<TaskEntry>>,
    task_rows: RefCell<Vec<gtk::ListBoxRow>>,
    task_selected_index: Cell<usize>,
    bluetooth_worker: Option<BluetoothWorker>,
    bluetooth_spinner: gtk::Spinner,
    bluetooth_device_list: gtk::ListBox,
    bluetooth_error_label: gtk::Label,
    bluetooth_reorder_button: gtk::Button,
    bluetooth_action_popover: gtk::Popover,
    bluetooth_devices: RefCell<Vec<BluetoothDevice>>,
    bluetooth_device_rows: RefCell<Vec<gtk::ListBoxRow>>,
    bluetooth_selected_index: Cell<usize>,
    bluetooth_busy: Cell<bool>,
    bluetooth_popover_device: RefCell<Option<BluetoothDevice>>,
    controller_list: gtk::ListBox,
    controller_rows: RefCell<Vec<gtk::ListBoxRow>>,
    controller_order: RefCell<Vec<BluetoothDevice>>,
    controller_selected_index: Cell<usize>,
}

enum AudioUiMessage {
    Snapshot(AudioSnapshot),
    Error(String),
}

/// Slide-in side menu widget and gamepad selection controller.
#[derive(Clone)]
pub struct SideMenu {
    revealer: gtk::Revealer,
    state: Rc<SideMenuState>,
}

impl SideMenu {
    /// Returns the side menu revealer widget.
    pub fn revealer(&self) -> &gtk::Revealer {
        &self.revealer
    }

    /// Moves the selected side menu row or adjusts the focused row value.
    pub fn move_selection(&self, direction: KeyboardDirection) {
        match direction {
            KeyboardDirection::Up | KeyboardDirection::Down
                if self.state.is_task_panel_open() && self.state.move_task_selection(direction) =>
            {
                return;
            }
            _ if self.state.is_controller_panel_open()
                && self.state.move_controller_selection(direction) =>
            {
                return;
            }
            _ if self.state.is_bluetooth_panel_open()
                && self.state.move_bluetooth_selection(direction) =>
            {
                return;
            }
            KeyboardDirection::Up | KeyboardDirection::Down
                if self.state.is_wifi_panel_open() && self.state.move_wifi_selection(direction) =>
            {
                return;
            }
            KeyboardDirection::Up | KeyboardDirection::Down => {
                let current = self.state.selected_index.get();
                let last = self.state.rows.len().saturating_sub(1);
                let next = match direction {
                    KeyboardDirection::Up => current.saturating_sub(1),
                    KeyboardDirection::Down => (current + 1).min(last),
                    KeyboardDirection::Left | KeyboardDirection::Right => current,
                };

                self.state.select(next);
            }
            KeyboardDirection::Left | KeyboardDirection::Right => {
                if self.state.selected_kind() == SideMenuRowKind::Volume {
                    self.state.adjust_volume(direction);
                }
            }
        }
    }

    /// Activates the selected row and returns its action.
    pub fn activate_selected(&self) -> SideMenuAction {
        if self.state.is_task_panel_open() {
            return self.state.activate_task_selection();
        }
        if self.state.is_controller_panel_open() {
            return SideMenuAction::None;
        }
        if self.state.is_bluetooth_panel_open() {
            return self.state.activate_bluetooth_selection();
        }

        match self.state.selected_kind() {
            SideMenuRowKind::Volume => {
                self.state.toggle_mute();
                SideMenuAction::None
            }
            SideMenuRowKind::Wifi => self.state.activate_wifi_selection(),
            SideMenuRowKind::TaskSwitcher => {
                self.state.open_task_panel();
                SideMenuAction::None
            }
            SideMenuRowKind::Bluetooth => {
                self.state.open_bluetooth_panel();
                SideMenuAction::None
            }
            SideMenuRowKind::Quit => SideMenuAction::Quit,
            SideMenuRowKind::Brightness => SideMenuAction::None,
        }
    }

    /// Long-activates the selected row when the current panel supports it.
    pub fn long_activate_selected(&self) {
        if self.state.is_bluetooth_panel_open() {
            self.state.long_activate_bluetooth_selection();
        }
    }

    /// Toggles scanning in the active side-menu panel.
    pub fn toggle_scan(&self) {
        if self.state.is_bluetooth_panel_open() {
            self.state.scan_bluetooth();
        }
    }

    /// Handles a cancel request and returns whether it was consumed by an active sub-panel.
    pub fn cancel_active_panel(&self) -> bool {
        self.state.cancel_active_panel()
    }

    /// Closes every side-menu sub-panel.
    pub fn close_subpanels(&self) {
        self.state.close_subpanels();
    }

    /// Terminates the currently selected item when supported by the active panel.
    pub fn terminate_selected(&self) {
        if self.state.is_task_panel_open() {
            self.state.terminate_task_selection();
        }
    }

    /// Returns whether the side menu currently expects OSK text input.
    pub fn is_keyboard_entry_active(&self) -> bool {
        self.state.is_wifi_password_panel_visible()
    }

    /// Runs a callback whenever password-entry mode is opened or closed.
    pub fn connect_keyboard_entry_active_notify(&self, notify: impl Fn() + 'static) {
        self.state
            .wifi_stack
            .connect_visible_child_name_notify(move |_| notify());
    }

    /// Focuses the active password entry.
    pub fn focus_keyboard_entry(&self) {
        if self.is_keyboard_entry_active() {
            self.state.wifi_password_entry.grab_focus();
        }
    }
}

impl SideMenuState {
    fn selected_kind(&self) -> SideMenuRowKind {
        self.row_kinds[self.selected_index.get()]
    }

    fn select(&self, index: usize) {
        self.rows[self.selected_index.get()].remove_css_class("side-menu-selected");
        self.selected_index.set(index);
        self.rows[index].add_css_class("side-menu-selected");
        self.rows[index].grab_focus();
    }

    fn adjust_volume(&self, direction: KeyboardDirection) {
        let delta = match direction {
            KeyboardDirection::Left => -VOLUME_STEP,
            KeyboardDirection::Right => VOLUME_STEP,
            KeyboardDirection::Up | KeyboardDirection::Down => 0.0,
        };
        let next = (self.volume_scale.value() + delta).clamp(0.0, MAX_VOLUME);
        self.set_volume_from_ui(next);
    }

    fn toggle_mute(&self) {
        let Some(audio) = self.audio.clone() else {
            return;
        };
        let sender = self.audio_result_sender.clone();

        thread::spawn(move || {
            let result = with_audio_controller(&audio, |controller| {
                controller.toggle_mute()?;
                controller.get_snapshot()
            });
            send_audio_result(sender, result);
        });
    }

    fn set_volume_from_ui(&self, value: f64) {
        let value = value.clamp(0.0, MAX_VOLUME);
        self.updating_audio_widgets.set(true);
        self.volume_scale.set_value(value);
        self.updating_audio_widgets.set(false);

        let Some(audio) = self.audio.clone() else {
            return;
        };
        let sender = self.audio_result_sender.clone();

        thread::spawn(move || {
            let result = with_audio_controller(&audio, |controller| {
                controller.set_volume(value.round() as u8)?;
                controller.get_snapshot()
            });
            send_audio_result(sender, result);
        });
    }

    fn refresh_audio_widgets(&self) {
        if self.audio_snapshot_pending.get() {
            return;
        }

        let Some(audio) = self.audio.clone() else {
            return;
        };
        let sender = self.audio_result_sender.clone();
        self.audio_snapshot_pending.set(true);

        thread::spawn(move || {
            let result = with_audio_controller(&audio, |controller| controller.get_snapshot());
            send_audio_result(sender, result);
        });
    }

    fn apply_audio_snapshot(&self, snapshot: AudioSnapshot) {
        self.updating_audio_widgets.set(true);
        self.volume_scale.set_value(f64::from(snapshot.volume));
        self.updating_audio_widgets.set(false);

        self.mute_image.set_icon_name(Some(if snapshot.muted {
            "microphone-sensitivity-muted-symbolic"
        } else {
            "audio-volume-high-symbolic"
        }));
    }

    fn handle_audio_message(&self, message: AudioUiMessage) {
        self.audio_snapshot_pending.set(false);

        match message {
            AudioUiMessage::Snapshot(snapshot) => self.apply_audio_snapshot(snapshot),
            AudioUiMessage::Error(error) => eprintln!("Failed to update audio state: {error}"),
        }
    }

    fn open_task_panel(&self) {
        if self.is_task_panel_open() {
            return;
        }

        self.close_wifi_panel();
        self.close_bluetooth_panel();
        self.task_panel.set_visible(true);
        self.root_panel
            .set_width_request(SIDE_MENU_WIDTH + TASK_PANEL_WIDTH);
        self.revealer
            .set_width_request(SIDE_MENU_WIDTH + TASK_PANEL_WIDTH);
        self.refresh_tasks();
    }

    fn close_task_panel(&self) {
        if !self.is_task_panel_open() {
            return;
        }

        self.task_panel.set_visible(false);
        self.root_panel.set_width_request(SIDE_MENU_WIDTH);
        self.revealer.set_width_request(SIDE_MENU_WIDTH);
        self.rows[self.selected_index.get()].grab_focus();
    }

    fn close_subpanels(&self) {
        self.close_wifi_panel();
        self.close_task_panel();
        self.close_bluetooth_panel();
    }

    fn cancel_active_panel(&self) -> bool {
        if self.bluetooth_action_popover.is_visible() {
            self.bluetooth_action_popover.popdown();
            return true;
        }
        if self.is_controller_panel_open() {
            self.close_controller_panel();
            return true;
        }
        if self.is_bluetooth_panel_open() {
            self.close_bluetooth_panel();
            return true;
        }
        if self.is_wifi_panel_open() {
            self.close_wifi_panel();
            return true;
        }
        if self.is_task_panel_open() {
            self.close_task_panel();
            return true;
        }
        false
    }

    fn refresh_tasks(&self) {
        self.clear_task_error();

        match TaskManager::list() {
            Ok(tasks) => {
                self.tasks.replace(tasks);
                self.rebuild_task_rows();
            }
            Err(error) => {
                self.tasks.replace(Vec::new());
                self.rebuild_task_rows();
                self.set_task_error(&error.to_string());
            }
        }
    }

    fn move_task_selection(&self, direction: KeyboardDirection) -> bool {
        if !self.is_task_panel_open() {
            return false;
        }

        let row_count = self.task_rows.borrow().len();
        if row_count == 0 {
            return true;
        }

        let current = self
            .task_selected_index
            .get()
            .min(row_count.saturating_sub(1));
        let next = match direction {
            KeyboardDirection::Up if current > 0 => current - 1,
            KeyboardDirection::Down if current + 1 < row_count => current + 1,
            KeyboardDirection::Up | KeyboardDirection::Down => return true,
            KeyboardDirection::Left | KeyboardDirection::Right => current,
        };

        self.set_task_selection(next);
        true
    }

    fn activate_task_selection(&self) -> SideMenuAction {
        let tasks = self.tasks.borrow();
        if tasks.is_empty() {
            drop(tasks);
            self.refresh_tasks();
            return SideMenuAction::None;
        }

        let index = self
            .task_selected_index
            .get()
            .min(tasks.len().saturating_sub(1));
        let Some(task) = tasks.get(index).cloned() else {
            return SideMenuAction::None;
        };
        drop(tasks);

        match TaskManager::focus(&task.id) {
            Ok(()) => SideMenuAction::CloseMenu,
            Err(error) => {
                self.set_task_error(&error.to_string());
                SideMenuAction::None
            }
        }
    }

    fn terminate_task_selection(&self) {
        let tasks = self.tasks.borrow();
        if tasks.is_empty() {
            drop(tasks);
            self.refresh_tasks();
            return;
        }

        let index = self
            .task_selected_index
            .get()
            .min(tasks.len().saturating_sub(1));
        let Some(task) = tasks.get(index).cloned() else {
            return;
        };
        drop(tasks);

        match TaskManager::terminate(&task) {
            Ok(()) => {
                let mut tasks = self.tasks.borrow_mut();
                if index < tasks.len() {
                    tasks.remove(index);
                }
                drop(tasks);
                self.rebuild_task_rows();
            }
            Err(error) => self.set_task_error(&error.to_string()),
        }
    }

    fn rebuild_task_rows(&self) {
        while let Some(child) = self.task_list.first_child() {
            self.task_list.remove(&child);
        }

        let tasks = self.tasks.borrow();
        let mut rows = Vec::new();

        for task in tasks.iter() {
            let row = build_task_row(task);
            self.task_list.append(&row);
            rows.push(row);
        }

        if rows.is_empty() {
            let row = build_task_placeholder_row("No apps found");
            self.task_list.append(&row);
        }

        self.task_rows.replace(rows);
        let row_count = self.task_rows.borrow().len();
        if row_count > 0 {
            let index = self.task_selected_index.get().min(row_count - 1);
            self.set_task_selection(index);
        } else {
            self.task_selected_index.set(0);
        }
    }

    fn set_task_selection(&self, index: usize) {
        let rows = self.task_rows.borrow();
        if rows.is_empty() {
            self.task_selected_index.set(0);
            return;
        }

        let previous = self
            .task_selected_index
            .get()
            .min(rows.len().saturating_sub(1));
        rows[previous].remove_css_class("task-row-selected");

        let next = index.min(rows.len().saturating_sub(1));
        self.task_selected_index.set(next);
        rows[next].add_css_class("task-row-selected");
        rows[next].grab_focus();
    }

    fn set_task_error(&self, error: &str) {
        self.task_error_label.set_text(error);
        self.task_error_label.set_visible(true);
    }

    fn clear_task_error(&self) {
        self.task_error_label.set_text("");
        self.task_error_label.set_visible(false);
    }

    fn scan_wifi(&self) {
        self.clear_wifi_error();

        let Some(worker) = self.wifi_worker.clone() else {
            self.set_wifi_error("Wi-Fi backend unavailable");
            return;
        };

        self.set_wifi_busy(true);
        worker.scan();
    }

    fn move_wifi_selection(&self, direction: KeyboardDirection) -> bool {
        if !self.is_wifi_panel_open() {
            return false;
        }

        if self.is_wifi_password_panel_visible() {
            return true;
        }

        let item_count = self.wifi_selectable_count();
        let current = self
            .wifi_selected_index
            .get()
            .min(item_count.saturating_sub(1));
        let next = match direction {
            KeyboardDirection::Up if current > 0 => current - 1,
            KeyboardDirection::Down if current + 1 < item_count => current + 1,
            KeyboardDirection::Up | KeyboardDirection::Down => return true,
            KeyboardDirection::Left | KeyboardDirection::Right => current,
        };

        self.set_wifi_selection(next);
        true
    }

    fn activate_wifi_selection(&self) -> SideMenuAction {
        if !self.is_wifi_panel_open() {
            self.open_wifi_panel();
            return SideMenuAction::None;
        }

        if self.is_wifi_password_panel_visible() {
            self.connect_pending_wifi();
            return SideMenuAction::None;
        }

        let network_count = self.wifi_network_rows.borrow().len();
        let index = self.wifi_selected_index.get().min(network_count);
        if index == network_count {
            self.scan_wifi();
            return SideMenuAction::None;
        }

        let networks = self.wifi_networks.borrow();
        if networks.is_empty() {
            drop(networks);
            self.scan_wifi();
            return SideMenuAction::None;
        }

        let Some(network) = networks.get(index).cloned() else {
            return SideMenuAction::None;
        };
        drop(networks);

        if network.connected {
            return SideMenuAction::None;
        }

        if network.secured {
            self.start_wifi_saved_connection_or_prompt(network.ssid);
            SideMenuAction::None
        } else {
            self.start_wifi_connect(network.ssid, None);
            SideMenuAction::None
        }
    }

    fn open_wifi_panel(&self) {
        if self.is_wifi_panel_open() {
            return;
        }

        self.close_task_panel();
        self.close_bluetooth_panel();
        self.wifi_panel.set_visible(true);
        self.root_panel
            .set_width_request(SIDE_MENU_WIDTH + WIFI_PANEL_WIDTH);
        self.revealer
            .set_width_request(SIDE_MENU_WIDTH + WIFI_PANEL_WIDTH);
        self.set_wifi_selection(self.wifi_selected_index.get());
        self.scan_wifi();
    }

    fn close_wifi_panel(&self) {
        if !self.is_wifi_panel_open() {
            return;
        }

        self.wifi_stack.set_visible_child_name("networks");
        self.wifi_pending_ssid.replace(None);
        self.wifi_password_entry.set_text("");
        self.wifi_password_error.set_text("");
        self.wifi_password_error.set_visible(false);
        self.wifi_panel.set_visible(false);
        self.root_panel.set_width_request(SIDE_MENU_WIDTH);
        self.revealer.set_width_request(SIDE_MENU_WIDTH);
        self.wifi_scan_button
            .remove_css_class("wifi-network-selected");
        self.rows[self.selected_index.get()].grab_focus();
    }

    fn show_wifi_password_panel(&self, ssid: &str) {
        self.wifi_pending_ssid.replace(Some(ssid.to_string()));
        self.wifi_password_title
            .set_text(&format!("Connect to {ssid}"));
        self.wifi_password_entry.set_text("");
        self.wifi_password_error.set_text("");
        self.wifi_password_error.set_visible(false);
        self.wifi_stack.set_visible_child_name("password");
        self.wifi_password_entry.grab_focus();
    }

    fn connect_pending_wifi(&self) {
        let Some(ssid) = self.wifi_pending_ssid.borrow().clone() else {
            return;
        };
        let password = self.wifi_password_entry.text().to_string();

        if password.is_empty() {
            self.wifi_password_error.set_text("Password required");
            self.wifi_password_error.set_visible(true);
            return;
        }

        self.start_wifi_connect(ssid, Some(password));
    }

    fn start_wifi_connect(&self, ssid: String, password: Option<String>) {
        self.clear_wifi_error();

        let Some(worker) = self.wifi_worker.clone() else {
            self.set_wifi_error("Wi-Fi backend unavailable");
            return;
        };

        self.wifi_connecting_ssid.replace(Some(ssid.clone()));
        self.rebuild_wifi_rows();
        worker.connect(ssid, password);
    }

    fn start_wifi_saved_connection_or_prompt(&self, ssid: String) {
        self.clear_wifi_error();

        let Some(worker) = self.wifi_worker.clone() else {
            self.set_wifi_error("Wi-Fi backend unavailable");
            return;
        };

        self.wifi_connecting_ssid.replace(Some(ssid.clone()));
        self.rebuild_wifi_rows();
        worker.connect_or_request_password(ssid);
    }

    fn handle_wifi_event(&self, event: WifiEvent) {
        match event {
            WifiEvent::ScanStarted => {
                self.set_wifi_busy(true);
                self.clear_wifi_error();
            }
            WifiEvent::ScanFinished(networks) => {
                self.set_wifi_busy(false);
                self.wifi_connecting_ssid.replace(None);
                self.wifi_networks.replace(networks);
                self.rebuild_wifi_rows();
            }
            WifiEvent::ConnectStarted(ssid) => {
                self.wifi_connecting_ssid.replace(Some(ssid));
                self.rebuild_wifi_rows();
            }
            WifiEvent::PasswordRequired(ssid) => {
                self.wifi_connecting_ssid.replace(None);
                self.rebuild_wifi_rows();
                self.show_wifi_password_panel(&ssid);
            }
            WifiEvent::ConnectFinished(Ok(())) => {
                self.wifi_pending_ssid.replace(None);
                self.wifi_password_entry.set_text("");
                self.wifi_password_error.set_text("");
                self.wifi_password_error.set_visible(false);
                self.wifi_stack.set_visible_child_name("networks");
            }
            WifiEvent::ConnectFinished(Err(error)) => {
                self.wifi_connecting_ssid.replace(None);
                if self.is_wifi_password_panel_visible() {
                    self.wifi_password_error.set_text(&error);
                    self.wifi_password_error.set_visible(true);
                } else {
                    self.set_wifi_error(&error);
                }
                self.rebuild_wifi_rows();
            }
            WifiEvent::Error(error) => {
                self.set_wifi_busy(false);
                self.wifi_connecting_ssid.replace(None);
                self.set_wifi_error(&error);
                self.rebuild_wifi_rows();
            }
        }
    }

    fn rebuild_wifi_rows(&self) {
        while let Some(child) = self.wifi_network_list.first_child() {
            self.wifi_network_list.remove(&child);
        }

        let networks = self.wifi_networks.borrow();
        let connecting_ssid = self.wifi_connecting_ssid.borrow().clone();
        let mut rows = Vec::new();

        for network in networks.iter() {
            let row = build_wifi_network_row(
                network,
                connecting_ssid.as_deref() == Some(network.ssid.as_str()),
            );
            self.wifi_network_list.append(&row);
            rows.push(row);
        }

        if rows.is_empty() {
            let row = build_wifi_placeholder_row("No networks found");
            self.wifi_network_list.append(&row);
        }

        self.wifi_network_rows.replace(rows);
        let item_count = self.wifi_selectable_count();
        let index = self
            .wifi_selected_index
            .get()
            .min(item_count.saturating_sub(1));
        self.set_wifi_selection(index);
    }

    fn set_wifi_selection(&self, index: usize) {
        let rows = self.wifi_network_rows.borrow();
        let previous = self.wifi_selected_index.get().min(rows.len());
        if previous < rows.len() {
            rows[previous].remove_css_class("wifi-network-selected");
        } else {
            self.wifi_scan_button
                .remove_css_class("wifi-network-selected");
        }

        let next = index.min(rows.len());
        self.wifi_selected_index.set(next);
        if next < rows.len() {
            rows[next].add_css_class("wifi-network-selected");
            rows[next].grab_focus();
        } else {
            self.wifi_scan_button.add_css_class("wifi-network-selected");
            self.wifi_scan_button.grab_focus();
        }
    }

    fn wifi_selectable_count(&self) -> usize {
        self.wifi_network_rows.borrow().len() + 1
    }

    fn set_wifi_busy(&self, busy: bool) {
        self.wifi_scan_button.set_sensitive(!busy);
        self.wifi_spinner.set_visible(busy);
        if busy {
            self.wifi_spinner.start();
        } else {
            self.wifi_spinner.stop();
        }
    }

    fn set_wifi_error(&self, error: &str) {
        self.wifi_error_label.set_text(error);
        self.wifi_error_label.set_visible(true);
    }

    fn clear_wifi_error(&self) {
        self.wifi_error_label.set_text("");
        self.wifi_error_label.set_visible(false);
    }

    fn is_wifi_password_panel_visible(&self) -> bool {
        self.wifi_stack
            .visible_child_name()
            .as_deref()
            .is_some_and(|name| name == "password")
    }

    fn open_bluetooth_panel(&self) {
        if self.is_bluetooth_panel_open() {
            return;
        }

        self.close_wifi_panel();
        self.close_task_panel();
        self.bluetooth_panel.set_visible(true);
        self.root_panel
            .set_width_request(SIDE_MENU_WIDTH + BLUETOOTH_PANEL_WIDTH);
        self.revealer
            .set_width_request(SIDE_MENU_WIDTH + BLUETOOTH_PANEL_WIDTH);
        self.set_bluetooth_selection(self.bluetooth_selected_index.get());
        self.refresh_bluetooth();
    }

    fn close_bluetooth_panel(&self) {
        if !self.is_bluetooth_panel_open() {
            return;
        }

        self.close_controller_panel();
        self.bluetooth_action_popover.popdown();
        self.bluetooth_panel.set_visible(false);
        self.root_panel.set_width_request(SIDE_MENU_WIDTH);
        self.revealer.set_width_request(SIDE_MENU_WIDTH);
        self.bluetooth_reorder_button
            .remove_css_class("bluetooth-device-selected");
        self.rows[self.selected_index.get()].grab_focus();
    }

    fn open_controller_panel(&self) {
        if self.is_controller_panel_open() {
            return;
        }

        self.rebuild_controller_rows();
        self.controller_panel.set_visible(true);
        self.root_panel
            .set_width_request(SIDE_MENU_WIDTH + BLUETOOTH_PANEL_WIDTH + CONTROLLER_PANEL_WIDTH);
        self.revealer
            .set_width_request(SIDE_MENU_WIDTH + BLUETOOTH_PANEL_WIDTH + CONTROLLER_PANEL_WIDTH);
    }

    fn close_controller_panel(&self) {
        if !self.is_controller_panel_open() {
            return;
        }

        self.controller_panel.set_visible(false);
        self.root_panel
            .set_width_request(SIDE_MENU_WIDTH + BLUETOOTH_PANEL_WIDTH);
        self.revealer
            .set_width_request(SIDE_MENU_WIDTH + BLUETOOTH_PANEL_WIDTH);
    }

    fn refresh_bluetooth(&self) {
        let Some(worker) = self.bluetooth_worker.clone() else {
            self.set_bluetooth_error("Bluetooth backend unavailable");
            return;
        };
        self.clear_bluetooth_error();
        worker.refresh();
    }

    fn scan_bluetooth(&self) {
        let Some(worker) = self.bluetooth_worker.clone() else {
            self.set_bluetooth_error("Bluetooth backend unavailable");
            return;
        };
        self.set_bluetooth_busy(true);
        self.clear_bluetooth_error();
        worker.scan();
    }

    fn move_bluetooth_selection(&self, direction: KeyboardDirection) -> bool {
        if !self.is_bluetooth_panel_open() {
            return false;
        }

        let item_count = self.bluetooth_selectable_count();
        let current = self
            .bluetooth_selected_index
            .get()
            .min(item_count.saturating_sub(1));
        let next = match direction {
            KeyboardDirection::Up if current > 0 => current - 1,
            KeyboardDirection::Down if current + 1 < item_count => current + 1,
            KeyboardDirection::Up | KeyboardDirection::Down => return true,
            KeyboardDirection::Left | KeyboardDirection::Right => current,
        };

        self.set_bluetooth_selection(next);
        true
    }

    fn activate_bluetooth_selection(&self) -> SideMenuAction {
        let device_count = self.bluetooth_device_rows.borrow().len();
        let index = self.bluetooth_selected_index.get().min(device_count);
        if index == device_count {
            self.open_controller_panel();
            return SideMenuAction::None;
        }

        let devices = self.bluetooth_devices.borrow();
        let Some(device) = devices.get(index).cloned() else {
            return SideMenuAction::None;
        };
        drop(devices);

        if device.connected {
            return SideMenuAction::None;
        }

        let Some(worker) = self.bluetooth_worker.clone() else {
            self.set_bluetooth_error("Bluetooth backend unavailable");
            return SideMenuAction::None;
        };

        self.clear_bluetooth_error();
        self.set_bluetooth_busy(true);
        if device.paired {
            worker.connect(device.id, device.name);
        } else {
            worker.pair_and_connect(device.id, device.name);
        }

        SideMenuAction::None
    }

    fn long_activate_bluetooth_selection(&self) {
        let device_count = self.bluetooth_device_rows.borrow().len();
        let index = self.bluetooth_selected_index.get().min(device_count);
        if index >= device_count {
            return;
        }

        let devices = self.bluetooth_devices.borrow();
        let Some(device) = devices
            .get(index)
            .cloned()
            .filter(|device| device.connected)
        else {
            return;
        };
        drop(devices);

        let rows = self.bluetooth_device_rows.borrow();
        if let Some(row) = rows.get(index) {
            self.bluetooth_popover_device.replace(Some(device));
            if self.bluetooth_action_popover.parent().is_some() {
                self.bluetooth_action_popover.unparent();
            }
            self.bluetooth_action_popover.set_parent(row);
            self.bluetooth_action_popover.popup();
        }
    }

    fn handle_bluetooth_event(&self, event: BluetoothEvent) {
        match event {
            BluetoothEvent::ScanStarted => {
                self.set_bluetooth_busy(true);
                self.clear_bluetooth_error();
            }
            BluetoothEvent::DevicesUpdated(devices) => {
                self.set_bluetooth_busy(false);
                self.bluetooth_devices.replace(devices);
                self.rebuild_bluetooth_rows();
                self.rebuild_controller_rows();
            }
            BluetoothEvent::OperationStarted(name) => {
                self.set_bluetooth_busy(true);
                self.clear_bluetooth_error();
                self.set_bluetooth_error(&format!("Working on {name}..."));
            }
            BluetoothEvent::OperationFinished(Ok(())) => {
                self.set_bluetooth_busy(false);
                self.clear_bluetooth_error();
            }
            BluetoothEvent::OperationFinished(Err(error)) | BluetoothEvent::Error(error) => {
                self.set_bluetooth_busy(false);
                self.set_bluetooth_error(&error);
            }
        }
    }

    fn rebuild_bluetooth_rows(&self) {
        while let Some(child) = self.bluetooth_device_list.first_child() {
            self.bluetooth_device_list.remove(&child);
        }

        let devices = self.bluetooth_devices.borrow();
        let mut rows = Vec::new();

        for device in devices.iter() {
            let row = build_bluetooth_device_row(device);
            self.bluetooth_device_list.append(&row);
            rows.push(row);
        }

        if rows.is_empty() {
            self.bluetooth_device_list
                .append(&build_bluetooth_placeholder_row("No devices found"));
        }

        self.bluetooth_device_rows.replace(rows);
        let index = self
            .bluetooth_selected_index
            .get()
            .min(self.bluetooth_selectable_count().saturating_sub(1));
        self.set_bluetooth_selection(index);
    }

    fn set_bluetooth_selection(&self, index: usize) {
        let rows = self.bluetooth_device_rows.borrow();
        let previous = self.bluetooth_selected_index.get().min(rows.len());
        if previous < rows.len() {
            rows[previous].remove_css_class("bluetooth-device-selected");
        } else {
            self.bluetooth_reorder_button
                .remove_css_class("bluetooth-device-selected");
        }

        let next = index.min(rows.len());
        self.bluetooth_selected_index.set(next);
        if next < rows.len() {
            rows[next].add_css_class("bluetooth-device-selected");
            rows[next].grab_focus();
        } else {
            self.bluetooth_reorder_button
                .add_css_class("bluetooth-device-selected");
            self.bluetooth_reorder_button.grab_focus();
        }
    }

    fn bluetooth_selectable_count(&self) -> usize {
        self.bluetooth_device_rows.borrow().len() + 1
    }

    fn move_controller_selection(&self, direction: KeyboardDirection) -> bool {
        if !self.is_controller_panel_open() {
            return false;
        }

        match direction {
            KeyboardDirection::Left => self.move_controller_order(-1),
            KeyboardDirection::Right => self.move_controller_order(1),
            KeyboardDirection::Up | KeyboardDirection::Down => {
                let row_count = self.controller_rows.borrow().len();
                if row_count == 0 {
                    return true;
                }
                let current = self.controller_selected_index.get().min(row_count - 1);
                let next = match direction {
                    KeyboardDirection::Up => current.saturating_sub(1),
                    KeyboardDirection::Down => (current + 1).min(row_count - 1),
                    KeyboardDirection::Left | KeyboardDirection::Right => current,
                };
                self.set_controller_selection(next);
            }
        }

        true
    }

    fn move_controller_order(&self, delta: isize) {
        let mut controllers = self.controller_order.borrow_mut();
        if controllers.len() < 2 {
            return;
        }
        let current = self
            .controller_selected_index
            .get()
            .min(controllers.len().saturating_sub(1));
        let next = if delta < 0 {
            current.saturating_sub(1)
        } else {
            (current + 1).min(controllers.len() - 1)
        };
        if current == next {
            return;
        }
        controllers.swap(current, next);
        drop(controllers);
        self.rebuild_controller_rows();
        self.set_controller_selection(next);
    }

    fn rebuild_controller_rows(&self) {
        while let Some(child) = self.controller_list.first_child() {
            self.controller_list.remove(&child);
        }

        let connected_gamepads: Vec<_> = self
            .bluetooth_devices
            .borrow()
            .iter()
            .filter(|device| device.connected && device.gamepad)
            .cloned()
            .collect();

        let mut current_order = self.controller_order.borrow().clone();
        current_order.retain(|device| {
            connected_gamepads
                .iter()
                .any(|candidate| candidate.id == device.id)
        });
        for device in connected_gamepads {
            if !current_order
                .iter()
                .any(|candidate| candidate.id == device.id)
            {
                current_order.push(device);
            }
        }
        self.controller_order.replace(current_order);

        let controllers = self.controller_order.borrow();
        let mut rows = Vec::new();
        for (index, controller) in controllers.iter().enumerate() {
            let row = build_controller_row(index, controller);
            self.controller_list.append(&row);
            rows.push(row);
        }

        if rows.is_empty() {
            self.controller_list
                .append(&build_bluetooth_placeholder_row("No connected gamepads"));
        }

        self.controller_rows.replace(rows);
        let row_count = self.controller_rows.borrow().len();
        if row_count > 0 {
            self.set_controller_selection(self.controller_selected_index.get().min(row_count - 1));
        } else {
            self.controller_selected_index.set(0);
        }
    }

    fn set_controller_selection(&self, index: usize) {
        let rows = self.controller_rows.borrow();
        if rows.is_empty() {
            self.controller_selected_index.set(0);
            return;
        }
        let previous = self.controller_selected_index.get().min(rows.len() - 1);
        rows[previous].remove_css_class("controller-row-selected");
        let next = index.min(rows.len() - 1);
        self.controller_selected_index.set(next);
        rows[next].add_css_class("controller-row-selected");
        rows[next].grab_focus();
    }

    fn set_bluetooth_busy(&self, busy: bool) {
        self.bluetooth_busy.set(busy);
        self.bluetooth_spinner.set_visible(busy);
        if busy {
            self.bluetooth_spinner.start();
        } else {
            self.bluetooth_spinner.stop();
        }
    }

    fn set_bluetooth_error(&self, error: &str) {
        self.bluetooth_error_label.set_text(error);
        self.bluetooth_error_label.set_visible(true);
    }

    fn clear_bluetooth_error(&self) {
        self.bluetooth_error_label.set_text("");
        self.bluetooth_error_label.set_visible(false);
    }

    fn is_wifi_panel_open(&self) -> bool {
        self.wifi_panel.get_visible()
    }

    fn is_task_panel_open(&self) -> bool {
        self.task_panel.get_visible()
    }

    fn is_bluetooth_panel_open(&self) -> bool {
        self.bluetooth_panel.get_visible()
    }

    fn is_controller_panel_open(&self) -> bool {
        self.controller_panel.get_visible()
    }
}

/// Builds the slide-in side menu revealer.
pub fn build_sidemenu() -> SideMenu {
    install_css();

    let audio = match AudioController::new_shared() {
        Ok(audio) => Some(audio),
        Err(error) => {
            eprintln!("Failed to initialise audio controller: {error:#}");
            None
        }
    };
    let (wifi_event_sender, wifi_event_receiver) = mpsc::channel();
    let wifi_worker = match WifiManager::spawn_worker(wifi_event_sender) {
        Ok(worker) => Some(worker),
        Err(error) => {
            eprintln!("Failed to initialise Wi-Fi worker: {error:#}");
            None
        }
    };
    let (bluetooth_event_sender, bluetooth_event_receiver) = mpsc::channel();
    let bluetooth_worker = match BluetoothManager::spawn_worker(bluetooth_event_sender) {
        Ok(worker) => Some(worker),
        Err(error) => {
            eprintln!("Failed to initialise Bluetooth worker: {error:#}");
            None
        }
    };

    let header = gtk::Label::builder()
        .label("GameEase")
        .halign(gtk::Align::Start)
        .margin_bottom(12)
        .margin_start(16)
        .margin_top(18)
        .build();
    header.add_css_class("title-2");

    let list = gtk::ListBox::builder()
        .margin_bottom(16)
        .margin_end(12)
        .margin_start(12)
        .margin_top(12)
        .selection_mode(gtk::SelectionMode::None)
        .build();

    let volume_scale =
        gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, MAX_VOLUME, VOLUME_STEP);
    volume_scale.set_draw_value(false);
    volume_scale.set_hexpand(true);
    volume_scale.set_sensitive(audio.is_some());
    let mute_image = gtk::Image::from_icon_name("audio-volume-high-symbolic");
    let mute_button = gtk::Button::builder()
        .can_focus(false)
        .focusable(false)
        .sensitive(audio.is_some())
        .child(&mute_image)
        .build();
    mute_button.add_css_class("side-menu-icon-button");

    let wifi_widgets = build_wifi_widgets(wifi_worker.is_some());
    let task_widgets = build_task_widgets();
    let bluetooth_widgets = build_bluetooth_widgets(bluetooth_worker.is_some());
    let controller_widgets = build_controller_widgets();

    let mut rows = Vec::new();
    let mut row_kinds = Vec::new();
    let volume_row = build_volume_row(&volume_scale, &mute_button);
    list.append(&volume_row);
    rows.push(volume_row);
    row_kinds.push(SideMenuRowKind::Volume);

    for (label, kind) in [("Brightness", SideMenuRowKind::Brightness)] {
        let row = build_label_row(label);
        list.append(&row);
        rows.push(row);
        row_kinds.push(kind);
    }

    let wifi_row = build_label_row("Wi-Fi");
    list.append(&wifi_row);
    rows.push(wifi_row);
    row_kinds.push(SideMenuRowKind::Wifi);

    let task_row = build_label_row("Task Switcher");
    list.append(&task_row);
    rows.push(task_row);
    row_kinds.push(SideMenuRowKind::TaskSwitcher);

    for (label, kind) in [
        ("Bluetooth", SideMenuRowKind::Bluetooth),
        ("Quit", SideMenuRowKind::Quit),
    ] {
        let row = build_label_row(label);
        list.append(&row);
        rows.push(row);
        row_kinds.push(kind);
    }

    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .width_request(SIDE_MENU_WIDTH)
        .build();
    panel.add_css_class("side-menu");
    panel.append(&header);
    panel.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    panel.append(&list);

    let root_panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .vexpand(true)
        .width_request(SIDE_MENU_WIDTH)
        .build();
    root_panel.append(&panel);
    root_panel.append(&wifi_widgets.panel);
    root_panel.append(&task_widgets.panel);
    root_panel.append(&bluetooth_widgets.panel);
    root_panel.append(&controller_widgets.panel);

    let revealer = gtk::Revealer::builder()
        .halign(gtk::Align::Start)
        .hexpand(false)
        .transition_duration(200)
        .transition_type(gtk::RevealerTransitionType::SlideRight)
        .valign(gtk::Align::Fill)
        .vexpand(true)
        .width_request(SIDE_MENU_WIDTH)
        .build();
    revealer.set_child(Some(&root_panel));
    revealer.set_reveal_child(false);

    let (audio_result_sender, audio_result_receiver) = mpsc::channel();
    let state = Rc::new(SideMenuState {
        revealer: revealer.clone(),
        root_panel: root_panel.clone(),
        wifi_panel: wifi_widgets.panel.clone(),
        task_panel: task_widgets.panel.clone(),
        bluetooth_panel: bluetooth_widgets.panel.clone(),
        controller_panel: controller_widgets.panel.clone(),
        rows,
        row_kinds,
        selected_index: Cell::new(0),
        audio,
        volume_scale: volume_scale.clone(),
        mute_image: mute_image.clone(),
        audio_result_sender,
        updating_audio_widgets: Cell::new(false),
        audio_snapshot_pending: Cell::new(false),
        wifi_worker,
        wifi_stack: wifi_widgets.stack.clone(),
        wifi_spinner: wifi_widgets.spinner.clone(),
        wifi_scan_button: wifi_widgets.scan_button.clone(),
        wifi_network_list: wifi_widgets.network_list.clone(),
        wifi_error_label: wifi_widgets.error_label.clone(),
        wifi_password_title: wifi_widgets.password_title.clone(),
        wifi_password_entry: wifi_widgets.password_entry.clone(),
        wifi_password_error: wifi_widgets.password_error.clone(),
        wifi_networks: RefCell::new(Vec::new()),
        wifi_network_rows: RefCell::new(Vec::new()),
        wifi_selected_index: Cell::new(0),
        wifi_pending_ssid: RefCell::new(None),
        wifi_connecting_ssid: RefCell::new(None),
        task_list: task_widgets.list.clone(),
        task_error_label: task_widgets.error_label.clone(),
        tasks: RefCell::new(Vec::new()),
        task_rows: RefCell::new(Vec::new()),
        task_selected_index: Cell::new(0),
        bluetooth_worker,
        bluetooth_spinner: bluetooth_widgets.spinner.clone(),
        bluetooth_device_list: bluetooth_widgets.device_list.clone(),
        bluetooth_error_label: bluetooth_widgets.error_label.clone(),
        bluetooth_reorder_button: bluetooth_widgets.reorder_button.clone(),
        bluetooth_action_popover: bluetooth_widgets.action_popover.clone(),
        bluetooth_devices: RefCell::new(Vec::new()),
        bluetooth_device_rows: RefCell::new(Vec::new()),
        bluetooth_selected_index: Cell::new(0),
        bluetooth_busy: Cell::new(false),
        bluetooth_popover_device: RefCell::new(None),
        controller_list: controller_widgets.list.clone(),
        controller_rows: RefCell::new(Vec::new()),
        controller_order: RefCell::new(Vec::new()),
        controller_selected_index: Cell::new(0),
    });
    state.rows[0].add_css_class("side-menu-selected");

    install_volume_handlers(&state, &volume_scale, &mute_button);
    install_wifi_handlers(&state, &wifi_widgets);
    install_task_handlers(&state, &task_widgets);
    install_bluetooth_handlers(&state, &bluetooth_widgets);
    state.refresh_audio_widgets();
    install_audio_result_poll(&state, audio_result_receiver);
    install_audio_poll(&state);
    install_wifi_result_poll(&state, wifi_event_receiver);
    install_bluetooth_result_poll(&state, bluetooth_event_receiver);

    SideMenu { revealer, state }
}

struct WifiWidgets {
    panel: gtk::Box,
    stack: gtk::Stack,
    spinner: gtk::Spinner,
    scan_button: gtk::Button,
    network_list: gtk::ListBox,
    error_label: gtk::Label,
    password_title: gtk::Label,
    password_entry: gtk::Entry,
    password_error: gtk::Label,
    connect_button: gtk::Button,
    cancel_button: gtk::Button,
}

struct TaskWidgets {
    panel: gtk::Box,
    list: gtk::ListBox,
    error_label: gtk::Label,
    refresh_button: gtk::Button,
}

struct BluetoothWidgets {
    panel: gtk::Box,
    spinner: gtk::Spinner,
    device_list: gtk::ListBox,
    error_label: gtk::Label,
    reorder_button: gtk::Button,
    action_popover: gtk::Popover,
    disconnect_button: gtk::Button,
    remove_button: gtk::Button,
}

struct ControllerWidgets {
    panel: gtk::Box,
    list: gtk::ListBox,
}

fn build_volume_row(volume_scale: &gtk::Scale, mute_button: &gtk::Button) -> gtk::ListBoxRow {
    let speaker = gtk::Image::from_icon_name("audio-volume-high-symbolic");
    let box_ = gtk::Box::builder()
        .margin_bottom(8)
        .margin_end(10)
        .margin_start(10)
        .margin_top(8)
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    box_.append(&speaker);
    box_.append(volume_scale);
    box_.append(mute_button);

    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .can_focus(true)
        .focusable(true)
        .selectable(false)
        .child(&box_)
        .build();
    row.add_css_class("side-menu-row");
    row
}

fn build_wifi_widgets(wifi_available: bool) -> WifiWidgets {
    let wifi_icon = gtk::Image::from_icon_name("network-wireless-symbolic");
    let header_label = gtk::Label::builder()
        .label("Wi-Fi")
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    header_label.add_css_class("heading");

    let spinner = gtk::Spinner::builder().visible(false).build();
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    header.add_css_class("wifi-section-header");
    header.append(&wifi_icon);
    header.append(&header_label);
    header.append(&spinner);

    let network_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    network_list.add_css_class("wifi-network-list");

    let error_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .margin_top(4)
        .visible(false)
        .wrap(true)
        .build();
    error_label.add_css_class("wifi-error");

    let scan_button = gtk::Button::builder()
        .can_focus(true)
        .focusable(true)
        .label("Scan")
        .sensitive(wifi_available)
        .build();
    scan_button.add_css_class("side-menu-action-button");

    let list_page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    list_page.append(&header);
    list_page.append(&network_list);
    list_page.append(&error_label);
    list_page.append(&scan_button);

    let password_title = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    password_title.add_css_class("heading");

    let password_entry = gtk::Entry::builder()
        .placeholder_text("Password")
        .visibility(false)
        .build();
    let password_error = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .visible(false)
        .wrap(true)
        .build();
    password_error.add_css_class("wifi-error");

    let connect_button = gtk::Button::builder()
        .can_focus(false)
        .focusable(false)
        .label("Connect")
        .build();
    let cancel_button = gtk::Button::builder()
        .can_focus(false)
        .focusable(false)
        .label("Cancel")
        .build();
    connect_button.add_css_class("side-menu-action-button");
    cancel_button.add_css_class("side-menu-action-button");

    let password_actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    password_actions.append(&cancel_button);
    password_actions.append(&connect_button);

    let password_page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    password_page.add_css_class("wifi-password-panel");
    password_page.append(&password_title);
    password_page.append(&password_entry);
    password_page.append(&password_error);
    password_page.append(&password_actions);

    let stack = gtk::Stack::builder()
        .transition_duration(200)
        .transition_type(gtk::StackTransitionType::SlideLeftRight)
        .build();
    stack.add_named(&list_page, Some("networks"));
    stack.add_named(&password_page, Some("password"));
    stack.set_visible_child_name("networks");

    let box_ = gtk::Box::builder()
        .margin_bottom(10)
        .margin_end(10)
        .margin_start(10)
        .margin_top(10)
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    box_.append(&stack);

    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .visible(false)
        .width_request(WIFI_PANEL_WIDTH)
        .build();
    panel.add_css_class("side-menu");
    panel.add_css_class("wifi-panel");
    panel.append(&box_);

    WifiWidgets {
        panel,
        stack,
        spinner,
        scan_button,
        network_list,
        error_label,
        password_title,
        password_entry,
        password_error,
        connect_button,
        cancel_button,
    }
}

fn build_task_widgets() -> TaskWidgets {
    let icon = gtk::Image::from_icon_name("view-grid-symbolic");
    let title = gtk::Label::builder()
        .label("Task Switcher")
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    title.add_css_class("heading");

    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    header.add_css_class("task-section-header");
    header.append(&icon);
    header.append(&title);

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.add_css_class("task-list");

    let error_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .margin_top(4)
        .visible(false)
        .wrap(true)
        .build();
    error_label.add_css_class("task-error");

    let refresh_button = gtk::Button::builder()
        .can_focus(false)
        .focusable(false)
        .label("Refresh")
        .build();
    refresh_button.add_css_class("side-menu-action-button");

    let content = gtk::Box::builder()
        .margin_bottom(10)
        .margin_end(10)
        .margin_start(10)
        .margin_top(10)
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    content.append(&header);
    content.append(&list);
    content.append(&error_label);
    content.append(&refresh_button);

    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .visible(false)
        .width_request(TASK_PANEL_WIDTH)
        .build();
    panel.add_css_class("side-menu");
    panel.add_css_class("task-panel");
    panel.append(&content);

    TaskWidgets {
        panel,
        list,
        error_label,
        refresh_button,
    }
}

fn build_bluetooth_widgets(bluetooth_available: bool) -> BluetoothWidgets {
    let icon = gtk::Image::from_icon_name("bluetooth-active-symbolic");
    let title = gtk::Label::builder()
        .label("Bluetooth")
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    title.add_css_class("heading");

    let spinner = gtk::Spinner::builder().visible(false).build();
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    header.add_css_class("bluetooth-section-header");
    header.append(&icon);
    header.append(&title);
    header.append(&spinner);

    let device_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    device_list.add_css_class("bluetooth-device-list");

    let error_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .margin_top(4)
        .visible(false)
        .wrap(true)
        .build();
    error_label.add_css_class("bluetooth-error");

    let reorder_button = gtk::Button::builder()
        .can_focus(true)
        .focusable(true)
        .label("Reorder Controllers")
        .sensitive(bluetooth_available)
        .build();
    reorder_button.add_css_class("side-menu-action-button");

    let disconnect_button = gtk::Button::builder()
        .can_focus(false)
        .focusable(false)
        .label("Disconnect")
        .build();
    let remove_button = gtk::Button::builder()
        .can_focus(false)
        .focusable(false)
        .label("Remove")
        .build();
    disconnect_button.add_css_class("side-menu-action-button");
    remove_button.add_css_class("side-menu-action-button");

    let popover_box = gtk::Box::builder()
        .margin_bottom(8)
        .margin_end(8)
        .margin_start(8)
        .margin_top(8)
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    popover_box.append(&disconnect_button);
    popover_box.append(&remove_button);

    let action_popover = gtk::Popover::builder().has_arrow(true).build();
    action_popover.add_css_class("bluetooth-action-popover");
    action_popover.set_child(Some(&popover_box));

    let hint = gtk::Label::builder()
        .label("Y scans. Hold A on a connected device for actions.")
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    hint.add_css_class("bluetooth-hint");

    let content = gtk::Box::builder()
        .margin_bottom(10)
        .margin_end(10)
        .margin_start(10)
        .margin_top(10)
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    content.append(&header);
    content.append(&device_list);
    content.append(&error_label);
    content.append(&reorder_button);
    content.append(&hint);

    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .visible(false)
        .width_request(BLUETOOTH_PANEL_WIDTH)
        .build();
    panel.add_css_class("side-menu");
    panel.add_css_class("bluetooth-panel");
    panel.append(&content);

    BluetoothWidgets {
        panel,
        spinner,
        device_list,
        error_label,
        reorder_button,
        action_popover,
        disconnect_button,
        remove_button,
    }
}

fn build_controller_widgets() -> ControllerWidgets {
    let icon = gtk::Image::from_icon_name("input-gaming-symbolic");
    let title = gtk::Label::builder()
        .label("Controller Order")
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    title.add_css_class("heading");

    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    header.add_css_class("controller-section-header");
    header.append(&icon);
    header.append(&title);

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.add_css_class("controller-list");

    let hint = gtk::Label::builder()
        .label("Left and Right move the selected controller in GameEase priority.")
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    hint.add_css_class("bluetooth-hint");

    let content = gtk::Box::builder()
        .margin_bottom(10)
        .margin_end(10)
        .margin_start(10)
        .margin_top(10)
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    content.append(&header);
    content.append(&list);
    content.append(&hint);

    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .visible(false)
        .width_request(CONTROLLER_PANEL_WIDTH)
        .build();
    panel.add_css_class("side-menu");
    panel.add_css_class("controller-panel");
    panel.append(&content);

    ControllerWidgets { panel, list }
}

fn build_wifi_network_row(network: &WifiNetwork, connecting: bool) -> gtk::ListBoxRow {
    let signal = gtk::Image::from_icon_name(signal_icon_name(network.strength));
    let ssid_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .hexpand(true)
        .wrap(true)
        .build();
    if network.connected {
        let escaped = glib::markup_escape_text(&network.ssid);
        ssid_label.set_markup(&format!("<b>{escaped}</b>"));
    } else {
        ssid_label.set_text(&network.ssid);
    }

    let lock = gtk::Image::from_icon_name("changes-prevent-symbolic");
    lock.set_visible(network.secured);

    let connected = gtk::Label::builder()
        .label("Connected")
        .visible(network.connected)
        .build();
    connected.add_css_class("wifi-connected-badge");

    let spinner = gtk::Spinner::builder().visible(connecting).build();
    if connecting {
        spinner.start();
    }

    let row_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    row_box.append(&signal);
    row_box.append(&ssid_label);
    row_box.append(&lock);
    row_box.append(&connected);
    row_box.append(&spinner);

    let row = gtk::ListBoxRow::builder()
        .activatable(true)
        .can_focus(true)
        .focusable(true)
        .selectable(false)
        .child(&row_box)
        .build();
    row.add_css_class("wifi-network-row");
    row
}

fn build_task_row(task: &TaskEntry) -> gtk::ListBoxRow {
    let icon = gtk::Image::from_icon_name("application-x-executable-symbolic");
    let title = gtk::Label::builder()
        .label(&task.title)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .wrap(true)
        .build();

    let subtitle_text = match task.pid {
        Some(pid) if !task.app_id.is_empty() => format!("{} · pid {pid}", task.app_id),
        Some(pid) => format!("pid {pid}"),
        None => task.app_id.clone(),
    };
    let subtitle = gtk::Label::builder()
        .label(&subtitle_text)
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    subtitle.add_css_class("task-subtitle");

    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .spacing(2)
        .build();
    labels.append(&title);
    if !subtitle_text.is_empty() {
        labels.append(&subtitle);
    }

    let active = gtk::Label::builder()
        .label("Active")
        .visible(task.active)
        .build();
    active.add_css_class("task-active-badge");

    let row_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    row_box.append(&icon);
    row_box.append(&labels);
    row_box.append(&active);

    let row = gtk::ListBoxRow::builder()
        .activatable(true)
        .can_focus(true)
        .focusable(true)
        .selectable(false)
        .child(&row_box)
        .build();
    row.add_css_class("task-row");
    row
}

fn build_bluetooth_device_row(device: &BluetoothDevice) -> gtk::ListBoxRow {
    let icon = gtk::Image::from_icon_name(if device.gamepad {
        "input-gaming-symbolic"
    } else if device.connected {
        "bluetooth-active-symbolic"
    } else {
        "bluetooth-symbolic"
    });
    let title = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .hexpand(true)
        .wrap(true)
        .build();
    if device.connected {
        let escaped = glib::markup_escape_text(&device.name);
        title.set_markup(&format!("<b>{escaped}</b>"));
    } else {
        title.set_text(&device.name);
    }

    let subtitle_text = if device.paired {
        device.address.clone()
    } else {
        format!("{} · unpaired", device.address)
    };
    let subtitle = gtk::Label::builder()
        .label(&subtitle_text)
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    subtitle.add_css_class("task-subtitle");

    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .spacing(2)
        .build();
    labels.append(&title);
    labels.append(&subtitle);

    let status = gtk::Label::builder()
        .label(if device.connected {
            "Connected"
        } else if device.paired {
            "Paired"
        } else {
            "New"
        })
        .build();
    status.add_css_class(if device.connected {
        "bluetooth-connected-badge"
    } else {
        "bluetooth-paired-badge"
    });

    let row_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    row_box.append(&icon);
    row_box.append(&labels);
    row_box.append(&status);

    let row = gtk::ListBoxRow::builder()
        .activatable(true)
        .can_focus(true)
        .focusable(true)
        .selectable(false)
        .child(&row_box)
        .build();
    row.add_css_class("bluetooth-device-row");
    row
}

fn build_controller_row(index: usize, device: &BluetoothDevice) -> gtk::ListBoxRow {
    let player = gtk::Label::builder()
        .label(&format!("P{}", index + 1))
        .width_request(32)
        .build();
    player.add_css_class("controller-player-badge");

    let name = gtk::Label::builder()
        .label(&device.name)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .wrap(true)
        .build();

    let row_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    row_box.append(&player);
    row_box.append(&name);

    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .can_focus(true)
        .focusable(true)
        .selectable(false)
        .child(&row_box)
        .build();
    row.add_css_class("controller-row");
    row
}

fn build_task_placeholder_row(label: &str) -> gtk::ListBoxRow {
    let row_label = gtk::Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .margin_bottom(6)
        .margin_top(6)
        .build();
    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(&row_label)
        .build();
    row.add_css_class("task-placeholder-row");
    row
}

fn build_wifi_placeholder_row(label: &str) -> gtk::ListBoxRow {
    let row_label = gtk::Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .margin_bottom(6)
        .margin_top(6)
        .build();
    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(&row_label)
        .build();
    row.add_css_class("wifi-placeholder-row");
    row
}

fn build_bluetooth_placeholder_row(label: &str) -> gtk::ListBoxRow {
    let row_label = gtk::Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .margin_bottom(6)
        .margin_top(6)
        .wrap(true)
        .build();
    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(&row_label)
        .build();
    row.add_css_class("bluetooth-placeholder-row");
    row
}

fn signal_icon_name(strength: u8) -> &'static str {
    match strength {
        0 => "network-wireless-signal-none-symbolic",
        1..=25 => "network-wireless-signal-weak-symbolic",
        26..=50 => "network-wireless-signal-ok-symbolic",
        51..=75 => "network-wireless-signal-good-symbolic",
        _ => "network-wireless-signal-excellent-symbolic",
    }
}

fn build_label_row(label: &str) -> gtk::ListBoxRow {
    let row_label = gtk::Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .margin_bottom(12)
        .margin_end(12)
        .margin_start(12)
        .margin_top(12)
        .build();
    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .can_focus(true)
        .focusable(true)
        .selectable(false)
        .child(&row_label)
        .build();
    row.add_css_class("side-menu-row");
    row
}

fn install_volume_handlers(
    state: &Rc<SideMenuState>,
    volume_scale: &gtk::Scale,
    mute_button: &gtk::Button,
) {
    let state_for_scale = Rc::clone(state);
    volume_scale.connect_value_changed(move |scale| {
        if state_for_scale.updating_audio_widgets.get() {
            return;
        }

        state_for_scale.set_volume_from_ui(scale.value());
    });

    let state_for_button = Rc::clone(state);
    mute_button.connect_clicked(move |_| {
        state_for_button.toggle_mute();
    });
}

fn install_wifi_handlers(state: &Rc<SideMenuState>, widgets: &WifiWidgets) {
    let state_for_scan = Rc::clone(state);
    widgets.scan_button.connect_clicked(move |_| {
        state_for_scan.scan_wifi();
    });

    let state_for_row = Rc::clone(state);
    widgets.network_list.connect_row_activated(move |_, row| {
        let index = row.index();
        if index >= 0 {
            state_for_row.set_wifi_selection(index as usize);
            state_for_row.activate_wifi_selection();
        }
    });

    let state_for_connect = Rc::clone(state);
    widgets.connect_button.connect_clicked(move |_| {
        state_for_connect.connect_pending_wifi();
    });

    let state_for_entry = Rc::clone(state);
    widgets.password_entry.connect_activate(move |_| {
        state_for_entry.connect_pending_wifi();
    });

    let state_for_cancel = Rc::clone(state);
    widgets.cancel_button.connect_clicked(move |_| {
        state_for_cancel.close_wifi_panel();
    });
}

fn install_task_handlers(state: &Rc<SideMenuState>, widgets: &TaskWidgets) {
    let state_for_refresh = Rc::clone(state);
    widgets.refresh_button.connect_clicked(move |_| {
        state_for_refresh.refresh_tasks();
    });

    let state_for_row = Rc::clone(state);
    widgets.list.connect_row_activated(move |_, row| {
        let index = row.index();
        if index >= 0 {
            state_for_row.set_task_selection(index as usize);
            state_for_row.activate_task_selection();
        }
    });
}

fn install_bluetooth_handlers(state: &Rc<SideMenuState>, widgets: &BluetoothWidgets) {
    let state_for_row = Rc::clone(state);
    widgets.device_list.connect_row_activated(move |_, row| {
        let index = row.index();
        if index >= 0 {
            state_for_row.set_bluetooth_selection(index as usize);
            state_for_row.activate_bluetooth_selection();
        }
    });

    let state_for_reorder = Rc::clone(state);
    widgets.reorder_button.connect_clicked(move |_| {
        state_for_reorder.open_controller_panel();
    });

    let state_for_disconnect = Rc::clone(state);
    widgets.disconnect_button.connect_clicked(move |_| {
        let Some(device) = state_for_disconnect
            .bluetooth_popover_device
            .borrow()
            .clone()
        else {
            return;
        };
        state_for_disconnect.bluetooth_action_popover.popdown();
        if let Some(worker) = state_for_disconnect.bluetooth_worker.clone() {
            state_for_disconnect.set_bluetooth_busy(true);
            worker.disconnect(device.id, device.name);
        }
    });

    let state_for_remove = Rc::clone(state);
    widgets.remove_button.connect_clicked(move |_| {
        let Some(device) = state_for_remove.bluetooth_popover_device.borrow().clone() else {
            return;
        };
        state_for_remove.bluetooth_action_popover.popdown();
        if let Some(worker) = state_for_remove.bluetooth_worker.clone() {
            state_for_remove.set_bluetooth_busy(true);
            worker.remove(device.id, device.name);
        }
    });
}

fn install_audio_poll(state: &Rc<SideMenuState>) {
    let state = Rc::clone(state);
    glib::timeout_add_seconds_local(1, move || {
        state.refresh_audio_widgets();
        glib::ControlFlow::Continue
    });
}

fn install_audio_result_poll(state: &Rc<SideMenuState>, receiver: Receiver<AudioUiMessage>) {
    let state = Rc::clone(state);
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        while let Ok(message) = receiver.try_recv() {
            state.handle_audio_message(message);
        }

        glib::ControlFlow::Continue
    });
}

fn install_wifi_result_poll(state: &Rc<SideMenuState>, receiver: Receiver<WifiEvent>) {
    let state = Rc::clone(state);
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        while let Ok(event) = receiver.try_recv() {
            state.handle_wifi_event(event);
        }

        glib::ControlFlow::Continue
    });
}

fn install_bluetooth_result_poll(state: &Rc<SideMenuState>, receiver: Receiver<BluetoothEvent>) {
    let state = Rc::clone(state);
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        while let Ok(event) = receiver.try_recv() {
            state.handle_bluetooth_event(event);
        }

        glib::ControlFlow::Continue
    });
}

fn with_audio_controller<T>(
    audio: &SharedAudioController,
    run: impl FnOnce(&AudioController) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let controller = audio
        .lock()
        .map_err(|_| anyhow::anyhow!("audio controller lock poisoned"))?;
    run(&controller)
}

fn send_audio_result(sender: Sender<AudioUiMessage>, result: anyhow::Result<AudioSnapshot>) {
    let message = match result {
        Ok(snapshot) => AudioUiMessage::Snapshot(snapshot),
        Err(error) => AudioUiMessage::Error(error.to_string()),
    };
    let _ = sender.send(message);
}

fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };

    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "
        .side-menu {
            background: rgba(20, 20, 20, 0.92);
            color: white;
        }

        .side-menu-row {
            border-radius: 4px;
        }

        .side-menu-selected {
            background: rgba(114, 199, 216, 0.22);
            outline: 2px solid #72c7d8;
        }

        .side-menu-icon-button {
            min-height: 32px;
            min-width: 32px;
            padding: 4px;
        }

        .side-menu-action-button {
            min-height: 30px;
            padding: 4px 8px;
        }

        .side-menu-action-button.wifi-network-selected {
            background: rgba(255, 255, 255, 0.14);
            outline: 2px solid #72c7d8;
        }

        .wifi-section-header {
            margin-bottom: 2px;
        }

        .wifi-network-list {
            background: transparent;
        }

        .wifi-network-row,
        .wifi-placeholder-row {
            border-radius: 4px;
            padding: 6px;
        }

        .wifi-network-selected {
            background: rgba(255, 255, 255, 0.14);
        }

        .wifi-connected-badge {
            background: rgba(114, 199, 216, 0.24);
            border-radius: 4px;
            padding: 2px 5px;
        }

        .wifi-error {
            color: #ffb4a8;
        }

        .task-section-header {
            margin-bottom: 2px;
        }

        .task-list {
            background: transparent;
        }

        .task-row,
        .task-placeholder-row {
            border-radius: 4px;
            padding: 6px;
        }

        .task-row-selected {
            background: rgba(255, 255, 255, 0.14);
        }

        .task-subtitle {
            color: rgba(255, 255, 255, 0.68);
            font-size: 12px;
        }

        .task-active-badge {
            background: rgba(114, 199, 216, 0.24);
            border-radius: 4px;
            padding: 2px 5px;
        }

        .task-error {
            color: #ffb4a8;
        }

        .bluetooth-section-header,
        .controller-section-header {
            margin-bottom: 2px;
        }

        .bluetooth-device-list,
        .controller-list {
            background: transparent;
        }

        .bluetooth-device-row,
        .bluetooth-placeholder-row,
        .controller-row {
            border-radius: 4px;
            padding: 6px;
        }

        .bluetooth-device-selected,
        .controller-row-selected {
            background: rgba(255, 255, 255, 0.14);
            outline: 2px solid #72c7d8;
        }

        .bluetooth-connected-badge,
        .controller-player-badge {
            background: rgba(114, 199, 216, 0.24);
            border-radius: 4px;
            padding: 2px 5px;
        }

        .bluetooth-paired-badge {
            background: rgba(255, 255, 255, 0.12);
            border-radius: 4px;
            padding: 2px 5px;
        }

        .bluetooth-error {
            color: #ffb4a8;
        }

        .bluetooth-hint {
            color: rgba(255, 255, 255, 0.68);
            font-size: 12px;
        }

        .bluetooth-action-popover {
            background: rgba(20, 20, 20, 0.96);
            color: white;
        }
        ",
    );

    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

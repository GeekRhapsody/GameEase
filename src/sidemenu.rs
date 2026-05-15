use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::audio::{AudioController, AudioSnapshot, SharedAudioController};
use crate::gamepad::KeyboardDirection;

const SIDE_MENU_WIDTH: i32 = 280;
const VOLUME_STEP: f64 = 5.0;
const MAX_VOLUME: f64 = 100.0;

/// Action produced by activating a side menu row.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum SideMenuAction {
    /// The selected row is currently a placeholder.
    None,
    /// Quit GameEase.
    Quit,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SideMenuRowKind {
    Volume,
    Placeholder,
    Quit,
}

struct SideMenuState {
    rows: Vec<gtk::ListBoxRow>,
    row_kinds: Vec<SideMenuRowKind>,
    selected_index: Cell<usize>,
    audio: Option<SharedAudioController>,
    volume_scale: gtk::Scale,
    mute_image: gtk::Image,
    audio_result_sender: Sender<AudioUiMessage>,
    updating_audio_widgets: Cell<bool>,
    audio_snapshot_pending: Cell<bool>,
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
        match self.state.selected_kind() {
            SideMenuRowKind::Volume => {
                self.state.toggle_mute();
                SideMenuAction::None
            }
            SideMenuRowKind::Quit => SideMenuAction::Quit,
            SideMenuRowKind::Placeholder => SideMenuAction::None,
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

    let mut rows = Vec::new();
    let mut row_kinds = Vec::new();
    let volume_row = build_volume_row(&volume_scale, &mute_button);
    list.append(&volume_row);
    rows.push(volume_row);
    row_kinds.push(SideMenuRowKind::Volume);

    for (label, kind) in [
        ("Button Mapping", SideMenuRowKind::Placeholder),
        ("Profiles", SideMenuRowKind::Placeholder),
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

    let revealer = gtk::Revealer::builder()
        .halign(gtk::Align::Start)
        .hexpand(false)
        .transition_duration(200)
        .transition_type(gtk::RevealerTransitionType::SlideRight)
        .valign(gtk::Align::Fill)
        .vexpand(true)
        .width_request(SIDE_MENU_WIDTH)
        .build();
    revealer.set_child(Some(&panel));
    revealer.set_reveal_child(false);

    let (audio_result_sender, audio_result_receiver) = mpsc::channel();
    let state = Rc::new(SideMenuState {
        rows,
        row_kinds,
        selected_index: Cell::new(0),
        audio,
        volume_scale: volume_scale.clone(),
        mute_image: mute_image.clone(),
        audio_result_sender,
        updating_audio_widgets: Cell::new(false),
        audio_snapshot_pending: Cell::new(false),
    });
    state.rows[0].add_css_class("side-menu-selected");

    install_volume_handlers(&state, &volume_scale, &mute_button);
    state.refresh_audio_widgets();
    install_audio_result_poll(&state, audio_result_receiver);
    install_audio_poll(&state);

    SideMenu { revealer, state }
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
        ",
    );

    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

use std::cell::{Cell, RefCell};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

use evdev::Key;
use gtk::prelude::*;
use gtk4 as gtk;
use xkbcommon::xkb;

use crate::gamepad::KeyboardDirection;
use crate::uinput::SharedVirtualKeyboard;

const INITIAL_ROW: usize = 2;
const INITIAL_COLUMN: usize = 6;
const GAMEPAD_HINT_ICON_SIZE: i32 = 44;

const KEY_ROWS: &[&[KeySpec]] = &[
    &[
        KeySpec::tap("Esc", Key::KEY_ESC, 2),
        KeySpec::tap_shift("`", "~", Key::KEY_GRAVE, 1),
        KeySpec::tap_shift("1", "!", Key::KEY_1, 1),
        KeySpec::tap_shift("2", "@", Key::KEY_2, 1),
        KeySpec::tap_shift("3", "#", Key::KEY_3, 1),
        KeySpec::tap_shift("4", "$", Key::KEY_4, 1),
        KeySpec::tap_shift("5", "%", Key::KEY_5, 1),
        KeySpec::tap_shift("6", "^", Key::KEY_6, 1),
        KeySpec::tap_shift("7", "&", Key::KEY_7, 1),
        KeySpec::tap_shift("8", "*", Key::KEY_8, 1),
        KeySpec::tap_shift("9", "(", Key::KEY_9, 1),
        KeySpec::tap_shift("0", ")", Key::KEY_0, 1),
        KeySpec::tap("Back", Key::KEY_BACKSPACE, 2),
    ],
    &[
        KeySpec::tap("Tab", Key::KEY_TAB, 2),
        KeySpec::tap_letter("q", "Q", Key::KEY_Q, 1),
        KeySpec::tap_letter("w", "W", Key::KEY_W, 1),
        KeySpec::tap_letter("e", "E", Key::KEY_E, 1),
        KeySpec::tap_letter("r", "R", Key::KEY_R, 1),
        KeySpec::tap_letter("t", "T", Key::KEY_T, 1),
        KeySpec::tap_letter("y", "Y", Key::KEY_Y, 1),
        KeySpec::tap_letter("u", "U", Key::KEY_U, 1),
        KeySpec::tap_letter("i", "I", Key::KEY_I, 1),
        KeySpec::tap_letter("o", "O", Key::KEY_O, 1),
        KeySpec::tap_letter("p", "P", Key::KEY_P, 1),
        KeySpec::tap_shift("[", "{", Key::KEY_LEFTBRACE, 1),
        KeySpec::tap_shift("]", "}", Key::KEY_RIGHTBRACE, 1),
        KeySpec::tap_shift("\\", "|", Key::KEY_BACKSLASH, 1),
    ],
    &[
        KeySpec::caps_lock("Caps", 2),
        KeySpec::tap_letter("a", "A", Key::KEY_A, 1),
        KeySpec::tap_letter("s", "S", Key::KEY_S, 1),
        KeySpec::tap_letter("d", "D", Key::KEY_D, 1),
        KeySpec::tap_letter("f", "F", Key::KEY_F, 1),
        KeySpec::tap_letter("g", "G", Key::KEY_G, 1),
        KeySpec::tap_letter("h", "H", Key::KEY_H, 1),
        KeySpec::tap_letter("j", "J", Key::KEY_J, 1),
        KeySpec::tap_letter("k", "K", Key::KEY_K, 1),
        KeySpec::tap_letter("l", "L", Key::KEY_L, 1),
        KeySpec::tap_shift(";", ":", Key::KEY_SEMICOLON, 1),
        KeySpec::tap_shift("'", "\"", Key::KEY_APOSTROPHE, 1),
        KeySpec::tap("Enter", Key::KEY_ENTER, 2),
    ],
    &[
        KeySpec::toggle_icon("Shift", Key::KEY_LEFTSHIFT, 2, "assets/LT.png"),
        KeySpec::tap_letter("z", "Z", Key::KEY_Z, 1),
        KeySpec::tap_letter("x", "X", Key::KEY_X, 1),
        KeySpec::tap_letter("c", "C", Key::KEY_C, 1),
        KeySpec::tap_letter("v", "V", Key::KEY_V, 1),
        KeySpec::tap_letter("b", "B", Key::KEY_B, 1),
        KeySpec::tap_letter("n", "N", Key::KEY_N, 1),
        KeySpec::tap_letter("m", "M", Key::KEY_M, 1),
        KeySpec::tap_shift(",", "<", Key::KEY_COMMA, 1),
        KeySpec::tap_shift(".", ">", Key::KEY_DOT, 1),
        KeySpec::tap_shift("/", "?", Key::KEY_SLASH, 1),
        KeySpec::toggle_icon("Shift", Key::KEY_RIGHTSHIFT, 2, "assets/LT.png"),
    ],
    &[
        KeySpec::toggle("Ctrl", Key::KEY_LEFTCTRL, 1),
        KeySpec::toggle("Super", Key::KEY_LEFTMETA, 1),
        KeySpec::toggle("Alt", Key::KEY_LEFTALT, 1),
        KeySpec::tap_icon(
            "Space",
            Key::KEY_SPACE,
            6,
            "assets/Positional_Prompts_Up.png",
        ),
        KeySpec::tap("<", Key::KEY_LEFT, 1),
        KeySpec::tap("Up", Key::KEY_UP, 1),
        KeySpec::tap("Dwn", Key::KEY_DOWN, 1),
        KeySpec::tap(">", Key::KEY_RIGHT, 1),
    ],
];

#[derive(Clone, Copy)]
struct KeySpec {
    label: &'static str,
    shifted_label: &'static str,
    caps_label: &'static str,
    shift_caps_label: &'static str,
    icon: Option<&'static str>,
    action: KeyAction,
    width: i32,
}

#[derive(Clone, Copy)]
struct KeyChord {
    key: Key,
    shift: bool,
    alt_gr: bool,
}

impl KeyChord {
    const fn new(key: Key, shift: bool, alt_gr: bool) -> Self {
        Self { key, shift, alt_gr }
    }
}

impl KeySpec {
    const fn tap(label: &'static str, key: Key, width: i32) -> Self {
        Self {
            label,
            shifted_label: label,
            caps_label: label,
            shift_caps_label: label,
            icon: None,
            action: KeyAction::Tap(key),
            width,
        }
    }

    const fn tap_icon(label: &'static str, key: Key, width: i32, icon: &'static str) -> Self {
        Self {
            label,
            shifted_label: label,
            caps_label: label,
            shift_caps_label: label,
            icon: Some(icon),
            action: KeyAction::Tap(key),
            width,
        }
    }

    const fn tap_letter(
        label: &'static str,
        shifted_label: &'static str,
        key: Key,
        width: i32,
    ) -> Self {
        Self {
            label,
            shifted_label,
            caps_label: shifted_label,
            shift_caps_label: label,
            icon: None,
            action: KeyAction::Tap(key),
            width,
        }
    }

    const fn tap_shift(
        label: &'static str,
        shifted_label: &'static str,
        key: Key,
        width: i32,
    ) -> Self {
        Self {
            label,
            shifted_label,
            caps_label: label,
            shift_caps_label: shifted_label,
            icon: None,
            action: KeyAction::Tap(key),
            width,
        }
    }

    const fn toggle(label: &'static str, key: Key, width: i32) -> Self {
        Self {
            label,
            shifted_label: label,
            caps_label: label,
            shift_caps_label: label,
            icon: None,
            action: KeyAction::Toggle(key),
            width,
        }
    }

    const fn toggle_icon(label: &'static str, key: Key, width: i32, icon: &'static str) -> Self {
        Self {
            label,
            shifted_label: label,
            caps_label: label,
            shift_caps_label: label,
            icon: Some(icon),
            action: KeyAction::Toggle(key),
            width,
        }
    }

    const fn caps_lock(label: &'static str, width: i32) -> Self {
        Self {
            label,
            shifted_label: label,
            caps_label: label,
            shift_caps_label: label,
            icon: Some("assets/Left_Stick_Click.png"),
            action: KeyAction::CapsLock,
            width,
        }
    }

    fn is_character(self) -> bool {
        matches!(self.action, KeyAction::Tap(_))
            && !matches!(
                self.action,
                KeyAction::Tap(Key::KEY_LEFT | Key::KEY_RIGHT | Key::KEY_UP | Key::KEY_DOWN)
            )
            && self.label.chars().count() == 1
    }

    fn is_letter(self) -> bool {
        matches!(self.action, KeyAction::Tap(_))
            && self.label.chars().count() == 1
            && self
                .label
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_lowercase())
            && self.caps_label == self.shifted_label
            && self.shift_caps_label == self.label
    }
}

#[derive(Clone, Copy)]
enum KeyAction {
    Tap(Key),
    Toggle(Key),
    CapsLock,
}

struct KeyboardKeymap {
    keymap: Option<xkb::Keymap>,
}

impl KeyboardKeymap {
    fn detect() -> Self {
        let names = XkbNames::detect();
        let keymap = build_keymap(&names).or_else(|| {
            eprintln!(
                "Failed to compile detected XKB keymap; falling back to libxkbcommon defaults"
            );
            build_keymap(&XkbNames::default())
        });

        Self { keymap }
    }

    fn resolve(&self, target: &str) -> Option<KeyChord> {
        let keymap = self.keymap.as_ref()?;

        for key in PRINTABLE_KEYS {
            for (shift, alt_gr) in [(false, false), (true, false), (false, true), (true, true)] {
                if key_outputs(keymap, *key, shift, alt_gr, target) {
                    return Some(KeyChord::new(*key, shift, alt_gr));
                }
            }
        }

        None
    }
}

#[derive(Default)]
struct XkbNames {
    model: String,
    layout: String,
    variant: String,
    options: Option<String>,
}

impl XkbNames {
    fn detect() -> Self {
        let mut names = Self::default();

        names.model = env::var("XKB_DEFAULT_MODEL")
            .ok()
            .or_else(|| localectl_value("X11 Model"))
            .unwrap_or_default();
        names.layout = env::var("GAMEEASE_KEYBOARD_LAYOUT")
            .ok()
            .or_else(|| env::var("XKB_DEFAULT_LAYOUT").ok())
            .or_else(kde_layout_value)
            .or_else(|| localectl_value("X11 Layout"))
            .unwrap_or_default();
        names.variant = env::var("XKB_DEFAULT_VARIANT")
            .ok()
            .or_else(kde_variant_value)
            .or_else(|| localectl_value("X11 Variant"))
            .unwrap_or_default();
        names.options = env::var("XKB_DEFAULT_OPTIONS")
            .ok()
            .or_else(kde_options_value)
            .or_else(|| localectl_value("X11 Options"));

        names
    }
}

const PRINTABLE_KEYS: &[Key] = &[
    Key::KEY_GRAVE,
    Key::KEY_1,
    Key::KEY_2,
    Key::KEY_3,
    Key::KEY_4,
    Key::KEY_5,
    Key::KEY_6,
    Key::KEY_7,
    Key::KEY_8,
    Key::KEY_9,
    Key::KEY_0,
    Key::KEY_MINUS,
    Key::KEY_EQUAL,
    Key::KEY_Q,
    Key::KEY_W,
    Key::KEY_E,
    Key::KEY_R,
    Key::KEY_T,
    Key::KEY_Y,
    Key::KEY_U,
    Key::KEY_I,
    Key::KEY_O,
    Key::KEY_P,
    Key::KEY_LEFTBRACE,
    Key::KEY_RIGHTBRACE,
    Key::KEY_A,
    Key::KEY_S,
    Key::KEY_D,
    Key::KEY_F,
    Key::KEY_G,
    Key::KEY_H,
    Key::KEY_J,
    Key::KEY_K,
    Key::KEY_L,
    Key::KEY_SEMICOLON,
    Key::KEY_APOSTROPHE,
    Key::KEY_BACKSLASH,
    Key::KEY_102ND,
    Key::KEY_Z,
    Key::KEY_X,
    Key::KEY_C,
    Key::KEY_V,
    Key::KEY_B,
    Key::KEY_N,
    Key::KEY_M,
    Key::KEY_COMMA,
    Key::KEY_DOT,
    Key::KEY_SLASH,
];

fn build_keymap(names: &XkbNames) -> Option<xkb::Keymap> {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);

    xkb::Keymap::new_from_names(
        &context,
        "",
        names.model.as_str(),
        names.layout.as_str(),
        names.variant.as_str(),
        names.options.clone(),
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
}

fn key_outputs(keymap: &xkb::Keymap, key: Key, shift: bool, alt_gr: bool, target: &str) -> bool {
    let mut state = xkb::State::new(keymap);

    if shift {
        state.update_key(xkb_keycode(Key::KEY_LEFTSHIFT), xkb::KeyDirection::Down);
    }
    if alt_gr {
        state.update_key(xkb_keycode(Key::KEY_RIGHTALT), xkb::KeyDirection::Down);
    }

    state.key_get_utf8(xkb_keycode(key)) == target
}

fn xkb_keycode(key: Key) -> xkb::Keycode {
    xkb::Keycode::new(u32::from(key.code()) + 8)
}

fn kde_layout_value() -> Option<String> {
    kde_config_value("LayoutList")
}

fn kde_variant_value() -> Option<String> {
    kde_config_value("VariantList")
}

fn kde_options_value() -> Option<String> {
    kde_config_value("Options")
}

fn kde_config_value(key: &str) -> Option<String> {
    let home = env::var_os("HOME")?;
    let path = PathBuf::from(home).join(".config/kxkbrc");
    let contents = fs::read_to_string(path).ok()?;

    contents.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim().to_string())
    })
}

fn localectl_value(key: &str) -> Option<String> {
    let output = Command::new("localectl").arg("status").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let status = String::from_utf8_lossy(&output.stdout);
    status.lines().find_map(|line| {
        let (name, value) = line.trim().split_once(':')?;
        (name.trim() == key).then(|| value.trim().to_string())
    })
}

#[derive(Clone)]
struct KeyCell {
    button: gtk::Button,
    width: i32,
}

struct KeyboardState {
    rows: Vec<Vec<KeyCell>>,
    selected_row: Cell<usize>,
    selected_column: Cell<usize>,
    shift_state: Rc<ShiftState>,
}

/// On-screen keyboard widget and gamepad selection controller.
#[derive(Clone)]
pub struct OnScreenKeyboard {
    widget: gtk::Grid,
    state: Rc<KeyboardState>,
}

impl OnScreenKeyboard {
    /// Returns the GTK widget used for layout and input-region calculations.
    pub fn widget(&self) -> &gtk::Grid {
        &self.widget
    }

    /// Moves the selected key in the requested direction.
    pub fn move_selection(&self, direction: KeyboardDirection) {
        let row = self.state.selected_row.get();
        let column = self.state.selected_column.get();

        let (next_row, next_column) = match direction {
            KeyboardDirection::Left => (row, column.saturating_sub(1)),
            KeyboardDirection::Right => {
                let last_column = self.state.rows[row].len().saturating_sub(1);
                (row, (column + 1).min(last_column))
            }
            KeyboardDirection::Up => self.state.closest_key_in_row(row.saturating_sub(1)),
            KeyboardDirection::Down => {
                let last_row = self.state.rows.len().saturating_sub(1);
                self.state.closest_key_in_row((row + 1).min(last_row))
            }
        };

        self.state.select(next_row, next_column);
    }

    /// Activates the currently selected key.
    pub fn activate_selected(&self) {
        self.state.selected_button().emit_clicked();
    }

    /// Sets whether a physical gamepad trigger is holding Shift.
    pub fn set_shift_held(&self, active: bool) {
        if let Err(error) = self.state.shift_state.set_held(active) {
            eprintln!("Failed to update held OSK shift state: {error:#}");
        }
    }

    /// Toggles Caps Lock from the gamepad.
    pub fn toggle_caps_lock(&self) {
        if let Err(error) = self.state.shift_state.toggle_caps_lock() {
            eprintln!("Failed to toggle OSK caps lock state: {error:#}");
        }
    }

    /// Sends a Space key tap without changing the selected OSK key.
    pub fn activate_space(&self) {
        if let Err(error) = self.state.shift_state.tap_space() {
            eprintln!("Failed to tap OSK space key: {error:#}");
        }
    }
}

impl KeyboardState {
    fn select(&self, row: usize, column: usize) {
        self.selected_button().remove_css_class("osk-selected");
        self.selected_row.set(row);
        self.selected_column.set(column);
        self.selected_button().add_css_class("osk-selected");
    }

    fn selected_button(&self) -> gtk::Button {
        self.rows[self.selected_row.get()][self.selected_column.get()]
            .button
            .clone()
    }

    fn closest_key_in_row(&self, target_row: usize) -> (usize, usize) {
        let current_center = self.key_center(self.selected_row.get(), self.selected_column.get());
        let mut best_column = 0;
        let mut best_distance = i32::MAX;

        for column in 0..self.rows[target_row].len() {
            let distance = (self.key_center(target_row, column) - current_center).abs();
            if distance < best_distance {
                best_column = column;
                best_distance = distance;
            }
        }

        (target_row, best_column)
    }

    fn key_center(&self, row: usize, column: usize) -> i32 {
        let mut left = 0;

        for key in self.rows[row].iter().take(column) {
            left += key.width;
        }

        left + self.rows[row][column].width / 2
    }
}

struct KeyLabel {
    label_widget: gtk::Label,
    label: &'static str,
    shifted_label: &'static str,
    caps_label: &'static str,
    shift_caps_label: &'static str,
}

struct ShiftState {
    virtual_keyboard: SharedVirtualKeyboard,
    keymap: KeyboardKeymap,
    toggle_active: Cell<bool>,
    held_active: Cell<bool>,
    caps_active: Cell<bool>,
    injected_down: Cell<bool>,
    buttons: RefCell<Vec<gtk::Button>>,
    caps_buttons: RefCell<Vec<gtk::Button>>,
    labels: RefCell<Vec<KeyLabel>>,
}

impl ShiftState {
    fn new(virtual_keyboard: SharedVirtualKeyboard) -> Self {
        Self {
            virtual_keyboard,
            keymap: KeyboardKeymap::detect(),
            toggle_active: Cell::new(false),
            held_active: Cell::new(false),
            caps_active: Cell::new(false),
            injected_down: Cell::new(false),
            buttons: RefCell::new(Vec::new()),
            caps_buttons: RefCell::new(Vec::new()),
            labels: RefCell::new(Vec::new()),
        }
    }

    fn add_button(&self, button: &gtk::Button) {
        self.buttons.borrow_mut().push(button.clone());
    }

    fn add_caps_button(&self, button: &gtk::Button) {
        self.caps_buttons.borrow_mut().push(button.clone());
    }

    fn add_label(
        &self,
        label_widget: &gtk::Label,
        label: &'static str,
        shifted_label: &'static str,
        caps_label: &'static str,
        shift_caps_label: &'static str,
    ) {
        self.labels.borrow_mut().push(KeyLabel {
            label_widget: label_widget.clone(),
            label,
            shifted_label,
            caps_label,
            shift_caps_label,
        });
    }

    fn toggle(&self) -> anyhow::Result<()> {
        self.toggle_active.set(!self.toggle_active.get());
        self.sync()
    }

    fn set_held(&self, active: bool) -> anyhow::Result<()> {
        if self.held_active.get() == active {
            return Ok(());
        }

        self.held_active.set(active);
        self.sync()
    }

    fn is_active(&self) -> bool {
        self.toggle_active.get() || self.held_active.get()
    }

    fn toggle_caps_lock(&self) -> anyhow::Result<()> {
        tap_key(&self.virtual_keyboard, Key::KEY_CAPSLOCK)?;
        self.caps_active.set(!self.caps_active.get());
        self.update_visuals();

        Ok(())
    }

    fn tap_key_for_spec(&self, spec: KeySpec) -> anyhow::Result<()> {
        let KeyAction::Tap(fallback_key) = spec.action else {
            return Ok(());
        };

        let Some(target) = self.target_character(spec) else {
            return tap_key(&self.virtual_keyboard, fallback_key);
        };

        let Some(chord) = self.keymap.resolve(target) else {
            return tap_key(&self.virtual_keyboard, fallback_key);
        };

        if spec.is_letter() {
            tap_key(&self.virtual_keyboard, chord.key)
        } else {
            self.tap_chord(chord)
        }
    }

    fn target_character(&self, spec: KeySpec) -> Option<&'static str> {
        if !spec.is_character() {
            return None;
        }

        if spec.is_letter() {
            Some(spec.label)
        } else if self.is_active() && self.caps_active.get() {
            Some(spec.shift_caps_label)
        } else if self.is_active() {
            Some(spec.shifted_label)
        } else if self.caps_active.get() {
            Some(spec.caps_label)
        } else {
            Some(spec.label)
        }
    }

    fn tap_chord(&self, chord: KeyChord) -> anyhow::Result<()> {
        let was_shift_down = self.injected_down.get();

        self.set_temporary_modifier(Key::KEY_LEFTSHIFT, was_shift_down, chord.shift)?;
        self.set_temporary_modifier(Key::KEY_RIGHTALT, false, chord.alt_gr)?;

        let tap_result = tap_key(&self.virtual_keyboard, chord.key);
        let restore_alt_gr = self.set_temporary_modifier(Key::KEY_RIGHTALT, chord.alt_gr, false);
        let restore_shift =
            self.set_temporary_modifier(Key::KEY_LEFTSHIFT, chord.shift, was_shift_down);

        tap_result.and(restore_alt_gr).and(restore_shift)
    }

    fn set_temporary_modifier(
        &self,
        key: Key,
        current_active: bool,
        next_active: bool,
    ) -> anyhow::Result<()> {
        if current_active == next_active {
            return Ok(());
        }

        if next_active {
            press_key(&self.virtual_keyboard, key)
        } else {
            release_key(&self.virtual_keyboard, key)
        }
    }

    fn tap_space(&self) -> anyhow::Result<()> {
        tap_key(&self.virtual_keyboard, Key::KEY_SPACE)
    }

    fn sync(&self) -> anyhow::Result<()> {
        let active = self.is_active();
        let result = if active != self.injected_down.get() {
            if active {
                press_key(&self.virtual_keyboard, Key::KEY_LEFTSHIFT)
            } else {
                release_key(&self.virtual_keyboard, Key::KEY_LEFTSHIFT)
            }
        } else {
            Ok(())
        };

        if result.is_ok() {
            self.injected_down.set(active);
        }
        self.update_visuals();

        result
    }

    fn update_visuals(&self) {
        let shift_active = self.is_active();
        let caps_active = self.caps_active.get();

        for button in self.buttons.borrow().iter() {
            if shift_active {
                button.add_css_class("suggested-action");
            } else {
                button.remove_css_class("suggested-action");
            }
        }

        for button in self.caps_buttons.borrow().iter() {
            if caps_active {
                button.add_css_class("suggested-action");
            } else {
                button.remove_css_class("suggested-action");
            }
        }

        for label in self.labels.borrow().iter() {
            label
                .label_widget
                .set_label(if shift_active && caps_active {
                    label.shift_caps_label
                } else if shift_active {
                    label.shifted_label
                } else if caps_active {
                    label.caps_label
                } else {
                    label.label
                });
        }
    }
}

/// Builds the QWERTY on-screen keyboard widget.
pub fn build_keyboard(virtual_keyboard: SharedVirtualKeyboard) -> OnScreenKeyboard {
    install_css();

    let grid = gtk::Grid::builder()
        .column_homogeneous(true)
        .column_spacing(6)
        .row_spacing(6)
        .build();
    grid.add_css_class("osk-panel");

    let shift_state = Rc::new(ShiftState::new(virtual_keyboard.clone()));
    let ctrl_active = Rc::new(Cell::new(false));
    let meta_active = Rc::new(Cell::new(false));
    let alt_active = Rc::new(Cell::new(false));
    let mut rows = Vec::new();

    for (row_index, row) in KEY_ROWS.iter().enumerate() {
        let mut column_index = 0;
        let mut cells = Vec::new();

        for spec in row.iter() {
            let button = gtk::Button::builder()
                .width_request(spec.width * 58)
                .height_request(58)
                .can_focus(false)
                .focus_on_click(false)
                .focusable(false)
                .build();
            button.add_css_class("osk-key");

            let label = gtk::Label::new(Some(spec.label));
            label.set_xalign(0.5);
            button.set_child(Some(&build_key_content(spec, &label)));

            if spec.label != spec.shifted_label || spec.label != spec.caps_label {
                shift_state.add_label(
                    &label,
                    spec.label,
                    spec.shifted_label,
                    spec.caps_label,
                    spec.shift_caps_label,
                );
            }

            if is_shift_key(spec.action) {
                shift_state.add_button(&button);
            }

            if is_caps_lock_key(spec.action) {
                shift_state.add_caps_button(&button);
            }

            connect_button(
                &button,
                virtual_keyboard.clone(),
                *spec,
                shift_state.clone(),
                modifier_state_for_action(spec.action, &ctrl_active, &meta_active, &alt_active),
            );

            grid.attach(&button, column_index, row_index as i32, spec.width, 1);
            cells.push(KeyCell {
                button,
                width: spec.width,
            });
            column_index += spec.width;
        }

        rows.push(cells);
    }

    let state = Rc::new(KeyboardState {
        rows,
        selected_row: Cell::new(INITIAL_ROW),
        selected_column: Cell::new(INITIAL_COLUMN),
        shift_state,
    });
    state.selected_button().add_css_class("osk-selected");

    OnScreenKeyboard {
        widget: grid,
        state,
    }
}

fn modifier_state_for_action(
    action: KeyAction,
    ctrl_active: &Rc<Cell<bool>>,
    meta_active: &Rc<Cell<bool>>,
    alt_active: &Rc<Cell<bool>>,
) -> Rc<Cell<bool>> {
    match action {
        KeyAction::Toggle(Key::KEY_LEFTCTRL) => ctrl_active.clone(),
        KeyAction::Toggle(Key::KEY_LEFTMETA) => meta_active.clone(),
        KeyAction::Toggle(Key::KEY_LEFTALT) => alt_active.clone(),
        _ => Rc::new(Cell::new(false)),
    }
}

fn build_key_content(spec: &KeySpec, label: &gtk::Label) -> gtk::Box {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();

    if let Some(icon) = spec.icon {
        let image = gtk::Image::from_file(asset_path(icon));
        image.set_pixel_size(GAMEPAD_HINT_ICON_SIZE);
        image.add_css_class("osk-key-icon");
        content.append(&image);
    }

    content.append(label);
    content
}

fn asset_path(relative_path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative_path)
}

fn connect_button(
    button: &gtk::Button,
    virtual_keyboard: SharedVirtualKeyboard,
    spec: KeySpec,
    shift_state: Rc<ShiftState>,
    modifier_active: Rc<Cell<bool>>,
) {
    let action = spec.action;

    button.connect_clicked(move |button| match action {
        KeyAction::Tap(_) => {
            if let Err(error) = shift_state.tap_key_for_spec(spec) {
                eprintln!("Failed to tap OSK key: {error:#}");
            }
        }
        KeyAction::Toggle(_) if is_shift_key(action) => {
            if let Err(error) = shift_state.toggle() {
                eprintln!("Failed to toggle OSK shift: {error:#}");
            }
        }
        KeyAction::CapsLock => {
            if let Err(error) = shift_state.toggle_caps_lock() {
                eprintln!("Failed to toggle OSK caps lock: {error:#}");
            }
        }
        KeyAction::Toggle(key) => {
            let next_active = !modifier_active.get();
            let result = if next_active {
                press_key(&virtual_keyboard, key)
            } else {
                release_key(&virtual_keyboard, key)
            };

            match result {
                Ok(()) => {
                    modifier_active.set(next_active);
                    if next_active {
                        button.add_css_class("suggested-action");
                    } else {
                        button.remove_css_class("suggested-action");
                    }
                }
                Err(error) => eprintln!("Failed to toggle OSK modifier: {error:#}"),
            }
        }
    });
}

fn is_shift_key(action: KeyAction) -> bool {
    matches!(
        action,
        KeyAction::Toggle(Key::KEY_LEFTSHIFT | Key::KEY_RIGHTSHIFT)
    )
}

fn is_caps_lock_key(action: KeyAction) -> bool {
    matches!(action, KeyAction::CapsLock)
}

fn tap_key(virtual_keyboard: &SharedVirtualKeyboard, key: Key) -> anyhow::Result<()> {
    let mut virtual_keyboard = virtual_keyboard
        .lock()
        .map_err(|error| anyhow::anyhow!("virtual keyboard lock poisoned: {error}"))?;
    virtual_keyboard.tap(key)
}

fn press_key(virtual_keyboard: &SharedVirtualKeyboard, key: Key) -> anyhow::Result<()> {
    let mut virtual_keyboard = virtual_keyboard
        .lock()
        .map_err(|error| anyhow::anyhow!("virtual keyboard lock poisoned: {error}"))?;
    virtual_keyboard.press(key)
}

fn release_key(virtual_keyboard: &SharedVirtualKeyboard, key: Key) -> anyhow::Result<()> {
    let mut virtual_keyboard = virtual_keyboard
        .lock()
        .map_err(|error| anyhow::anyhow!("virtual keyboard lock poisoned: {error}"))?;
    virtual_keyboard.release(key)
}

fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };

    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "
        .osk-panel {
            background: #242424;
            border-radius: 6px;
            padding: 5px;
        }

        .osk-key {
            background: #333333;
            color: #f0f0f0;
            border-radius: 4px;
            font-size: 18px;
            padding: 4px;
        }

        .osk-selected {
            border: 3px solid #72c7d8;
        }
        ",
    );

    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

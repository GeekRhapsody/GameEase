use std::cell::{Cell, RefCell};
use std::rc::Rc;

use evdev::Key;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::gamepad::KeyboardDirection;
use crate::uinput::SharedVirtualKeyboard;

const INITIAL_ROW: usize = 2;
const INITIAL_COLUMN: usize = 5;

const KEY_ROWS: &[&[KeySpec]] = &[
    &[
        KeySpec::tap("Esc", Key::KEY_ESC, 2),
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
        KeySpec::tap_shift("q", "Q", Key::KEY_Q, 1),
        KeySpec::tap_shift("w", "W", Key::KEY_W, 1),
        KeySpec::tap_shift("e", "E", Key::KEY_E, 1),
        KeySpec::tap_shift("r", "R", Key::KEY_R, 1),
        KeySpec::tap_shift("t", "T", Key::KEY_T, 1),
        KeySpec::tap_shift("y", "Y", Key::KEY_Y, 1),
        KeySpec::tap_shift("u", "U", Key::KEY_U, 1),
        KeySpec::tap_shift("i", "I", Key::KEY_I, 1),
        KeySpec::tap_shift("o", "O", Key::KEY_O, 1),
        KeySpec::tap_shift("p", "P", Key::KEY_P, 1),
        KeySpec::tap("Enter", Key::KEY_ENTER, 2),
    ],
    &[
        KeySpec::tap_shift("a", "A", Key::KEY_A, 1),
        KeySpec::tap_shift("s", "S", Key::KEY_S, 1),
        KeySpec::tap_shift("d", "D", Key::KEY_D, 1),
        KeySpec::tap_shift("f", "F", Key::KEY_F, 1),
        KeySpec::tap_shift("g", "G", Key::KEY_G, 1),
        KeySpec::tap_shift("h", "H", Key::KEY_H, 1),
        KeySpec::tap_shift("j", "J", Key::KEY_J, 1),
        KeySpec::tap_shift("k", "K", Key::KEY_K, 1),
        KeySpec::tap_shift("l", "L", Key::KEY_L, 1),
        KeySpec::tap_shift(";", ":", Key::KEY_SEMICOLON, 1),
        KeySpec::tap_shift("'", "\"", Key::KEY_APOSTROPHE, 1),
        KeySpec::tap("Enter", Key::KEY_ENTER, 2),
    ],
    &[
        KeySpec::toggle("Shift", Key::KEY_LEFTSHIFT, 2),
        KeySpec::tap_shift("z", "Z", Key::KEY_Z, 1),
        KeySpec::tap_shift("x", "X", Key::KEY_X, 1),
        KeySpec::tap_shift("c", "C", Key::KEY_C, 1),
        KeySpec::tap_shift("v", "V", Key::KEY_V, 1),
        KeySpec::tap_shift("b", "B", Key::KEY_B, 1),
        KeySpec::tap_shift("n", "N", Key::KEY_N, 1),
        KeySpec::tap_shift("m", "M", Key::KEY_M, 1),
        KeySpec::tap_shift(",", "<", Key::KEY_COMMA, 1),
        KeySpec::tap_shift(".", ">", Key::KEY_DOT, 1),
        KeySpec::tap_shift("/", "?", Key::KEY_SLASH, 1),
        KeySpec::toggle("Shift", Key::KEY_RIGHTSHIFT, 2),
    ],
    &[
        KeySpec::noop("&123", 2),
        KeySpec::toggle("Ctrl", Key::KEY_LEFTCTRL, 1),
        KeySpec::toggle("Super", Key::KEY_LEFTMETA, 1),
        KeySpec::toggle("Alt", Key::KEY_LEFTALT, 1),
        KeySpec::tap("Space", Key::KEY_SPACE, 6),
        KeySpec::tap("<", Key::KEY_LEFT, 1),
        KeySpec::tap(">", Key::KEY_RIGHT, 1),
    ],
];

#[derive(Clone, Copy)]
struct KeySpec {
    label: &'static str,
    shifted_label: &'static str,
    action: KeyAction,
    width: i32,
}

impl KeySpec {
    const fn tap(label: &'static str, key: Key, width: i32) -> Self {
        Self {
            label,
            shifted_label: label,
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
            action: KeyAction::Tap(key),
            width,
        }
    }

    const fn toggle(label: &'static str, key: Key, width: i32) -> Self {
        Self {
            label,
            shifted_label: label,
            action: KeyAction::Toggle(key),
            width,
        }
    }

    const fn noop(label: &'static str, width: i32) -> Self {
        Self {
            label,
            shifted_label: label,
            action: KeyAction::Noop,
            width,
        }
    }
}

#[derive(Clone, Copy)]
enum KeyAction {
    Tap(Key),
    Toggle(Key),
    Noop,
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
    button: gtk::Button,
    label: &'static str,
    shifted_label: &'static str,
}

struct ShiftState {
    virtual_keyboard: SharedVirtualKeyboard,
    toggle_active: Cell<bool>,
    held_active: Cell<bool>,
    injected_down: Cell<bool>,
    buttons: RefCell<Vec<gtk::Button>>,
    labels: RefCell<Vec<KeyLabel>>,
}

impl ShiftState {
    fn new(virtual_keyboard: SharedVirtualKeyboard) -> Self {
        Self {
            virtual_keyboard,
            toggle_active: Cell::new(false),
            held_active: Cell::new(false),
            injected_down: Cell::new(false),
            buttons: RefCell::new(Vec::new()),
            labels: RefCell::new(Vec::new()),
        }
    }

    fn add_button(&self, button: &gtk::Button) {
        self.buttons.borrow_mut().push(button.clone());
    }

    fn add_label(&self, button: &gtk::Button, label: &'static str, shifted_label: &'static str) {
        self.labels.borrow_mut().push(KeyLabel {
            button: button.clone(),
            label,
            shifted_label,
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
        let active = self.is_active();

        for button in self.buttons.borrow().iter() {
            if active {
                button.add_css_class("suggested-action");
            } else {
                button.remove_css_class("suggested-action");
            }
        }

        for label in self.labels.borrow().iter() {
            label.button.set_label(if active {
                label.shifted_label
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
                .label(spec.label)
                .width_request(spec.width * 58)
                .height_request(58)
                .can_focus(false)
                .focus_on_click(false)
                .focusable(false)
                .build();
            button.add_css_class("osk-key");

            if spec.label != spec.shifted_label {
                shift_state.add_label(&button, spec.label, spec.shifted_label);
            }

            if is_shift_key(spec.action) {
                shift_state.add_button(&button);
            }

            connect_button(
                &button,
                virtual_keyboard.clone(),
                spec.action,
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

fn connect_button(
    button: &gtk::Button,
    virtual_keyboard: SharedVirtualKeyboard,
    action: KeyAction,
    shift_state: Rc<ShiftState>,
    modifier_active: Rc<Cell<bool>>,
) {
    button.connect_clicked(move |button| match action {
        KeyAction::Tap(key) => {
            if let Err(error) = tap_key(&virtual_keyboard, key) {
                eprintln!("Failed to tap OSK key: {error:#}");
            }
        }
        KeyAction::Toggle(_) if is_shift_key(action) => {
            if let Err(error) = shift_state.toggle() {
                eprintln!("Failed to toggle OSK shift: {error:#}");
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
        KeyAction::Noop => {}
    });
}

fn is_shift_key(action: KeyAction) -> bool {
    matches!(
        action,
        KeyAction::Toggle(Key::KEY_LEFTSHIFT | Key::KEY_RIGHTSHIFT)
    )
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

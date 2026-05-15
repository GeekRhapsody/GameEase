use std::cell::Cell;
use std::rc::Rc;

use evdev::Key;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::gamepad::KeyboardDirection;
use crate::uinput::SharedVirtualKeyboard;

const INITIAL_ROW: usize = 1;
const INITIAL_COLUMN: usize = 6;

const KEY_ROWS: &[&[KeySpec]] = &[
    &[
        KeySpec::tap("Esc", Key::KEY_ESC, 2),
        KeySpec::tap("1\nq", Key::KEY_Q, 1),
        KeySpec::tap("2\nw", Key::KEY_W, 1),
        KeySpec::tap("3\ne", Key::KEY_E, 1),
        KeySpec::tap("4\nr", Key::KEY_R, 1),
        KeySpec::tap("5\nt", Key::KEY_T, 1),
        KeySpec::tap("6\ny", Key::KEY_Y, 1),
        KeySpec::tap("7\nu", Key::KEY_U, 1),
        KeySpec::tap("8\ni", Key::KEY_I, 1),
        KeySpec::tap("9\no", Key::KEY_O, 1),
        KeySpec::tap("0\np", Key::KEY_P, 1),
        KeySpec::tap("Back", Key::KEY_BACKSPACE, 2),
    ],
    &[
        KeySpec::tap("Tab", Key::KEY_TAB, 2),
        KeySpec::tap("a", Key::KEY_A, 1),
        KeySpec::tap("s", Key::KEY_S, 1),
        KeySpec::tap("d", Key::KEY_D, 1),
        KeySpec::tap("f", Key::KEY_F, 1),
        KeySpec::tap("g", Key::KEY_G, 1),
        KeySpec::tap("h", Key::KEY_H, 1),
        KeySpec::tap("j", Key::KEY_J, 1),
        KeySpec::tap("k", Key::KEY_K, 1),
        KeySpec::tap("l", Key::KEY_L, 1),
        KeySpec::tap("'\n;", Key::KEY_SEMICOLON, 1),
        KeySpec::tap("Enter", Key::KEY_ENTER, 2),
    ],
    &[
        KeySpec::toggle("Shift", Key::KEY_LEFTSHIFT, 2),
        KeySpec::tap("z", Key::KEY_Z, 1),
        KeySpec::tap("x", Key::KEY_X, 1),
        KeySpec::tap("c", Key::KEY_C, 1),
        KeySpec::tap("v", Key::KEY_V, 1),
        KeySpec::tap("b", Key::KEY_B, 1),
        KeySpec::tap("n", Key::KEY_N, 1),
        KeySpec::tap("m", Key::KEY_M, 1),
        KeySpec::tap(";\n,", Key::KEY_COMMA, 1),
        KeySpec::tap(":\n.", Key::KEY_DOT, 1),
        KeySpec::tap("!\n?", Key::KEY_SLASH, 1),
        KeySpec::toggle("Shift", Key::KEY_RIGHTSHIFT, 2),
    ],
    &[
        KeySpec::noop("&123", 2),
        KeySpec::toggle("Ctrl", Key::KEY_LEFTCTRL, 1),
        KeySpec::toggle("Win", Key::KEY_LEFTMETA, 1),
        KeySpec::toggle("Alt", Key::KEY_LEFTALT, 1),
        KeySpec::tap("Space", Key::KEY_SPACE, 6),
        KeySpec::tap("Mic", Key::KEY_MICMUTE, 1),
        KeySpec::tap("<", Key::KEY_LEFT, 1),
        KeySpec::tap(">", Key::KEY_RIGHT, 1),
    ],
];

#[derive(Clone, Copy)]
struct KeySpec {
    label: &'static str,
    action: KeyAction,
    width: i32,
}

impl KeySpec {
    const fn tap(label: &'static str, key: Key, width: i32) -> Self {
        Self {
            label,
            action: KeyAction::Tap(key),
            width,
        }
    }

    const fn toggle(label: &'static str, key: Key, width: i32) -> Self {
        Self {
            label,
            action: KeyAction::Toggle(key),
            width,
        }
    }

    const fn noop(label: &'static str, width: i32) -> Self {
        Self {
            label,
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

/// Builds the QWERTY on-screen keyboard widget.
pub fn build_keyboard(virtual_keyboard: SharedVirtualKeyboard) -> OnScreenKeyboard {
    install_css();

    let grid = gtk::Grid::builder()
        .column_homogeneous(true)
        .column_spacing(6)
        .row_spacing(6)
        .build();
    grid.add_css_class("osk-panel");

    let left_shift_active = Rc::new(Cell::new(false));
    let right_shift_active = Rc::new(Cell::new(false));
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

            connect_button(
                &button,
                virtual_keyboard.clone(),
                spec.action,
                modifier_state_for_action(
                    spec.action,
                    &left_shift_active,
                    &right_shift_active,
                    &ctrl_active,
                    &meta_active,
                    &alt_active,
                ),
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
    });
    state.selected_button().add_css_class("osk-selected");

    OnScreenKeyboard {
        widget: grid,
        state,
    }
}

fn modifier_state_for_action(
    action: KeyAction,
    left_shift_active: &Rc<Cell<bool>>,
    right_shift_active: &Rc<Cell<bool>>,
    ctrl_active: &Rc<Cell<bool>>,
    meta_active: &Rc<Cell<bool>>,
    alt_active: &Rc<Cell<bool>>,
) -> Rc<Cell<bool>> {
    match action {
        KeyAction::Toggle(Key::KEY_LEFTSHIFT) => left_shift_active.clone(),
        KeyAction::Toggle(Key::KEY_RIGHTSHIFT) => right_shift_active.clone(),
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
    modifier_active: Rc<Cell<bool>>,
) {
    button.connect_clicked(move |button| match action {
        KeyAction::Tap(key) => {
            if let Err(error) = tap_key(&virtual_keyboard, key) {
                eprintln!("Failed to tap OSK key: {error:#}");
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

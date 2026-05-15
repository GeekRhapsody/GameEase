use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::gamepad::KeyboardDirection;

const SIDE_MENU_WIDTH: i32 = 280;
const MENU_ROWS: &[(&str, SideMenuAction)] = &[
    ("OSK Settings", SideMenuAction::None),
    ("Button Mapping", SideMenuAction::None),
    ("Profiles", SideMenuAction::None),
    ("Quit", SideMenuAction::Quit),
];

/// Action produced by activating a side menu row.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum SideMenuAction {
    /// The selected row is currently a placeholder.
    None,
    /// Quit GameEase.
    Quit,
}

struct SideMenuState {
    rows: Vec<gtk::ListBoxRow>,
    selected_index: Cell<usize>,
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

    /// Moves the selected side menu row.
    pub fn move_selection(&self, direction: KeyboardDirection) {
        let current = self.state.selected_index.get();
        let last = self.state.rows.len().saturating_sub(1);
        let next = match direction {
            KeyboardDirection::Up => current.saturating_sub(1),
            KeyboardDirection::Down => (current + 1).min(last),
            KeyboardDirection::Left | KeyboardDirection::Right => current,
        };

        self.state.select(next);
    }

    /// Activates the selected row and returns its action.
    pub fn activate_selected(&self) -> SideMenuAction {
        MENU_ROWS[self.state.selected_index.get()].1
    }
}

impl SideMenuState {
    fn select(&self, index: usize) {
        self.rows[self.selected_index.get()].remove_css_class("side-menu-selected");
        self.selected_index.set(index);
        self.rows[index].add_css_class("side-menu-selected");
    }
}

/// Builds the slide-in side menu revealer.
pub fn build_sidemenu() -> SideMenu {
    install_css();

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

    let mut rows = Vec::new();
    for (label, _) in MENU_ROWS {
        let row_label = gtk::Label::builder()
            .label(*label)
            .halign(gtk::Align::Start)
            .margin_bottom(12)
            .margin_end(12)
            .margin_start(12)
            .margin_top(12)
            .build();
        let row = gtk::ListBoxRow::builder()
            .activatable(false)
            .can_focus(false)
            .focusable(false)
            .selectable(false)
            .child(&row_label)
            .build();
        row.add_css_class("side-menu-row");
        list.append(&row);
        rows.push(row);
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

    let state = Rc::new(SideMenuState {
        rows,
        selected_index: Cell::new(0),
    });
    state.rows[0].add_css_class("side-menu-selected");

    SideMenu { revealer, state }
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
        ",
    );

    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

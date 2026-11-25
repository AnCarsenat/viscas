//! Command palette (Ctrl+P, or Space outside text fields): a centered overlay with a search entry and a
//! fuzzy-filtered list of the registered commands.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::commands::Command;
use crate::ctx::AppCtx;
use crate::fuzzy;

/// Maximum number of rows shown at once.
const MAX_ROWS: usize = 40;
/// Recently run commands remembered for the empty query.
const MAX_RECENT: usize = 8;

thread_local! {
    /// Ids of recently run commands, newest first.
    static RECENT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Remembers `id` as the most recent command.
fn remember(id: &str) {
    RECENT.with_borrow_mut(|r| {
        r.retain(|x| x != id);
        r.insert(0, id.to_owned());
        r.truncate(MAX_RECENT);
    });
}

/// Handle to the palette. Cloning is cheap.
#[derive(Clone)]
pub struct Palette {
    inner: Rc<Inner>,
}

struct Inner {
    ctx: AppCtx,
    backdrop: gtk::Box,
    card: gtk::Box,
    entry: gtk::SearchEntry,
    list: gtk::ListBox,
    /// Commands collected from the context's providers when opening.
    commands: RefCell<Vec<Command>>,
    /// Command index of each visible row.
    shown: RefCell<Vec<usize>>,
    /// Widget focused before opening, refocused on close.
    return_focus: RefCell<Option<glib::WeakRef<gtk::Widget>>>,
}

impl Palette {
    /// Creates the palette and adds it on top of `overlay`. Commands come
    /// from `ctx`'s providers, queried each time the palette opens.
    pub fn new(overlay: &gtk::Overlay, ctx: &AppCtx) -> Self {
        let backdrop = gtk::Box::builder()
            .hexpand(true)
            .vexpand(true)
            .visible(false)
            .css_classes(["palette-backdrop"])
            .build();
        let entry = gtk::SearchEntry::builder()
            .placeholder_text("Search commands…")
            .hexpand(true)
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Browse)
            .activate_on_single_click(true)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(380)
            .build();
        let footer = gtk::Label::builder()
            .label("↑↓ navigate    ↵ run    esc close")
            .xalign(0.0)
            .css_classes(["palette-footer", "mono"])
            .build();
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .visible(false)
            .css_classes(["palette"])
            .build();
        card.append(&entry);
        card.append(&scrolled);
        card.append(&footer);
        overlay.add_overlay(&backdrop);
        overlay.add_overlay(&card);

        let palette = Self {
            inner: Rc::new(Inner {
                ctx: ctx.clone(),
                backdrop,
                card,
                entry,
                list,
                commands: RefCell::default(),
                shown: RefCell::default(),
                return_focus: RefCell::default(),
            }),
        };
        palette.connect_signals();
        palette
    }

    pub fn is_open(&self) -> bool {
        self.inner.card.is_visible()
    }

    pub fn toggle(&self) {
        if self.is_open() {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn open(&self) {
        let i = &self.inner;
        let focus = i.card.root().and_then(|r| r.focus());
        *i.return_focus.borrow_mut() = focus.map(|w| w.downgrade());
        *i.commands.borrow_mut() = i.ctx.commands();
        i.entry.set_text("");
        self.refilter();
        i.backdrop.set_visible(true);
        i.card.set_visible(true);
        i.entry.grab_focus();
    }

    pub fn close(&self) {
        let i = &self.inner;
        i.backdrop.set_visible(false);
        i.card.set_visible(false);
        if let Some(w) = i.return_focus.take().and_then(|w| w.upgrade()) {
            w.grab_focus();
        }
    }

    /// Keeps keyboard focus inside the palette while it is open: focus that
    /// moves elsewhere in `window` (e.g. with Tab) returns to the entry.
    pub fn contain_focus(&self, window: &gtk::ApplicationWindow) {
        let weak = self.downgrade();
        window.connect_focus_widget_notify(move |window| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if !inner.card.is_visible() {
                return;
            }
            let card: &gtk::Widget = inner.card.upcast_ref();
            let inside =
                GtkWindowExt::focus(window).is_some_and(|f| f == *card || f.is_ancestor(card));
            if !inside {
                inner.entry.grab_focus();
            }
        });
    }

    fn downgrade(&self) -> std::rc::Weak<Inner> {
        Rc::downgrade(&self.inner)
    }

    fn connect_signals(&self) {
        let i = &self.inner;
        let weak = self.downgrade();
        let with = move |f: fn(&Palette)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&Palette { inner });
                }
            }
        };

        let refilter = with(Palette::refilter);
        i.entry.connect_search_changed(move |_| refilter());
        let run = with(Palette::run_selected);
        i.entry.connect_activate(move |_| run());
        let close = with(Palette::close);
        i.entry.connect_stop_search(move |_| close());

        // Arrow keys move the selection while typing.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(inner) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let palette = Palette { inner };
            match key {
                gdk::Key::Down | gdk::Key::KP_Down => palette.move_selection(1),
                gdk::Key::Up | gdk::Key::KP_Up => palette.move_selection(-1),
                gdk::Key::Page_Down => palette.move_selection(8),
                gdk::Key::Page_Up => palette.move_selection(-8),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        i.entry.add_controller(keys);

        // Escape closes from anywhere inside the card (entry, list, ...).
        let escape = gtk::EventControllerKey::new();
        escape.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        escape.connect_key_pressed(move |_, key, _, _| {
            if key != gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            if let Some(inner) = weak.upgrade() {
                Palette { inner }.close();
            }
            glib::Propagation::Stop
        });
        i.card.add_controller(escape);

        let weak = self.downgrade();
        i.list.connect_row_activated(move |_, row| {
            if let Some(inner) = weak.upgrade() {
                let palette = Palette { inner };
                palette.run_row(row.index());
            }
        });

        let click = gtk::GestureClick::new();
        let close = with(Palette::close);
        click.connect_pressed(move |_, _, _, _| close());
        i.backdrop.add_controller(click);
    }

    fn move_selection(&self, delta: i32) {
        let list = &self.inner.list;
        let count = i32::try_from(self.inner.shown.borrow().len()).unwrap_or(0);
        if count == 0 {
            return;
        }
        let current = list.selected_row().map_or(0, |r| r.index());
        let next = (current + delta).clamp(0, count - 1);
        if let Some(row) = list.row_at_index(next) {
            list.select_row(Some(&row));
            // Keep the selection visible without moving keyboard focus.
            if let Some(adj) = list
                .parent()
                .and_then(|p| p.parent())
                .and_downcast::<gtk::ScrolledWindow>()
                .map(|s| s.vadjustment())
                && let Some(bounds) = row.compute_bounds(list)
            {
                let (top, bottom) = (
                    f64::from(bounds.y()),
                    f64::from(bounds.y() + bounds.height()),
                );
                if top < adj.value() {
                    adj.set_value(top);
                } else if bottom > adj.value() + adj.page_size() {
                    adj.set_value(bottom - adj.page_size());
                }
            }
        }
    }

    fn run_selected(&self) {
        if let Some(row) = self.inner.list.selected_row() {
            self.run_row(row.index());
        }
    }

    fn run_row(&self, row: i32) {
        let index = usize::try_from(row)
            .ok()
            .and_then(|r| self.inner.shown.borrow().get(r).copied());
        let Some(index) = index else {
            return;
        };
        let run = self.inner.commands.borrow().get(index).map(|c| {
            remember(&c.id);
            c.run.clone()
        });
        self.close();
        if let Some(run) = run {
            // Run after the palette is hidden and the event is finished.
            glib::idle_add_local_once(move || run());
        }
    }

    fn refilter(&self) {
        let i = &self.inner;
        let query = i.entry.text();
        let commands = i.commands.borrow();
        // Matched characters use the theme's readable accent text color.
        let match_color = i
            .ctx
            .preferences()
            .with(|p| p.effective_theme().ui.accent_text.to_rgb_hex());
        let mut ranked = fuzzy::rank(&query, commands.iter().map(|c| c.title.as_str()));
        if query.trim().is_empty() {
            // Empty query: recent commands first, then window/app actions
            // (the node, function and setting indexes need a query).
            let recent = RECENT.with_borrow(|r| r.clone());
            let pos = |i: usize| recent.iter().position(|id| *id == commands[i].id);
            ranked.retain(|(i, _)| {
                pos(*i).is_some()
                    || !matches!(commands[*i].category, "Node" | "Function" | "Settings")
            });
            ranked.sort_by_key(|(i, _)| pos(*i).unwrap_or(usize::MAX));
        }
        while let Some(child) = i.list.first_child() {
            i.list.remove(&child);
        }
        let mut shown = Vec::new();
        for (index, m) in ranked.into_iter().take(MAX_ROWS) {
            let cmd = &commands[index];
            i.list.append(&command_row(cmd, &m.positions, &match_color));
            shown.push(index);
        }
        if shown.is_empty() {
            let empty = gtk::Label::builder()
                .label("No matching command")
                .css_classes(["palette-empty"])
                .build();
            let row = gtk::ListBoxRow::builder()
                .child(&empty)
                .activatable(false)
                .selectable(false)
                .build();
            i.list.append(&row);
        }
        *i.shown.borrow_mut() = shown;
        if let Some(first) = i.list.row_at_index(0) {
            i.list.select_row(Some(&first));
        }
    }
}

fn command_row(cmd: &Command, positions: &[usize], match_color: &str) -> gtk::ListBoxRow {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    row.append(
        &gtk::Label::builder()
            .label(cmd.category)
            .xalign(0.0)
            .css_classes(["command-category"])
            .build(),
    );
    row.append(
        &gtk::Label::builder()
            .use_markup(true)
            .label(highlight(&cmd.title, positions, match_color))
            .xalign(0.0)
            .hexpand(true)
            .build(),
    );
    if let Some(shortcut) = &cmd.shortcut {
        row.append(
            &gtk::Label::builder()
                .label(shortcut)
                .valign(gtk::Align::Center)
                .css_classes(["keycap"])
                .build(),
        );
    }
    gtk::ListBoxRow::builder()
        .child(&row)
        .tooltip_text(&cmd.id)
        .build()
}

/// Pango markup of `text` with the chars at `positions` highlighted in
/// `color` (a `#rrggbb` value).
fn highlight(text: &str, positions: &[usize], color: &str) -> String {
    let mut out = String::new();
    for (i, c) in text.chars().enumerate() {
        let escaped = glib::markup_escape_text(c.encode_utf8(&mut [0; 4]));
        if positions.contains(&i) {
            out.push_str(&format!(
                "<span foreground=\"{color}\" weight=\"bold\">{escaped}</span>"
            ));
        } else {
            out.push_str(&escaped);
        }
    }
    out
}

//! Preferences ▸ Keymap: the Blender-style keymap editor. Search by name
//! or by key, collapsible categories, one row per action with its
//! bindings (click to capture a new chord, × to remove, + to add),
//! conflict warnings with Replace / Swap, per-row and global reset,
//! presets and TOML import/export. Edits go through the preferences, so
//! the live keymap (and every menu and palette label) follows at once.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::ctx::AppCtx;
use crate::keymap::model::{ActionSpec, Chord, KeymapPrefs, Preset};
use crate::keymap::{live, registry};
use crate::prefs;
use crate::widgets::Panel;

/// What the next key press is captured for.
enum Capture {
    /// Binding `index` of action `id` (`None`: a new binding).
    Binding { id: String, index: Option<usize> },
    /// The key filter of the search.
    KeySearch,
}

/// The last capture that created a conflict: action, new chord, and the
/// chord it replaced (what "Swap" hands to the other action).
type SwapHint = (String, Chord, Option<Chord>);

struct Editor {
    ctx: AppCtx,
    list: gtk::Box,
    search: gtk::SearchEntry,
    key_button: gtk::Button,
    key_clear: gtk::Button,
    preset: gtk::DropDown,
    summary: gtk::Label,
    key_filter: RefCell<Option<Chord>>,
    capture: RefCell<Option<Capture>>,
    swap_hint: RefCell<Option<SwapHint>>,
    collapsed: RefCell<HashSet<String>>,
    /// Keymap the list was last built for.
    shown: RefCell<Option<KeymapPrefs>>,
    rendering: Cell<bool>,
    syncing: Cell<bool>,
}

/// Builds the Keymap page.
pub fn page(ctx: &AppCtx) -> gtk::Box {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .css_classes(["prefs-page", "keymap-page"])
        .build();

    let names: Vec<&str> = Preset::ALL.iter().map(|p| p.title()).collect();
    let preset = gtk::DropDown::from_strings(&names);
    preset.set_tooltip_text(Some(
        "Default bindings to start from; switching resets your changes",
    ));
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search by name or key…")
        .hexpand(true)
        .css_classes(["keymap-search"])
        .build();
    let key_button = gtk::Button::builder()
        .label("Search by Key")
        .tooltip_text("Press a key combination to find the actions bound to it")
        .css_classes(["keymap-key-filter"])
        .build();
    let key_clear = gtk::Button::builder()
        .icon_name("window-close-symbolic")
        .tooltip_text("Clear key filter")
        .visible(false)
        .css_classes(["keymap-icon", "flat"])
        .build();

    let top = gtk::Box::builder()
        .spacing(4)
        .css_classes(["keymap-toolbar"])
        .build();
    top.append(&preset);
    top.append(&search);
    top.append(&key_button);
    top.append(&key_clear);
    page.append(&top);

    let tools = gtk::Box::builder()
        .spacing(4)
        .css_classes(["keymap-toolbar"])
        .build();
    let import = gtk::Button::with_label("Import…");
    import.set_tooltip_text(Some("Load a keymap from a TOML file"));
    let export = gtk::Button::with_label("Export…");
    export.set_tooltip_text(Some("Save the keymap to a TOML file"));
    let reset_all = gtk::Button::with_label("Reset All");
    reset_all.set_tooltip_text(Some("Restore every binding of the preset"));
    tools.append(&import);
    tools.append(&export);
    let summary = gtk::Label::builder()
        .hexpand(true)
        .xalign(1.0)
        .visible(false)
        .css_classes(["chip", "error"])
        .build();
    tools.append(&summary);
    tools.append(&reset_all);
    page.append(&tools);

    let list = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    page.append(&list);
    page.append(
        &gtk::Label::builder()
            .label(
                "Click a binding to change it: Esc cancels, Backspace removes it. \
                 Keys are matched by the symbol your layout types (AZERTY é is É). \
                 Bindings conflict only within one context; a focused editor's own \
                 keys win over Window keys.",
            )
            .wrap(true)
            .xalign(0.0)
            .css_classes(["prefs-note"])
            .build(),
    );

    let editor = Rc::new(Editor {
        ctx: ctx.clone(),
        list,
        search: search.clone(),
        key_button: key_button.clone(),
        key_clear: key_clear.clone(),
        preset: preset.clone(),
        summary,
        key_filter: RefCell::default(),
        capture: RefCell::default(),
        swap_hint: RefCell::default(),
        collapsed: RefCell::default(),
        shown: RefCell::default(),
        rendering: Cell::new(false),
        syncing: Cell::new(false),
    });
    let weak = Rc::downgrade(&editor);

    preset.connect_selected_notify(glib::clone!(
        #[strong]
        weak,
        move |d| {
            let Some(e) = weak.upgrade() else { return };
            if e.syncing.get() {
                return;
            }
            if let Some(p) = Preset::ALL.get(d.selected() as usize).copied() {
                let specs = e.specs();
                e.ctx
                    .preferences()
                    .update(|prefs| prefs.keymap.set_preset(p, &specs));
            }
        }
    ));
    search.connect_search_changed(glib::clone!(
        #[strong]
        weak,
        move |_| {
            if let Some(e) = weak.upgrade() {
                e.render(true);
            }
        }
    ));
    key_button.connect_clicked(glib::clone!(
        #[strong]
        weak,
        move |b| {
            if let Some(e) = weak.upgrade() {
                e.start_capture(Capture::KeySearch, b.upcast_ref());
            }
        }
    ));
    key_clear.connect_clicked(glib::clone!(
        #[strong]
        weak,
        move |_| {
            if let Some(e) = weak.upgrade() {
                e.key_filter.replace(None);
                e.render(true);
            }
        }
    ));
    reset_all.connect_clicked(glib::clone!(
        #[strong]
        weak,
        move |_| {
            if let Some(e) = weak.upgrade() {
                let specs = e.specs();
                e.ctx.preferences().update(|p| p.keymap.reset_all(&specs));
            }
        }
    ));
    import.connect_clicked(glib::clone!(
        #[strong]
        weak,
        move |b| {
            if let Some(e) = weak.upgrade() {
                import_keymap(b, &e.ctx);
            }
        }
    ));
    export.connect_clicked(glib::clone!(
        #[strong]
        weak,
        move |b| {
            if let Some(e) = weak.upgrade() {
                export_keymap(b, &e.ctx);
            }
        }
    ));

    // Key capture runs before the focused button sees the key.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(glib::clone!(
        #[strong]
        weak,
        move |_, key, keycode, state| match weak.upgrade() {
            Some(e) => e.on_key(key, keycode, state),
            None => glib::Propagation::Proceed,
        }
    ));
    page.add_controller(keys);

    editor.render(true);
    ctx.preferences()
        .connect_changed_for(weak, |e, _| e.render(false));
    // The page owns the editor (its widgets point back only weakly).
    page.connect_destroy(move |_| {
        live::set_capturing(false);
        let _ = &editor;
    });
    page
}

impl Editor {
    fn keymap(&self) -> KeymapPrefs {
        self.ctx.preferences().with(|p| p.keymap.clone())
    }

    /// Registered actions plus the palette's bindable commands.
    fn specs(&self) -> Vec<ActionSpec> {
        registry::with_commands(&self.ctx.commands())
    }

    fn weak(self: &Rc<Self>) -> Weak<Self> {
        Rc::downgrade(self)
    }

    /// Rebuilds the list; unless `force`, only when the keymap changed.
    fn render(self: &Rc<Self>, force: bool) {
        let km = self.keymap();
        if !force && self.shown.borrow().as_ref() == Some(&km) {
            return;
        }
        self.shown.replace(Some(km.clone()));
        self.rendering.set(true);
        self.syncing.set(true);
        let index = Preset::ALL
            .iter()
            .position(|p| *p == km.preset())
            .unwrap_or(0) as u32;
        if self.preset.selected() != index {
            self.preset.set_selected(index);
        }
        self.syncing.set(false);
        self.sync_key_button();

        // Never remove the focused widget: move focus out of the list
        // first (a focused row button would be destroyed under GTK).
        if let Some(root) = self.list.root()
            && root.focus().is_some_and(|f| {
                f == *self.list.upcast_ref::<gtk::Widget>() || f.is_ancestor(&self.list)
            })
        {
            root.set_focus(None::<&gtk::Widget>);
        }
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let specs = self.specs();
        let n = km.conflicts(&specs).len();
        self.summary.set_visible(n > 0);
        self.summary.set_halign(gtk::Align::End);
        self.summary
            .set_label(&format!("⚠ {n} conflict{}", if n == 1 { "" } else { "s" }));
        let query = self.search.text().to_lowercase();
        let key_filter = self.key_filter.borrow().clone();
        let filtering = !query.is_empty() || key_filter.is_some();

        let mut categories: Vec<String> = Vec::new();
        for s in &specs {
            if !categories.contains(&s.category) {
                categories.push(s.category.clone());
            }
        }
        let mut shown = 0;
        for category in categories {
            let rows: Vec<&ActionSpec> = specs
                .iter()
                .filter(|s| s.category == category)
                .filter(|s| matches(s, &km.bindings_of(s), &query, key_filter.as_ref()))
                .collect();
            if rows.is_empty() {
                continue;
            }
            shown += rows.len();
            let expanded = filtering || !self.collapsed.borrow().contains(&category);
            let panel = Panel::new(&category, expanded);
            if !filtering {
                let weak = self.weak();
                let cat = category.clone();
                panel.connect_expanded(move |open| {
                    if let Some(e) = weak.upgrade() {
                        let mut set = e.collapsed.borrow_mut();
                        if open {
                            set.remove(&cat);
                        } else {
                            set.insert(cat.clone());
                        }
                    }
                });
            }
            for spec in rows {
                panel.append(&self.row(spec, &specs, &km));
            }
            self.list.append(panel.widget());
        }
        if shown == 0 {
            self.list.append(
                &gtk::Label::builder()
                    .label("No matching actions")
                    .xalign(0.0)
                    .css_classes(["prefs-note"])
                    .build(),
            );
        }
        self.rendering.set(false);
    }

    fn sync_key_button(&self) {
        let capturing = matches!(*self.capture.borrow(), Some(Capture::KeySearch));
        let filter = self.key_filter.borrow().clone();
        let label = match (&filter, capturing) {
            (_, true) => "Press a key…".to_owned(),
            (Some(c), false) => format!("Key: {}", c.label()),
            (None, false) => "Search by Key".to_owned(),
        };
        self.key_button.set_label(&label);
        if capturing {
            self.key_button.add_css_class("capturing");
        } else {
            self.key_button.remove_css_class("capturing");
        }
        self.key_clear.set_visible(filter.is_some() && !capturing);
    }

    /// One action row.
    fn row(self: &Rc<Self>, spec: &ActionSpec, specs: &[ActionSpec], km: &KeymapPrefs) -> gtk::Box {
        let row = gtk::Box::builder()
            .spacing(4)
            .css_classes(["keymap-row"])
            .tooltip_text(&spec.id)
            .build();
        let overridden = km.is_overridden(spec);
        if overridden {
            row.add_css_class("overridden");
        }
        row.append(
            &gtk::Label::builder()
                .label(&spec.title)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["keymap-name"])
                .build(),
        );
        let bindings = km.bindings_of(spec);
        let capture_index = match &*self.capture.borrow() {
            Some(Capture::Binding { id, index }) if *id == spec.id => Some(*index),
            _ => None,
        };
        let mut conflicts: Vec<(Chord, Vec<String>)> = Vec::new();
        for (i, chord) in bindings.iter().enumerate() {
            let others = km.conflicts_with(specs, &spec.id, spec.context, chord);
            let capturing = capture_index == Some(Some(i));
            let button = self.binding_button(
                &spec.id,
                Some(i),
                if capturing {
                    "Press a key combination…".to_owned()
                } else {
                    chord.label()
                },
                capturing,
            );
            if !others.is_empty() {
                button.add_css_class("conflict");
                button.set_tooltip_text(Some(&format!(
                    "{} is also bound to {}",
                    chord.label(),
                    titles(specs, &others)
                )));
                conflicts.push((chord.clone(), others));
            } else {
                button
                    .set_tooltip_text(Some("Click to change · Backspace while capturing removes"));
            }
            row.append(&button);
            let remove = icon_button("window-close-symbolic", "Remove this binding");
            let weak = self.weak();
            let (id, idx) = (spec.id.clone(), i);
            remove.connect_clicked(move |_| {
                if let Some(e) = weak.upgrade() {
                    e.assign(&id, Some(idx), None);
                }
            });
            row.append(&remove);
        }
        let adding = capture_index == Some(None);
        if adding || bindings.is_empty() {
            let label = if adding {
                "Press a key combination…"
            } else {
                "None"
            };
            let b = self.binding_button(&spec.id, None, label.to_owned(), adding);
            if !adding {
                b.add_css_class("unbound");
                b.set_tooltip_text(Some("Not bound · click to assign a key"));
            }
            row.append(&b);
        } else if bindings.len() < crate::keymap::model::MAX_BINDINGS {
            let add = icon_button("list-add-symbolic", "Add another binding");
            let weak = self.weak();
            let id = spec.id.clone();
            add.connect_clicked(move |b| {
                if let Some(e) = weak.upgrade() {
                    e.start_capture(
                        Capture::Binding {
                            id: id.clone(),
                            index: None,
                        },
                        b.upcast_ref(),
                    );
                }
            });
            row.append(&add);
        }
        for (chord, others) in conflicts {
            row.append(&self.conflict_chip(spec, specs, &chord, &others));
        }
        let reset = icon_button("edit-undo-symbolic", "");
        let defaults: Vec<String> = spec
            .defaults_for(km.preset())
            .iter()
            .map(Chord::label)
            .collect();
        reset.set_tooltip_text(Some(&format!(
            "Reset to default ({})",
            if defaults.is_empty() {
                "none".to_owned()
            } else {
                defaults.join(", ")
            }
        )));
        // Keeps its space so rows stay aligned.
        reset.set_opacity(if overridden { 1.0 } else { 0.0 });
        reset.set_sensitive(overridden);
        let weak = self.weak();
        let id = spec.id.clone();
        reset.connect_clicked(move |_| {
            if let Some(e) = weak.upgrade() {
                e.ctx.preferences().update(|p| p.keymap.reset(&id));
            }
        });
        row.append(&reset);
        row
    }

    fn binding_button(
        self: &Rc<Self>,
        id: &str,
        index: Option<usize>,
        label: String,
        capturing: bool,
    ) -> gtk::Button {
        let button = gtk::Button::builder()
            .label(label)
            .css_classes(["binding"])
            .valign(gtk::Align::Center)
            .build();
        if capturing {
            button.add_css_class("capturing");
            // Clicking elsewhere cancels the capture.
            let focus = gtk::EventControllerFocus::new();
            let weak = self.weak();
            let (cid, cindex) = (id.to_owned(), index);
            focus.connect_leave(move |_| {
                // Deferred (the list may be rebuilding right now), and only
                // if this is still the binding being captured.
                let (weak, cid) = (weak.clone(), cid.clone());
                glib::idle_add_local_once(move || {
                    let Some(e) = weak.upgrade() else { return };
                    let same = matches!(
                        &*e.capture.borrow(),
                        Some(Capture::Binding { id, index }) if *id == cid && *index == cindex
                    );
                    if same {
                        e.cancel_capture();
                    }
                });
            });
            button.add_controller(focus);
            glib::idle_add_local_once(glib::clone!(
                #[weak]
                button,
                move || {
                    button.grab_focus();
                }
            ));
        }
        let weak = self.weak();
        let id = id.to_owned();
        button.connect_clicked(move |b| {
            if let Some(e) = weak.upgrade() {
                if b.has_css_class("capturing") {
                    return;
                }
                e.start_capture(
                    Capture::Binding {
                        id: id.clone(),
                        index,
                    },
                    b.upcast_ref(),
                );
            }
        });
        button
    }

    /// The warning chip of a conflict on `chord`, with Replace and Swap.
    fn conflict_chip(
        self: &Rc<Self>,
        spec: &ActionSpec,
        specs: &[ActionSpec],
        chord: &Chord,
        others: &[String],
    ) -> gtk::MenuButton {
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        body.append(
            &gtk::Label::builder()
                .label(format!(
                    "{} is also bound to {}",
                    chord.label(),
                    titles(specs, others)
                ))
                .wrap(true)
                .max_width_chars(36)
                .xalign(0.0)
                .build(),
        );
        let replace = gtk::Button::with_label("Replace");
        replace.set_tooltip_text(Some(&format!(
            "Keep {} here and remove it from the other actions",
            chord.label()
        )));
        let hint = self.swap_hint.borrow().clone();
        let previous = hint
            .filter(|(id, c, _)| *id == spec.id && c == chord)
            .map(|(_, _, prev)| prev);
        let swap = gtk::Button::with_label("Swap");
        match &previous {
            Some(Some(p)) => swap.set_tooltip_text(Some(&format!(
                "Give the other actions {} (this action's previous key)",
                p.label()
            ))),
            Some(None) => swap.set_tooltip_text(Some(
                "This action had no previous key: the others lose this one",
            )),
            None => {
                swap.set_sensitive(false);
                swap.set_tooltip_text(Some("Available right after assigning a key"));
            }
        }
        let buttons = gtk::Box::builder().spacing(4).homogeneous(true).build();
        buttons.append(&replace);
        buttons.append(&swap);
        body.append(&buttons);
        let popover = gtk::Popover::builder().child(&body).build();
        let chip = gtk::MenuButton::builder()
            .label("⚠ Conflict")
            .popover(&popover)
            .always_show_arrow(false)
            .valign(gtk::Align::Center)
            .tooltip_text("Resolve the conflict")
            .css_classes(["chip", "error", "flat"])
            .build();
        let weak = self.weak();
        let (id, c) = (spec.id.clone(), chord.clone());
        replace.connect_clicked(glib::clone!(
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                if let Some(e) = weak.upgrade() {
                    let specs = e.specs();
                    e.swap_hint.replace(None);
                    e.ctx
                        .preferences()
                        .update(|p| p.keymap.replace_conflict(&specs, &id, &c));
                }
            }
        ));
        let weak = self.weak();
        let (id, c) = (spec.id.clone(), chord.clone());
        swap.connect_clicked(glib::clone!(
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                if let (Some(e), Some(prev)) = (weak.upgrade(), previous.clone()) {
                    let specs = e.specs();
                    e.swap_hint.replace(None);
                    e.ctx.preferences().update(|p| {
                        p.keymap.swap_conflict(&specs, &id, &c, prev.as_ref());
                    });
                }
            }
        ));
        chip
    }

    fn start_capture(self: &Rc<Self>, capture: Capture, _from: &gtk::Widget) {
        self.capture.replace(Some(capture));
        live::set_capturing(true);
        self.render(true);
    }

    fn cancel_capture(self: &Rc<Self>) {
        if self.capture.take().is_some() {
            live::set_capturing(false);
            if !self.rendering.get() {
                self.render(true);
            }
        }
    }

    /// Sets binding `index` of `id` (`None`: append), or removes it.
    fn assign(self: &Rc<Self>, id: &str, index: Option<usize>, chord: Option<Chord>) {
        let specs = self.specs();
        let Some(spec) = specs.iter().find(|s| s.id == id) else {
            return;
        };
        let km = self.keymap();
        let previous = index.and_then(|i| km.bindings_of(spec).get(i).cloned());
        if let Some(c) = &chord
            && !km.conflicts_with(&specs, id, spec.context, c).is_empty()
        {
            self.swap_hint
                .replace(Some((id.to_owned(), c.clone(), previous)));
        }
        self.ctx
            .preferences()
            .update(|p| p.keymap.assign(spec, index, chord));
        // Unchanged keymap (same chord again): still leave capture mode.
        self.render(true);
    }

    fn on_key(
        self: &Rc<Self>,
        key: gdk::Key,
        keycode: u32,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        let stop = glib::Propagation::Stop;
        if self.capture.borrow().is_none() {
            return glib::Propagation::Proceed;
        }
        let plain = !state.intersects(
            gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::SHIFT_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK,
        );
        if key == gdk::Key::Escape && plain {
            self.cancel_capture();
            return stop;
        }
        let clear = key == gdk::Key::BackSpace && plain;
        let chord = live::event_chords(key, keycode, state).into_iter().next();
        if chord.is_none() && !clear {
            // A lone modifier or a dead key: keep waiting.
            return stop;
        }
        let Some(capture) = self.capture.take() else {
            return stop;
        };
        live::set_capturing(false);
        match capture {
            Capture::KeySearch => {
                self.key_filter.replace(if clear { None } else { chord });
                self.render(true);
            }
            Capture::Binding { id, index } => {
                if clear {
                    match index {
                        Some(_) => self.assign(&id, index, None),
                        None => self.render(true),
                    }
                } else {
                    self.assign(&id, index, chord);
                }
            }
        }
        stop
    }
}

/// Whether `spec` passes the search: `query` in its title, category, id
/// or a binding label, and `key` among its bindings.
fn matches(spec: &ActionSpec, bindings: &[Chord], query: &str, key: Option<&Chord>) -> bool {
    let text_ok = query.is_empty()
        || spec.title.to_lowercase().contains(query)
        || spec.category.to_lowercase().contains(query)
        || spec.id.to_lowercase().contains(query)
        || key_text_matches(bindings, query);
    text_ok && key.is_none_or(|k| bindings.contains(k))
}

/// Whether a typed key query matches a binding: a full chord
/// (`ctrl+s`, `Ctrl+É`) must equal one, so `ctrl+s` does not find
/// `Ctrl+Shift+Tab`; a lone word matches inside the labels (`f2`, `tab`).
fn key_text_matches(bindings: &[Chord], query: &str) -> bool {
    if query.contains('+') && query.len() > 1 {
        return Chord::parse(query).is_ok_and(|c| bindings.contains(&c));
    }
    bindings.iter().any(|b| {
        b.label()
            .to_lowercase()
            .split('+')
            .any(|part| part == query || (query.len() > 1 && part.contains(query)))
    })
}

fn titles(specs: &[ActionSpec], ids: &[String]) -> String {
    ids.iter()
        .map(|id| {
            specs.iter().find(|s| &s.id == id).map_or_else(
                || id.clone(),
                |s| format!("“{}” ({})", s.title, s.context.title()),
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tip)
        .valign(gtk::Align::Center)
        .css_classes(["keymap-icon", "flat"])
        .build()
}

fn toml_filter() -> gio::ListStore {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Viscas keymaps (*.toml)"));
    filter.add_pattern("*.toml");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    filters
}

fn alert(parent: Option<&gtk::Window>, message: &str, detail: &str) {
    gtk::AlertDialog::builder()
        .message(message)
        .detail(detail)
        .modal(true)
        .build()
        .show(parent);
}

fn import_keymap(button: &gtk::Button, ctx: &AppCtx) {
    let parent = button.root().and_downcast::<gtk::Window>();
    let dialog = gtk::FileDialog::builder()
        .title("Import Keymap")
        .filters(&toml_filter())
        .modal(true)
        .build();
    let ctx = ctx.clone();
    let transient = parent.clone();
    dialog.open(
        transient.as_ref(),
        None::<&gio::Cancellable>,
        move |result| {
            let Ok(path) = result.map(|f| f.path()) else {
                return; // cancelled
            };
            let loaded = path
                .as_ref()
                .ok_or_else(|| "not a local file".to_owned())
                .and_then(|p| prefs::read_capped(p).map_err(|e| e.to_string()))
                .and_then(|text| KeymapPrefs::from_toml(&text));
            match loaded {
                Ok((km, warnings)) => {
                    ctx.preferences().update(|p| p.keymap = km);
                    if !warnings.is_empty() {
                        alert(
                            parent.as_ref(),
                            "Keymap imported with warnings",
                            &warnings.join("\n"),
                        );
                    }
                }
                Err(e) => alert(parent.as_ref(), "Could not import keymap", &e),
            }
        },
    );
}

fn export_keymap(button: &gtk::Button, ctx: &AppCtx) {
    let parent = button.root().and_downcast::<gtk::Window>();
    let km = ctx.preferences().with(|p| p.keymap.clone());
    let dialog = gtk::FileDialog::builder()
        .title("Export Keymap")
        .filters(&toml_filter())
        .initial_name("viscas-keymap.toml")
        .modal(true)
        .build();
    let transient = parent.clone();
    dialog.save(
        transient.as_ref(),
        None::<&gio::Cancellable>,
        move |result| {
            let Some(path) = result.ok().and_then(|f| f.path()) else {
                return;
            };
            if let Err(e) = km
                .to_toml()
                .map_err(std::io::Error::other)
                .and_then(|text| prefs::write_atomic(&path, &text))
            {
                alert(parent.as_ref(), "Could not export keymap", &e.to_string());
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::matches;
    use crate::keymap::model::{ActionSpec, Chord, Context};

    #[test]
    fn search_matches_names_and_keys() {
        let spec = ActionSpec::new("win.save", "Save Document", "File", Context::Window, &[]);
        let keys = [Chord::parse("Ctrl+S").unwrap()];
        assert!(matches(&spec, &keys, "", None));
        assert!(matches(&spec, &keys, "save", None));
        assert!(matches(&spec, &keys, "ctrl+s", None));
        assert!(matches(&spec, &keys, "Ctrl+S", None));
        let tab = [Chord::parse("Ctrl+Shift+Tab").unwrap()];
        let other = ActionSpec::new("win.prev-tab", "Previous Tab", "Area", Context::Window, &[]);
        assert!(!matches(&other, &tab, "ctrl+s", None));
        assert!(matches(&other, &tab, "tab", None));
        assert!(matches(&spec, &keys, "file", None));
        assert!(!matches(&spec, &keys, "undo", None));
        assert!(matches(&spec, &keys, "", Some(&keys[0])));
        assert!(!matches(
            &spec,
            &keys,
            "",
            Some(&Chord::parse("Ctrl+Z").unwrap())
        ));
    }
}

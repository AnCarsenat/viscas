//! IDE-grade assistance for every math input: the console input, the
//! expression rows and the inspector's expression fields share one
//! [`Assist`] controller.
//!
//! - **Completion popup** as you type (Ctrl+Space forces it, Tab with a
//!   fragment too): functions, constants, scene nodes with their value,
//!   console locals and, in the console, `:commands`. Items are ranked
//!   with the palette's fuzzy matcher ([`rank`]); each row shows a kind
//!   badge, the name with matched letters in bold, its parameters and a
//!   short doc; a side pane shows the full doc and examples. ↑/↓ move,
//!   Tab/Enter accept, Esc dismisses.
//! - **Snippets**: accepting a function inserts `f()` with the caret
//!   inside, or `f(a, b)` with each parameter a placeholder Tab/Shift+Tab
//!   walk through ([`edit::SnippetSession`]).
//! - **Signature help**: inside `f(`…`)` a hint shows the active form
//!   with the current parameter in bold (scene functions included).
//! - **Diagnostics** (debounced): parse errors and unknown names get a
//!   squiggle with a tooltip; Ctrl+. applies the first "did you mean".
//! - **Hover docs** on names in the field and, through
//!   [`install_label_hover`], in console transcript entries.
//! - **Auto-pairing** of `()`, `[]`, `{}`, `||` and smart backspace.
//! - **Inline preview** (console): `= 1/2` ghost text after a cheap pure
//!   input, evaluated on a worker with a step budget, never blocking.
//!
//! Keys: editors call [`Assist::on_key`] first from their own
//! capture-phase key controller (or [`Assist::install_keys`] adds one).
//! Only printable bracket keys, Backspace, Tab, Enter, arrows, Esc,
//! Ctrl+Space and Ctrl+. are ever consumed; Space and dead keys pass
//! through untouched, so the compose fallback and text undo keep working.
//! Every feature has a toggle in Preferences ▸ Editing.

pub mod edit;
pub mod field;
pub mod markup;
pub mod rank;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use viscas_engine::{Diagnostic, Scope, Severity};

use crate::ctx::AppCtx;
use crate::editors::console::complete::{byte_to_char, char_to_byte, words};
use crate::editors::console::style;
use crate::theme::model::Color;
use edit::{Edit, SnippetSession};
pub use field::Field;
use rank::Ranked;

/// Delay between the last keystroke and diagnostics / preview.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// A preview still running after this is cancelled.
const PREVIEW_TIMEOUT: Duration = Duration::from_millis(400);
/// Squiggle color of warnings (themes have no warning color).
const WARNING: Color = Color::hex(0xd8a032);

struct Popup {
    popover: gtk::Popover,
    list: gtk::ListBox,
    scroll: gtk::ScrolledWindow,
    doc: gtk::Label,
}

struct Hint {
    popover: gtk::Popover,
    label: gtk::Label,
}

struct Ghost {
    overlay: gtk::Overlay,
    label: gtk::Label,
}

struct Inner {
    ctx: AppCtx,
    field: Field,
    /// Parent of the popovers (a box around the field).
    host: gtk::Widget,
    scope: Scope,
    popup: RefCell<Option<Popup>>,
    hint: RefCell<Option<Hint>>,
    ghost: RefCell<Option<Ghost>>,
    items: RefCell<Vec<Ranked>>,
    /// Char offset of the fragment the popup completes.
    start: Cell<usize>,
    /// Text length (chars) at the last change.
    last_len: Cell<usize>,
    /// An edit made by the assistant itself (no auto-popup after it).
    own_edit: Cell<bool>,
    change_pending: Cell<bool>,
    snippet: RefCell<Option<SnippetSession>>,
    /// Diagnostics and the text they belong to.
    diags: RefCell<(String, Vec<Diagnostic>)>,
    /// Byte ranges the editor reports (committed errors).
    external: RefCell<Vec<(usize, usize)>>,
    diag_timer: RefCell<Option<glib::SourceId>>,
    preview_timer: RefCell<Option<glib::SourceId>>,
    preview_cancel: RefCell<Option<Arc<AtomicBool>>>,
    generation: Cell<u64>,
}

/// Assistance attached to one math input. Cloning yields a handle to the
/// same controller.
#[derive(Clone)]
pub struct Assist {
    inner: Rc<Inner>,
}

/// Which assistance features are on (Preferences ▸ Editing).
#[derive(Clone, Copy, Debug)]
struct Features {
    popup: bool,
    pairs: bool,
    preview: bool,
    diagnostics: bool,
}

impl Assist {
    /// Attaches to `field`; popovers are parented to `host`, a box that
    /// contains the field (never the field itself: text widgets do not
    /// manage foreign children).
    pub fn new(ctx: &AppCtx, field: Field, host: &impl IsA<gtk::Widget>, scope: Scope) -> Assist {
        style::ensure(ctx);
        let assist = Assist {
            inner: Rc::new(Inner {
                ctx: ctx.clone(),
                last_len: Cell::new(field.text().chars().count()),
                field,
                host: host.clone().upcast(),
                scope,
                popup: RefCell::default(),
                hint: RefCell::default(),
                ghost: RefCell::default(),
                items: RefCell::default(),
                start: Cell::new(0),
                own_edit: Cell::new(false),
                change_pending: Cell::new(false),
                snippet: RefCell::default(),
                diags: RefCell::default(),
                external: RefCell::default(),
                diag_timer: RefCell::default(),
                preview_timer: RefCell::default(),
                preview_cancel: RefCell::default(),
                generation: Cell::new(0),
            }),
        };
        assist.connect();
        assist
    }

    /// Attaches to a text view (console input).
    pub fn for_text_view(
        ctx: &AppCtx,
        view: &gtk::TextView,
        host: &impl IsA<gtk::Widget>,
        scope: Scope,
    ) -> Assist {
        Assist::new(ctx, Field::View(view.clone()), host, scope)
    }

    /// Attaches to an entry; popovers go to its parent.
    pub fn for_entry(ctx: &AppCtx, entry: &gtk::Entry, scope: Scope) -> Assist {
        let host = entry
            .parent()
            .unwrap_or_else(|| entry.clone().upcast::<gtk::Widget>());
        Assist::new(ctx, Field::Entry(entry.clone()), &host, scope)
    }

    fn downgrade(&self) -> Weak<Inner> {
        Rc::downgrade(&self.inner)
    }

    fn from_weak(weak: &Weak<Inner>) -> Option<Assist> {
        weak.upgrade().map(|inner| Assist { inner })
    }

    fn features(&self) -> Features {
        self.inner.ctx.preferences().with(|p| Features {
            popup: p.editing.auto_complete_popup,
            pairs: p.editing.auto_pair_brackets,
            preview: p.editing.inline_preview,
            diagnostics: p.editing.live_diagnostics,
        })
    }

    /// Adds a capture-phase key controller calling [`Assist::on_key`],
    /// for fields whose editor has none.
    pub fn install_keys(&self) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, mods| match Assist::from_weak(&weak) {
            Some(a) => a.on_key(key, mods),
            None => glib::Propagation::Proceed,
        });
        self.inner.field.widget().add_controller(keys);
    }

    /// Shows the inline value preview over `overlay` (which holds the
    /// field).
    pub fn enable_preview(&self, overlay: &gtk::Overlay) {
        let label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .can_target(false)
            .visible(false)
            .css_classes(["assist-ghost"])
            .build();
        overlay.add_overlay(&label);
        *self.inner.ghost.borrow_mut() = Some(Ghost {
            overlay: overlay.clone(),
            label,
        });
    }

    /// Byte ranges of committed errors the editor wants underlined too.
    pub fn set_external_marks(&self, ranges: Vec<(usize, usize)>) {
        *self.inner.external.borrow_mut() = ranges;
        self.render_marks();
    }

    /// Whether the completion popup is showing.
    pub fn popup_open(&self) -> bool {
        self.inner
            .popup
            .borrow()
            .as_ref()
            .is_some_and(|p| p.popover.is_visible())
    }

    /// Hides every popup (the editor evaluated, committed, lost focus).
    pub fn dismiss(&self) {
        self.hide_popup();
        self.hide_hint();
        self.inner.snippet.borrow_mut().take();
    }

    // -------------------------------------------------------- signals

    fn connect(&self) {
        let f = &self.inner.field;
        let weak = self.downgrade();
        f.connect_changed(move || {
            if let Some(a) = Assist::from_weak(&weak) {
                a.on_changed();
            }
        });
        let weak = self.downgrade();
        f.connect_cursor_moved(move || {
            if let Some(a) = Assist::from_weak(&weak)
                && !a.inner.change_pending.get()
            {
                a.on_cursor_moved();
            }
        });
        let widget = f.widget();
        let focus = gtk::EventControllerFocus::new();
        let weak = self.downgrade();
        focus.connect_leave(move |_| {
            if let Some(a) = Assist::from_weak(&weak) {
                a.dismiss();
                a.hide_ghost();
                a.schedule_diagnostics();
            }
        });
        widget.add_controller(focus);
        let weak = self.downgrade();
        widget.connect_unmap(move |_| {
            if let Some(a) = Assist::from_weak(&weak) {
                a.dismiss();
            }
        });
        widget.set_has_tooltip(true);
        let weak = self.downgrade();
        widget.connect_query_tooltip(move |_, x, y, keyboard, tooltip| {
            !keyboard
                && Assist::from_weak(&weak)
                    .is_some_and(|a| a.query_tooltip(f64::from(x), f64::from(y), tooltip))
        });
    }

    fn on_changed(&self) {
        // Let the caret settle (entries move it after "changed").
        if self.inner.change_pending.replace(true) {
            return;
        }
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(a) = Assist::from_weak(&weak) {
                a.inner.change_pending.set(false);
                a.after_change();
            }
        });
    }

    fn after_change(&self) {
        let field = &self.inner.field;
        let text = field.text();
        let len = text.chars().count();
        let caret = field.cursor();
        let grew = len > self.inner.last_len.get();
        self.inner.last_len.set(len);
        let own = self.inner.own_edit.replace(false);
        {
            let mut snip = self.inner.snippet.borrow_mut();
            if let Some(s) = snip.as_mut()
                && !s.update(len, caret)
            {
                *snip = None;
            }
        }
        self.hide_ghost();
        self.schedule_diagnostics();
        self.schedule_preview();
        if !field.has_focus() {
            self.dismiss();
            return;
        }
        let ident_before = text
            .chars()
            .nth(caret.wrapping_sub(1))
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':');
        if self.popup_open() {
            self.refresh_completions(false);
        } else if grew && !own && ident_before && self.features().popup {
            self.refresh_completions(false);
        }
        self.refresh_hint();
    }

    fn on_cursor_moved(&self) {
        if !self.inner.field.has_focus() {
            return;
        }
        if self.popup_open() {
            let caret = self.inner.field.cursor();
            if caret < self.inner.start.get() {
                self.hide_popup();
            } else {
                self.refresh_completions(false);
            }
        }
        self.refresh_hint();
    }

    // ----------------------------------------------------------- keys

    /// Handles a key press first (call it from the editor's capture
    /// controller before its own bindings). Returns `Stop` when consumed.
    pub fn on_key(&self, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        let stop = glib::Propagation::Stop;
        let action = text_action(key, mods);
        if self.popup_open() {
            match action {
                Some(TextAction::Up) => self.move_selection(-1),
                Some(TextAction::Down) => self.move_selection(1),
                Some(TextAction::PageUp) => self.move_selection(-8),
                Some(TextAction::PageDown) => self.move_selection(8),
                Some(TextAction::Accept | TextAction::NextField | TextAction::PrevField) => {
                    self.accept_selected();
                }
                Some(TextAction::Dismiss) => self.hide_popup(),
                _ => return self.on_key_closed(key, mods, action),
            }
            return stop;
        }
        self.on_key_closed(key, mods, action)
    }

    /// [`Assist::on_key`] when the completion popup is closed.
    fn on_key_closed(
        &self,
        key: gdk::Key,
        mods: gdk::ModifierType,
        action: Option<TextAction>,
    ) -> glib::Propagation {
        use gdk::Key;
        let stop = glib::Propagation::Stop;
        let go = glib::Propagation::Proceed;
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = mods.contains(gdk::ModifierType::ALT_MASK)
            || mods.contains(gdk::ModifierType::SUPER_MASK);
        match action {
            Some(TextAction::Complete) => {
                self.refresh_completions(true);
                return stop;
            }
            Some(TextAction::QuickFix) => {
                if !self.quick_fix() {
                    self.inner.field.widget().error_bell();
                }
                return stop;
            }
            _ => {}
        }
        if ctrl || alt {
            return go;
        }
        match key {
            _ if matches!(action, Some(TextAction::NextField | TextAction::PrevField))
                && self.inner.snippet.borrow().is_some() =>
            {
                self.snippet_jump(action == Some(TextAction::PrevField));
                return stop;
            }
            Key::Tab if self.has_fragment() => {
                self.refresh_completions(true);
                let only = self.inner.items.borrow().len() == 1;
                if only {
                    self.accept(0);
                }
                return stop;
            }
            _ if action == Some(TextAction::Dismiss)
                && (self.inner.snippet.borrow().is_some() || self.hint_open()) =>
            {
                self.inner.snippet.borrow_mut().take();
                self.hide_hint();
                return stop;
            }
            Key::BackSpace if self.features().pairs => {
                let f = &self.inner.field;
                if let Some(e) = edit::on_backspace(&f.text(), f.cursor(), f.selection()) {
                    self.apply_edit(&e);
                    return stop;
                }
            }
            _ => {}
        }
        if let Some(ch) = key.to_unicode()
            && matches!(ch, '(' | '[' | '{' | ')' | ']' | '}' | '|')
            && self.features().pairs
        {
            let f = &self.inner.field;
            if let Some(e) = edit::on_char(&f.text(), f.cursor(), f.selection(), ch) {
                // A closer typed over the auto-inserted one ends the
                // snippet's last placeholder naturally.
                self.apply_edit(&e);
                return stop;
            }
        }
        go
    }

    fn has_fragment(&self) -> bool {
        let text = self.inner.field.text();
        let caret = char_to_byte(&text, self.inner.field.cursor());
        let start = viscas_engine::completion_start(&text[..caret]);
        start < caret
    }

    fn apply_edit(&self, e: &Edit) {
        let f = &self.inner.field;
        self.inner.own_edit.set(true);
        f.replace(e.start, e.end, &e.text);
        match e.select {
            Some((s, t)) => f.select(s, t),
            None => f.set_cursor(e.caret),
        }
        if e.text.is_empty() && e.start == e.end {
            // Stepped over a closer: no change signal follows.
            self.inner.own_edit.set(false);
            self.refresh_hint();
        }
    }

    // ---------------------------------------------------- completion

    fn refresh_completions(&self, force: bool) {
        let field = &self.inner.field;
        if !field.widget().is_mapped() {
            return;
        }
        let text = field.text();
        let caret = char_to_byte(&text, field.cursor());
        let scope = self.inner.scope;
        let (set, all) = self.inner.ctx.engine().with(|e| {
            let set = e.completions(&text, caret, scope);
            let all = (force && set.fragment.is_empty()).then(|| e.completion_candidates());
            (set, all)
        });
        let items = all.unwrap_or(set.items);
        let ranked = rank::rank(&set.fragment, items);
        if ranked.is_empty() || (!force && rank::is_redundant(&set.fragment, &ranked)) {
            self.hide_popup();
            if force && ranked.is_empty() {
                field.widget().error_bell();
            }
            return;
        }
        self.inner.start.set(byte_to_char(&text, set.start));
        self.show_popup(ranked);
    }

    fn ensure_popup(&self) {
        if self.inner.popup.borrow().is_some() {
            return;
        }
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Browse)
            .can_focus(false)
            .activate_on_single_click(true)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(240)
            .min_content_width(320)
            .child(&list)
            .build();
        let doc = gtk::Label::builder()
            .use_markup(true)
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .xalign(0.0)
            .yalign(0.0)
            .valign(gtk::Align::Start)
            .width_chars(30)
            .max_width_chars(34)
            .css_classes(["assist-doc"])
            .build();
        let row = gtk::Box::builder().spacing(0).build();
        row.append(&scroll);
        row.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        row.append(&doc);
        let popover = gtk::Popover::builder()
            .autohide(false)
            .has_arrow(false)
            .can_focus(false)
            .position(match self.inner.scope {
                Scope::Console => gtk::PositionType::Top,
                Scope::Expression => gtk::PositionType::Bottom,
            })
            .css_classes(["completion", "assist-popup"])
            .child(&row)
            .build();
        popover.set_parent(&self.inner.host);
        let weak = self.downgrade();
        list.connect_row_activated(move |_, row| {
            if let Some(a) = Assist::from_weak(&weak) {
                a.accept(row.index().max(0) as usize);
                a.inner.field.widget().grab_focus();
            }
        });
        let weak = self.downgrade();
        list.connect_row_selected(move |_, row| {
            if let (Some(a), Some(row)) = (Assist::from_weak(&weak), row) {
                a.show_doc(row.index().max(0) as usize);
            }
        });
        *self.inner.popup.borrow_mut() = Some(Popup {
            popover,
            list,
            scroll,
            doc,
        });
    }

    fn show_popup(&self, ranked: Vec<Ranked>) {
        self.ensure_popup();
        self.hide_hint();
        let popup = self.inner.popup.borrow();
        let Some(p) = popup.as_ref() else {
            return;
        };
        p.list.remove_all();
        for r in &ranked {
            p.list.append(&completion_row(r));
        }
        *self.inner.items.borrow_mut() = ranked;
        if let Some(row) = p.list.row_at_index(0) {
            p.list.select_row(Some(&row));
        }
        self.show_doc(0);
        if let Some(rect) = self
            .inner
            .field
            .caret_rect(self.inner.start.get(), &self.inner.host)
        {
            p.popover.set_pointing_to(Some(&rect));
        }
        if !p.popover.is_visible() {
            p.popover.popup();
        }
    }

    fn show_doc(&self, index: usize) {
        let popup = self.inner.popup.borrow();
        let Some(p) = popup.as_ref() else {
            return;
        };
        if let Some(r) = self.inner.items.borrow().get(index) {
            p.doc.set_markup(&markup::doc_markup(&r.item));
        }
    }

    fn hide_popup(&self) {
        if let Some(p) = self.inner.popup.borrow().as_ref() {
            p.popover.popdown();
        }
        self.inner.items.borrow_mut().clear();
    }

    fn move_selection(&self, delta: i32) {
        let popup = self.inner.popup.borrow();
        let Some(p) = popup.as_ref() else {
            return;
        };
        let n = self.inner.items.borrow().len() as i32;
        if n == 0 {
            return;
        }
        let cur = p.list.selected_row().map_or(0, |r| r.index());
        let next = if delta.abs() == 1 {
            (cur + delta).rem_euclid(n)
        } else {
            (cur + delta).clamp(0, n - 1)
        };
        if let Some(row) = p.list.row_at_index(next) {
            p.list.select_row(Some(&row));
            let adj = p.scroll.vadjustment();
            if let Some(b) = row.compute_bounds(&p.list) {
                let (top, bottom) = (f64::from(b.y()), f64::from(b.y() + b.height()));
                if top < adj.value() {
                    adj.set_value(top);
                } else if bottom > adj.value() + adj.page_size() {
                    adj.set_value(bottom - adj.page_size());
                }
            }
        }
    }

    fn accept_selected(&self) {
        let i = self
            .inner
            .popup
            .borrow()
            .as_ref()
            .and_then(|p| p.list.selected_row())
            .map_or(0, |r| r.index().max(0) as usize);
        self.accept(i);
    }

    fn accept(&self, index: usize) {
        let Some(r) = self.inner.items.borrow().get(index).cloned() else {
            return;
        };
        self.hide_popup();
        let f = &self.inner.field;
        let text = f.text();
        let (e, holes) = edit::accept(&text, self.inner.start.get(), f.cursor(), &r.item);
        let inserted = e.text.chars().count();
        let new_len = text.chars().count() - (e.end - e.start) + inserted;
        self.apply_edit(&e);
        *self.inner.snippet.borrow_mut() = SnippetSession::new(holes, e.start + inserted, new_len);
        self.inner.last_len.set(new_len);
    }

    fn snippet_jump(&self, back: bool) {
        let f = &self.inner.field;
        let mut snip = self.inner.snippet.borrow_mut();
        let Some(s) = snip.as_mut() else {
            return;
        };
        if !s.contains(f.cursor()) {
            *snip = None;
            return;
        }
        if back {
            let (a, b) = s.prev();
            f.select(a, b);
        } else {
            match s.next() {
                Some((a, b)) => f.select(a, b),
                None => {
                    f.set_cursor(s.end);
                    *snip = None;
                }
            }
        }
    }

    // -------------------------------------------------- signature hint

    fn hint_open(&self) -> bool {
        self.inner
            .hint
            .borrow()
            .as_ref()
            .is_some_and(|h| h.popover.is_visible())
    }

    fn refresh_hint(&self) {
        let f = &self.inner.field;
        if self.popup_open() || !f.has_focus() || !f.widget().is_mapped() {
            self.hide_hint();
            return;
        }
        let text = f.text();
        let caret = char_to_byte(&text, f.cursor());
        let help = self
            .inner
            .ctx
            .engine()
            .with(|e| e.signature_at(&text, caret));
        // The problem under the caret, for keyboard users (no hover).
        let problem = {
            let diags = self.inner.diags.borrow();
            (diags.0 == text)
                .then(|| {
                    diags
                        .1
                        .iter()
                        .find(|d| d.span.start <= caret && caret <= d.span.end)
                        .map(markup::diagnostic_markup)
                })
                .flatten()
        };
        let parts: Vec<String> = help
            .as_ref()
            .map(markup::signature_markup)
            .into_iter()
            .chain(problem)
            .collect();
        if parts.is_empty() {
            self.hide_hint();
            return;
        }
        if self.inner.hint.borrow().is_none() {
            let label = gtk::Label::builder()
                .use_markup(true)
                .xalign(0.0)
                .wrap(true)
                .max_width_chars(60)
                .css_classes(["assist-hint-text"])
                .build();
            let popover = gtk::Popover::builder()
                .autohide(false)
                .has_arrow(false)
                .can_focus(false)
                .can_target(false)
                .position(match self.inner.scope {
                    Scope::Console => gtk::PositionType::Top,
                    Scope::Expression => gtk::PositionType::Bottom,
                })
                .css_classes(["assist-hint"])
                .child(&label)
                .build();
            popover.set_parent(&self.inner.host);
            *self.inner.hint.borrow_mut() = Some(Hint { popover, label });
        }
        let hint = self.inner.hint.borrow();
        let Some(h) = hint.as_ref() else {
            return;
        };
        h.label.set_markup(&parts.join("\n"));
        let open = byte_to_char(&text, help.as_ref().map_or(caret, |h| h.open));
        if let Some(rect) = f.caret_rect(open, &self.inner.host) {
            h.popover.set_pointing_to(Some(&rect));
        }
        if !h.popover.is_visible() {
            h.popover.popup();
        }
    }

    fn hide_hint(&self) {
        if let Some(h) = self.inner.hint.borrow().as_ref() {
            h.popover.popdown();
        }
    }

    // ----------------------------------------------------- diagnostics

    fn schedule_diagnostics(&self) {
        if let Some(id) = self.inner.diag_timer.take() {
            id.remove();
        }
        let weak = self.downgrade();
        let id = glib::timeout_add_local_once(DEBOUNCE, move || {
            if let Some(a) = Assist::from_weak(&weak) {
                a.inner.diag_timer.take();
                a.run_diagnostics();
            }
        });
        *self.inner.diag_timer.borrow_mut() = Some(id);
    }

    fn run_diagnostics(&self) {
        let f = &self.inner.field;
        let text = f.text();
        let diags = if self.features().diagnostics {
            let scope = self.inner.scope;
            let all = self
                .inner
                .ctx
                .engine()
                .with(|e| e.diagnostics(&text, scope));
            let typing = f.has_focus();
            let caret = char_to_byte(&text, f.cursor());
            all.into_iter()
                .filter(|d| {
                    // Not what is still being typed at the caret.
                    !(typing
                        && (d.at_end(&text) && caret >= text.trim_end().len()
                            || d.severity == Severity::Warning && d.span.end == caret))
                })
                .collect()
        } else {
            Vec::new()
        };
        *self.inner.diags.borrow_mut() = (text, diags);
        self.render_marks();
        if f.has_focus() {
            self.refresh_hint();
        }
    }

    fn render_marks(&self) {
        let text = self.inner.field.text();
        let diags = self.inner.diags.borrow();
        let marks: Vec<(usize, usize, Severity)> = if diags.0 == text {
            diags
                .1
                .iter()
                .map(|d| (d.span.start, d.span.end.max(d.span.start + 1), d.severity))
                .map(|(s, e, sev)| {
                    // Zero-width / end-of-input spans mark the last char.
                    let e = e.min(text.len());
                    let s = if s >= e {
                        text[..e]
                            .char_indices()
                            .last()
                            .map_or(0, |(i, _)| i)
                    } else {
                        s
                    };
                    (s, e, sev)
                })
                .collect()
        } else {
            Vec::new()
        };
        let error = self
            .inner
            .ctx
            .preferences()
            .with(|p| p.effective_theme().console.error);
        self.inner
            .field
            .set_marks(&marks, &self.inner.external.borrow(), error, WARNING);
    }

    /// Applies the first suggestion of the diagnostic at the caret (or
    /// the first fixable one). `false` when there is nothing to fix.
    pub fn quick_fix(&self) -> bool {
        let f = &self.inner.field;
        let text = f.text();
        let scope = self.inner.scope;
        let diags = self
            .inner
            .ctx
            .engine()
            .with(|e| e.diagnostics(&text, scope));
        let caret = char_to_byte(&text, f.cursor());
        let fixable: Vec<&Diagnostic> = diags.iter().filter(|d| !d.suggestions.is_empty()).collect();
        let Some(d) = fixable
            .iter()
            .find(|d| d.span.start <= caret && caret <= d.span.end)
            .or(fixable.first())
        else {
            return false;
        };
        let (s, e) = (byte_to_char(&text, d.span.start), byte_to_char(&text, d.span.end));
        let fix = &d.suggestions[0];
        self.apply_edit(&Edit {
            start: s,
            end: e,
            text: fix.clone(),
            caret: s + fix.chars().count(),
            select: None,
        });
        true
    }

    // ---------------------------------------------------------- hover

    fn query_tooltip(&self, x: f64, y: f64, tooltip: &gtk::Tooltip) -> bool {
        let f = &self.inner.field;
        let Some(ch) = f.char_at(x, y) else {
            return false;
        };
        let text = f.text();
        let byte = char_to_byte(&text, ch);
        {
            let diags = self.inner.diags.borrow();
            if diags.0 == text
                && let Some(d) = diags
                    .1
                    .iter()
                    .find(|d| d.span.start <= byte && byte < d.span.end.max(d.span.start + 1))
            {
                tooltip.set_markup(Some(&markup::diagnostic_markup(d)));
                return true;
            }
        }
        hover_name(&self.inner.ctx, &text, byte, tooltip)
    }

    // -------------------------------------------------------- preview

    fn hide_ghost(&self) {
        if let Some(g) = self.inner.ghost.borrow().as_ref() {
            g.label.set_visible(false);
        }
    }

    fn schedule_preview(&self) {
        if self.inner.ghost.borrow().is_none() {
            return;
        }
        if let Some(id) = self.inner.preview_timer.take() {
            id.remove();
        }
        if let Some(c) = self.inner.preview_cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.inner.generation.set(self.inner.generation.get() + 1);
        let weak = self.downgrade();
        let id = glib::timeout_add_local_once(DEBOUNCE, move || {
            if let Some(a) = Assist::from_weak(&weak) {
                a.inner.preview_timer.take();
                a.run_preview();
            }
        });
        *self.inner.preview_timer.borrow_mut() = Some(id);
    }

    fn run_preview(&self) {
        let f = &self.inner.field;
        if !self.features().preview || !f.has_focus() {
            return;
        }
        let text = f.text();
        let Some(task) = self.inner.ctx.engine().with(|e| e.preview_task(&text)) else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        *self.inner.preview_cancel.borrow_mut() = Some(cancel.clone());
        let generation = self.inner.generation.get();
        let timeout = cancel.clone();
        glib::timeout_add_local_once(PREVIEW_TIMEOUT, move || {
            timeout.store(true, Ordering::Relaxed);
        });
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || task.run(cancel)).await;
            let Some(a) = Assist::from_weak(&weak) else {
                return;
            };
            if a.inner.generation.get() != generation || a.inner.field.text() != text {
                return;
            }
            if let Ok(Some((exact, approx))) = result {
                a.show_ghost(&preview_text(&text, &exact, &approx));
            }
        });
    }

    fn show_ghost(&self, shown: &str) {
        let ghost = self.inner.ghost.borrow();
        let Some(g) = ghost.as_ref() else {
            return;
        };
        let f = &self.inner.field;
        if shown.is_empty() || !f.has_focus() {
            g.label.set_visible(false);
            return;
        }
        let end = f.text().chars().count();
        let Some(rect) = f.caret_rect(end, &g.overlay.clone().upcast()) else {
            return;
        };
        g.label.set_label(shown);
        let (_, natural, _, _) = g.label.measure(gtk::Orientation::Horizontal, -1);
        let x = rect.x() + 10;
        if x + natural > g.overlay.width() - 4 {
            g.label.set_visible(false);
            return;
        }
        g.label.set_margin_start(x.max(0));
        g.label.set_margin_top(rect.y().max(0));
        g.label.set_visible(true);
    }
}

/// Keyboard actions of math inputs. Every binding goes through
/// [`text_action`], the one place to hand over to a keymap registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAction {
    /// Open the completion popup (Ctrl+Space).
    Complete,
    /// Apply the first suggestion of the problem at the caret (Ctrl+.).
    QuickFix,
    Up,
    Down,
    PageUp,
    PageDown,
    /// Accept the selected completion (Enter; Tab also accepts).
    Accept,
    /// Next snippet placeholder / accept (Tab).
    NextField,
    /// Previous snippet placeholder (Shift+Tab).
    PrevField,
    /// Close the popup, hint or snippet (Escape).
    Dismiss,
}

/// The action `key` with `mods` triggers in a math input.
pub fn text_action(key: gdk::Key, mods: gdk::ModifierType) -> Option<TextAction> {
    use gdk::Key;
    let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
    let alt = mods.contains(gdk::ModifierType::ALT_MASK)
        || mods.contains(gdk::ModifierType::SUPER_MASK);
    let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
    if alt {
        return None;
    }
    if ctrl {
        return match key {
            Key::space | Key::nobreakspace => Some(TextAction::Complete),
            Key::period | Key::KP_Decimal | Key::semicolon => Some(TextAction::QuickFix),
            _ => None,
        };
    }
    match key {
        Key::Up | Key::KP_Up => Some(TextAction::Up),
        Key::Down | Key::KP_Down => Some(TextAction::Down),
        Key::Page_Up => Some(TextAction::PageUp),
        Key::Page_Down => Some(TextAction::PageDown),
        Key::Return | Key::KP_Enter => Some(TextAction::Accept),
        Key::ISO_Left_Tab => Some(TextAction::PrevField),
        Key::Tab if shift => Some(TextAction::PrevField),
        Key::Tab => Some(TextAction::NextField),
        Key::Escape => Some(TextAction::Dismiss),
        _ => None,
    }
}

/// Ghost text for a preview: `= exact`, plus `≈ decimal` when it differs;
/// empty when it only repeats the input.
pub fn preview_text(input: &str, exact: &str, approx: &str) -> String {
    let approx = approx.trim_start_matches('≈').trim();
    if exact.trim() == input.trim() {
        return String::new();
    }
    if approx.is_empty() || approx == exact {
        format!("= {exact}")
    } else {
        format!("= {exact}  ≈ {approx}")
    }
}

fn completion_row(r: &Ranked) -> gtk::ListBoxRow {
    let (badge, class) = markup::kind_badge(r.item.kind, r.item.is_function());
    let row = gtk::Box::builder().spacing(6).build();
    row.append(
        &gtk::Label::builder()
            .label(badge)
            .css_classes(["kind-badge", class])
            .build(),
    );
    row.append(
        &gtk::Label::builder()
            .use_markup(true)
            .label(markup::name_markup(&r.item, &r.positions))
            .xalign(0.0)
            .css_classes(["completion-text"])
            .build(),
    );
    row.append(
        &gtk::Label::builder()
            .label(markup::short_doc(&r.item))
            .xalign(1.0)
            .hexpand(true)
            .ellipsize(pango::EllipsizeMode::End)
            .max_width_chars(24)
            .css_classes(["completion-detail"])
            .build(),
    );
    gtk::ListBoxRow::builder()
        .child(&row)
        .focusable(false)
        .build()
}

/// Tooltip with the docs of the name around byte `at` of `text`.
fn hover_name(ctx: &AppCtx, text: &str, at: usize, tooltip: &gtk::Tooltip) -> bool {
    let Some((s, e)) = words(text).into_iter().find(|(s, e)| *s <= at && at < *e) else {
        return false;
    };
    let mut name = &text[s..e];
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        return false;
    }
    let colon = s > 0 && text[..s].ends_with(':') && text[..s - 1].trim().is_empty();
    let cmd;
    if colon {
        cmd = format!(":{name}");
        name = &cmd;
    }
    match ctx.engine().with(|e| e.describe(name)) {
        Some(item) => {
            tooltip.set_markup(Some(&markup::hover_markup(&item)));
            true
        }
        None => false,
    }
}

/// Hover docs on a label showing math (console transcript echo lines).
pub fn install_label_hover(ctx: &AppCtx, label: &gtk::Label) {
    label.set_has_tooltip(true);
    let ctx = ctx.clone();
    label.connect_query_tooltip(move |label, x, y, keyboard, tooltip| {
        if keyboard {
            return false;
        }
        let layout = label.layout();
        let (ox, oy) = label.layout_offsets();
        let (inside, index, _) = layout.xy_to_index(
            (x - ox) * pango::SCALE,
            (y - oy) * pango::SCALE,
        );
        if !inside {
            return false;
        }
        let text = layout.text();
        hover_name(&ctx, text.as_str(), index.max(0) as usize, tooltip)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_bindings() {
        use gdk::{Key, ModifierType as M};
        assert_eq!(
            text_action(Key::space, M::CONTROL_MASK),
            Some(TextAction::Complete)
        );
        assert_eq!(text_action(Key::space, M::empty()), None, "Space types");
        assert_eq!(
            text_action(Key::period, M::CONTROL_MASK),
            Some(TextAction::QuickFix)
        );
        assert_eq!(
            text_action(Key::Tab, M::SHIFT_MASK),
            Some(TextAction::PrevField)
        );
        assert_eq!(text_action(Key::Tab, M::empty()), Some(TextAction::NextField));
        assert_eq!(text_action(Key::dead_circumflex, M::empty()), None);
        assert_eq!(text_action(Key::Up, M::ALT_MASK), None);
    }

    #[test]
    fn preview_ghost_text() {
        assert_eq!(preview_text("1/4 + 1/4", "1/2", "0.5"), "= 1/2  ≈ 0.5");
        assert_eq!(preview_text("2 + 2", "4", "4"), "= 4");
        assert_eq!(preview_text("2 + 2", "4", "≈4"), "= 4");
        assert_eq!(preview_text("π", "π", "3.14159"), "");
    }
}

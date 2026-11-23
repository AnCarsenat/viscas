//! Console editor: the primary input of the app (REQUIREMENTS §7).
//!
//! A transcript of entries above a multi-line input. Each entry echoes
//! the input (errors underline the offending span), shows results with
//! the exact form and a dim `≈` decimal side by side, "did you mean"
//! chips, and hover actions (copy exact/approx, pin as node, insert).
//!
//! Input: Enter evaluates, Shift+Enter adds a line, Up/Down recall the
//! (persisted) history, Tab completes through a popover, Ctrl+L clears
//! the transcript, Escape clears the input. Space always types: the
//! window's Space-for-palette key ignores focused text views.
//!
//! Pure helpers live in [`render`] (markup), [`history`] (persistence,
//! recall) and [`complete`] (insertion math); [`style`] holds the CSS
//! shared with the Expressions editor.

pub mod complete;
pub mod history;
pub mod render;
pub mod style;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, glib};
use viscas_engine::{Completion, CompletionKind, ConsoleOutput, ItemKind};
use viscas_scene::NodeId;

use crate::ctx::AppCtx;
use crate::doc::{DocEvent, Subscription};
use crate::editors::pick_icon;
use history::HistoryNav;

/// Entries kept in the transcript; older ones are dropped.
const MAX_ENTRIES: usize = 400;
/// Completion candidates shown at once.
const MAX_COMPLETIONS: usize = 60;

thread_local! {
    /// The history file was read into the engine (once per process).
    static HISTORY_LOADED: Cell<bool> = const { Cell::new(false) };
}

/// One transcript entry and the data to re-render it.
struct Entry {
    output: ConsoleOutput,
    widget: gtk::Box,
}

struct Inner {
    ctx: AppCtx,
    root: gtk::Box,
    scroller: gtk::ScrolledWindow,
    transcript: gtk::Box,
    empty: gtk::Box,
    input: gtk::TextView,
    popover: gtk::Popover,
    list: gtk::ListBox,
    completions: RefCell<Vec<Completion>>,
    nav: RefCell<HistoryNav>,
    entries: RefCell<Vec<Entry>>,
    /// Index of the newest entry `:pin` would pin as is.
    last_pinnable: Cell<Option<usize>>,
    /// Header chips (exact/approx, rad/deg) of every built header.
    chips: RefCell<Vec<(glib::WeakRef<gtk::Button>, glib::WeakRef<gtk::Button>)>>,
    /// Programmatic buffer edit in progress (keeps history browsing).
    setting_text: Cell<bool>,
    /// Scroll to the end once the transcript grows.
    stick: Cell<bool>,
    /// Error color the transcript was rendered with.
    error_color: RefCell<String>,
    subs: RefCell<Vec<Subscription>>,
}

/// The console editor. Cloning yields a handle to the same widget.
#[derive(Clone)]
pub struct Console {
    inner: Rc<Inner>,
}

impl Console {
    pub fn new(ctx: &AppCtx) -> Self {
        style::ensure(ctx);
        load_history_once(ctx);

        let transcript = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["transcript"])
            .valign(gtk::Align::End)
            .build();
        let empty = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(3)
            .css_classes(["console-empty"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .vexpand(true)
            .build();
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        column.append(&empty);
        column.append(&transcript);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&column)
            .build();

        let input = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .hexpand(true)
            .css_classes(["console-input"])
            .build();
        input.update_property(&[gtk::accessible::Property::Label("Console input")]);
        let prompt = gtk::Label::builder()
            .label("›")
            .valign(gtk::Align::Start)
            .css_classes(["console-prompt"])
            .build();
        let input_bar = gtk::Box::builder()
            .spacing(6)
            .css_classes(["console-input-bar"])
            .build();
        input_bar.append(&prompt);
        input_bar.append(&input);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Browse)
            .can_focus(false)
            .activate_on_single_click(true)
            .build();
        let list_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(220)
            .child(&list)
            .build();
        let popover = gtk::Popover::builder()
            .autohide(false)
            .has_arrow(false)
            .can_focus(false)
            .position(gtk::PositionType::Top)
            .css_classes(["completion"])
            .child(&list_scroll)
            .build();
        popover.set_parent(&input);
        input.connect_destroy(glib::clone!(
            #[weak]
            popover,
            move |_| popover.unparent()
        ));

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["console"])
            .build();
        root.append(&scroller);
        root.append(&input_bar);

        let console = Self {
            inner: Rc::new(Inner {
                ctx: ctx.clone(),
                root,
                scroller,
                transcript,
                empty,
                input,
                popover,
                list,
                completions: RefCell::default(),
                nav: RefCell::default(),
                entries: RefCell::default(),
                last_pinnable: Cell::new(None),
                chips: RefCell::default(),
                setting_text: Cell::new(false),
                stick: Cell::new(false),
                error_color: RefCell::new(error_color(ctx)),
                subs: RefCell::default(),
            }),
        };
        console.build_empty_state();
        console.connect_input(&input_bar);
        console.connect_scrolling();
        console.connect_updates();
        console
    }

    pub fn widget(&self) -> gtk::Widget {
        self.inner.root.clone().upcast()
    }

    fn downgrade(&self) -> Weak<Inner> {
        Rc::downgrade(&self.inner)
    }

    fn from_weak(weak: &Weak<Inner>) -> Option<Console> {
        weak.upgrade().map(|inner| Console { inner })
    }

    // ----------------------------------------------------------- header

    /// Header widgets: mode chips mirroring the engine settings (synced
    /// with the status bar) and a Clear button. Built fresh per header.
    pub fn header_content(&self) -> Vec<gtk::Widget> {
        let chip = |tip: &str| {
            gtk::Button::builder()
                .tooltip_text(tip)
                .css_classes(["chip", "flat"])
                .valign(gtk::Align::Center)
                .build()
        };
        let mode = chip("Exact or decimal results");
        let angle = chip("Angle unit");
        let ctx = self.inner.ctx.downgrade();
        mode.connect_clicked(glib::clone!(
            #[strong]
            ctx,
            move |_| {
                if let Some(ctx) = ctx.upgrade() {
                    let e = ctx.engine();
                    e.set_settings_deferred(crate::doc_ui::toggled_approx(e.effective_settings()));
                }
            }
        ));
        angle.connect_clicked(move |_| {
            if let Some(ctx) = ctx.upgrade() {
                let e = ctx.engine();
                e.set_settings_deferred(crate::doc_ui::toggled_angle(e.effective_settings()));
            }
        });
        let settings = self.inner.ctx.engine().effective_settings();
        update_mode_chips(&mode, &angle, &settings);
        {
            let mut chips = self.inner.chips.borrow_mut();
            chips.retain(|(m, _)| m.upgrade().is_some());
            chips.push((mode.downgrade(), angle.downgrade()));
        }
        let clear = gtk::Button::builder()
            .icon_name(pick_icon(&[
                "edit-clear-all-symbolic",
                "edit-clear-symbolic",
            ]))
            .tooltip_text("Clear transcript (Ctrl+L) — `:clear` also forgets console definitions")
            .css_classes(["area-tool"])
            .build();
        let weak = self.downgrade();
        clear.connect_clicked(move |_| {
            if let Some(c) = Console::from_weak(&weak) {
                c.clear_transcript();
                c.inner.input.grab_focus();
            }
        });
        vec![mode.upcast(), angle.upcast(), clear.upcast()]
    }

    // ------------------------------------------------------ empty state

    fn build_empty_state(&self) {
        let empty = &self.inner.empty;
        while let Some(child) = empty.first_child() {
            empty.remove(&child);
        }
        let op = definition_op(&self.inner.ctx);
        let title = gtk::Label::builder()
            .label("Console")
            .css_classes(["placeholder-title"])
            .build();
        let desc = gtk::Label::builder()
            .label(
                "Expressions, definitions and relations. Tab completes, ↑ recalls, \
                 Shift+Enter adds a line.",
            )
            .wrap(true)
            .max_width_chars(44)
            .justify(gtk::Justification::Center)
            .css_classes(["console-info"])
            .build();
        let quip = gtk::Label::builder()
            .label("Ask it anything. It prefers exact answers.")
            .css_classes(["console-hint"])
            .visible(self.inner.ctx.preferences().with(|p| p.interface.playful))
            .build();
        let weak_quip = quip.downgrade();
        self.inner
            .ctx
            .preferences()
            .connect_changed(move |p| match weak_quip.upgrade() {
                Some(q) => {
                    q.set_visible(p.interface.playful);
                    true
                }
                None => false,
            });
        let tries = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(6)
            .halign(gtk::Align::Center)
            .margin_top(8)
            .column_spacing(4)
            .row_spacing(4)
            .build();
        let examples = [
            "2^100".to_owned(),
            format!("f(x) {op} x^2 - 2"),
            "solve(f(x) = 0, x)".to_owned(),
            "y = sin x".to_owned(),
            ":help".to_owned(),
        ];
        for ex in examples {
            let b = gtk::Button::builder()
                .label(ex.as_str())
                .css_classes(["console-chip"])
                .tooltip_text("Put into the input")
                .build();
            let weak = self.downgrade();
            b.connect_clicked(move |_| {
                if let Some(c) = Console::from_weak(&weak) {
                    c.set_input(&ex, None);
                    c.inner.input.grab_focus();
                }
            });
            tries.append(&b);
        }
        empty.append(&title);
        empty.append(&desc);
        empty.append(&quip);
        empty.append(&tries);
        self.update_empty();
    }

    fn update_empty(&self) {
        let has = !self.inner.entries.borrow().is_empty();
        self.inner.empty.set_visible(!has);
        self.inner.transcript.set_visible(has);
    }

    // ------------------------------------------------------------ input

    fn connect_input(&self, input_bar: &gtk::Box) {
        let input = &self.inner.input;
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        keys.connect_key_pressed(move |_, key, _, mods| match Console::from_weak(&weak) {
            Some(c) => c.on_key(key, mods),
            None => glib::Propagation::Proceed,
        });
        input.add_controller(keys);

        let weak = self.downgrade();
        input.buffer().connect_changed(move |_| {
            if let Some(c) = Console::from_weak(&weak) {
                if !c.inner.setting_text.get() {
                    c.inner.nav.borrow_mut().reset();
                }
                if c.inner.popover.is_visible() {
                    c.refresh_completions();
                }
            }
        });

        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(glib::clone!(
            #[weak]
            input_bar,
            move |_| input_bar.add_css_class("focused")
        ));
        let weak = self.downgrade();
        focus.connect_leave(glib::clone!(
            #[weak]
            input_bar,
            move |_| {
                input_bar.remove_css_class("focused");
                if let Some(c) = Console::from_weak(&weak) {
                    c.inner.popover.popdown();
                }
            }
        ));
        input.add_controller(focus);

        // Clicking the empty transcript area focuses the input.
        let click = gtk::GestureClick::new();
        let weak = self.downgrade();
        click.connect_released(move |_, _, _, _| {
            if let Some(c) = Console::from_weak(&weak) {
                c.inner.input.grab_focus();
            }
        });
        input_bar.add_controller(click);

        let weak = self.downgrade();
        self.inner.list.connect_row_activated(move |_, row| {
            if let Some(c) = Console::from_weak(&weak) {
                c.accept_completion(row.index().max(0) as usize);
                c.inner.input.grab_focus();
            }
        });
    }

    fn on_key(&self, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        use gdk::Key;
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let stop = glib::Propagation::Stop;
        if self.inner.popover.is_visible() {
            match key {
                Key::Up | Key::Down => {
                    self.move_completion(if key == Key::Up { -1 } else { 1 });
                    return stop;
                }
                Key::Tab | Key::Return | Key::KP_Enter | Key::ISO_Left_Tab => {
                    let i = self
                        .inner
                        .list
                        .selected_row()
                        .map_or(0, |r| r.index().max(0) as usize);
                    self.accept_completion(i);
                    return stop;
                }
                Key::Escape => {
                    self.inner.popover.popdown();
                    return stop;
                }
                _ => {}
            }
        }
        match key {
            Key::Return | Key::KP_Enter if !shift => {
                self.evaluate();
                stop
            }
            Key::Tab if !ctrl && !shift => {
                self.complete();
                stop
            }
            Key::Up if !ctrl && self.cursor_on_first_line() => {
                self.history_up();
                stop
            }
            Key::Down if !ctrl && self.cursor_on_last_line() => {
                self.history_down();
                stop
            }
            Key::Escape if !self.input_text().is_empty() => {
                self.set_input("", None);
                self.inner.nav.borrow_mut().reset();
                stop
            }
            Key::l | Key::L if ctrl => {
                self.clear_transcript();
                stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn input_text(&self) -> String {
        let b = self.inner.input.buffer();
        b.text(&b.start_iter(), &b.end_iter(), false).to_string()
    }

    /// Cursor position in bytes.
    fn cursor_byte(&self) -> usize {
        let b = self.inner.input.buffer();
        complete::char_to_byte(&self.input_text(), b.cursor_position().max(0) as usize)
    }

    /// Replaces the input; the cursor goes to byte `cursor` (end if `None`).
    fn set_input(&self, text: &str, cursor: Option<usize>) {
        let b = self.inner.input.buffer();
        self.inner.setting_text.set(true);
        b.set_text(text);
        self.inner.setting_text.set(false);
        let chars = complete::byte_to_char(text, cursor.unwrap_or(text.len()));
        let iter = b.iter_at_offset(chars as i32);
        b.place_cursor(&iter);
    }

    fn cursor_iter(&self) -> gtk::TextIter {
        let b = self.inner.input.buffer();
        b.iter_at_mark(&b.get_insert())
    }

    fn cursor_on_first_line(&self) -> bool {
        // Display lines (wrapped) count, like a terminal.
        let mut it = self.cursor_iter();
        if !self.inner.input.starts_display_line(&it) {
            self.inner.input.backward_display_line_start(&mut it);
        }
        it.offset() == 0
    }

    fn cursor_on_last_line(&self) -> bool {
        let mut it = self.cursor_iter();
        if it.is_end() {
            return true;
        }
        self.inner.input.forward_display_line_end(&mut it);
        it.is_end()
    }

    fn history_up(&self) {
        let current = self.input_text();
        let next = self.inner.ctx.engine().with(|e| {
            let mut nav = self.inner.nav.borrow_mut();
            nav.up(e.history(), &current)
        });
        if let Some(text) = next {
            self.set_input(&text, None);
        }
    }

    fn history_down(&self) {
        let next = self.inner.ctx.engine().with(|e| {
            let mut nav = self.inner.nav.borrow_mut();
            nav.down(e.history())
        });
        if let Some(text) = next {
            self.set_input(&text, None);
        }
    }

    // ------------------------------------------------------- evaluation

    fn evaluate(&self) {
        let text = self.input_text();
        if text.trim().is_empty() {
            return;
        }
        self.inner.popover.popdown();
        let out = self.inner.ctx.engine().edit(|e| e.console_eval(&text));
        self.after_eval(out);
        self.set_input("", None);
        self.inner.nav.borrow_mut().reset();
    }

    fn after_eval(&self, out: ConsoleOutput) {
        save_history(&self.inner.ctx);
        if out.cleared {
            self.clear_transcript();
            return;
        }
        let pinnable = is_pinnable(&out);
        self.append(out);
        let last = self.inner.entries.borrow().len().checked_sub(1);
        self.inner
            .last_pinnable
            .set(if pinnable { last } else { None });
    }

    /// Pins entry `index` as a node: `:pin` directly when it is the last
    /// pinnable input, else re-evaluating its input first. The history is
    /// left as it was.
    fn pin_entry(&self, index: usize) {
        let Some(input) = self
            .inner
            .entries
            .borrow()
            .get(index)
            .map(|e| e.output.input_echo.clone())
        else {
            return;
        };
        let direct = self.inner.last_pinnable.get() == Some(index);
        let out = self.inner.ctx.engine().edit(|e| {
            let history = e.history().to_vec();
            let mut out = None;
            if !direct {
                let again = e.console_eval(&input);
                if again.error.is_some() {
                    out = Some(again);
                }
            }
            let out = out.unwrap_or_else(|| e.console_eval(":pin"));
            e.set_history(history);
            out
        });
        let mut out = out;
        out.input_echo = format!(":pin  {}", first_line(&input));
        self.append(out);
        self.inner.last_pinnable.set(None);
    }

    // ------------------------------------------------------- transcript

    fn append(&self, output: ConsoleOutput) {
        let index = self.inner.entries.borrow().len();
        let widget = self.build_entry(&output, index);
        self.inner.transcript.append(&widget);
        self.inner
            .entries
            .borrow_mut()
            .push(Entry { output, widget });
        // Drop the oldest entries beyond the cap (indices shift).
        let excess = self
            .inner
            .entries
            .borrow()
            .len()
            .saturating_sub(MAX_ENTRIES);
        if excess > 0 {
            let removed: Vec<Entry> = self.inner.entries.borrow_mut().drain(..excess).collect();
            for e in removed {
                self.inner.transcript.remove(&e.widget);
            }
            self.inner.last_pinnable.set(
                self.inner
                    .last_pinnable
                    .get()
                    .and_then(|i| i.checked_sub(excess)),
            );
            self.rebuild_entries();
        }
        self.update_empty();
        self.scroll_to_end();
    }

    fn clear_transcript(&self) {
        let entries = std::mem::take(&mut *self.inner.entries.borrow_mut());
        for e in entries {
            self.inner.transcript.remove(&e.widget);
        }
        self.inner.last_pinnable.set(None);
        self.update_empty();
    }

    /// Re-renders every entry (theme change, index shift).
    fn rebuild_entries(&self) {
        let outputs: Vec<ConsoleOutput> = self
            .inner
            .entries
            .borrow()
            .iter()
            .map(|e| e.output.clone())
            .collect();
        let old = std::mem::take(&mut *self.inner.entries.borrow_mut());
        for e in old {
            self.inner.transcript.remove(&e.widget);
        }
        for (i, output) in outputs.into_iter().enumerate() {
            let widget = self.build_entry(&output, i);
            self.inner.transcript.append(&widget);
            self.inner
                .entries
                .borrow_mut()
                .push(Entry { output, widget });
        }
    }

    fn connect_scrolling(&self) {
        let adj = self.inner.scroller.vadjustment();
        let weak = self.downgrade();
        adj.connect_changed(move |adj| {
            if let Some(c) = Console::from_weak(&weak)
                && c.inner.stick.get()
            {
                adj.set_value(adj.upper() - adj.page_size());
            }
        });
    }

    fn scroll_to_end(&self) {
        self.inner.stick.set(true);
        let weak = self.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
            if let Some(c) = Console::from_weak(&weak) {
                let adj = c.inner.scroller.vadjustment();
                adj.set_value(adj.upper() - adj.page_size());
                c.inner.stick.set(false);
            }
        });
    }

    fn build_entry(&self, out: &ConsoleOutput, index: usize) -> gtk::Box {
        let error_color = self.inner.error_color.borrow().clone();
        let entry = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(1)
            .css_classes(["console-entry"])
            .build();

        // Echo line with hover actions on the right.
        let head = gtk::Box::builder().spacing(6).build();
        let prompt = gtk::Label::builder()
            .label("›")
            .valign(gtk::Align::Start)
            .css_classes(["console-prompt"])
            .build();
        let echo = gtk::Label::builder()
            .use_markup(true)
            .label(render::underline_span(
                &out.input_echo,
                out.error.as_ref().and_then(|e| e.span),
                &error_color,
            ))
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .hexpand(true)
            .css_classes(["console-echo"])
            .build();
        // Selectable labels would take focus on click; keep it in the input.
        echo.set_focusable(false);
        head.append(&prompt);
        head.append(&echo);
        let actions = self.build_actions(out, index);
        head.append(&actions);
        entry.append(&head);

        for item in &out.results {
            entry.append(&self.build_item(item));
        }
        if let Some(err) = &out.error {
            let label = gtk::Label::builder()
                .label(&err.message)
                .xalign(0.0)
                .wrap(true)
                .margin_start(16)
                .css_classes(["console-error"])
                .build();
            entry.append(&label);
        }
        if !out.suggestions.is_empty() {
            let row = gtk::Box::builder().spacing(4).margin_start(16).build();
            row.append(
                &gtk::Label::builder()
                    .label("did you mean")
                    .css_classes(["console-hint"])
                    .build(),
            );
            for s in &out.suggestions {
                let chip = gtk::Button::builder()
                    .label(s.as_str())
                    .css_classes(["console-chip"])
                    .tooltip_text("Fix the input with this name")
                    .build();
                let fixed = complete::apply_suggestion(
                    &out.input_echo,
                    out.error.as_ref().and_then(|e| e.span),
                    s,
                );
                let weak = self.downgrade();
                chip.connect_clicked(move |_| {
                    if let Some(c) = Console::from_weak(&weak) {
                        c.set_input(&fixed, None);
                        c.inner.input.grab_focus();
                    }
                });
                row.append(&chip);
            }
            entry.append(&row);
        }

        // Hover: reveal actions, highlight the node the entry refers to.
        let motion = gtk::EventControllerMotion::new();
        let weak = self.downgrade();
        motion.connect_enter(glib::clone!(
            #[weak]
            entry,
            #[weak]
            actions,
            move |_, _, _| {
                entry.add_css_class("hovered");
                actions.set_opacity(1.0);
                actions.set_can_target(true);
                if let Some(c) = Console::from_weak(&weak) {
                    let node = c.entry_node(index);
                    c.inner.ctx.selection().set_hovered(node);
                }
            }
        ));
        let weak = self.downgrade();
        motion.connect_leave(glib::clone!(
            #[weak]
            entry,
            #[weak]
            actions,
            move |_| {
                entry.remove_css_class("hovered");
                actions.set_opacity(0.0);
                actions.set_can_target(false);
                if let Some(c) = Console::from_weak(&weak) {
                    let sel = c.inner.ctx.selection();
                    let node = c.entry_node(index);
                    if node.is_some() && sel.hovered() == node {
                        sel.set_hovered(None);
                    }
                }
            }
        ));
        entry.add_controller(motion);
        entry
    }

    fn build_item(&self, item: &viscas_engine::ConsoleItem) -> gtk::Widget {
        let row = gtk::Box::builder().spacing(6).margin_start(16).build();
        let tag = |text: &str, node: bool| {
            let l = gtk::Label::builder()
                .label(text)
                .valign(gtk::Align::Center)
                .css_classes(["console-tag"])
                .build();
            if node {
                l.add_css_class("node");
            }
            l
        };
        let value = |prefix: Option<&str>| {
            let mut markup = String::new();
            if let Some(p) = prefix {
                markup.push_str(&format!("<b>{}</b> = ", render::escape(p)));
            }
            markup.push_str(&render::math_markup(&item.exact));
            gtk::Label::builder()
                .use_markup(true)
                .label(markup)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .selectable(true)
                .focusable(false)
                .css_classes(["console-result"])
                .build()
        };
        let arrow = || {
            gtk::Label::builder()
                .label("→")
                .valign(gtk::Align::Center)
                .css_classes(["console-arrow"])
                .build()
        };
        let mut show_value = true;
        match &item.kind {
            ItemKind::Value => {
                row.append(&arrow());
                row.append(&value(None));
            }
            ItemKind::Definition { name } => {
                row.append(&arrow());
                row.append(&value(Some(name)));
                row.append(&tag("console", false));
            }
            ItemKind::UpdatedNode { name, .. } => {
                row.append(&arrow());
                row.append(&value(Some(name)));
                row.append(&tag("node updated", true));
            }
            ItemKind::Pinned { node } => {
                let title = self
                    .inner
                    .ctx
                    .engine()
                    .with(|e| e.scene().get(*node).map(|n| n.title()))
                    .unwrap_or_else(|| "node".into());
                row.append(&tag("pinned", true));
                row.append(&value(Some(&title)));
                if item.exact.is_empty() {
                    show_value = false;
                }
            }
            ItemKind::Variable { name, scene } => {
                row.append(&value(Some(name)));
                row.append(&tag(if *scene { "node" } else { "console" }, *scene));
            }
            ItemKind::Relation { kind } => {
                row.append(&tag(kind, false));
                row.append(
                    &gtk::Label::builder()
                        .label("relation — pin it to plot")
                        .css_classes(["console-hint"])
                        .build(),
                );
                show_value = false;
            }
            ItemKind::Info => {
                row.append(
                    &gtk::Label::builder()
                        .use_markup(true)
                        .label(render::info_markup(&item.exact))
                        .xalign(0.0)
                        .wrap(true)
                        .selectable(true)
                        .focusable(false)
                        .css_classes(["console-info"])
                        .build(),
                );
                show_value = false;
            }
        }
        if show_value && render::shows_approx(&item.exact, &item.approx) {
            row.append(
                &gtk::Label::builder()
                    .label(format!("≈ {}", item.approx.trim_start_matches('≈')))
                    .valign(gtk::Align::Center)
                    .selectable(true)
                    .focusable(false)
                    .css_classes(["console-approx"])
                    .build(),
            );
        }
        row.upcast()
    }

    fn build_actions(&self, out: &ConsoleOutput, index: usize) -> gtk::Box {
        let actions = gtk::Box::builder()
            .spacing(0)
            .valign(gtk::Align::Start)
            .css_classes(["console-actions"])
            .opacity(0.0)
            .can_target(false)
            .build();
        let first = out
            .results
            .iter()
            .find(|i| !matches!(i.kind, ItemKind::Info | ItemKind::Relation { .. }));
        let exact = first.map(|i| i.exact.clone()).filter(|s| !s.is_empty());
        let approx = first
            .map(|i| i.approx.trim_start_matches('≈').to_owned())
            .filter(|s| !s.is_empty());
        let button = |icons: &'static [&'static str], tip: &str| {
            gtk::Button::builder()
                .icon_name(pick_icon(icons))
                .tooltip_text(tip)
                .build()
        };
        if let Some(text) = exact.clone() {
            let b = button(&["edit-copy-symbolic"], "Copy exact result");
            b.connect_clicked(move |b| b.clipboard().set_text(&text));
            actions.append(&b);
        }
        if let Some(text) = approx {
            let b = button(
                &["accessories-calculator-symbolic", "edit-copy-symbolic"],
                "Copy decimal (≈) result",
            );
            b.connect_clicked(move |b| b.clipboard().set_text(&text));
            actions.append(&b);
        }
        if is_pinnable(out) {
            let b = button(
                &["view-pin-symbolic", "pin-symbolic", "list-add-symbolic"],
                "Pin as node",
            );
            let weak = self.downgrade();
            b.connect_clicked(move |_| {
                if let Some(c) = Console::from_weak(&weak) {
                    c.pin_entry(index);
                }
            });
            actions.append(&b);
        }
        let insert = exact.unwrap_or_else(|| out.input_echo.clone());
        let b = button(
            &["insert-text-symbolic", "edit-paste-symbolic"],
            "Insert into input",
        );
        let weak = self.downgrade();
        b.connect_clicked(move |_| {
            if let Some(c) = Console::from_weak(&weak) {
                c.insert_at_cursor(&insert);
            }
        });
        actions.append(&b);
        actions
    }

    fn insert_at_cursor(&self, text: &str) {
        let b = self.inner.input.buffer();
        b.insert_at_cursor(text);
        self.inner.input.grab_focus();
    }

    /// The scene node entry `index` refers to: a node it updated, pinned
    /// or listed, else the first scene name in its input.
    fn entry_node(&self, index: usize) -> Option<NodeId> {
        let entries = self.inner.entries.borrow();
        let out = &entries.get(index)?.output;
        self.inner.ctx.engine().with(|e| {
            for item in &out.results {
                match &item.kind {
                    ItemKind::UpdatedNode { node, .. } | ItemKind::Pinned { node } => {
                        return Some(*node);
                    }
                    ItemKind::Variable { name, scene: true } => {
                        if let Some(id) = e.scene().lookup(name) {
                            return Some(id);
                        }
                    }
                    _ => {}
                }
            }
            if out.input_echo.starts_with(':') {
                return None;
            }
            identifiers(&out.input_echo).find_map(|name| e.scene().lookup(name))
        })
    }

    // ------------------------------------------------------- completion

    fn complete(&self) {
        let text = self.input_text();
        let cursor = self.cursor_byte();
        let items = self
            .inner
            .ctx
            .engine()
            .with(|e| e.complete(&text[..cursor]));
        match items.len() {
            0 => self.inner.input.error_bell(),
            1 => {
                let (t, c) = complete::apply(&text, cursor, &items[0]);
                self.set_input(&t, Some(c));
            }
            _ => {
                if let Some((t, c)) = complete::extend_to_common(&text, cursor, &items) {
                    self.set_input(&t, Some(c));
                }
                self.show_completions(items);
            }
        }
    }

    fn refresh_completions(&self) {
        let text = self.input_text();
        let cursor = self.cursor_byte();
        let items = self
            .inner
            .ctx
            .engine()
            .with(|e| e.complete(&text[..cursor]));
        if items.is_empty() {
            self.inner.popover.popdown();
        } else {
            self.show_completions(items);
        }
    }

    fn show_completions(&self, mut items: Vec<Completion>) {
        // A detached editor (other workspace) has no surface to pop up on.
        if !self.inner.input.is_mapped() {
            return;
        }
        items.truncate(MAX_COMPLETIONS);
        let list = &self.inner.list;
        list.remove_all();
        for c in &items {
            list.append(&completion_row(c));
        }
        *self.inner.completions.borrow_mut() = items;
        if let Some(row) = list.row_at_index(0) {
            list.select_row(Some(&row));
        }
        // Point at the cursor.
        let input = &self.inner.input;
        let rect = input.iter_location(&self.cursor_iter());
        let (x, y) = input.buffer_to_window_coords(gtk::TextWindowType::Widget, rect.x(), rect.y());
        self.inner
            .popover
            .set_pointing_to(Some(&gdk::Rectangle::new(x, y, 1, rect.height().max(1))));
        if !self.inner.popover.is_visible() {
            self.inner.popover.popup();
        }
    }

    fn move_completion(&self, delta: i32) {
        let list = &self.inner.list;
        let n = self.inner.completions.borrow().len() as i32;
        if n == 0 {
            return;
        }
        let cur = list.selected_row().map_or(0, |r| r.index());
        let next = (cur + delta).rem_euclid(n);
        if let Some(row) = list.row_at_index(next) {
            list.select_row(Some(&row));
            // Keep the selection visible.
            if let Some(adj) = list
                .parent()
                .and_then(|p| p.parent())
                .and_then(|s| s.downcast::<gtk::ScrolledWindow>().ok())
                .map(|s| s.vadjustment())
                && let Some(b) = row.compute_bounds(list)
            {
                let (top, bottom) = (f64::from(b.y()), f64::from(b.y() + b.height()));
                if top < adj.value() {
                    adj.set_value(top);
                } else if bottom > adj.value() + adj.page_size() {
                    adj.set_value(bottom - adj.page_size());
                }
            }
        }
        self.inner.input.grab_focus();
    }

    fn accept_completion(&self, index: usize) {
        let Some(c) = self.inner.completions.borrow().get(index).cloned() else {
            return;
        };
        let text = self.input_text();
        let (t, cursor) = complete::apply(&text, self.cursor_byte(), &c);
        self.inner.popover.popdown();
        self.set_input(&t, Some(cursor));
    }

    // ---------------------------------------------------------- updates

    fn connect_updates(&self) {
        let weak = self.downgrade();
        let sub = self.inner.ctx.engine().subscribe(move |ev| {
            let Some(c) = Console::from_weak(&weak) else {
                return;
            };
            match ev {
                DocEvent::Settings(s) => {
                    let mut chips = c.inner.chips.borrow_mut();
                    chips.retain(|(m, a)| match (m.upgrade(), a.upgrade()) {
                        (Some(m), Some(a)) => {
                            update_mode_chips(&m, &a, s);
                            true
                        }
                        _ => false,
                    });
                    drop(chips);
                    c.build_empty_state();
                }
                DocEvent::Scene(_) => {
                    // A node changed: the last result may no longer pin as is.
                }
                _ => {}
            }
        });
        self.inner.subs.borrow_mut().push(sub);

        let weak = self.downgrade();
        self.inner.ctx.preferences().connect_changed(move |_| {
            let Some(c) = Console::from_weak(&weak) else {
                return false;
            };
            let color = error_color(&c.inner.ctx);
            if *c.inner.error_color.borrow() != color {
                *c.inner.error_color.borrow_mut() = color;
                c.rebuild_entries();
            }
            true
        });
    }
}

/// Status-bar-like labels for the header chips.
fn update_mode_chips(mode: &gtk::Button, angle: &gtk::Button, s: &viscas_engine::EngineSettings) {
    if s.approx {
        mode.set_label("APPROX");
        mode.remove_css_class("accent");
        mode.set_tooltip_text(Some("Results shown as decimals — click for exact"));
    } else {
        mode.set_label("EXACT");
        mode.add_css_class("accent");
        mode.set_tooltip_text(Some("Results stay exact — click for decimals"));
    }
    match s.angle {
        viscas_core::eval::AngleUnit::Radians => {
            angle.set_label("RAD");
            angle.set_tooltip_text(Some("Angle unit: radians — click for degrees"));
        }
        viscas_core::eval::AngleUnit::Degrees => {
            angle.set_label("DEG");
            angle.set_tooltip_text(Some("Angle unit: degrees — click for radians"));
        }
    }
}

fn completion_row(c: &Completion) -> gtk::ListBoxRow {
    let (badge, class) = match c.kind {
        CompletionKind::Node => ("n", "node"),
        CompletionKind::Console => ("c", "console"),
        CompletionKind::Builtin => ("ƒ", "builtin"),
        CompletionKind::Constant => ("π", "constant"),
        CompletionKind::Command => (":", "command"),
    };
    let row_box = gtk::Box::builder().spacing(6).build();
    row_box.append(
        &gtk::Label::builder()
            .label(badge)
            .css_classes(["kind-badge", class])
            .build(),
    );
    row_box.append(
        &gtk::Label::builder()
            .label(&c.text)
            .xalign(0.0)
            .css_classes(["completion-text"])
            .build(),
    );
    row_box.append(
        &gtk::Label::builder()
            .label(&c.detail)
            .xalign(1.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(36)
            .css_classes(["completion-detail"])
            .build(),
    );
    gtk::ListBoxRow::builder()
        .child(&row_box)
        .focusable(false)
        .tooltip_text(&c.detail)
        .build()
}

/// Could `:pin` turn this output into a node?
fn is_pinnable(out: &ConsoleOutput) -> bool {
    out.error.is_none()
        && !out.input_echo.starts_with(':')
        && out.results.iter().any(|i| {
            matches!(
                i.kind,
                ItemKind::Value | ItemKind::Definition { .. } | ItemKind::Relation { .. }
            )
        })
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// Identifier-like words of `s`, in order.
fn identifiers(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| w.chars().next().is_some_and(char::is_alphabetic))
}

fn error_color(ctx: &AppCtx) -> String {
    ctx.preferences()
        .with(|p| style::hex(p.effective_theme().console.error))
}

fn definition_op(ctx: &AppCtx) -> &'static str {
    match ctx.engine().with(|e| e.settings().definition_op) {
        viscas_core::syntax::DefinitionOp::Eq => "=",
        viscas_core::syntax::DefinitionOp::ColonEq => ":=",
    }
}

/// Reads the history file into the engine once per process (unless the
/// engine already has a history).
fn load_history_once(ctx: &AppCtx) {
    if HISTORY_LOADED.replace(true) {
        return;
    }
    let Some(path) = history::history_path() else {
        return;
    };
    let entries = history::load(&path);
    if !entries.is_empty() {
        ctx.engine().edit(|e| {
            if e.history().is_empty() {
                e.set_history(entries);
            }
        });
    }
}

fn save_history(ctx: &AppCtx) {
    let Some(path) = history::history_path() else {
        return;
    };
    let entries = ctx.engine().with(|e| e.history().to_vec());
    if let Err(err) = history::save(&path, &entries) {
        glib::g_warning!("viscas", "could not save console history: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viscas_engine::{ConsoleError, ConsoleItem};

    fn out(input: &str, kind: ItemKind) -> ConsoleOutput {
        ConsoleOutput {
            input_echo: input.into(),
            results: vec![ConsoleItem {
                exact: "1".into(),
                approx: "1".into(),
                kind,
            }],
            ..ConsoleOutput::default()
        }
    }

    #[test]
    fn pinnable_outputs() {
        assert!(is_pinnable(&out("1", ItemKind::Value)));
        assert!(is_pinnable(&out(
            "a := 1",
            ItemKind::Definition { name: "a".into() }
        )));
        assert!(!is_pinnable(&out(":vars", ItemKind::Info)));
        let mut failed = out("1", ItemKind::Value);
        failed.error = Some(ConsoleError {
            message: "x".into(),
            span: None,
        });
        assert!(!is_pinnable(&failed));
    }

    #[test]
    fn identifier_words() {
        let words: Vec<&str> = identifiers("2a + f(x_1) - 3").collect();
        assert_eq!(words, vec!["f", "x_1"]);
        let words: Vec<&str> = identifiers("θ·b").collect();
        assert_eq!(words, vec!["θ", "b"]);
    }
}

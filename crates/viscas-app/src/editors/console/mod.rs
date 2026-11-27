//! Console editor: the primary input of the app (REQUIREMENTS §7).
//!
//! A transcript of entries above a multi-line input. Each entry echoes
//! the input (errors underline the offending span), shows results with
//! the exact form and a dim `≈` decimal side by side, "did you mean"
//! chips, and hover actions (copy exact/approx, add to board, insert).
//!
//! Board: typing never pins anything by default. Every entry that can
//! become a node (plots, points, definitions, values) gets a prominent
//! "+ Add to board" button and Ctrl+Shift+Enter; `:add` (alias `:pin`)
//! and `:plot <input>` do the same from the input. Preferences ▸ Editing
//! can opt into auto-adding drawings or definitions.
//!
//! Input: Enter evaluates, Shift+Enter adds a line, Up/Down recall the
//! (persisted) history, Ctrl+L clears the transcript, Escape clears the input. Space always types: the
//! window's Space-for-palette key ignores focused text views.
//!
//! Completion, signature help, diagnostics, auto-pairing and the inline
//! value preview come from [`crate::intellisense::Assist`].
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
use gtk::{gdk, gio, glib};
use viscas_engine::{AutoAdd, ConsoleError, ConsoleItem, ConsoleOutput, ItemKind};
use viscas_scene::{NodeContent, NodeId};

use crate::ctx::AppCtx;
use crate::doc::{DocEvent, Subscription};
use crate::editors::pick_icon;
use history::HistoryNav;

/// A header-menu action: name and what it does.
type MenuAction = (&'static str, fn(&Console));

/// Inserts `text` at the cursor of the active console (palette: insert
/// a function).
/// Returns `false` when no console is on screen (the caller reveals
/// one first: text never goes into an invisible console).
pub fn insert_into_active(text: &str) -> bool {
    match Console::active() {
        Some(c) => {
            c.insert_at_cursor(text);
            true
        }
        None => false,
    }
}

/// Entries kept in the transcript; older ones are dropped.
const MAX_ENTRIES: usize = 400;

thread_local! {
    /// The history file was read into the engine (once per process).
    static HISTORY_LOADED: Cell<bool> = const { Cell::new(false) };
    /// Every console editor, most recently used last (palette commands
    /// act on the newest one that is on screen).
    static INSTANCES: RefCell<Vec<Weak<Inner>>> = const { RefCell::new(Vec::new()) };
    /// The one console session of the app: every Console editor (in any
    /// workspace or area) shows this transcript.
    static SESSION: RefCell<Session> = RefCell::new(Session::default());
}

/// Transcript records shared by every console editor.
#[derive(Default)]
struct Session {
    records: Vec<(u64, ConsoleOutput, PinSnapshot)>,
    next_id: u64,
}

impl Session {
    /// Records an entry; returns its id.
    fn push(&mut self, output: ConsoleOutput, pin: PinSnapshot) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.records.push((id, output, pin));
        let excess = trim_count(self.records.len(), MAX_ENTRIES);
        self.records.drain(..excess);
        id
    }
}

/// What "Add to board" does for an entry, captured when it was evaluated
/// (`Engine::pin_plan`): the node contents to add, or why it cannot be
/// added. `None` for entries with nothing to add (commands, errors).
/// Kept for the entry's lifetime: the button hides while the input is on
/// the board and comes back if it goes away (undo).
type PinSnapshot = Option<Result<Vec<NodeContent>, String>>;

/// One transcript entry and the data to re-render it.
struct Entry {
    /// Monotonic id (indices shift when old entries are dropped).
    id: u64,
    output: ConsoleOutput,
    pin: PinSnapshot,
    /// Board state the widget was built for ([`entry_state`]).
    state: Vec<bool>,
    widget: gtk::Box,
}

struct Inner {
    ctx: AppCtx,
    /// Last known title of nodes shown in the transcript (for rows whose
    /// node was removed since).
    titles: RefCell<std::collections::HashMap<NodeId, String>>,
    root: gtk::Box,
    scroller: gtk::ScrolledWindow,
    transcript: gtk::Box,
    empty: gtk::Box,
    input: gtk::TextView,
    assist: crate::intellisense::Assist,
    nav: RefCell<HistoryNav>,
    entries: RefCell<Vec<Entry>>,
    /// Example chips shown above the input while the transcript is empty.
    examples: gtk::FlowBox,
    /// The shared session changed while this console was off screen: it
    /// rebuilds its transcript when mapped again.
    stale: Cell<bool>,
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
        let placeholder = gtk::Label::builder()
            .label(format!("Type math…  y = x^2 · a {} 2", definition_op(ctx)))
            .xalign(0.0)
            .valign(gtk::Align::Start)
            .can_target(false)
            .css_classes(["console-placeholder"])
            .build();
        let field_overlay = gtk::Overlay::builder().child(&input).hexpand(true).build();
        field_overlay.add_overlay(&placeholder);
        let buffer = input.buffer();
        buffer.connect_changed(glib::clone!(
            #[weak]
            placeholder,
            move |b| placeholder.set_visible(b.char_count() == 0)
        ));
        let field = gtk::Box::builder()
            .spacing(6)
            .hexpand(true)
            .css_classes(["console-field"])
            .build();
        field.append(&prompt);
        field.append(&field_overlay);
        input_bar.append(&field);
        let examples = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .halign(gtk::Align::Start)
            .min_children_per_line(2)
            .max_children_per_line(8)
            .column_spacing(4)
            .row_spacing(4)
            .css_classes(["console-examples"])
            .build();

        let assist = crate::intellisense::Assist::for_text_view(
            ctx,
            &input,
            &input_bar,
            viscas_engine::Scope::Console,
        );
        assist.enable_preview(&field_overlay);
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["console", "keymap-console"])
            .build();
        root.append(&scroller);
        root.append(&examples);
        root.append(&input_bar);

        let console = Self {
            inner: Rc::new(Inner {
                ctx: ctx.clone(),
                root,
                scroller,
                transcript,
                empty,
                input,
                assist,
                nav: RefCell::default(),
                entries: RefCell::default(),
                examples,
                stale: Cell::new(false),
                setting_text: Cell::new(false),
                stick: Cell::new(false),
                error_color: RefCell::new(error_color(ctx)),
                subs: RefCell::default(),
                titles: RefCell::default(),
            }),
        };
        console.build_empty_state();
        console.connect_input(&input_bar);
        console.connect_scrolling();
        console.connect_updates();
        // Show the shared session's transcript (B4: one console app-wide),
        // and catch up when shown after changes made elsewhere.
        console.replay_session();
        let weak = console.downgrade();
        console.inner.root.connect_map(move |_| {
            if let Some(c) = Console::from_weak(&weak)
                && c.inner.stale.replace(false)
            {
                c.replay_session();
            }
        });
        console.register(ctx);
        console.focus_at_startup();
        console
    }

    /// The first console grabs keyboard focus once it is on screen.
    fn focus_at_startup(&self) {
        thread_local! {
            static FOCUSED: Cell<bool> = const { Cell::new(false) };
        }
        if FOCUSED.get() {
            return;
        }
        let weak = self.downgrade();
        self.inner.input.connect_map(move |_| {
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                // Claimed only by a console that is still alive and shown.
                if let Some(c) = Console::from_weak(&weak)
                    && c.inner.input.is_mapped()
                    && !FOCUSED.replace(true)
                {
                    c.inner.input.grab_focus();
                }
            });
        });
    }

    /// Every live console editor.
    fn all() -> Vec<Console> {
        INSTANCES.with_borrow(|v| v.iter().filter_map(Console::from_weak).collect())
    }

    /// Tracks this console for palette commands; installs the command
    /// provider once per process.
    fn register(&self, ctx: &AppCtx) {
        let first = INSTANCES.with_borrow_mut(|v| {
            let first = v.is_empty();
            v.retain(|w| w.strong_count() > 0);
            v.push(self.downgrade());
            first
        });
        if first {
            ctx.add_command_provider(ConsoleCommands);
        }
        // Focusing a console makes it the palette's target.
        let focus = gtk::EventControllerFocus::new();
        let weak = self.downgrade();
        focus.connect_enter(move |_| {
            INSTANCES.with_borrow_mut(|v| {
                v.retain(|w| w.strong_count() > 0 && !w.ptr_eq(&weak));
                v.push(weak.clone());
            });
        });
        self.inner.root.add_controller(focus);
    }

    /// The most recently used console that is on screen.
    fn active() -> Option<Console> {
        INSTANCES.with_borrow(|v| {
            v.iter()
                .rev()
                .filter_map(Console::from_weak)
                .find(|c| c.inner.root.is_mapped())
        })
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

    /// Header widgets: the Session menu and a Clear button (the exact /
    /// radians chips live in the status bar only). Built fresh per header.
    pub fn header_content(&self) -> Vec<gtk::Widget> {
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
        let group = gio::SimpleActionGroup::new();
        let actions: [MenuAction; 4] = [
            ("clear", |c| c.clear_transcript()),
            ("copy-last", |c| c.copy_last_result()),
            ("add-last", |c| c.pin_last()),
            ("forget", |c| c.forget_definitions()),
        ];
        for (name, f) in actions {
            let action = gio::SimpleAction::new(name, None);
            let weak = self.downgrade();
            action.connect_activate(move |_, _| {
                if let Some(c) = Console::from_weak(&weak) {
                    f(&c);
                }
            });
            group.add_action(&action);
        }
        let menu = header_menu_button(
            "Session",
            "console",
            &group,
            &[
                ("Clear Transcript", "clear", Some("<Control>l")),
                ("Copy Last Result", "copy-last", None),
                (
                    "Add Last Result to Board",
                    "add-last",
                    Some("<Control><Shift>Return"),
                ),
                ("Forget Console Definitions", "forget", None),
            ],
        );
        vec![menu.upcast(), clear.upcast()]
    }

    /// The newest entry matching `f`.
    fn last_entry<R>(&self, f: impl Fn(&Entry) -> Option<R>) -> Option<R> {
        self.inner.entries.borrow().iter().rev().find_map(f)
    }

    fn copy_last_result(&self) {
        let text = self.last_entry(|e| {
            e.output
                .results
                .iter()
                .find(|i| !matches!(i.kind, ItemKind::Info | ItemKind::Relation { .. }))
                .map(|i| i.exact.clone())
                .filter(|s| !s.is_empty())
        });
        match text {
            Some(t) => self.inner.root.clipboard().set_text(&t),
            None => self.inner.root.error_bell(),
        }
    }

    /// Adds the newest entry that can become a node to the board ("Add to
    /// board", Ctrl+Shift+Enter, `:add`).
    fn pin_last(&self) {
        let id = self.last_entry(|e| matches!(e.pin, Some(Ok(_))).then_some(e.id));
        match id {
            Some(id) => self.pin_entry(id),
            None => {
                self.append_info_error("nothing to add to the board yet: evaluate something first")
            }
        }
    }

    /// Forgets console-only definitions and `ans` (`:clear` without
    /// touching the input history).
    fn forget_definitions(&self) {
        self.inner.ctx.engine().edit(|e| {
            let history = e.history().to_vec();
            e.console_eval(":clear");
            e.set_history(history);
        });
        self.clear_transcript();
        self.append(
            ConsoleOutput {
                input_echo: ":clear".into(),
                results: vec![ConsoleItem {
                    exact: "console definitions and `ans` forgotten".into(),
                    approx: String::new(),
                    kind: ItemKind::Info,
                }],
                ..ConsoleOutput::default()
            },
            None,
        );
    }

    fn append_info_error(&self, message: &str) {
        self.append(
            ConsoleOutput {
                input_echo: ":add".into(),
                error: Some(ConsoleError {
                    message: message.into(),
                    span: None,
                }),
                ..ConsoleOutput::default()
            },
            None,
        );
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
        let hint = gtk::Label::builder()
            .label("Type y = x^2 to try it · Ctrl+Shift+Enter adds it to the board")
            .wrap(true)
            .max_width_chars(44)
            .justify(gtk::Justification::Center)
            .css_classes(["console-board-hint"])
            .build();
        let desc = gtk::Label::builder()
            .label(
                "Nothing goes on the board until you say so: + Add to board, \
                 Ctrl+Shift+Enter or :add. Tab completes, ↑ recalls, \
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
        self.inner
            .ctx
            .preferences()
            .connect_changed_for(quip.downgrade(), |q, p| q.set_visible(p.interface.playful));
        // Example chips sit just above the input (not in the empty state).
        let tries = &self.inner.examples;
        while let Some(child) = tries.first_child() {
            tries.remove(&child);
        }
        let examples = [
            "y = x^2".to_owned(),
            format!("a {op} 3"),
            "(1, 2)".to_owned(),
            format!("f(x) {op} x^2 - 2"),
            "solve(f(x) = 0, x)".to_owned(),
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
        empty.append(&hint);
        empty.append(&desc);
        empty.append(&quip);
        self.update_empty();
    }

    fn update_empty(&self) {
        let has = !self.inner.entries.borrow().is_empty();
        self.inner.empty.set_visible(!has);
        self.inner.transcript.set_visible(has);
        self.inner.examples.set_visible(!has);
    }

    // ------------------------------------------------------------ input

    fn connect_input(&self, input_bar: &gtk::Box) {
        let input = &self.inner.input;
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        keys.connect_key_pressed(
            move |_, key, keycode, mods| match Console::from_weak(&weak) {
                Some(c) => c.on_key(key, keycode, mods),
                None => glib::Propagation::Proceed,
            },
        );
        input.add_controller(keys);

        let weak = self.downgrade();
        input.buffer().connect_changed(move |_| {
            if let Some(c) = Console::from_weak(&weak)
                && !c.inner.setting_text.get()
            {
                c.inner.nav.borrow_mut().reset();
            }
        });

        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(glib::clone!(
            #[weak]
            input_bar,
            move |_| input_bar.add_css_class("focused")
        ));
        focus.connect_leave(glib::clone!(
            #[weak]
            input_bar,
            move |_| input_bar.remove_css_class("focused")
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
    }

    fn on_key(&self, key: gdk::Key, keycode: u32, mods: gdk::ModifierType) -> glib::Propagation {
        use gdk::Key;
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let stop = glib::Propagation::Stop;
        if self.inner.assist.on_key(key, keycode, mods) == stop {
            return stop;
        }
        // Rebindable console keys (Preferences ▸ Keymap ▸ Console).
        let bound =
            crate::keymap::live::action_for(crate::keymap::Context::Console, key, keycode, mods);
        match bound.as_deref() {
            Some("console.add-to-board") => {
                self.pin_last();
                return stop;
            }
            Some("console.clear") => {
                self.clear_transcript();
                return stop;
            }
            _ => {}
        }
        match key {
            Key::Return | Key::KP_Enter if !shift => {
                self.evaluate();
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
            _ => glib::Propagation::Proceed,
        }
    }

    fn input_text(&self) -> String {
        let b = self.inner.input.buffer();
        b.text(&b.start_iter(), &b.end_iter(), false).to_string()
    }

    /// Replaces the input; the cursor goes to byte `cursor` (end if `None`).
    fn set_input(&self, text: &str, cursor: Option<usize>) {
        let b = self.inner.input.buffer();
        self.inner.setting_text.set(true);
        // Not a user edit: keep it (and older typing) out of text undo, so
        // Ctrl+Z after Enter undoes the document instead.
        b.begin_irreversible_action();
        b.set_text(text);
        b.end_irreversible_action();
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
        self.inner.assist.dismiss();
        let auto = self.inner.ctx.preferences().with(|p| AutoAdd {
            plots: p.editing.console_add_plots,
            definitions: p.editing.console_add_definitions,
            values: false,
        });
        let (out, pin) = self.inner.ctx.engine().edit(|e| {
            let out = e.console_eval_with(&text, auto);
            // Snapshot what pinning this entry means now, so it can be
            // pinned later without evaluating it again.
            let added = e.take_added_plan();
            let pin = if is_pinnable(&out) {
                Some(e.pin_plan())
            } else if out.error.is_none() && out.input_echo.starts_with(':') {
                None
            } else {
                // Auto-added: keep the plan so it can be added again after
                // an undo removed it.
                added.map(Ok)
            };
            (out, pin)
        });
        save_history(text.trim());
        if out.cleared {
            self.clear_transcript();
        } else {
            self.append(out, pin);
        }
        self.set_input("", None);
        self.inner.nav.borrow_mut().reset();
    }

    /// Adds entry `id` to the board from its snapshot (no re-evaluation,
    /// history untouched).
    fn pin_entry(&self, id: u64) {
        let Some((plan, input)) = self
            .inner
            .entries
            .borrow()
            .iter()
            .find_map(|e| (e.id == id).then(|| (e.pin.clone(), e.output.input_echo.clone())))
        else {
            return;
        };
        let newest = self.last_entry(|e| e.pin.is_some().then_some(e.id)) == Some(id);
        let result = match plan {
            Some(Ok(plan)) => self.inner.ctx.engine().edit(|e| {
                // Re-checked against the scene now: an identical node is
                // reported, a name taken since is an error (nothing added).
                e.add_pin_plan(plan).inspect(|_| {
                    if newest {
                        // `:add` must not add the same input again.
                        e.forget_console_last();
                    }
                })
            }),
            Some(Err(reason)) => Err(reason),
            None => Err("this entry has nothing to add to the board".into()),
        };
        let mut out = ConsoleOutput {
            input_echo: format!(":add  {}", first_line(&input)),
            ..ConsoleOutput::default()
        };
        match result {
            Ok(items) => out.results = items,
            Err(message) => {
                out.error = Some(ConsoleError {
                    message,
                    span: None,
                })
            }
        }
        self.append(out, None);
    }

    // ------------------------------------------------------- transcript

    /// Adds an entry to the shared session and to every console editor.
    fn append(&self, output: ConsoleOutput, pin: PinSnapshot) {
        let id = SESSION.with_borrow_mut(|s| s.push(output.clone(), pin.clone()));
        let mut all = Console::all();
        if !all.iter().any(|c| Rc::ptr_eq(&c.inner, &self.inner)) {
            all.push(self.clone());
        }
        for c in all {
            // Off-screen consoles catch up when mapped.
            if Rc::ptr_eq(&c.inner, &self.inner) || c.inner.root.is_mapped() {
                c.append_local(id, output.clone(), pin.clone());
            } else {
                c.inner.stale.set(true);
            }
        }
    }

    /// Rebuilds this console's transcript from the shared session in one
    /// pass (one board-state pass, one scroll).
    fn replay_session(&self) {
        self.clear_local();
        let records: Vec<(u64, ConsoleOutput, PinSnapshot)> =
            SESSION.with_borrow(|s| s.records.clone());
        let mut entries = Vec::with_capacity(records.len());
        for (id, output, pin) in records {
            let (widget, state) = self.render_entry(&output, &pin, id);
            self.inner.transcript.append(&widget);
            entries.push(Entry {
                id,
                output,
                pin,
                state,
                widget,
            });
        }
        *self.inner.entries.borrow_mut() = entries;
        self.update_empty();
        self.scroll_to_end();
    }

    /// Adds an entry to this editor's transcript widgets.
    fn append_local(&self, id: u64, output: ConsoleOutput, pin: PinSnapshot) {
        // The input may have changed the board: update older entries.
        self.refresh_entries();
        let (widget, state) = self.render_entry(&output, &pin, id);
        self.inner.transcript.append(&widget);
        self.inner.entries.borrow_mut().push(Entry {
            id,
            output,
            pin,
            state,
            widget,
        });
        // Drop the oldest entries beyond the cap; ids stay valid.
        let excess = trim_count(self.inner.entries.borrow().len(), MAX_ENTRIES);
        if excess > 0 {
            let removed: Vec<Entry> = self.inner.entries.borrow_mut().drain(..excess).collect();
            for e in removed {
                self.inner.transcript.remove(&e.widget);
            }
        }
        self.update_empty();
        self.scroll_to_end();
    }

    /// Clears the shared transcript in every console editor.
    fn clear_transcript(&self) {
        SESSION.with_borrow_mut(|s| s.records.clear());
        let mut all = Console::all();
        if !all.iter().any(|c| Rc::ptr_eq(&c.inner, &self.inner)) {
            all.push(self.clone());
        }
        for c in all {
            c.clear_local();
            c.inner.stale.set(false);
        }
    }

    fn clear_local(&self) {
        let entries = std::mem::take(&mut *self.inner.entries.borrow_mut());
        for e in entries {
            self.inner.transcript.remove(&e.widget);
        }
        self.update_empty();
    }

    /// Re-renders every entry in place (theme change).
    fn rebuild_entries(&self) {
        self.rerender(|_, _| true);
    }

    /// Re-renders the entries whose board state changed (nodes added,
    /// removed by undo, redone): "Add to board" buttons come back when
    /// their node is gone and hide while it exists.
    fn refresh_entries(&self) {
        self.rerender(|old, new| old != new);
    }

    /// Re-renders the entries for which `changed(old state, new state)`.
    /// No `entries` borrow is held while widgets are built or swapped.
    fn rerender(&self, changed: impl Fn(&[bool], &[bool]) -> bool) {
        let todo: Vec<(u64, ConsoleOutput, PinSnapshot)> = self.inner.ctx.engine().with(|e| {
            self.inner
                .entries
                .borrow()
                .iter()
                .filter(|en| changed(&en.state, &entry_state(e, &en.output, &en.pin)))
                .map(|en| (en.id, en.output.clone(), en.pin.clone()))
                .collect()
        });
        for (id, output, pin) in todo {
            let (widget, state) = self.render_entry(&output, &pin, id);
            let old = {
                let mut entries = self.inner.entries.borrow_mut();
                let Some(en) = entries.iter_mut().find(|en| en.id == id) else {
                    continue;
                };
                en.state = state;
                std::mem::replace(&mut en.widget, widget.clone())
            };
            self.inner
                .transcript
                .insert_child_after(&widget, Some(&old));
            self.inner.transcript.remove(&old);
        }
    }

    /// Builds an entry for the current board state.
    fn render_entry(
        &self,
        out: &ConsoleOutput,
        pin: &PinSnapshot,
        id: u64,
    ) -> (gtk::Box, Vec<bool>) {
        let state = self.inner.ctx.engine().with(|e| entry_state(e, out, pin));
        // On the board already: no "Add to board" (reasons why it can't
        // be added are still shown).
        let shown = match pin {
            Some(Ok(_)) if !state[0] => None,
            other => other.clone(),
        };
        (self.build_entry(out, &shown, id), state)
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

    fn build_entry(&self, out: &ConsoleOutput, pin: &PinSnapshot, id: u64) -> gtk::Box {
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
        crate::intellisense::install_label_hover(&self.inner.ctx, &echo);
        head.append(&prompt);
        head.append(&echo);
        let actions = self.build_actions(out, pin, id);
        head.append(&actions);
        entry.append(&head);

        // The item "Add to board" refers to gets an inline button too.
        let inline_add = matches!(pin, Some(Ok(_)))
            .then(|| {
                out.results.iter().rposition(|i| {
                    matches!(
                        i.kind,
                        ItemKind::Definition { .. } | ItemKind::Relation { .. }
                    )
                })
            })
            .flatten();
        for (i, item) in out.results.iter().enumerate() {
            entry.append(&self.build_item(item, (inline_add == Some(i)).then_some(id)));
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
                    let node = c.entry_node(id);
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
                    let node = c.entry_node(id);
                    if node.is_some() && sel.hovered() == node {
                        sel.set_hovered(None);
                    }
                }
            }
        ));
        entry.add_controller(motion);
        entry
    }

    /// One result row; `add` is the entry id when the row offers an
    /// inline "Add to board" button.
    fn build_item(&self, item: &viscas_engine::ConsoleItem, add: Option<u64>) -> gtk::Widget {
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
                row.append(&tag("updated on board", true));
            }
            ItemKind::Added { node } | ItemKind::OnBoard { node } => {
                let exists = self.inner.ctx.engine().with(|e| e.scene().contains(*node));
                row.append(&self.added_row(*node, item));
                let label = match (&item.kind, exists) {
                    (_, false) => "removed from board",
                    (ItemKind::OnBoard { .. }, true) => "already on board",
                    _ => "added to board",
                };
                row.append(&tag(label, exists));
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
                        .label("not on the board yet")
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
        if let Some(id) = add {
            let b = gtk::Button::builder()
                .label("+ Add to board")
                .valign(gtk::Align::Center)
                .tooltip_text("Add to board (Ctrl+Shift+Enter, :add)")
                .css_classes(["console-add"])
                .build();
            let weak = self.downgrade();
            b.connect_clicked(move |_| {
                if let Some(c) = Console::from_weak(&weak) {
                    c.pin_entry(id);
                    c.inner.input.grab_focus();
                }
            });
            row.append(&b);
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

    /// Color swatch, a link selecting `node` and its value ("= 3").
    fn added_row(&self, node: NodeId, item: &ConsoleItem) -> gtk::Box {
        let row = gtk::Box::builder().spacing(4).build();
        let (title, color) = self.inner.ctx.engine().with(|e| {
            // Unnamed nodes (plots) read best as their definition.
            let title = e.scene().get(node).map(|n| match &n.name {
                Some(name) => name.clone(),
                None => {
                    viscas_scene::definition_line_with(&n.content(), e.settings().parse_options())
                        .unwrap_or_else(|| n.title())
                }
            });
            (title, e.node_color(node))
        });
        let title = match title {
            Some(t) => {
                self.inner.titles.borrow_mut().insert(node, t.clone());
                Some(t)
            }
            None => None,
        };
        let known = self.inner.titles.borrow().get(&node).cloned();
        let swatch = gtk::DrawingArea::builder()
            .content_width(10)
            .content_height(10)
            .valign(gtk::Align::Center)
            .css_classes(["console-swatch"])
            .build();
        let rgb = color.map(|c| (c.r, c.g, c.b));
        swatch.set_draw_func(move |_, cr, w, h| {
            let (r, g, b) = rgb.unwrap_or((128, 128, 128));
            cr.set_source_rgb(
                f64::from(r) / 255.0,
                f64::from(g) / 255.0,
                f64::from(b) / 255.0,
            );
            let (w, h) = (f64::from(w), f64::from(h));
            let r = 2.5;
            cr.new_sub_path();
            cr.arc(w - r, r, r, -std::f64::consts::FRAC_PI_2, 0.0);
            cr.arc(w - r, h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
            cr.arc(
                r,
                h - r,
                r,
                std::f64::consts::FRAC_PI_2,
                std::f64::consts::PI,
            );
            cr.arc(r, r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
            cr.close_path();
            let _ = cr.fill();
        });
        row.append(&swatch);
        let exists = title.is_some();
        let same = title.as_deref() == Some(item.exact.as_str());
        let link = gtk::Button::builder()
            .label(title.or(known).unwrap_or_else(|| "node".into()))
            .sensitive(exists)
            .tooltip_text(if exists {
                "Select this node on the board"
            } else {
                "This node is no longer on the board"
            })
            .css_classes(["console-node-link"])
            .build();
        let ctx = self.inner.ctx.downgrade();
        link.connect_clicked(move |_| {
            if let Some(ctx) = ctx.upgrade()
                && ctx.engine().with(|e| e.scene().get(node).is_some())
            {
                ctx.selection().set_selected([node]);
            }
        });
        row.append(&link);
        if !item.exact.is_empty() && !same {
            row.append(
                &gtk::Label::builder()
                    .use_markup(true)
                    .label(format!("= {}", render::math_markup(&item.exact)))
                    .xalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .selectable(true)
                    .focusable(false)
                    .css_classes(["console-result"])
                    .build(),
            );
        }
        row
    }

    fn build_actions(&self, out: &ConsoleOutput, pin: &PinSnapshot, id: u64) -> gtk::Box {
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
        if let Some(plan) = pin {
            let b = button(
                &["list-add-symbolic", "view-pin-symbolic"],
                "Add to board (Ctrl+Shift+Enter)",
            );
            if let Err(reason) = plan {
                // Cannot be added safely: say why instead of trying.
                b.set_sensitive(false);
                b.set_tooltip_text(Some(&format!("Can't add to board: {reason}")));
            }
            let weak = self.downgrade();
            b.connect_clicked(move |_| {
                if let Some(c) = Console::from_weak(&weak) {
                    c.pin_entry(id);
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

    /// The scene node entry `id` refers to: a node it updated, pinned
    /// or listed, else the first scene name in its input.
    fn entry_node(&self, id: u64) -> Option<NodeId> {
        let entries = self.inner.entries.borrow();
        let out = &entries.iter().find(|e| e.id == id)?.output;
        self.inner.ctx.engine().with(|e| {
            for item in &out.results {
                match &item.kind {
                    ItemKind::UpdatedNode { node, .. }
                    | ItemKind::Added { node }
                    | ItemKind::OnBoard { node } => {
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

    // ---------------------------------------------------------- updates

    fn connect_updates(&self) {
        let weak = self.downgrade();
        let sub = self.inner.ctx.engine().subscribe(move |ev| {
            let Some(c) = Console::from_weak(&weak) else {
                return;
            };
            if matches!(
                ev,
                DocEvent::Scene(_) | DocEvent::Colors | DocEvent::Replaced
            ) {
                c.refresh_entries();
            }
            if let DocEvent::Settings(_) = ev {
                c.build_empty_state();
            }
        });
        self.inner.subs.borrow_mut().push(sub);

        self.inner
            .ctx
            .preferences()
            .connect_changed_for(self.downgrade(), |inner, _| {
                let c = Console { inner };
                let color = error_color(&c.inner.ctx);
                if *c.inner.error_color.borrow() != color {
                    *c.inner.error_color.borrow_mut() = color;
                    c.rebuild_entries();
                }
            });
    }
}

/// Could "Add to board" turn this output into a node? Its last result
/// (the input `:add` refers to) must not be on the board already.
fn is_pinnable(out: &ConsoleOutput) -> bool {
    out.error.is_none()
        && !out.input_echo.starts_with(':')
        && out.results.last().is_some_and(|i| {
            matches!(
                i.kind,
                ItemKind::Value | ItemKind::Definition { .. } | ItemKind::Relation { .. }
            )
        })
}

/// Board state an entry's widget depends on: whether "Add to board"
/// applies (first), then whether each node it added still exists.
fn entry_state(e: &viscas_engine::Engine, out: &ConsoleOutput, pin: &PinSnapshot) -> Vec<bool> {
    let addable = match pin {
        Some(Ok(plan)) => !on_board(e, plan),
        _ => false,
    };
    let mut state = vec![addable];
    state.extend(out.results.iter().filter_map(|i| match i.kind {
        ItemKind::Added { node } | ItemKind::OnBoard { node } => Some(e.scene().contains(node)),
        _ => None,
    }));
    state
}

/// Is the input of `plan` on the board: an identical node, or a node
/// providing the name it defines?
fn on_board(e: &viscas_engine::Engine, plan: &[NodeContent]) -> bool {
    e.existing_node(plan).is_some()
        || plan.last().is_some_and(|c| {
            c.kind.defines_by_name()
                && c.name
                    .as_ref()
                    .is_some_and(|n| e.scene().is_name_taken(n, None))
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

/// Adds `input` to the history file (merged with what other instances
/// wrote meanwhile).
fn save_history(input: &str) {
    let Some(path) = history::history_path() else {
        return;
    };
    if let Err(err) = history::append(&path, &[input.to_owned()]) {
        glib::g_warning!("viscas", "could not save console history: {err}");
    }
}

/// How many of `len` entries to drop to keep at most `cap`.
fn trim_count(len: usize, cap: usize) -> usize {
    len.saturating_sub(cap)
}

/// A header menu whose items activate `group`'s actions (installed on the
/// button as `prefix`). Items: label, action name, accelerator shown.
pub(crate) fn header_menu_button(
    title: &str,
    prefix: &str,
    group: &gio::SimpleActionGroup,
    items: &[(&str, &str, Option<&str>)],
) -> gtk::MenuButton {
    let menu = gio::Menu::new();
    for (label, action, accel) in items {
        let item = gio::MenuItem::new(Some(label), Some(&format!("{prefix}.{action}")));
        if let Some(accel) = accel {
            item.set_attribute_value("accel", Some(&accel.to_variant()));
        }
        menu.append_item(&item);
    }
    let button = gtk::MenuButton::builder()
        .label(title)
        .menu_model(&menu)
        .always_show_arrow(false)
        .css_classes(["header-menu", "flat"])
        .build();
    button.insert_action_group(prefix, Some(group));
    button
}

/// Palette commands acting on the active console.
struct ConsoleCommands;

impl crate::commands::CommandProvider for ConsoleCommands {
    fn commands(&self) -> Vec<crate::commands::Command> {
        if Console::active().is_none() {
            return Vec::new();
        }
        let cmd = |id: &str, title: &str, shortcut: Option<&str>, f: fn(&Console)| {
            crate::commands::Command {
                id: format!("console.{id}"),
                title: title.to_owned(),
                category: "Console",
                shortcut: shortcut.map(str::to_owned),
                run: Rc::new(move || {
                    // After the palette closed and restored focus.
                    glib::idle_add_local_once(move || {
                        if let Some(c) = Console::active() {
                            f(&c);
                        }
                    });
                }),
            }
        };
        vec![
            cmd(
                "add-last",
                "Add Last Result to Board",
                Some("Ctrl+Shift+Enter"),
                |c| c.pin_last(),
            ),
            cmd("copy-last", "Copy Last Result", None, |c| {
                c.copy_last_result()
            }),
            cmd("clear", "Clear Console Transcript", Some("Ctrl+L"), |c| {
                c.clear_transcript()
            }),
            cmd("forget", "Forget Console Definitions", None, |c| {
                c.forget_definitions()
            }),
        ]
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

    #[test]
    fn transcript_trim_counts() {
        assert_eq!(trim_count(10, 400), 0);
        assert_eq!(trim_count(400, 400), 0);
        assert_eq!(trim_count(401, 400), 1);
    }
}

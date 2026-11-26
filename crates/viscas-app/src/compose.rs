//! Dead-key and Compose fallback for every text field of a window.
//!
//! On Wayland, GTK picks its `wayland` input-method module whenever the
//! compositor offers `text-input-v3` (KWin always does). With no input
//! method actually running, GTK 4.24 does not compose dead keys there: on
//! a French AZERTY keyboard `^` (a dead key) then `e` gives `e`, and `^`
//! then Space gives a lone space, so `^` cannot be typed at all.
//!
//! This module feeds such keys, and the keys that follow them, to GTK's
//! own [`gtk::IMContextSimple`] (the same Compose tables every GTK app
//! uses) and inserts what it commits into the focused text widget:
//!
//! - a bubble-phase controller starts a sequence when a dead key or
//!   `Multi_key` bubbles up unconsumed while an editable text widget has
//!   focus (so it never engages when the widget's own input method
//!   composes: X11, a working IM module);
//! - a capture-phase controller sends the next keys of a started
//!   sequence to the context before the text widget sees them.
//!
//! A sequence ends on a commit, and is abandoned (the key then goes on
//! normally) on a key that produces no text (Return, Escape, BackSpace,
//! Tab, arrows…), on any key with Ctrl/Alt/Super (shortcuts keep
//! working), when the context goes idle without committing, when focus
//! moves or leaves the window, and after [`SEQUENCE_TIMEOUT_US`].
//!
//! One deviation from the Compose tables, for math: dead `^` followed by
//! a digit, sign, bracket or operator types a literal `^` and that
//! character (`x^2`, `x^-1`, `2^(n+1)`), not `²`/`⁻`. Letters still
//! compose (`^` `e` → `ê`), `^` Space still gives `^`.
//!
//! When a real input method is configured (see [`fallback_enabled`]) the
//! fallback is not installed at all, so it can never compete with it.
//!
//! Tested (nested KWin 6.8, GTK 4.24, `fr` layout): Wayland backend with
//! no input method — the fallback engages; `GTK_IM_MODULE=gtk-im-context-simple`
//! — GTK composes itself, the fallback stays idle. Reasoned only: IBus or
//! fcitx5 (environment variables, or input-method-v2 grabbing keys in the
//! compositor, where dead keys never reach the application).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib::translate::IntoGlib;
use gtk::prelude::*;
use gtk::{gdk, glib};

/// A sequence untouched for this long is abandoned (µs).
pub const SEQUENCE_TIMEOUT_US: i64 = 3_000_000;

/// Input methods that compose (and may own dead keys) themselves.
const REAL_IMS: [&str; 9] = [
    "ibus", "fcitx", "uim", "scim", "kime", "nimf", "hime", "gcin", "maliit",
];

/// Whether the fallback should run, given `GTK_IM_MODULE`,
/// `QT_IM_MODULE`, `XMODIFIERS` and GTK's `gtk-im-module` setting: not
/// when any of them names a real input method, nor when GTK is told to
/// use a module other than its built-in simple/wayland ones.
pub fn fallback_enabled(
    gtk_im_module: Option<&str>,
    qt_im_module: Option<&str>,
    xmodifiers: Option<&str>,
    gtk_setting: Option<&str>,
) -> bool {
    let names_real_im = |v: Option<&str>| {
        v.is_some_and(|v| {
            let v = v.to_ascii_lowercase();
            REAL_IMS.iter().any(|im| v.contains(im))
        })
    };
    let builtin = |v: Option<&str>| {
        matches!(
            v.map(str::trim).unwrap_or(""),
            "" | "gtk-im-context-simple" | "simple" | "wayland" | "none"
        )
    };
    !(names_real_im(gtk_im_module)
        || names_real_im(qt_im_module)
        || names_real_im(xmodifiers)
        || names_real_im(gtk_setting)
        || !builtin(gtk_im_module)
        || !builtin(gtk_setting))
}

/// Whether `keyval` starts a compose sequence: a dead key (`dead_grave`
/// … `dead_longsolidusoverlay`) or the Compose key (`Multi_key`).
pub fn starts_sequence(keyval: u32) -> bool {
    const DEAD_FIRST: u32 = 0xfe50; // dead_grave
    const DEAD_LAST: u32 = 0xfe93; // dead_longsolidusoverlay
    (DEAD_FIRST..=DEAD_LAST).contains(&keyval) || keyval == MULTI_KEY
}

/// Keyval of the Compose key.
const MULTI_KEY: u32 = 0xff20;

/// Keyval of the dead circumflex (`^` on AZERTY, a plain key there).
const DEAD_CIRCUMFLEX: u32 = 0xfe52;

/// Text to type, instead of composing, when `next` follows a sequence
/// started by `start`: a dead circumflex before an exponent-like
/// character (digit, sign, bracket, operator, …) is a power, so it gives
/// `^` plus that character. `None` lets the Compose tables decide (`²`
/// and `³` are left to them, see [`unmatched_text`]).
pub fn literal_after(start: u32, next: char) -> Option<String> {
    let exponent_like = next.is_ascii_digit()
        || next.is_ascii_punctuation() && next != '^'
        || matches!(next, '−' | '·' | '×' | '÷' | 'π' | '√');
    (start == DEAD_CIRCUMFLEX && exponent_like).then(|| format!("^{next}"))
}

/// Text typed when `next` ends a sequence started by `start` that the
/// Compose tables have no entry for (GTK would drop the key, e.g. `^`
/// then `²`): the dead key's spacing character, if known, then `next`.
pub fn unmatched_text(start: u32, next: char) -> String {
    let spacing = match start {
        0xfe50 => Some('`'), // dead_grave
        0xfe51 => Some('´'), // dead_acute
        DEAD_CIRCUMFLEX => Some('^'),
        0xfe53 => Some('~'), // dead_tilde
        0xfe57 => Some('¨'), // dead_diaeresis
        0xfe5b => Some('¸'), // dead_cedilla
        _ => None,
    };
    spacing.into_iter().chain(std::iter::once(next)).collect()
}

/// Whether `keyval` is a modifier key (pressing Shift inside a sequence,
/// for `^` then `E`, must not end it).
pub fn is_modifier(keyval: u32) -> bool {
    // Shift_L … Hyper_R, the ISO level/group shifts and locks
    // (ISO_Lock … ISO_Level5_Lock), Mode_switch and Num_Lock.
    (0xffe1..=0xffee).contains(&keyval)
        || (0xfe01..=0xfe13).contains(&keyval)
        || keyval == 0xff7e
        || keyval == 0xff7f
}

/// What to do with a key pressed while a sequence is pending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Type this text and end the sequence.
    Literal(String),
    /// Drop the sequence and let the key through untouched.
    Abort,
    /// Give the key to the compose context.
    Feed,
}

/// Decides the [`Step`] for the key `keyval` (producing `ch`, if any)
/// after a sequence started by `start`. `shortcut` is whether Ctrl, Alt,
/// Super or Meta is held.
pub fn next_step(start: u32, keyval: u32, ch: Option<char>, shortcut: bool) -> Step {
    if shortcut {
        return Step::Abort;
    }
    if starts_sequence(keyval) {
        return Step::Feed; // `^` `^` → `^`, chained dead keys
    }
    match ch.filter(|c| !c.is_control()) {
        None => Step::Abort, // Return, Escape, BackSpace, Tab, arrows…
        Some(c) => literal_after(start, c).map_or(Step::Feed, Step::Literal),
    }
}

/// Whether `mods` holds a shortcut modifier.
fn is_shortcut(mods: gdk::ModifierType) -> bool {
    mods.intersects(
        gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK
            | gdk::ModifierType::META_MASK,
    )
}

/// The focused widget if it takes typed text and is editable.
fn editable_focus(window: &gtk::Window) -> Option<gtk::Widget> {
    let focus = gtk::prelude::GtkWindowExt::focus(window)?;
    if let Some(view) = focus.downcast_ref::<gtk::TextView>() {
        return view.is_editable().then_some(focus);
    }
    let editable = focus.dynamic_cast_ref::<gtk::Editable>()?;
    editable.is_editable().then_some(focus.clone())
}

/// Inserts `text` at the cursor of `widget`, replacing the selection,
/// as if it had been typed.
fn insert(widget: &gtk::Widget, text: &str) {
    if let Some(view) = widget.downcast_ref::<gtk::TextView>() {
        // One user action: a single text-undo step.
        let buffer = view.buffer();
        buffer.begin_user_action();
        buffer.delete_selection(true, true);
        buffer.insert_interactive_at_cursor(text, true);
        buffer.end_user_action();
        view.scroll_mark_onscreen(&buffer.get_insert());
    } else if let Some(editable) = widget.dynamic_cast_ref::<gtk::Editable>() {
        editable.delete_selection();
        if widget.is::<gtk::Text>() {
            // GtkText's own insert-at-cursor keybinding signal: the same
            // path as typing (one change, IM reset, cursor after the text).
            widget.emit_by_name::<()>("insert-at-cursor", &[&text]);
        } else {
            let mut pos = editable.position();
            editable.insert_text(text, &mut pos);
            editable.set_position(pos);
        }
    }
}

thread_local! {
    /// Every window's fallback state, for [`is_pending`].
    static INSTANCES: RefCell<Vec<std::rc::Weak<Compose>>> = const { RefCell::new(Vec::new()) };
}

/// Whether a dead-key / Compose sequence is in progress in any window
/// (the keymap dispatcher then leaves plain keys to it).
pub fn is_pending() -> bool {
    INSTANCES.with_borrow_mut(|list| {
        list.retain(|w| w.strong_count() > 0);
        list.iter()
            .filter_map(std::rc::Weak::upgrade)
            .any(|s| s.pending.get())
    })
}

/// State of the window's compose fallback.
struct Compose {
    im: gtk::IMContextSimple,
    window: glib::WeakRef<gtk::Window>,
    /// A sequence is in progress.
    pending: Cell<bool>,
    /// Keyval that started it.
    started: Cell<u32>,
    /// When it last advanced (`glib::monotonic_time`, µs).
    touched: Cell<i64>,
    /// The context committed during the current feed.
    committed: Cell<bool>,
}

impl Compose {
    fn abandon(&self) {
        if self.pending.replace(false) {
            self.im.reset();
        }
    }

    /// Gives the current key event to the context; whether it took it.
    /// A key it takes without committing or keeping a sequence ends it.
    /// `unmatched` (given for the key after a dead key, which always ends
    /// the sequence) is typed if the context takes the key without
    /// committing, i.e. the Compose tables have no entry for it.
    fn feed(&self, ctrl: &gtk::EventControllerKey, unmatched: Option<String>) -> bool {
        let Some(event) = ctrl.current_event() else {
            return false;
        };
        self.committed.set(false);
        let taken = self.im.filter_keypress(&event);
        if !taken {
            self.im.reset();
            self.pending.set(false);
        } else if self.committed.get() {
            // Done (the commit handler typed the text).
        } else if let Some(text) = unmatched {
            // GtkIMContextSimple swallows a key with no Compose entry and
            // keeps waiting (`^` then `²`); end the sequence, keep both.
            self.abandon();
            if let Some(target) = self.window.upgrade().and_then(|w| editable_focus(&w)) {
                insert(&target, &text);
            }
        } else if self.im.preedit_string().0.is_empty() {
            self.pending.set(false); // idle: sequence cancelled
        }
        self.touched.set(glib::monotonic_time());
        taken
    }

    /// Capture phase: the next key of a pending sequence.
    fn on_next(
        &self,
        ctrl: &gtk::EventControllerKey,
        key: gdk::Key,
        mods: gdk::ModifierType,
    ) -> glib::Propagation {
        let keyval = key.into_glib();
        if !self.pending.get() || is_modifier(keyval) {
            return glib::Propagation::Proceed;
        }
        let target = self.window.upgrade().and_then(|w| editable_focus(&w));
        let stale = glib::monotonic_time() - self.touched.get() > SEQUENCE_TIMEOUT_US;
        let Some(target) = target.filter(|_| !stale) else {
            self.abandon();
            return glib::Propagation::Proceed;
        };
        match next_step(
            self.started.get(),
            keyval,
            key.to_unicode(),
            is_shortcut(mods),
        ) {
            Step::Abort => {
                self.abandon();
                glib::Propagation::Proceed
            }
            Step::Literal(text) => {
                self.abandon();
                insert(&target, &text);
                glib::Propagation::Stop
            }
            Step::Feed => {
                // A dead key plus one non-dead key always ends the
                // sequence; Compose-key sequences may be longer, so only
                // dead-key starts get the unmatched fallback.
                let start = self.started.get();
                let unmatched = (start != MULTI_KEY && !starts_sequence(keyval))
                    .then(|| key.to_unicode())
                    .flatten()
                    .map(|c| unmatched_text(start, c));
                if self.feed(ctrl, unmatched) {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        }
    }

    /// Bubble phase: a dead key nobody composed starts a sequence.
    fn on_start(
        &self,
        ctrl: &gtk::EventControllerKey,
        key: gdk::Key,
        mods: gdk::ModifierType,
    ) -> glib::Propagation {
        let keyval = key.into_glib();
        let in_text = self
            .window
            .upgrade()
            .is_some_and(|w| editable_focus(&w).is_some());
        if !starts_sequence(keyval) || is_shortcut(mods) || !in_text {
            return glib::Propagation::Proceed;
        }
        let Some(event) = ctrl.current_event() else {
            return glib::Propagation::Proceed;
        };
        self.committed.set(false);
        if self.im.filter_keypress(&event) {
            self.pending.set(!self.committed.get());
            self.started.set(keyval);
            self.touched.set(glib::monotonic_time());
            glib::Propagation::Stop
        } else {
            self.im.reset();
            glib::Propagation::Proceed
        }
    }
}

/// Installs the fallback on `window` (see the module docs), unless a
/// real input method is configured.
pub fn install(window: &impl IsA<gtk::Window>) {
    let window: &gtk::Window = window.upcast_ref();
    let env = |k: &str| std::env::var(k).ok();
    let setting = gtk::Settings::for_display(&WidgetExt::display(window))
        .gtk_im_module()
        .map(|s| s.to_string());
    if !fallback_enabled(
        env("GTK_IM_MODULE").as_deref(),
        env("QT_IM_MODULE").as_deref(),
        env("XMODIFIERS").as_deref(),
        setting.as_deref(),
    ) {
        return;
    }
    let im = gtk::IMContextSimple::new();
    im.set_client_widget(Some(window));
    im.focus_in();
    let state = Rc::new(Compose {
        im: im.clone(),
        window: window.downgrade(),
        pending: Cell::new(false),
        started: Cell::new(0),
        touched: Cell::new(0),
        committed: Cell::new(false),
    });

    INSTANCES.with_borrow_mut(|list| list.push(Rc::downgrade(&state)));
    let weak = Rc::downgrade(&state);
    im.connect_commit(move |_, text| {
        let Some(s) = weak.upgrade() else { return };
        s.committed.set(true);
        s.pending.set(false);
        if let Some(target) = s.window.upgrade().and_then(|w| editable_focus(&w)) {
            insert(&target, text);
        }
    });

    let capture = gtk::EventControllerKey::new();
    capture.set_propagation_phase(gtk::PropagationPhase::Capture);
    let s = state.clone();
    capture.connect_key_pressed(move |ctrl, key, _, mods| s.on_next(ctrl, key, mods));
    window.add_controller(capture);

    let bubble = gtk::EventControllerKey::new();
    let s = state.clone();
    bubble.connect_key_pressed(move |ctrl, key, _, mods| s.on_start(ctrl, key, mods));
    window.add_controller(bubble);

    // Moving focus or leaving the window abandons a half-typed sequence.
    let s = state.clone();
    window.connect_focus_widget_notify(move |_| s.abandon());
    window.connect_is_active_notify(move |_| state.abandon());
}

#[cfg(test)]
mod tests {
    use super::{
        DEAD_CIRCUMFLEX, Step, fallback_enabled, is_modifier, literal_after, next_step,
        starts_sequence, unmatched_text,
    };

    #[test]
    fn unmatched_sequences_keep_both_characters() {
        assert_eq!(unmatched_text(DEAD_CIRCUMFLEX, '²'), "^²");
        assert_eq!(unmatched_text(0xfe57, '²'), "¨²");
        assert_eq!(unmatched_text(0xfe50, 'q'), "`q");
        // Unknown dead keys keep just the typed character.
        assert_eq!(unmatched_text(0xfe90, 'x'), "x");
    }
    use gtk::gdk::Key;
    use gtk::glib::translate::IntoGlib;

    #[test]
    fn dead_keys_and_compose_start_sequences() {
        for key in [
            Key::dead_circumflex,
            Key::dead_diaeresis,
            Key::dead_grave,
            Key::dead_acute,
            Key::dead_tilde,
            Key::dead_cedilla,
            Key::dead_longsolidusoverlay,
            Key::Multi_key,
        ] {
            assert!(starts_sequence(key.into_glib()), "{key:?}");
        }
    }

    #[test]
    fn printable_keys_do_not_start_sequences() {
        for key in [
            Key::asciicircum,
            Key::equal,
            Key::minus,
            Key::plus,
            Key::space,
            Key::e,
            Key::twosuperior,
            Key::ecircumflex,
            Key::ISO_Level3_Shift,
            Key::Return,
        ] {
            assert!(!starts_sequence(key.into_glib()), "{key:?}");
        }
    }

    #[test]
    fn modifiers_keep_a_sequence_going() {
        for key in [
            Key::Shift_L,
            Key::Shift_R,
            Key::Caps_Lock,
            Key::Control_L,
            Key::Alt_R,
            Key::ISO_Level3_Shift,
            Key::ISO_Level5_Shift,
        ] {
            assert!(is_modifier(key.into_glib()), "{key:?}");
        }
        for key in [Key::e, Key::space, Key::equal, Key::dead_circumflex] {
            assert!(!is_modifier(key.into_glib()), "{key:?}");
        }
    }

    #[test]
    fn circumflex_before_exponents_is_a_power() {
        for (next, want) in [
            ('2', "^2"),
            ('-', "^-"),
            ('(', "^("),
            ('=', "^="),
            ('+', "^+"),
            ('−', "^−"),
        ] {
            assert_eq!(literal_after(DEAD_CIRCUMFLEX, next).as_deref(), Some(want));
        }
        // Letters, space, a second `^` and ²/³ keep the Compose meaning.
        for next in ['e', 'E', 'a', ' ', '^', '²', '³'] {
            assert_eq!(literal_after(DEAD_CIRCUMFLEX, next), None, "{next:?}");
        }
        // Other dead keys always compose.
        let dead_diaeresis = 0xfe57;
        assert_eq!(literal_after(dead_diaeresis, '2'), None);
    }

    fn step(key: Key, shortcut: bool) -> Step {
        next_step(DEAD_CIRCUMFLEX, key.into_glib(), key.to_unicode(), shortcut)
    }

    #[test]
    fn keys_without_text_abandon_the_sequence() {
        for key in [
            Key::Return,
            Key::KP_Enter,
            Key::Escape,
            Key::BackSpace,
            Key::Tab,
            Key::ISO_Left_Tab,
            Key::Left,
            Key::Up,
            Key::Home,
            Key::Delete,
            Key::F2,
        ] {
            assert_eq!(step(key, false), Step::Abort, "{key:?}");
        }
    }

    #[test]
    fn shortcut_modifiers_abandon_the_sequence() {
        for key in [Key::z, Key::_2, Key::e, Key::space, Key::dead_circumflex] {
            assert_eq!(step(key, true), Step::Abort, "{key:?}");
        }
    }

    #[test]
    fn text_keys_compose_or_type_literally() {
        assert_eq!(step(Key::e, false), Step::Feed);
        assert_eq!(step(Key::space, false), Step::Feed);
        assert_eq!(step(Key::dead_circumflex, false), Step::Feed);
        assert_eq!(step(Key::twosuperior, false), Step::Feed);
        assert_eq!(step(Key::_2, false), Step::Literal("^2".into()));
        assert_eq!(step(Key::minus, false), Step::Literal("^-".into()));
    }

    #[test]
    fn real_input_methods_disable_the_fallback() {
        assert!(fallback_enabled(None, None, None, None));
        assert!(fallback_enabled(Some(""), None, Some(""), Some("")));
        assert!(fallback_enabled(
            Some("wayland"),
            None,
            None,
            Some("wayland")
        ));
        assert!(fallback_enabled(
            Some("gtk-im-context-simple"),
            None,
            None,
            None
        ));
        assert!(!fallback_enabled(Some("ibus"), None, None, None));
        assert!(!fallback_enabled(None, Some("fcitx"), None, None));
        assert!(!fallback_enabled(None, None, Some("@im=fcitx"), None));
        assert!(!fallback_enabled(None, None, Some("@im=ibus"), None));
        assert!(!fallback_enabled(None, None, None, Some("ibus")));
        assert!(!fallback_enabled(Some("xim"), None, None, None));
    }
}

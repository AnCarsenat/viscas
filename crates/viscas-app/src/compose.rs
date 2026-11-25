//! Dead-key and Compose fallback for every text field of a window.
//!
//! On Wayland, GTK picks its `wayland` input-method module whenever the
//! compositor offers `text-input-v3` (KWin always does). With no input
//! method actually running, GTK 4.24 does not compose dead keys there: on
//! a French AZERTY keyboard `^` (a dead key) then `e` gives `e`, and `^`
//! then Space gives a lone space, so `^` cannot be typed at all.
//!
//! A real input method (fcitx5, IBus over `input-method-v2`) sees keys
//! before the application and composes them itself, so a dead key that
//! still reaches the window was not handled by anyone. This module feeds
//! such keys, and the keys that follow them, to GTK's own
//! [`gtk::IMContextSimple`] (the same Compose tables every GTK app uses)
//! and inserts what it commits into the focused text widget:
//!
//! - a bubble-phase controller starts a sequence when a dead key or
//!   `Multi_key` bubbles up unconsumed while a text widget has focus;
//! - a capture-phase controller sends the next keys of a started
//!   sequence to the context before the text widget sees them.
//!
//! One deviation from the Compose tables, for math: dead `^` followed by
//! a digit, sign, bracket or operator types a literal `^` and that
//! character (`x^2`, `x^-1`, `2^(n+1)`), not `²`/`⁻`. Letters still
//! compose (`^` `e` → `ê`) and `^` Space still gives `^`.
//!
//! When the text widget's own input method composes (X11, a working IM
//! module), it consumes the dead key and this fallback never engages.

use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::glib::translate::IntoGlib;
use gtk::prelude::*;

/// Whether `keyval` starts a compose sequence: a dead key (`dead_grave`
/// … `dead_longsolidusoverlay`) or the Compose key (`Multi_key`).
pub fn starts_sequence(keyval: u32) -> bool {
    const DEAD_FIRST: u32 = 0xfe50; // dead_grave
    const DEAD_LAST: u32 = 0xfe93; // dead_longsolidusoverlay
    const MULTI_KEY: u32 = 0xff20;
    (DEAD_FIRST..=DEAD_LAST).contains(&keyval) || keyval == MULTI_KEY
}

/// Keyval of the dead circumflex (`^` on AZERTY, a plain key there).
const DEAD_CIRCUMFLEX: u32 = 0xfe52;

/// Text to type, instead of composing, when `next` follows a sequence
/// started by `start`: a dead circumflex before an exponent-like
/// character (digit, sign, bracket, operator, …) is a power, so it gives
/// `^` plus that character. `None` lets the Compose tables decide.
pub fn literal_after(start: u32, next: char) -> Option<String> {
    let exponent_like = next.is_ascii_digit()
        || next.is_ascii_punctuation() && next != '^'
        || matches!(next, '²' | '³' | '−' | '·' | '×' | '÷' | 'π' | '√');
    (start == DEAD_CIRCUMFLEX && exponent_like).then(|| format!("^{next}"))
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
        let buffer = view.buffer();
        buffer.begin_user_action();
        buffer.delete_selection(true, view.is_editable());
        buffer.insert_interactive_at_cursor(text, view.is_editable());
        buffer.end_user_action();
        view.scroll_mark_onscreen(&buffer.get_insert());
    } else if let Some(editable) = widget.dynamic_cast_ref::<gtk::Editable>() {
        editable.delete_selection();
        let mut pos = editable.position();
        editable.insert_text(text, &mut pos);
        editable.set_position(pos);
    }
}

/// Installs the fallback on `window` (see the module docs).
pub fn install(window: &impl IsA<gtk::Window>) {
    let window: &gtk::Window = window.upcast_ref();
    let im = gtk::IMContextSimple::new();
    im.set_client_widget(Some(window));
    im.focus_in();
    // A sequence is in progress (the context holds a dead key).
    let pending = Rc::new(Cell::new(false));
    // Keyval that started it.
    let started = Rc::new(Cell::new(0u32));

    let weak = window.downgrade();
    let p = pending.clone();
    im.connect_commit(move |_, text| {
        p.set(false);
        if let Some(window) = weak.upgrade()
            && let Some(target) = editable_focus(&window)
        {
            insert(&target, text);
        }
    });

    // Feeds the current key event to the context; whether it took it.
    let feed = {
        let im = im.clone();
        let pending = pending.clone();
        move |ctrl: &gtk::EventControllerKey| -> bool {
            let Some(event) = ctrl.current_event() else {
                return false;
            };
            let taken = im.filter_keypress(&event);
            if !taken {
                im.reset();
                pending.set(false);
            }
            taken
        }
    };

    // Next keys of a started sequence, before the text widget sees them.
    let capture = gtk::EventControllerKey::new();
    capture.set_propagation_phase(gtk::PropagationPhase::Capture);
    let p = pending.clone();
    let feed_next = feed.clone();
    let weak = window.downgrade();
    let im_next = im.clone();
    let s = started.clone();
    capture.connect_key_pressed(move |ctrl, key, _, _| {
        if !p.get() || is_modifier(key.into_glib()) {
            return glib::Propagation::Proceed;
        }
        let Some(target) = weak.upgrade().and_then(|w| editable_focus(&w)) else {
            return glib::Propagation::Proceed;
        };
        if let Some(text) = key.to_unicode().and_then(|c| literal_after(s.get(), c)) {
            im_next.reset();
            p.set(false);
            insert(&target, &text);
            return glib::Propagation::Stop;
        }
        if feed_next(ctrl) {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(capture);

    // A dead key nobody composed: start a sequence.
    let bubble = gtk::EventControllerKey::new();
    let p = pending.clone();
    let weak = window.downgrade();
    bubble.connect_key_pressed(move |ctrl, key, _, _| {
        if !starts_sequence(key.into_glib())
            || weak.upgrade().is_none_or(|w| editable_focus(&w).is_none())
        {
            return glib::Propagation::Proceed;
        }
        if feed(ctrl) {
            p.set(true);
            started.set(key.into_glib());
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(bubble);

    // Moving focus abandons a half-typed sequence.
    window.connect_focus_widget_notify(move |_| {
        if pending.replace(false) {
            im.reset();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{DEAD_CIRCUMFLEX, is_modifier, literal_after, starts_sequence};
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
        ] {
            assert_eq!(literal_after(DEAD_CIRCUMFLEX, next).as_deref(), Some(want));
        }
        // Letters, space and a second `^` keep the Compose meaning.
        for next in ['e', 'E', 'a', ' ', '^'] {
            assert_eq!(literal_after(DEAD_CIRCUMFLEX, next), None, "{next:?}");
        }
        // Other dead keys always compose.
        let dead_diaeresis = 0xfe57;
        assert_eq!(literal_after(dead_diaeresis, '2'), None);
    }
}

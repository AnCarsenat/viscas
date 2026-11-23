//! Blender-style numeric field: the value is shown centered between two
//! step arrows; drag horizontally to scrub (Shift for fine control),
//! click to type a value, Enter to commit, Escape to cancel.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::theme::model::trim_float;

/// Pointer travel before a press becomes a drag, in logical px.
const DRAG_THRESHOLD: f64 = 3.0;
/// Fine scrubbing (Shift) moves this fraction of a step per step distance.
const FINE: f64 = 0.1;

/// The numeric behaviour of a scrub field: range, step and precision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrubSpec {
    pub min: f64,
    pub max: f64,
    /// Change per arrow click, and per `px_per_step` of drag.
    pub step: f64,
    /// Decimal digits shown and kept.
    pub digits: u32,
    /// Drag distance worth one step, in logical px.
    pub px_per_step: f64,
}

impl ScrubSpec {
    pub fn new(min: f64, max: f64, step: f64, digits: u32) -> Self {
        Self {
            min: min.min(max),
            max: max.max(min),
            step: if step > 0.0 { step } else { 1.0 },
            digits: digits.min(9),
            px_per_step: 8.0,
        }
    }

    /// Rounds to the precision, then clamps; non-finite values give `min`.
    pub fn normalize(&self, v: f64) -> f64 {
        if !v.is_finite() {
            return self.min;
        }
        let scale = 10f64.powi(self.digits as i32);
        let rounded = (v * scale).round() / scale;
        // `+ 0.0` turns -0.0 into 0.0 so it never displays as "-0".
        rounded.clamp(self.min, self.max) + 0.0
    }

    /// `v` moved by `n` steps.
    pub fn stepped(&self, v: f64, n: i32) -> f64 {
        self.normalize(v + f64::from(n) * self.step)
    }

    /// Value after dragging `dx` px from a drag that started at `start`.
    /// Normal drags move in whole steps; fine drags move continuously at a
    /// tenth of the speed.
    pub fn dragged(&self, start: f64, dx: f64, fine: bool) -> f64 {
        let steps = dx / self.px_per_step;
        let delta = if fine {
            steps * self.step * FINE
        } else {
            steps.trunc() * self.step
        };
        self.normalize(start + delta)
    }

    /// The value as displayed: fixed number of digits.
    pub fn format(&self, v: f64) -> String {
        let digits = self.digits as usize;
        format!("{v:.digits$}")
    }

    /// Parses typed text (`,` accepted as decimal separator, a trailing
    /// unit word ignored) and normalizes it.
    pub fn parse(&self, text: &str) -> Option<f64> {
        let t = text.trim().replace(',', ".");
        let number: String = t
            .trim_end_matches(|c: char| c.is_alphabetic() || c == '%' || c.is_whitespace())
            .to_owned();
        let v: f64 = number.trim().parse().ok()?;
        v.is_finite().then(|| self.normalize(v))
    }
}

impl From<crate::prefs::Range> for ScrubSpec {
    fn from(r: crate::prefs::Range) -> Self {
        ScrubSpec::new(r.min, r.max, r.step, r.digits)
    }
}

type Listener = Rc<dyn Fn(f64)>;

struct Inner {
    root: gtk::Box,
    stack: gtk::Stack,
    display: gtk::Label,
    entry: gtk::Entry,
    spec: ScrubSpec,
    value: Cell<f64>,
    suffix: RefCell<String>,
    drag_start: Cell<f64>,
    scrubbing: Cell<bool>,
    listeners: RefCell<Vec<Listener>>,
}

/// A numeric field. Cloning yields a handle to the same widget.
#[derive(Clone)]
pub struct ScrubField {
    inner: Rc<Inner>,
}

impl ScrubField {
    pub fn new(spec: ScrubSpec, value: f64) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .css_classes(["scrub"])
            .hexpand(true)
            .build();
        let arrow = |icon: &str, tip: &str| {
            gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tip)
                .focusable(false)
                .css_classes(["scrub-arrow"])
                .build()
        };
        let left = arrow("pan-start-symbolic", "Decrease");
        let right = arrow("pan-end-symbolic", "Increase");
        let display = gtk::Label::builder()
            .xalign(0.5)
            .hexpand(true)
            .css_classes(["scrub-value"])
            .build();
        let entry = gtk::Entry::builder()
            .hexpand(true)
            .xalign(0.5)
            .css_classes(["scrub-entry"])
            .build();
        let stack = gtk::Stack::builder()
            .hexpand(true)
            .hhomogeneous(true)
            .vhomogeneous(true)
            .build();
        stack.add_named(&display, Some("display"));
        stack.add_named(&entry, Some("edit"));
        root.append(&left);
        root.append(&stack);
        root.append(&right);
        root.set_cursor_from_name(Some("ew-resize"));
        left.set_cursor_from_name(Some("default"));
        right.set_cursor_from_name(Some("default"));

        let field = Self {
            inner: Rc::new(Inner {
                root,
                stack,
                display,
                entry,
                spec,
                value: Cell::new(spec.normalize(value)),
                suffix: RefCell::default(),
                drag_start: Cell::new(0.0),
                scrubbing: Cell::new(false),
                listeners: RefCell::default(),
            }),
        };
        field.refresh();

        let weak = Rc::downgrade(&field.inner);
        left.connect_clicked(glib::clone!(
            #[strong]
            weak,
            move |_| with(&weak, |f| f.user_set(f.inner.spec.stepped(f.value(), -1)))
        ));
        right.connect_clicked(glib::clone!(
            #[strong]
            weak,
            move |_| with(&weak, |f| f.user_set(f.inner.spec.stepped(f.value(), 1)))
        ));
        field.install_drag(&weak);
        field.install_entry(&weak);
        field
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.inner.root.upcast_ref()
    }

    pub fn value(&self) -> f64 {
        self.inner.value.get()
    }

    /// Sets the value without notifying listeners (external updates).
    pub fn set_value(&self, v: f64) {
        self.inner.value.set(self.inner.spec.normalize(v));
        self.refresh();
    }

    /// Text appended to the displayed value, e.g. `" px"`.
    pub fn set_suffix(&self, suffix: &str) {
        *self.inner.suffix.borrow_mut() = suffix.to_owned();
        self.refresh();
    }

    pub fn set_tooltip(&self, text: &str) {
        self.inner.stack.set_tooltip_text(Some(text));
    }

    /// Calls `f` with the new value after every user change.
    pub fn connect_changed(&self, f: impl Fn(f64) + 'static) {
        self.inner.listeners.borrow_mut().push(Rc::new(f));
    }

    fn user_set(&self, v: f64) {
        let v = self.inner.spec.normalize(v);
        if v == self.value() {
            return;
        }
        self.inner.value.set(v);
        self.refresh();
        let listeners = self.inner.listeners.borrow().clone();
        for f in listeners {
            f(v);
        }
    }

    fn refresh(&self) {
        let text = self.inner.spec.format(self.value());
        self.inner
            .display
            .set_label(&format!("{text}{}", self.inner.suffix.borrow()));
    }

    fn install_drag(&self, weak: &Weak<Inner>) {
        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin(glib::clone!(
            #[strong]
            weak,
            move |_, _, _| with(&weak, |f| {
                f.inner.drag_start.set(f.value());
                f.inner.scrubbing.set(false);
            })
        ));
        drag.connect_drag_update(glib::clone!(
            #[strong]
            weak,
            move |gesture, dx, _| with(&weak, |f| {
                if !f.inner.scrubbing.get() && dx.abs() < DRAG_THRESHOLD {
                    return;
                }
                if !f.inner.scrubbing.replace(true) {
                    f.inner.root.add_css_class("scrubbing");
                }
                let fine = gesture
                    .current_event_state()
                    .contains(gdk::ModifierType::SHIFT_MASK);
                let start = f.inner.drag_start.get();
                f.user_set(f.inner.spec.dragged(start, dx, fine));
            })
        ));
        drag.connect_drag_end(glib::clone!(
            #[strong]
            weak,
            move |_, _, _| with(&weak, |f| {
                f.inner.root.remove_css_class("scrubbing");
                if !f.inner.scrubbing.replace(false) {
                    f.begin_edit();
                }
            })
        ));
        self.inner.display.add_controller(drag);
    }

    fn install_entry(&self, weak: &Weak<Inner>) {
        let entry = &self.inner.entry;
        entry.connect_activate(glib::clone!(
            #[strong]
            weak,
            move |_| with(&weak, |f| f.end_edit(true))
        ));
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(glib::clone!(
            #[strong]
            weak,
            move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    with(&weak, |f| f.end_edit(false));
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        ));
        entry.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(glib::clone!(
            #[strong]
            weak,
            move |_| with(&weak, |f| f.end_edit(true))
        ));
        entry.add_controller(focus);
    }

    fn begin_edit(&self) {
        let i = &self.inner;
        i.entry
            .set_text(&trim_float(self.value(), i.spec.digits as usize));
        i.stack.set_visible_child_name("edit");
        i.root.add_css_class("editing");
        i.entry.grab_focus();
        i.entry.select_region(0, -1);
    }

    fn end_edit(&self, commit: bool) {
        let i = &self.inner;
        if i.stack.visible_child_name().as_deref() != Some("edit") {
            return;
        }
        i.stack.set_visible_child_name("display");
        i.root.remove_css_class("editing");
        if commit && let Some(v) = i.spec.parse(&i.entry.text()) {
            self.user_set(v);
        }
    }
}

fn with(weak: &Weak<Inner>, f: impl FnOnce(&ScrubField)) {
    if let Some(inner) = weak.upgrade() {
        f(&ScrubField { inner });
    }
}

#[cfg(test)]
mod tests {
    use super::ScrubSpec;

    fn spec() -> ScrubSpec {
        ScrubSpec::new(0.5, 2.0, 0.05, 2)
    }

    #[test]
    fn step_moves_and_clamps() {
        let s = spec();
        assert_eq!(s.stepped(1.0, 1), 1.05);
        assert_eq!(s.stepped(1.0, -3), 0.85);
        assert_eq!(s.stepped(1.98, 1), 2.0);
        assert_eq!(s.stepped(0.5, -1), 0.5);
    }

    #[test]
    fn precision_rounds_float_noise() {
        let s = spec();
        // 0.1 + 0.2 style noise must not leak into the value.
        let mut v = 0.5;
        for _ in 0..7 {
            v = s.stepped(v, 1);
        }
        assert_eq!(v, 0.85);
        assert_eq!(s.normalize(1.23456), 1.23);
        let ints = ScrubSpec::new(0.0, 100.0, 1.0, 0);
        assert_eq!(ints.normalize(41.6), 42.0);
        assert_eq!(ints.format(42.0), "42");
        assert_eq!(s.format(1.0), "1.00");
    }

    #[test]
    fn non_finite_and_inverted_ranges_are_safe() {
        let s = spec();
        assert_eq!(s.normalize(f64::NAN), 0.5);
        assert_eq!(s.normalize(f64::INFINITY), 0.5);
        let inv = ScrubSpec::new(10.0, 0.0, -1.0, 0);
        assert_eq!((inv.min, inv.max, inv.step), (0.0, 10.0, 1.0));
    }

    #[test]
    fn negative_zero_displays_as_zero() {
        let s = ScrubSpec::new(-1.0, 1.0, 0.1, 1);
        assert_eq!(s.format(s.normalize(-0.01)), "0.0");
        assert_eq!(s.format(s.stepped(0.1, -1)), "0.0");
    }

    #[test]
    fn drag_sensitivity() {
        let s = spec(); // 8 px per step
        assert_eq!(s.dragged(1.0, 0.0, false), 1.0);
        assert_eq!(s.dragged(1.0, 7.9, false), 1.0, "less than one step");
        assert_eq!(s.dragged(1.0, 8.0, false), 1.05);
        assert_eq!(s.dragged(1.0, 40.0, false), 1.25);
        assert_eq!(s.dragged(1.0, -16.0, false), 0.9);
        assert_eq!(s.dragged(1.0, 10_000.0, false), 2.0);
        // Fine: a tenth of the speed, continuous (then rounded to digits).
        assert_eq!(s.dragged(1.0, 80.0, true), 1.05);
        assert_eq!(s.dragged(1.0, 48.0, true), 1.03);
    }

    #[test]
    fn parse_typed_values() {
        let s = spec();
        assert_eq!(s.parse(" 1.5 "), Some(1.5));
        assert_eq!(s.parse("1,25"), Some(1.25));
        assert_eq!(s.parse("1.333"), Some(1.33));
        assert_eq!(s.parse("12 px"), Some(2.0));
        assert_eq!(s.parse("abc"), None);
        assert_eq!(s.parse(""), None);
        assert_eq!(s.parse("inf"), None);
    }
}

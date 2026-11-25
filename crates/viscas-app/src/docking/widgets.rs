//! Containers of the area system, built from one closure-driven layout
//! manager ([`FnLayout`]):
//!
//! - [`split_view`]: two children at a fixed ratio with a draggable sash.
//!   Unlike `gtk::Paned`, the ratio never moves when a child's size
//!   request changes (REQUIREMENTS audit B1).
//! - [`clip_bin`]: reports no minimum size and clips its child, so an
//!   editor's content can never push the layout around.
//! - [`overflow_row`]: a row that hides whole children that do not fit,
//!   instead of clipping them mid-word.
//! - [`header_row`] and [`switch_row`]: area header and editor switch,
//!   which collapse to icon-only below [`ICON_ONLY_WIDTH`].
//!
//! Closures read children at allocation time (never capture them), so
//! there are no reference cycles between containers and their children.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::layout::Dir;

type MeasureFn = dyn Fn(&gtk::Widget, gtk::Orientation) -> (i32, i32);
type AllocateFn = dyn Fn(&gtk::Widget, i32, i32);

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct FnLayout {
        pub measure: RefCell<Option<Box<MeasureFn>>>,
        pub allocate: RefCell<Option<Box<AllocateFn>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FnLayout {
        const NAME: &'static str = "ViscasFnLayout";
        type Type = super::FnLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for FnLayout {}

    impl LayoutManagerImpl for FnLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            _for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let (min, nat) = self
                .measure
                .borrow()
                .as_ref()
                .map_or((0, 0), |f| f(widget, orientation));
            (min.max(0), nat.max(min).max(0), -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            if let Some(f) = self.allocate.borrow().as_ref() {
                f(widget, width, height);
            }
        }
    }
}

glib::wrapper! {
    /// A layout manager running Rust closures (`gtk::CustomLayout` is not
    /// bound by gtk-rs).
    pub struct FnLayout(ObjectSubclass<imp::FnLayout>) @extends gtk::LayoutManager;
}

impl FnLayout {
    pub fn new(
        measure: impl Fn(&gtk::Widget, gtk::Orientation) -> (i32, i32) + 'static,
        allocate: impl Fn(&gtk::Widget, i32, i32) + 'static,
    ) -> Self {
        let this: Self = glib::Object::new();
        this.imp().measure.replace(Some(Box::new(measure)));
        this.imp().allocate.replace(Some(Box::new(allocate)));
        this
    }
}

/// A box laid out by `layout`.
fn container(layout: FnLayout, classes: &[&str]) -> gtk::Box {
    let b = gtk::Box::builder().css_classes(classes.to_vec()).build();
    b.set_layout_manager(Some(layout));
    b
}

fn children(w: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut c = w.first_child();
    while let Some(child) = c {
        c = child.next_sibling();
        if child.should_layout() {
            out.push(child);
        }
    }
    out
}

fn measure(w: &gtk::Widget, o: gtk::Orientation) -> (i32, i32) {
    let (min, nat, _, _) = w.measure(o, -1);
    (min, nat)
}

/// Places `w` at `x, y` with at least its minimum size (the container
/// clips anything beyond its own allocation).
fn place(w: &gtk::Widget, x: i32, y: i32, width: i32, height: i32) {
    let (min_w, _) = measure(w, gtk::Orientation::Horizontal);
    let (min_h, _) = measure(w, gtk::Orientation::Vertical);
    w.size_allocate(
        &gtk::Allocation::new(x, y, width.max(min_w), height.max(min_h)),
        -1,
    );
}

/// Children allocated here are out of sight (clipped by the container).
const OFFSCREEN: i32 = -100_000;

/// Wraps `child` in a container reporting no minimum size that clips it.
pub fn clip_bin(child: &impl IsA<gtk::Widget>, classes: &[&str]) -> gtk::Box {
    let layout = FnLayout::new(
        |w, o| {
            let nat = children(w).first().map_or(0, |c| measure(c, o).1);
            (0, nat)
        },
        |w, width, height| {
            if let Some(c) = children(w).first() {
                place(c, 0, 0, width, height);
            }
        },
    );
    let b = container(layout, classes);
    b.set_overflow(gtk::Overflow::Hidden);
    b.append(child);
    b
}

/// A row of children at their natural width; children that do not fit
/// entirely are hidden (with every child after them).
pub fn overflow_row(spacing: i32, classes: &[&str]) -> gtk::Box {
    let layout = FnLayout::new(
        move |w, o| {
            let kids = children(w);
            match o {
                gtk::Orientation::Horizontal => {
                    let n = i32::try_from(kids.len()).unwrap_or(0);
                    let nat: i32 = kids.iter().map(|c| measure(c, o).1).sum();
                    (0, nat + spacing * (n - 1).max(0))
                }
                _ => kids
                    .iter()
                    .map(|c| measure(c, o))
                    .fold((0, 0), |a, b| (a.0.max(b.0), a.1.max(b.1))),
            }
        },
        move |w, width, height| {
            let mut x = 0;
            let mut fits = true;
            for c in children(w) {
                let (_, nat) = measure(&c, gtk::Orientation::Horizontal);
                fits = fits && x + nat <= width;
                if fits {
                    place(&c, x, 0, nat, height);
                    x += nat + spacing;
                } else {
                    place(&c, OFFSCREEN, 0, nat, height);
                }
            }
        },
    );
    let b = container(layout, classes);
    b.set_overflow(gtk::Overflow::Hidden);
    b
}

/// Below this header width the editor switch shows only its icon.
pub const ICON_ONLY_WIDTH: i32 = 140;

/// Area header: `[switch][menus ...][more]`. The more button stays at
/// the right edge; the switch collapses to its minimum (icon-only) when
/// the header is narrower than [`ICON_ONLY_WIDTH`]; menus get the rest.
pub fn header_row(classes: &[&str]) -> gtk::Box {
    const GAP: i32 = 4;
    let layout = FnLayout::new(
        |w, o| {
            let kids = children(w);
            match o {
                gtk::Orientation::Horizontal => {
                    let nat: i32 = kids.iter().map(|c| measure(c, o).1).sum();
                    (0, nat + GAP * 2)
                }
                _ => kids
                    .iter()
                    .map(|c| measure(c, o))
                    .fold((0, 0), |a, b| (a.0.max(b.0), a.1.max(b.1))),
            }
        },
        |w, width, height| {
            let kids = children(w);
            let [switch, menus, more] = kids.as_slice() else {
                return;
            };
            let h = gtk::Orientation::Horizontal;
            let (_, more_w) = measure(more, h);
            let more_x = (width - more_w).max(0);
            place(more, more_x, 0, more_w, height);
            let (switch_min, switch_nat) = measure(switch, h);
            let room = (more_x - GAP).max(0);
            let switch_w = if width < ICON_ONLY_WIDTH {
                switch_min
            } else {
                switch_nat.min(room).max(switch_min)
            };
            place(switch, 0, 0, switch_w, height);
            let menus_x = switch_w + GAP;
            place(menus, menus_x, 0, (more_x - GAP - menus_x).max(0), height);
        },
    );
    let b = container(layout, classes);
    b.set_overflow(gtk::Overflow::Hidden);
    b
}

/// Editor switch content: `[icon][label][arrow]`; below its natural
/// width the label is hidden.
pub fn switch_row(classes: &[&str]) -> gtk::Box {
    const GAP: i32 = 5;
    let layout = FnLayout::new(
        |w, o| {
            let kids = children(w);
            match (o, kids.as_slice()) {
                (gtk::Orientation::Horizontal, [icon, label, arrow]) => {
                    let (i, l, a) = (measure(icon, o).1, measure(label, o).1, measure(arrow, o).1);
                    (i + a + GAP, i + l + a + 2 * GAP)
                }
                _ => kids
                    .iter()
                    .map(|c| measure(c, o))
                    .fold((0, 0), |a, b| (a.0.max(b.0), a.1.max(b.1))),
            }
        },
        |w, width, height| {
            let kids = children(w);
            let [icon, label, arrow] = kids.as_slice() else {
                return;
            };
            let h = gtk::Orientation::Horizontal;
            let (i, l, a) = (measure(icon, h).1, measure(label, h).1, measure(arrow, h).1);
            place(icon, 0, 0, i, height);
            if width >= i + l + a + 2 * GAP {
                place(label, i + GAP, 0, l, height);
                place(arrow, width - a, 0, a, height);
            } else {
                place(label, OFFSCREEN, 0, l, height);
                place(arrow, i + GAP, 0, a, height);
            }
        },
    );
    container(layout, classes)
}

/// Gap between split children, in px.
pub const GAP: i32 = 3;
/// Width of a sash's pointer hit zone, centered on the gap.
pub const SASH_HIT: i32 = 6;

/// What a sash reports to its owner.
pub enum SashEvent {
    /// A drag started.
    Begin,
    /// The ratio changed during a drag.
    Move(f64),
    /// The drag ended.
    End,
    /// Double-click: both sides get half.
    Equalize,
}

/// Two children split along `dir` at `ratio` (shared, so the owner can
/// update it), with a sash between them. Dragging the sash computes the
/// new ratio with `clamp(pos_px, span_px, snap)` and reports it.
pub fn split_view(
    dir: Dir,
    ratio: Rc<Cell<f64>>,
    start: &gtk::Widget,
    end: &gtk::Widget,
    clamp: impl Fn(f64, f64, bool) -> f64 + 'static,
    on_event: impl Fn(SashEvent) + 'static,
) -> gtk::Box {
    let horizontal = dir == Dir::Horizontal;
    let along = if horizontal {
        gtk::Orientation::Horizontal
    } else {
        gtk::Orientation::Vertical
    };
    let r = ratio.clone();
    let layout = FnLayout::new(
        move |w, o| {
            let kids = children(w);
            let (Some(a), Some(b)) = (kids.first(), kids.get(1)) else {
                return (0, 0);
            };
            let (ma, na) = measure(a, o);
            let (mb, nb) = measure(b, o);
            if o == along {
                (ma + mb + GAP, na + nb + GAP)
            } else {
                (ma.max(mb), na.max(nb))
            }
        },
        move |w, width, height| {
            let kids = children(w);
            let (Some(a), Some(b), Some(handle)) = (kids.first(), kids.get(1), kids.get(2)) else {
                return;
            };
            let size = if horizontal { width } else { height };
            let span = (size - GAP).max(0);
            let (min_a, _) = measure(a, along);
            let (min_b, _) = measure(b, along);
            let mut pos = (r.get() * f64::from(span)).round() as i32;
            if min_a + min_b <= span {
                pos = pos.clamp(min_a, span - min_b);
            }
            let handle_at = pos + GAP / 2 - SASH_HIT / 2;
            if horizontal {
                place(a, 0, 0, pos, height);
                place(b, pos + GAP, 0, span - pos, height);
                place(handle, handle_at, 0, SASH_HIT, height);
            } else {
                place(a, 0, 0, width, pos);
                place(b, 0, pos + GAP, width, span - pos);
                place(handle, 0, handle_at, width, SASH_HIT);
            }
        },
    );
    let view = container(layout, &["split-view"]);
    let handle = gtk::Box::builder()
        .css_classes(["sash", if horizontal { "horizontal" } else { "vertical" }])
        .build();
    handle.set_cursor_from_name(Some(if horizontal {
        "col-resize"
    } else {
        "row-resize"
    }));
    view.append(start);
    view.append(end);
    // Last child: drawn on top and picked first.
    view.append(&handle);

    let on_event = Rc::new(on_event);
    let drag = gtk::GestureDrag::new();
    let start_pos = Rc::new(Cell::new(0.0));
    let moving = Rc::new(Cell::new(false));
    drag.connect_drag_begin(glib::clone!(
        #[strong]
        ratio,
        #[strong]
        start_pos,
        #[strong]
        moving,
        #[weak]
        view,
        #[strong]
        on_event,
        move |_, _, _| {
            // Not claimed yet: a double-click must reach the click gesture.
            moving.set(false);
            let span = span_of(&view, horizontal);
            start_pos.set(ratio.get() * span);
            on_event(SashEvent::Begin);
        }
    ));
    let clamp = Rc::new(clamp);
    drag.connect_drag_update(glib::clone!(
        #[strong]
        ratio,
        #[weak]
        view,
        #[strong]
        on_event,
        move |g, dx, dy| {
            // Claim (and start moving) only past a small threshold.
            const THRESHOLD: f64 = 3.0;
            if !moving.get() {
                if dx.abs().max(dy.abs()) < THRESHOLD {
                    return;
                }
                moving.set(true);
                g.set_state(gtk::EventSequenceState::Claimed);
            }
            let span = span_of(&view, horizontal);
            let snap = g
                .current_event_state()
                .contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let pos = start_pos.get() + if horizontal { dx } else { dy };
            let new = clamp(pos, span, snap);
            if (new - ratio.get()).abs() > f64::EPSILON {
                ratio.set(new);
                view.queue_allocate();
                on_event(SashEvent::Move(new));
            }
        }
    ));
    drag.connect_drag_end(glib::clone!(
        #[strong]
        on_event,
        move |_, _, _| on_event(SashEvent::End)
    ));
    handle.add_controller(drag);

    let click = gtk::GestureClick::new();
    click.connect_pressed(glib::clone!(
        #[weak]
        view,
        move |_, n, _, _| {
            if n == 2 {
                ratio.set(0.5);
                view.queue_allocate();
                on_event(SashEvent::Equalize);
            }
        }
    ));
    handle.add_controller(click);
    view
}

fn span_of(view: &gtk::Box, horizontal: bool) -> f64 {
    let size = if horizontal {
        view.width()
    } else {
        view.height()
    };
    f64::from((size - GAP).max(0))
}

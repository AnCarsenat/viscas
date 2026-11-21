//! 2D graph viewport: a drawing area showing a stack of layers (grid, and
//! later plots) with pan and zoom.

mod draw;
pub mod view;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, gdk, glib};

use self::draw::GridLayer;
use self::view::View;

/// Zoom factor applied per discrete scroll-wheel step.
const ZOOM_PER_STEP: f64 = 1.15;
/// Largest scroll delta honored in one event, to keep zoom factors sane.
const MAX_SCROLL_DELTA: f64 = 50.0;

/// Something drawn in the viewport, in world space described by a [`View`].
///
/// Layers are drawn in insertion order, so the first one is the background.
pub trait Layer {
    /// Draws the layer. `widget` gives access to style colors and Pango.
    fn draw(&self, widget: &gtk::Widget, cr: &cairo::Context, view: &View);
}

type Layers = Rc<RefCell<Vec<Box<dyn Layer>>>>;

/// The graph viewport widget and its shared view state.
///
/// Cloning is cheap and yields a handle to the same viewport.
#[derive(Clone)]
pub struct Viewport {
    root: gtk::Overlay,
    area: gtk::DrawingArea,
    hud: gtk::Label,
    view: Rc<RefCell<View>>,
    layers: Layers,
}

impl Viewport {
    /// Creates a viewport showing the Cartesian grid.
    pub fn new() -> Self {
        let area = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .focusable(true)
            .can_focus(true)
            .css_classes(["viscas-viewport"])
            .build();
        area.update_property(&[gtk::accessible::Property::Label("Graph viewport")]);

        // Cursor readout in the bottom-left corner.
        let hud = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .css_classes(["viewport-hud", "mono"])
            .can_target(false)
            .build();
        let root = gtk::Overlay::builder().child(&area).build();
        root.add_overlay(&hud);

        let viewport = Self {
            root,
            area,
            hud,
            view: Rc::new(RefCell::new(View::default())),
            layers: Rc::default(),
        };
        viewport.add_layer(GridLayer);
        viewport.install_draw();
        viewport.install_drag();
        viewport.install_scroll();
        viewport.update_hud(None);
        viewport
    }

    /// The widget to put in the widget tree.
    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    /// Recenters on the origin at the default zoom.
    pub fn reset_view(&self) {
        self.update_view(|v| {
            let size = v.size;
            *v = View {
                size,
                ..View::default()
            };
        });
        self.update_hud(None);
    }

    /// Shows the world coordinates under the pointer, or the zoom level when
    /// the pointer is outside.
    fn update_hud(&self, pointer: Option<(f64, f64)>) {
        let text = hud_text(&self.view.borrow(), pointer);
        self.hud.set_label(&text);
    }

    /// A copy of the current view.
    #[allow(dead_code, reason = "API for upcoming plot layers")]
    pub fn view(&self) -> View {
        *self.view.borrow()
    }

    /// Modifies the view and schedules a redraw.
    pub fn update_view(&self, f: impl FnOnce(&mut View)) {
        f(&mut self.view.borrow_mut());
        self.queue_draw();
    }

    /// Appends a layer on top of the existing ones and schedules a redraw.
    pub fn add_layer(&self, layer: impl Layer + 'static) {
        self.layers.borrow_mut().push(Box::new(layer));
        self.queue_draw();
    }

    /// Schedules a redraw, e.g. after a layer's data changed.
    pub fn queue_draw(&self) {
        self.area.queue_draw();
    }

    fn install_draw(&self) {
        let view = self.view.clone();
        self.area.connect_resize(glib::clone!(
            #[strong]
            view,
            move |_, w, h| view.borrow_mut().size = (f64::from(w), f64::from(h))
        ));

        let layers = self.layers.clone();
        self.area.set_draw_func(move |area, cr, w, h| {
            // Use the allocation actually being drawn, even if a resize
            // notification is still pending.
            let mut v = *view.borrow();
            v.size = (f64::from(w), f64::from(h));
            for layer in layers.borrow().iter() {
                layer.draw(area.upcast_ref(), cr, &v);
            }
        });
    }

    fn install_drag(&self) {
        let area = &self.area;
        let drag = gtk::GestureDrag::new();
        let last = Rc::new(Cell::new((0.0, 0.0)));

        drag.connect_drag_begin(glib::clone!(
            #[strong]
            last,
            #[weak]
            area,
            move |_, _, _| {
                last.set((0.0, 0.0));
                area.grab_focus();
                area.set_cursor_from_name(Some("grabbing"));
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[strong]
            last,
            #[strong(rename_to = view)]
            self.view,
            #[weak]
            area,
            move |_, ox, oy| {
                let (lx, ly) = last.replace((ox, oy));
                view.borrow_mut().pan_by_pixels(ox - lx, oy - ly);
                area.queue_draw();
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[weak]
            area,
            move |_, _, _| area.set_cursor(None::<&gdk::Cursor>)
        ));
        area.add_controller(drag);
    }

    fn install_scroll(&self) {
        let area = &self.area;
        // Scroll events carry no position, so track the pointer separately.
        let pointer = Rc::new(Cell::new(None::<(f64, f64)>));
        let motion = gtk::EventControllerMotion::new();
        let this = self.downgrade();
        motion.connect_motion(glib::clone!(
            #[strong]
            pointer,
            #[strong]
            this,
            move |_, x, y| {
                pointer.set(Some((x, y)));
                if let Some(vp) = this.upgrade() {
                    vp.update_hud(Some((x, y)));
                }
            }
        ));
        motion.connect_leave(glib::clone!(
            #[strong]
            pointer,
            #[strong]
            this,
            move |_| {
                pointer.set(None);
                if let Some(vp) = this.upgrade() {
                    vp.update_hud(None);
                }
            }
        ));
        area.add_controller(motion);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll(glib::clone!(
            #[strong]
            this,
            #[strong(rename_to = view)]
            self.view,
            #[weak]
            area,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, _, dy| {
                {
                    let mut v = view.borrow_mut();
                    let (sx, sy) = pointer.get().unwrap_or((v.size.0 / 2.0, v.size.1 / 2.0));
                    let dy = dy.clamp(-MAX_SCROLL_DELTA, MAX_SCROLL_DELTA);
                    v.zoom_at(ZOOM_PER_STEP.powf(-dy), sx, sy);
                }
                if let Some(vp) = this.upgrade() {
                    vp.update_hud(pointer.get());
                }
                area.queue_draw();
                glib::Propagation::Stop
            }
        ));
        area.add_controller(scroll);
    }

    fn downgrade(&self) -> WeakViewport {
        WeakViewport {
            root: self.root.downgrade(),
            area: self.area.downgrade(),
            hud: self.hud.downgrade(),
            view: Rc::downgrade(&self.view),
            layers: Rc::downgrade(&self.layers),
        }
    }
}

/// Weak handle to a [`Viewport`], for signal closures owned by its widgets.
#[derive(Clone)]
struct WeakViewport {
    root: glib::WeakRef<gtk::Overlay>,
    area: glib::WeakRef<gtk::DrawingArea>,
    hud: glib::WeakRef<gtk::Label>,
    view: std::rc::Weak<RefCell<View>>,
    layers: std::rc::Weak<RefCell<Vec<Box<dyn Layer>>>>,
}

impl WeakViewport {
    fn upgrade(&self) -> Option<Viewport> {
        Some(Viewport {
            root: self.root.upgrade()?,
            area: self.area.upgrade()?,
            hud: self.hud.upgrade()?,
            view: self.view.upgrade()?,
            layers: self.layers.upgrade()?,
        })
    }
}

/// Text of the cursor readout: world coordinates under the pointer, or the
/// zoom level when there is no pointer.
fn hud_text(view: &View, pointer: Option<(f64, f64)>) -> String {
    match pointer {
        Some((sx, sy)) => {
            let (x, y) = view.screen_to_world(sx, sy);
            let digits = view::label_digits(view.scale);
            format!("x {x:>+10.digits$}   y {y:>+10.digits$}")
        }
        None => format!("{:.3} px/unit", view.scale),
    }
}

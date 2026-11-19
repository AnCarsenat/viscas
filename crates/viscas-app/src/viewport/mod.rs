//! 2D graph viewport: a drawing area showing a stack of layers (grid, and
//! later plots) with pan and zoom.

mod draw;
pub mod view;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
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
    area: gtk::DrawingArea,
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
            .css_classes(["view", "viscas-viewport"])
            .build();
        area.update_property(&[gtk::accessible::Property::Label("Graph viewport")]);

        let viewport = Self {
            area,
            view: Rc::new(RefCell::new(View::default())),
            layers: Rc::default(),
        };
        viewport.add_layer(GridLayer);
        viewport.install_draw();
        viewport.install_drag();
        viewport.install_scroll();
        viewport.redraw_on_theme_change();
        viewport
    }

    /// The widget to put in the widget tree.
    pub fn widget(&self) -> &gtk::Widget {
        self.area.upcast_ref()
    }

    /// A copy of the current view.
    #[allow(dead_code, reason = "API for upcoming plot layers")]
    pub fn view(&self) -> View {
        *self.view.borrow()
    }

    /// Modifies the view and schedules a redraw.
    #[allow(dead_code, reason = "API for upcoming plot layers")]
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
        motion.connect_motion(glib::clone!(
            #[strong]
            pointer,
            move |_, x, y| pointer.set(Some((x, y)))
        ));
        motion.connect_leave(glib::clone!(
            #[strong]
            pointer,
            move |_| pointer.set(None)
        ));
        area.add_controller(motion);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll(glib::clone!(
            #[strong(rename_to = view)]
            self.view,
            #[weak]
            area,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, _, dy| {
                let mut v = view.borrow_mut();
                let (sx, sy) = pointer.get().unwrap_or((v.size.0 / 2.0, v.size.1 / 2.0));
                let dy = dy.clamp(-MAX_SCROLL_DELTA, MAX_SCROLL_DELTA);
                v.zoom_at(ZOOM_PER_STEP.powf(-dy), sx, sy);
                area.queue_draw();
                glib::Propagation::Stop
            }
        ));
        area.add_controller(scroll);
    }

    /// Grid colors come from the theme, so redraw whenever it changes.
    /// The handlers are disconnected when the widget is destroyed.
    fn redraw_on_theme_change(&self) {
        let style = adw::StyleManager::default();
        let handlers: Vec<glib::SignalHandlerId> = ["dark", "high-contrast"]
            .into_iter()
            .map(|prop| {
                let weak = self.area.downgrade();
                style.connect_notify_local(Some(prop), move |_, _| {
                    if let Some(area) = weak.upgrade() {
                        area.queue_draw();
                    }
                })
            })
            .collect();
        let handlers = RefCell::new(Some(handlers));
        self.area.connect_destroy(move |_| {
            if let Some(handlers) = handlers.take() {
                let style = adw::StyleManager::default();
                for id in handlers {
                    style.disconnect(id);
                }
            }
        });
    }
}

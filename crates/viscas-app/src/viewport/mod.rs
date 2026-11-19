//! 2D graph viewport: a drawing area showing a Cartesian grid with pan and zoom.

mod draw;
pub mod view;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};

use self::view::View;

/// Zoom factor applied per discrete scroll-wheel step.
const ZOOM_PER_STEP: f64 = 1.15;

/// Builds the viewport widget.
pub fn build() -> gtk::Widget {
    let area = gtk::DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .focusable(true)
        .can_focus(true)
        .css_classes(["view", "viscas-viewport"])
        .build();
    area.update_property(&[gtk::accessible::Property::Label("Graph viewport")]);

    let view = Rc::new(RefCell::new(View::default()));
    let pointer = Rc::new(Cell::new(None::<(f64, f64)>));

    area.set_draw_func(glib::clone!(
        #[strong]
        view,
        move |area, cr, w, h| {
            let mut v = view.borrow_mut();
            v.size = (f64::from(w), f64::from(h));
            draw::draw_grid(area, cr, &v);
        }
    ));

    install_drag(&area, &view);
    install_scroll(&area, &view, &pointer);

    // Grid colors come from the theme, so redraw whenever it changes.
    let style = adw::StyleManager::default();
    for prop in ["dark", "high-contrast"] {
        let weak = area.downgrade();
        style.connect_notify_local(Some(prop), move |_, _| {
            if let Some(area) = weak.upgrade() {
                area.queue_draw();
            }
        });
    }

    area.upcast()
}

fn install_drag(area: &gtk::DrawingArea, view: &Rc<RefCell<View>>) {
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
        #[strong]
        view,
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

fn install_scroll(
    area: &gtk::DrawingArea,
    view: &Rc<RefCell<View>>,
    pointer: &Rc<Cell<Option<(f64, f64)>>>,
) {
    // Scroll events carry no position, so track the pointer separately.
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
        #[strong]
        view,
        #[strong]
        pointer,
        #[weak(rename_to = area)]
        area,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, _, dy| {
            let mut v = view.borrow_mut();
            let (sx, sy) = pointer.get().unwrap_or((v.size.0 / 2.0, v.size.1 / 2.0));
            v.zoom_at(ZOOM_PER_STEP.powf(-dy), sx, sy);
            area.queue_draw();
            glib::Propagation::Stop
        }
    ));
    area.add_controller(scroll);
}

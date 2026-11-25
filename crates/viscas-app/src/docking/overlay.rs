//! Drag feedback drawn over an area tree (split line, join overlay, drop
//! zones), the dragged-area ghost and the layout toast.

use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::layout::{EditorKind, Rect, Side};

/// What the preview layer shows.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Preview {
    #[default]
    None,
    /// Corner drag inward: dashed line where the area will be split.
    SplitLine { from: (f64, f64), to: (f64, f64) },
    /// Corner drag outward: the neighbour to be absorbed, darkened, with
    /// an arrow pointing in the direction of the join.
    Join { rect: Rect, toward: Side },
    /// Header or tab drag: where it will land, the hovered area (whose
    /// five drop zones are outlined) and the center action ("Tab",
    /// "Swap", "Replace") when over the center.
    Drop {
        rect: Rect,
        area: Option<Rect>,
        label: Option<&'static str>,
    },
    /// Tab drag over a tab strip: insertion line.
    Insert { x: f64, y0: f64, y1: f64 },
}

/// A non-interactive drawing layer for [`Preview`]s; returns the layer and
/// the state it draws (call `queue_draw` after changing it).
pub fn layer() -> (gtk::DrawingArea, Rc<Cell<Preview>>) {
    let state = Rc::new(Cell::new(Preview::None));
    let area = gtk::DrawingArea::builder()
        .can_target(false)
        .can_focus(false)
        .hexpand(true)
        .vexpand(true)
        .css_classes(["dock-layer"])
        .build();
    let s = state.clone();
    area.set_draw_func(move |area, cr, _, _| {
        let accent = area.color();
        let (r, g, b) = (
            f64::from(accent.red()),
            f64::from(accent.green()),
            f64::from(accent.blue()),
        );
        match s.get() {
            Preview::None => {}
            Preview::SplitLine { from, to } => {
                cr.set_source_rgba(r, g, b, 1.0);
                cr.set_line_width(2.0);
                cr.set_dash(&[7.0, 5.0], 0.0);
                cr.move_to(from.0, from.1);
                cr.line_to(to.0, to.1);
                let _ = cr.stroke();
            }
            Preview::Join { rect, toward } => {
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.5);
                cr.rectangle(rect.x, rect.y, rect.w, rect.h);
                let _ = cr.fill();
                draw_arrow(cr, rect, toward, (r, g, b));
            }
            Preview::Insert { x, y0, y1 } => {
                cr.set_source_rgba(r, g, b, 1.0);
                cr.set_line_width(3.0);
                cr.move_to(x, y0);
                cr.line_to(x, y1);
                let _ = cr.stroke();
            }
            Preview::Drop {
                rect,
                area: hovered,
                label,
            } => {
                if let Some(a) = hovered {
                    // The five zones: four 25% edge bands and the center.
                    cr.set_source_rgba(r, g, b, 0.45);
                    cr.set_line_width(1.0);
                    cr.set_dash(&[3.0, 3.0], 0.0);
                    let (bx, by) = (a.w * 0.25, a.h * 0.25);
                    cr.rectangle(
                        a.x + bx + 0.5,
                        a.y + by + 0.5,
                        a.w - 2.0 * bx,
                        a.h - 2.0 * by,
                    );
                    for (x0, y0, x1, y1) in [
                        (a.x, a.y, a.x + bx, a.y + by),
                        (a.x + a.w, a.y, a.x + a.w - bx, a.y + by),
                        (a.x, a.y + a.h, a.x + bx, a.y + a.h - by),
                        (a.x + a.w, a.y + a.h, a.x + a.w - bx, a.y + a.h - by),
                    ] {
                        cr.move_to(x0, y0);
                        cr.line_to(x1, y1);
                    }
                    let _ = cr.stroke();
                    cr.set_dash(&[], 0.0);
                }
                cr.set_source_rgba(r, g, b, 0.22);
                cr.rectangle(rect.x, rect.y, rect.w, rect.h);
                let _ = cr.fill();
                cr.set_source_rgba(r, g, b, 0.95);
                cr.set_line_width(2.0);
                cr.rectangle(rect.x + 1.0, rect.y + 1.0, rect.w - 2.0, rect.h - 2.0);
                let _ = cr.stroke();
                if let Some(text) = label {
                    draw_label(area, cr, rect, text, (r, g, b));
                }
            }
        }
    });
    (area, state)
}

/// A pill with `text` in the middle of `rect` (the center drop action).
fn draw_label(
    widget: &gtk::DrawingArea,
    cr: &gtk::cairo::Context,
    rect: Rect,
    text: &str,
    accent: (f64, f64, f64),
) {
    let layout = widget.create_pango_layout(Some(text));
    let mut font = widget
        .pango_context()
        .font_description()
        .unwrap_or_default();
    font.set_weight(gtk::pango::Weight::Bold);
    font.set_size(font.size() * 13 / 10);
    layout.set_font_description(Some(&font));
    let (_, ink) = layout.pixel_extents();
    let (tw, th) = (f64::from(ink.width()), f64::from(ink.height()));
    let (pw, ph) = (tw + 28.0, th + 14.0);
    let (x, y) = (rect.x + (rect.w - pw) / 2.0, rect.y + (rect.h - ph) / 2.0);
    let rad = ph / 2.0;
    cr.new_sub_path();
    cr.arc(x + pw - rad, y + rad, rad, -PI / 2.0, PI / 2.0);
    cr.arc(x + rad, y + rad, rad, PI / 2.0, 3.0 * PI / 2.0);
    cr.close_path();
    cr.set_source_rgba(accent.0, accent.1, accent.2, 0.95);
    let _ = cr.fill();
    // Dark text on the accent, like the accent buttons.
    cr.set_source_rgba(0.08, 0.06, 0.03, 1.0);
    cr.move_to(x + 14.0 - f64::from(ink.x()), y + 7.0 - f64::from(ink.y()));
    pangocairo::functions::show_layout(cr, &layout);
}

/// A big arrow centered in `rect`, pointing toward `side`.
fn draw_arrow(cr: &gtk::cairo::Context, rect: Rect, toward: Side, accent: (f64, f64, f64)) {
    let s = (rect.w.min(rect.h) * 0.4).clamp(28.0, 110.0);
    let angle = match toward {
        Side::Right => 0.0,
        Side::Bottom => PI / 2.0,
        Side::Left => PI,
        Side::Top => -PI / 2.0,
    };
    let _ = cr.save();
    cr.translate(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    cr.rotate(angle);
    cr.move_to(-s / 2.0, -s / 9.0);
    cr.line_to(s / 8.0, -s / 9.0);
    cr.line_to(s / 8.0, -s / 3.0);
    cr.line_to(s / 2.0, 0.0);
    cr.line_to(s / 8.0, s / 3.0);
    cr.line_to(s / 8.0, s / 9.0);
    cr.line_to(-s / 2.0, s / 9.0);
    cr.close_path();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(accent.0, accent.1, accent.2, 1.0);
    cr.set_line_width(2.0);
    let _ = cr.stroke();
    let _ = cr.restore();
}

/// The ghost shown under the pointer while dragging an area by its
/// header: a live thumbnail of the area with its editor's name.
pub fn ghost(area: &gtk::Widget, kind: EditorKind) -> gtk::Widget {
    let picture = gtk::Picture::builder()
        .paintable(&gtk::WidgetPaintable::new(Some(area)))
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Contain)
        .width_request(176)
        .height_request(104)
        .css_classes(["dock-ghost-thumb"])
        .build();
    let title = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(5)
        .build();
    title.append(&gtk::Image::from_icon_name(crate::editors::icon_name(kind)));
    title.append(&gtk::Label::new(Some(kind.title())));
    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .css_classes(["dock-ghost"])
        .opacity(0.85)
        .build();
    b.append(&picture);
    b.append(&title);
    b.upcast()
}

/// "Area closed · Undo" message at the bottom of the main area tree.
pub struct Toast {
    revealer: gtk::Revealer,
    label: gtk::Label,
    undo: gtk::Button,
    timer: RefCell<Option<glib::SourceId>>,
}

impl Toast {
    pub fn new() -> Rc<Self> {
        let label = gtk::Label::builder().css_classes(["toast-label"]).build();
        let undo = gtk::Button::builder()
            .label("Undo")
            .tooltip_text("Undo Layout Change")
            .css_classes(["toast-undo"])
            .build();
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Dismiss")
            .css_classes(["toast-close"])
            .build();
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .css_classes(["layout-toast"])
            .build();
        row.append(&label);
        row.append(&undo);
        row.append(&close);
        let revealer = gtk::Revealer::builder()
            .child(&row)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .margin_bottom(14)
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .build();
        let toast = Rc::new(Self {
            revealer,
            label,
            undo,
            timer: RefCell::default(),
        });
        let weak = Rc::downgrade(&toast);
        close.connect_clicked(move |_| {
            if let Some(t) = weak.upgrade() {
                t.hide();
            }
        });
        toast
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.revealer
    }

    pub fn connect_undo(&self, f: impl Fn() + 'static) {
        self.undo.connect_clicked(move |_| f());
    }

    /// Shows `text` for a few seconds, with an Undo button if `undo`.
    pub fn show(self: &Rc<Self>, text: &str, undo: bool) {
        self.label.set_label(text);
        self.undo.set_visible(undo);
        self.revealer.set_reveal_child(true);
        if let Some(id) = self.timer.take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(std::time::Duration::from_secs(6), move || {
            if let Some(t) = weak.upgrade() {
                t.timer.take();
                t.revealer.set_reveal_child(false);
            }
        });
        self.timer.replace(Some(id));
    }

    pub fn hide(&self) {
        if let Some(id) = self.timer.take() {
            id.remove();
        }
        self.revealer.set_reveal_child(false);
    }
}

impl Drop for Toast {
    fn drop(&mut self) {
        if let Some(id) = self.timer.take() {
            id.remove();
        }
    }
}

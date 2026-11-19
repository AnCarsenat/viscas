//! Cairo rendering of the Cartesian grid.

use adw::prelude::*;
use gtk::{cairo, gdk, pango};

use super::view::{View, format_tick, grid_steps, ticks};

/// Minimum distance between major gridlines, in logical pixels.
const MIN_MAJOR_PX: f64 = 90.0;
/// Gap between an axis and its tick labels.
const LABEL_GAP: f64 = 4.0;
/// Margin kept between labels and the widget edge when an axis is off-screen.
const EDGE_MARGIN: f64 = 4.0;

struct Palette {
    minor: gdk::RGBA,
    major: gdk::RGBA,
    axis: gdk::RGBA,
    label: gdk::RGBA,
}

impl Palette {
    /// Derives all grid colors from the widget's foreground color, so the
    /// grid follows light, dark and high-contrast styles automatically.
    fn from_widget(widget: &impl IsA<gtk::Widget>) -> Self {
        let fg = widget.as_ref().color();
        let hc = adw::StyleManager::default().is_high_contrast();
        let with_alpha = |a: f32| {
            let mut c = fg;
            c.set_alpha(fg.alpha() * if hc { (a * 2.0).min(1.0) } else { a });
            c
        };
        Self {
            minor: with_alpha(0.06),
            major: with_alpha(0.15),
            axis: with_alpha(0.6),
            label: with_alpha(0.7),
        }
    }
}

/// Snaps a coordinate to the center of a device pixel for crisp 1px lines.
fn crisp(v: f64) -> f64 {
    v.floor() + 0.5
}

pub fn draw_grid(widget: &gtk::DrawingArea, cr: &cairo::Context, view: &View) {
    let palette = Palette::from_widget(widget);
    let (w, h) = view.size;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let bounds = view.bounds();
    let steps = grid_steps(view.scale, MIN_MAJOR_PX);
    let per_major = (steps.major / steps.minor).round() as i64;

    cr.set_line_width(1.0);

    // Minor lines (skipping those that coincide with major lines).
    cr.set_source_color(&palette.minor);
    for (i, x) in ticks(bounds.x_min, bounds.x_max, steps.minor) {
        if i % per_major != 0 {
            let sx = crisp(view.world_to_screen(x, 0.0).0);
            cr.move_to(sx, 0.0);
            cr.line_to(sx, h);
        }
    }
    for (i, y) in ticks(bounds.y_min, bounds.y_max, steps.minor) {
        if i % per_major != 0 {
            let sy = crisp(view.world_to_screen(0.0, y).1);
            cr.move_to(0.0, sy);
            cr.line_to(w, sy);
        }
    }
    stroke(cr);

    // Major lines (the axes themselves are drawn on top afterwards).
    cr.set_source_color(&palette.major);
    for (i, x) in ticks(bounds.x_min, bounds.x_max, steps.major) {
        if i != 0 {
            let sx = crisp(view.world_to_screen(x, 0.0).0);
            cr.move_to(sx, 0.0);
            cr.line_to(sx, h);
        }
    }
    for (i, y) in ticks(bounds.y_min, bounds.y_max, steps.major) {
        if i != 0 {
            let sy = crisp(view.world_to_screen(0.0, y).1);
            cr.move_to(0.0, sy);
            cr.line_to(w, sy);
        }
    }
    stroke(cr);

    // Axes.
    let (ox, oy) = view.world_to_screen(0.0, 0.0);
    cr.set_source_color(&palette.axis);
    cr.set_line_width(1.5);
    if (0.0..=w).contains(&ox) {
        cr.move_to(ox, 0.0);
        cr.line_to(ox, h);
    }
    if (0.0..=h).contains(&oy) {
        cr.move_to(0.0, oy);
        cr.line_to(w, oy);
    }
    stroke(cr);

    // Tick labels, kept on screen when the axis scrolls out of view.
    cr.set_source_color(&palette.label);
    let attrs = pango::AttrList::new();
    attrs.insert(pango::AttrFloat::new_scale(0.85));
    attrs.insert(pango::AttrFontFeatures::new("tnum 1"));
    let layout = widget.create_pango_layout(None);
    layout.set_attributes(Some(&attrs));
    let text_size = |text: &str| {
        layout.set_text(text);
        let (tw, th) = layout.pixel_size();
        (f64::from(tw), f64::from(th))
    };

    for (i, x) in ticks(bounds.x_min, bounds.x_max, steps.major) {
        if i == 0 {
            continue;
        }
        let (tw, th) = text_size(&format_tick(x, steps.major));
        let sx = view.world_to_screen(x, 0.0).0 - tw / 2.0;
        let sy = (oy + LABEL_GAP).clamp(EDGE_MARGIN, h - th - EDGE_MARGIN);
        show_layout(cr, &layout, sx, sy);
    }
    for (i, y) in ticks(bounds.y_min, bounds.y_max, steps.major) {
        if i == 0 {
            continue;
        }
        let (tw, th) = text_size(&format_tick(y, steps.major));
        let sx = (ox - LABEL_GAP - tw).clamp(EDGE_MARGIN, w - tw - EDGE_MARGIN);
        let sy = view.world_to_screen(0.0, y).1 - th / 2.0;
        show_layout(cr, &layout, sx, sy);
    }
    if (0.0..=w).contains(&ox) && (0.0..=h).contains(&oy) {
        let (tw, _) = text_size("0");
        show_layout(cr, &layout, ox - LABEL_GAP - tw, oy + LABEL_GAP);
    }
}

fn show_layout(cr: &cairo::Context, layout: &pango::Layout, x: f64, y: f64) {
    cr.move_to(x, y);
    pangocairo::functions::show_layout(cr, layout);
}

fn stroke(cr: &cairo::Context) {
    // A failed stroke only means the cairo context is in an error state;
    // there is nothing useful to do about it mid-frame.
    let _ = cr.stroke();
}

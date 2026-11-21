//! Cairo rendering of the Cartesian grid layer.

use gtk::prelude::*;
use gtk::{cairo, pango};

use super::Layer;
use super::view::{View, format_tick, grid_steps, ticks};

/// Minimum distance between major gridlines, in logical pixels.
const MIN_MAJOR_PX: f64 = 90.0;
/// Gap between an axis and its tick labels.
const LABEL_GAP: f64 = 4.0;
/// Margin kept between labels and the widget edge when an axis is off-screen.
const EDGE_MARGIN: f64 = 4.0;

/// An RGBA color with components in `0..=1`.
#[derive(Clone, Copy)]
struct Rgba(f64, f64, f64, f64);

impl Rgba {
    const fn hex(rgb: u32, a: f64) -> Self {
        Self(
            ((rgb >> 16) & 0xff) as f64 / 255.0,
            ((rgb >> 8) & 0xff) as f64 / 255.0,
            (rgb & 0xff) as f64 / 255.0,
            a,
        )
    }

    fn set(self, cr: &cairo::Context) {
        cr.set_source_rgba(self.0, self.1, self.2, self.3);
    }
}

/// Grid colors, mirroring the palette in `resources/theme.css`: a dark
/// canvas, barely-there minor lines, brighter major lines and neutral
/// (accent-free) axes. The accent color is reserved for selection.
struct Palette {
    background: Rgba,
    minor: Rgba,
    major: Rgba,
    axis: Rgba,
    label: Rgba,
}

const DARK: Palette = Palette {
    background: Rgba::hex(0x1a1a1a, 1.0),
    minor: Rgba::hex(0xffffff, 0.035),
    major: Rgba::hex(0xffffff, 0.085),
    axis: Rgba::hex(0x8c8c8c, 0.9),
    label: Rgba::hex(0x8a8a8a, 1.0),
};

/// Monospace font for tick labels (Pango picks the first installed family).
const LABEL_FONT: &str =
    "JetBrains Mono, JetBrainsMono Nerd Font, Hack, DejaVu Sans Mono, monospace 8";

/// Snaps a coordinate to the center of a device pixel for crisp 1px lines.
fn crisp(v: f64) -> f64 {
    v.floor() + 0.5
}

/// Clamps without panicking when `lo > hi` (tiny widgets): `hi` wins.
fn clamp_soft(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

/// Background layer: minor/major gridlines, axes and tick labels.
pub struct GridLayer;

impl Layer for GridLayer {
    fn draw(&self, widget: &gtk::Widget, cr: &cairo::Context, view: &View) {
        draw_grid(widget, cr, view);
    }
}

fn draw_grid(widget: &gtk::Widget, cr: &cairo::Context, view: &View) {
    let palette = &DARK;
    let (w, h) = view.size;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    palette.background.set(cr);
    let _ = cr.paint();
    let bounds = view.bounds();
    let steps = grid_steps(view.scale, MIN_MAJOR_PX);
    let per_major = (steps.major / steps.minor).round() as i64;

    cr.set_line_width(1.0);

    // Minor lines (skipping those that coincide with major lines).
    palette.minor.set(cr);
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
    palette.major.set(cr);
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
    palette.axis.set(cr);
    cr.set_line_width(1.0);
    let (ox_px, oy_px) = (crisp(ox), crisp(oy));
    if (0.0..=w).contains(&ox) {
        cr.move_to(ox_px, 0.0);
        cr.line_to(ox_px, h);
    }
    if (0.0..=h).contains(&oy) {
        cr.move_to(0.0, oy_px);
        cr.line_to(w, oy_px);
    }
    stroke(cr);

    // Tick labels, kept on screen when the axis scrolls out of view.
    palette.label.set(cr);
    let attrs = pango::AttrList::new();
    attrs.insert(pango::AttrFontFeatures::new("tnum 1"));
    let layout = widget.create_pango_layout(None);
    layout.set_font_description(Some(&pango::FontDescription::from_string(LABEL_FONT)));
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
        let sy = clamp_soft(oy + LABEL_GAP, EDGE_MARGIN, h - th - EDGE_MARGIN);
        show_layout(cr, &layout, sx, sy);
    }
    for (i, y) in ticks(bounds.y_min, bounds.y_max, steps.major) {
        if i == 0 {
            continue;
        }
        let (tw, th) = text_size(&format_tick(y, steps.major));
        let sx = clamp_soft(ox - LABEL_GAP - tw, EDGE_MARGIN, w - tw - EDGE_MARGIN);
        let sy = view.world_to_screen(0.0, y).1 - th / 2.0;
        show_layout(cr, &layout, sx, sy);
    }
    if (0.0..=w).contains(&ox) && (0.0..=h).contains(&oy) {
        let (tw, th) = text_size("0");
        let sx = clamp_soft(ox - LABEL_GAP - tw, EDGE_MARGIN, w - tw - EDGE_MARGIN);
        let sy = clamp_soft(oy + LABEL_GAP, EDGE_MARGIN, h - th - EDGE_MARGIN);
        show_layout(cr, &layout, sx, sy);
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

#[cfg(test)]
mod tests {
    use super::clamp_soft;

    #[test]
    fn clamp_soft_never_panics() {
        assert_eq!(clamp_soft(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clamp_soft(-1.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp_soft(5.0, 4.0, -3.0), -3.0);
    }
}

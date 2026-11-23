//! Cairo rendering of the Cartesian grid layer.

use gtk::prelude::*;
use gtk::{cairo, pango};

use super::Layer;
use super::view::{View, format_tick, grid_steps, ticks};
use crate::ctx::AppCtx;
use crate::prefs::{GridStyle, Preferences};
use crate::theme::model::Color;

/// Minimum distance between major gridlines, in logical pixels.
const MIN_MAJOR_PX: f64 = 90.0;
/// Gap between an axis and its tick labels.
const LABEL_GAP: f64 = 4.0;
/// Margin kept between labels and the widget edge when an axis is off-screen.
const EDGE_MARGIN: f64 = 4.0;

/// Sets `color` as the cairo source.
fn set(cr: &cairo::Context, color: Color) {
    let (r, g, b, a) = color.rgba_f64();
    cr.set_source_rgba(r, g, b, a);
}

/// Fallback monospace families for tick labels after the user's choice.
const LABEL_FONT_FALLBACK: &str =
    "JetBrains Mono, JetBrainsMono Nerd Font, Hack, DejaVu Sans Mono, monospace";
/// Tick label size in points at UI scale 1.
const LABEL_POINTS: f64 = 8.0;

/// Pango font description for tick labels.
fn label_font(prefs: &Preferences) -> String {
    let family = prefs.interface.mono_font.replace(',', " ");
    let size = LABEL_POINTS * prefs.interface.ui_scale * prefs.interface.font_size / 12.0;
    if family.is_empty() {
        format!("{LABEL_FONT_FALLBACK} {size:.1}")
    } else {
        format!("{family}, {LABEL_FONT_FALLBACK} {size:.1}")
    }
}

/// Snaps a coordinate so a line of width `width` covers whole pixels.
fn crisp(v: f64, width: f64) -> f64 {
    if (width.round() as i64) % 2 == 1 {
        v.floor() + 0.5
    } else {
        v.round()
    }
}

/// Clamps without panicking when `lo > hi` (tiny widgets): `hi` wins.
fn clamp_soft(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

/// Background layer: minor/major gridlines, axes and tick labels, in the
/// theme's viewport colors and following the viewport preferences.
pub struct GridLayer {
    pub ctx: AppCtx,
}

impl Layer for GridLayer {
    fn draw(&self, widget: &gtk::Widget, cr: &cairo::Context, view: &View) {
        self.ctx
            .preferences()
            .with(|prefs| draw_grid(widget, cr, view, prefs));
    }
}

/// Draws gridlines (or dots) at multiples of `step`, skipping indices
/// `skip` rejects.
fn grid_pass(
    cr: &cairo::Context,
    view: &View,
    step: f64,
    style: GridStyle,
    width: f64,
    skip: impl Fn(i64) -> bool,
) {
    let (w, h) = view.size;
    let b = view.bounds();
    let xs: Vec<f64> = ticks(b.x_min, b.x_max, step)
        .filter(|(i, _)| !skip(*i))
        .map(|(_, x)| crisp(view.world_to_screen(x, 0.0).0, width))
        .collect();
    let ys: Vec<f64> = ticks(b.y_min, b.y_max, step)
        .filter(|(i, _)| !skip(*i))
        .map(|(_, y)| crisp(view.world_to_screen(0.0, y).1, width))
        .collect();
    match style {
        GridStyle::Lines => {
            for sx in &xs {
                cr.move_to(*sx, 0.0);
                cr.line_to(*sx, h);
            }
            for sy in &ys {
                cr.move_to(0.0, *sy);
                cr.line_to(w, *sy);
            }
            stroke(cr);
        }
        GridStyle::Dots => {
            let r = (width * 1.1).max(0.75);
            for sx in &xs {
                for sy in &ys {
                    cr.rectangle(sx - r, sy - r, 2.0 * r, 2.0 * r);
                }
            }
            let _ = cr.fill();
        }
    }
}

fn draw_grid(widget: &gtk::Widget, cr: &cairo::Context, view: &View, prefs: &Preferences) {
    let theme = prefs.effective_theme();
    let palette = &theme.viewport;
    let opts = &prefs.viewport;
    let (w, h) = view.size;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    cr.set_antialias(if opts.antialias {
        cairo::Antialias::Default
    } else {
        cairo::Antialias::None
    });
    set(cr, palette.background);
    let _ = cr.paint();
    let bounds = view.bounds();
    let steps = grid_steps(view.scale, MIN_MAJOR_PX);
    let per_major = ((steps.major / steps.minor).round() as i64).max(1);
    let width = opts.line_width;
    cr.set_line_width(width);

    if opts.grid {
        // Minor lines (skipping those that coincide with major lines);
        // major lines skip the axes, drawn on top afterwards.
        if opts.minor_grid {
            set(cr, palette.grid_minor);
            grid_pass(cr, view, steps.minor, opts.grid_style, width, |i| {
                i % per_major == 0
            });
        }
        set(cr, palette.grid_major);
        grid_pass(cr, view, steps.major, opts.grid_style, width, |i| i == 0);
    }

    // Axes.
    let (ox, oy) = view.world_to_screen(0.0, 0.0);
    set(cr, palette.axis);
    cr.set_line_width(width);
    let (ox_px, oy_px) = (crisp(ox, width), crisp(oy, width));
    if (0.0..=w).contains(&ox) {
        cr.move_to(ox_px, 0.0);
        cr.line_to(ox_px, h);
    }
    if (0.0..=h).contains(&oy) {
        cr.move_to(0.0, oy_px);
        cr.line_to(w, oy_px);
    }
    stroke(cr);

    if !opts.axis_labels {
        return;
    }
    // Tick labels, kept on screen when the axis scrolls out of view.
    set(cr, palette.label);
    let attrs = pango::AttrList::new();
    attrs.insert(pango::AttrFontFeatures::new("tnum 1"));
    let layout = widget.create_pango_layout(None);
    layout.set_font_description(Some(&pango::FontDescription::from_string(&label_font(
        prefs,
    ))));
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
    use super::{clamp_soft, crisp, label_font};
    use crate::prefs::Preferences;

    #[test]
    fn crisp_lines_cover_whole_pixels() {
        assert_eq!(crisp(10.3, 1.0), 10.5);
        assert_eq!(crisp(10.3, 2.0), 10.0);
        assert_eq!(crisp(10.7, 2.0), 11.0);
    }

    #[test]
    fn label_font_follows_preferences() {
        let mut p = Preferences::default();
        assert!(label_font(&p).ends_with(" 8.0"));
        p.interface.mono_font = "Hack".into();
        p.interface.ui_scale = 1.5;
        let f = label_font(&p);
        assert!(f.starts_with("Hack, "));
        assert!(f.ends_with(" 12.0"));
    }

    #[test]
    fn clamp_soft_never_panics() {
        assert_eq!(clamp_soft(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clamp_soft(-1.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp_soft(5.0, 4.0, -3.0), -3.0);
    }
}

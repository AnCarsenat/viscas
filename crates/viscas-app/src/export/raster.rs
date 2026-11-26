//! Cairo rendering of a [`Drawing`]: PNG, PDF, PostScript and EPS. Runs
//! on any thread (every cairo and pango object is created here).

use std::f64::consts::TAU;

use gtk::cairo;
use pangocairo::pango;
use viscas_plot::{Primitive, Vec2};
use viscas_scene::Color;

use super::drawing::{Anchor, Drawing, alpha};
use super::settings::Unit;

fn set(cr: &cairo::Context, c: Color, k: f64) {
    cr.set_source_rgba(
        f64::from(c.r) / 255.0,
        f64::from(c.g) / 255.0,
        f64::from(c.b) / 255.0,
        alpha(c, k),
    );
}

/// Paints `d` on `cr` in page pixels.
pub fn paint(cr: &cairo::Context, d: &Drawing) {
    let (w, h) = d.size;
    if let Some(bg) = d.background {
        set(cr, bg, 1.0);
        cr.rectangle(0.0, 0.0, w, h);
        let _ = cr.fill();
    }
    cr.save().ok();
    cr.rectangle(0.0, 0.0, w, h);
    cr.clip();
    paint_grid(cr, d);
    paint_plots(cr, d);
    cr.restore().ok();
    paint_labels(cr, d);
    paint_legend(cr, d);
}

fn paint_grid(cr: &cairo::Context, d: &Drawing) {
    let (w, h) = d.size;
    let (tx, ty) = d.ticks();
    cr.set_line_width(d.grid_width);
    let lines = |xs: &mut dyn Iterator<Item = f64>, ys: &mut dyn Iterator<Item = f64>| {
        for x in xs {
            let px = d.to_page(Vec2::new(x, 0.0)).0;
            cr.move_to(px, 0.0);
            cr.line_to(px, h);
        }
        for y in ys {
            let py = d.to_page(Vec2::new(0.0, y)).1;
            cr.move_to(0.0, py);
            cr.line_to(w, py);
        }
        let _ = cr.stroke();
    };
    if d.grid {
        if d.minor_grid {
            set(cr, d.colors.minor, 1.0);
            lines(&mut tx.minor.iter().copied(), &mut ty.minor.iter().copied());
        }
        set(cr, d.colors.major, 1.0);
        lines(
            &mut tx.major.iter().copied().filter(|x| *x != 0.0),
            &mut ty.major.iter().copied().filter(|y| *y != 0.0),
        );
    }
    if d.axes {
        let (ox, oy) = d.origin();
        set(cr, d.colors.axis, 1.0);
        if (0.0..=w).contains(&ox) {
            cr.move_to(ox, 0.0);
            cr.line_to(ox, h);
        }
        if (0.0..=h).contains(&oy) {
            cr.move_to(0.0, oy);
            cr.line_to(w, oy);
        }
        let _ = cr.stroke();
    }
}

/// Adds `pts` to the path; returns the number of points and whether an
/// undefined point split it (each split starts a new subpath).
fn trace(cr: &cairo::Context, d: &Drawing, pts: &[Vec2]) -> (usize, bool) {
    let (mut n, mut split, mut pen) = (0, false, false);
    for q in pts {
        let (x, y) = d.to_page(*q);
        if !(q.is_finite() && x.is_finite() && y.is_finite()) {
            split |= n > 0;
            pen = false;
            continue;
        }
        if pen {
            cr.line_to(x, y);
        } else {
            cr.move_to(x, y);
            pen = true;
        }
        n += 1;
    }
    (n, split)
}

fn paint_plots(cr: &cairo::Context, d: &Drawing) {
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    for layer in &d.layers {
        let st = &layer.style;
        // Fills: one path, nonzero (cells never overlap).
        cr.set_fill_rule(cairo::FillRule::Winding);
        let mut any = false;
        for p in layer.primitives.iter() {
            if let Primitive::Polygon(poly) = p
                && trace(cr, d, &poly.points).0 >= 3
            {
                cr.close_path();
                any = true;
            }
        }
        if any {
            set(cr, st.color, st.fill_opacity * st.opacity);
            let _ = cr.fill();
        }
        cr.new_path();
        cr.set_line_width(d.stroke_width(st));
        for p in layer.primitives.iter() {
            if let Primitive::Polyline { line, style } = p {
                let (n, split) = trace(cr, d, &line.points);
                if n < 2 {
                    cr.new_path();
                    continue;
                }
                if line.closed && !split {
                    cr.close_path();
                }
                cr.set_dash(&d.dash(st, *style), 0.0);
                set(cr, st.color, st.opacity);
                let _ = cr.stroke();
            }
        }
        cr.set_dash(&[], 0.0);
        let r = d.point_radius(st);
        for p in layer.primitives.iter() {
            if let Primitive::Points(pts) = p {
                for q in pts.iter().filter(|q| q.is_finite()) {
                    let (x, y) = d.to_page(*q);
                    cr.new_sub_path();
                    cr.arc(x, y, r, 0.0, TAU);
                }
                set(cr, st.color, st.opacity);
                let _ = cr.fill();
            }
        }
    }
}

fn layout(cr: &cairo::Context, d: &Drawing) -> pango::Layout {
    let layout = pangocairo::functions::create_layout(cr);
    let mut font = pango::FontDescription::from_string(&d.font_family);
    font.set_absolute_size(d.font_px * f64::from(pango::SCALE));
    layout.set_font_description(Some(&font));
    layout
}

/// Shows `text` with its baseline at `y`.
fn show(cr: &cairo::Context, layout: &pango::Layout, text: &str, x: f64, y: f64, anchor: Anchor) {
    layout.set_text(text);
    let (tw, _) = layout.pixel_size();
    let baseline = f64::from(layout.baseline()) / f64::from(pango::SCALE);
    let tw = f64::from(tw);
    let x = match anchor {
        Anchor::Start => x,
        Anchor::Middle => x - tw / 2.0,
        Anchor::End => x - tw,
    };
    cr.move_to(x, y - baseline);
    pangocairo::functions::show_layout(cr, layout);
}

fn paint_labels(cr: &cairo::Context, d: &Drawing) {
    let labels = d.tick_labels();
    if labels.is_empty() {
        return;
    }
    let layout = layout(cr, d);
    set(cr, d.colors.label, 1.0);
    for l in labels {
        show(cr, &layout, &l.text, l.x, l.y, l.anchor);
    }
}

fn paint_legend(cr: &cairo::Context, d: &Drawing) {
    let Some((x, y, w, row)) = d.legend_box() else {
        return;
    };
    let entries = d.legend_entries();
    let bg = d.background.unwrap_or(Color::rgb(255, 255, 255));
    cr.rectangle(x, y, w, row * entries.len() as f64 + 8.0);
    set(cr, bg, 0.85);
    let _ = cr.fill_preserve();
    set(cr, d.colors.major, 1.0);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    let layout = layout(cr, d);
    for (i, (color, dash, text)) in entries.iter().enumerate() {
        let cy = y + 4.0 + row * (i as f64 + 0.5);
        cr.set_dash(dash, 0.0);
        cr.set_line_width(2.0 * d.line_scale);
        set(cr, *color, 1.0);
        cr.move_to(x + 8.0, cy);
        cr.line_to(x + 30.0, cy);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);
        set(cr, d.colors.label, 1.0);
        show(
            cr,
            &layout,
            text,
            x + 38.0,
            cy + d.font_px * 0.35,
            Anchor::Start,
        );
    }
}

/// PNG bytes of `d` at `px` device pixels.
/// Largest PNG accepted, in pixels.
pub const MAX_PNG_PIXELS: u64 = 100_000_000;
/// Largest PNG side cairo supports.
pub const MAX_PNG_SIDE: u32 = 32_767;

/// Why a PNG of `px` pixels is refused, if it is.
pub fn png_size_error(px: (u32, u32)) -> Option<String> {
    let total = u64::from(px.0) * u64::from(px.1);
    if px.0 > MAX_PNG_SIDE || px.1 > MAX_PNG_SIDE || total > MAX_PNG_PIXELS {
        Some(format!(
            "PNG too large: {} × {} px (limit {} MP, {} px per side) — lower the size or DPI",
            px.0,
            px.1,
            MAX_PNG_PIXELS / 1_000_000,
            MAX_PNG_SIDE
        ))
    } else {
        None
    }
}

pub fn png(d: &Drawing, px: (u32, u32)) -> Result<Vec<u8>, String> {
    if let Some(e) = png_size_error(px) {
        return Err(e);
    }
    let (pw, ph) = (px.0.max(1) as i32, px.1.max(1) as i32);
    let surface =
        cairo::ImageSurface::create(cairo::Format::ARgb32, pw, ph).map_err(|e| e.to_string())?;
    {
        let cr = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
        cr.scale(f64::from(pw) / d.size.0, f64::from(ph) / d.size.1);
        paint(&cr, d);
        cr.status()
            .map_err(|e| format!("drawing the PNG failed: {e}"))?;
    }
    let mut out = Vec::new();
    surface.write_to_png(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Vector document kinds cairo writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vector {
    Pdf,
    Ps,
    Eps,
}

/// PDF / PostScript / EPS bytes of `d` (page size in points).
pub fn vector(d: &Drawing, kind: Vector) -> Result<Vec<u8>, String> {
    let to_pt = Unit::Px.convert(1.0, Unit::Pt);
    let (w, h) = (d.size.0 * to_pt, d.size.1 * to_pt);
    let draw = |surface: &cairo::Surface| -> Result<(), String> {
        let cr = cairo::Context::new(surface).map_err(|e| e.to_string())?;
        cr.scale(to_pt, to_pt);
        paint(&cr, d);
        cr.status().map_err(|e| format!("drawing failed: {e}"))?;
        cr.show_page().map_err(|e| e.to_string())?;
        Ok(())
    };
    let finish = |surface: &cairo::Surface| -> Result<Vec<u8>, String> {
        let any = surface.finish_output_stream().map_err(|e| e.to_string())?;
        any.downcast::<Vec<u8>>()
            .map(|b| *b)
            .map_err(|_| "unexpected output stream".to_owned())
    };
    match kind {
        Vector::Pdf => {
            let s =
                cairo::PdfSurface::for_stream(w, h, Vec::<u8>::new()).map_err(|e| e.to_string())?;
            draw(&s)?;
            finish(&s)
        }
        Vector::Ps | Vector::Eps => {
            let s =
                cairo::PsSurface::for_stream(w, h, Vec::<u8>::new()).map_err(|e| e.to_string())?;
            s.set_eps(kind == Vector::Eps);
            draw(&s)?;
            finish(&s)
        }
    }
}

#[cfg(test)]
mod tests {
    use viscas_plot::{Polyline, Rect};

    use super::*;
    use crate::export::svg::tests::layer;

    fn drawing() -> Drawing {
        let mut d = Drawing::new(Rect::new(-1.0, -1.0, 1.0, 1.0), (120.0, 80.0));
        d.legend = true;
        d.layers.push(layer(vec![Primitive::Polyline {
            line: Polyline::open(vec![Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0)]),
            style: Default::default(),
        }]));
        d
    }

    #[test]
    fn png_has_signature_and_size() {
        let bytes = png(&drawing(), (240, 160)).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        // IHDR width/height (big endian) at bytes 16..24.
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 240);
        assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 160);
    }

    #[test]
    fn huge_pngs_are_refused() {
        assert!(png_size_error((10_000, 10_000)).is_none());
        assert!(png_size_error((10_001, 10_000)).is_some());
        assert!(png_size_error((40_000, 10)).is_some());
        let e = png(&drawing(), (20_000, 20_000)).unwrap_err();
        assert!(e.contains("too large"), "{e}");
    }

    #[test]
    fn vector_documents_have_headers() {
        let d = drawing();
        assert!(vector(&d, Vector::Pdf).unwrap().starts_with(b"%PDF-"));
        assert!(vector(&d, Vector::Ps).unwrap().starts_with(b"%!PS-Adobe"));
        let eps = vector(&d, Vector::Eps).unwrap();
        assert!(String::from_utf8_lossy(&eps[..40]).contains("EPSF"));
    }
}

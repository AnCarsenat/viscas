//! Format-independent description of an exported picture: page size,
//! world box, colors, grid and the re-sampled plot layers. The SVG, TikZ
//! and cairo writers all render a [`Drawing`].

use std::sync::Arc;

use viscas_engine::{PlotKind, PlotStyle};
use viscas_plot::{LineStyle, Primitive, Rect, Ticks, Vec2};
use viscas_scene::Color;

use crate::viewport::view::format_tick;

/// Minimum distance between major gridlines, in page pixels (as on screen).
pub const MIN_MAJOR_PX: f64 = 90.0;
/// Gap between an axis and its tick labels.
const LABEL_GAP: f64 = 4.0;
/// Margin kept between labels and the page edge.
const EDGE_MARGIN: f64 = 4.0;
/// Approximate advance of one label character, in ems (monospace).
pub const CHAR_EM: f64 = 0.6;

/// Grid, axis and text colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridColors {
    pub minor: Color,
    pub major: Color,
    pub axis: Color,
    pub label: Color,
}

impl Default for GridColors {
    fn default() -> Self {
        Self {
            minor: Color::rgb(0xe4, 0xe7, 0xec),
            major: Color::rgb(0xc4, 0xc9, 0xd2),
            axis: Color::rgb(0x55, 0x5c, 0x68),
            label: Color::rgb(0x40, 0x46, 0x50),
        }
    }
}

/// One node's geometry and style.
#[derive(Clone, Debug)]
pub struct DrawLayer {
    pub name: String,
    pub kind: PlotKind,
    pub style: PlotStyle,
    pub primitives: Arc<Vec<Primitive>>,
}

/// Text anchor of a label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

impl Anchor {
    pub fn svg(self) -> &'static str {
        match self {
            Anchor::Start => "start",
            Anchor::Middle => "middle",
            Anchor::End => "end",
        }
    }
}

/// A text label at a page position (`y` is the baseline).
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub anchor: Anchor,
}

/// An exported picture.
#[derive(Clone, Debug)]
pub struct Drawing {
    /// World box shown on the page.
    pub world: Rect,
    /// Page size in CSS pixels.
    pub size: (f64, f64),
    pub background: Option<Color>,
    pub colors: GridColors,
    pub grid: bool,
    pub minor_grid: bool,
    pub axes: bool,
    pub labels: bool,
    pub legend: bool,
    /// Width of grid lines and axes, in page pixels.
    pub grid_width: f64,
    /// Multiplies plot stroke widths.
    pub line_scale: f64,
    pub font_family: String,
    /// Label font size in page pixels.
    pub font_px: f64,
    pub layers: Vec<DrawLayer>,
}

impl Drawing {
    /// An empty drawing of `world` on a `size` page with default styling.
    pub fn new(world: Rect, size: (f64, f64)) -> Drawing {
        Drawing {
            world,
            size,
            background: Some(Color::rgb(255, 255, 255)),
            colors: GridColors::default(),
            grid: true,
            minor_grid: true,
            axes: true,
            labels: true,
            legend: false,
            grid_width: 1.0,
            line_scale: 1.0,
            font_family: "DejaVu Sans Mono, monospace".to_owned(),
            font_px: 11.0,
            layers: Vec::new(),
        }
    }

    /// Page x per world unit.
    pub fn sx(&self) -> f64 {
        self.size.0 / self.world.width()
    }

    /// Page y per world unit (positive; the page y axis points down).
    pub fn sy(&self) -> f64 {
        self.size.1 / self.world.height()
    }

    /// World → page.
    pub fn to_page(&self, p: Vec2) -> (f64, f64) {
        (
            (p.x - self.world.x_min) * self.sx(),
            (self.world.y_max - p.y) * self.sy(),
        )
    }

    /// Grid ticks of both axes.
    pub fn ticks(&self) -> (Ticks, Ticks) {
        let w = &self.world;
        (
            Ticks::for_range(w.x_min, w.x_max, MIN_MAJOR_PX / self.sx()),
            Ticks::for_range(w.y_min, w.y_max, MIN_MAJOR_PX / self.sy()),
        )
    }

    /// Stroke width of a layer in page pixels.
    pub fn stroke_width(&self, style: &PlotStyle) -> f64 {
        style.line_width.clamp(0.5, 12.0) * self.line_scale
    }

    /// Dash pattern (page pixels) of a polyline: the node style wins over
    /// the sampler hint (strict inequality boundaries are dashed).
    pub fn dash(&self, style: &PlotStyle, hint: LineStyle) -> Vec<f64> {
        let base: &[f64] = match style.line_style {
            viscas_scene::LineStyle::Solid => match hint {
                LineStyle::Solid => &[],
                LineStyle::Dashed => &[7.0, 5.0],
            },
            viscas_scene::LineStyle::Dashed => &[7.0, 5.0],
            viscas_scene::LineStyle::Dotted => &[1.0, 4.0],
        };
        base.iter().map(|d| d * self.line_scale).collect()
    }

    /// Point radius of a layer in page pixels.
    pub fn point_radius(&self, style: &PlotStyle) -> f64 {
        style.point_size.clamp(1.0, 20.0) * self.line_scale
    }

    /// Estimated width of a label (monospace advance).
    pub fn text_width(&self, text: &str) -> f64 {
        text.chars().count() as f64 * self.font_px * CHAR_EM
    }

    /// Page position of the axes (may lie off the page).
    pub fn origin(&self) -> (f64, f64) {
        self.to_page(Vec2::new(0.0, 0.0))
    }

    /// Tick labels, placed like the viewport (kept on the page when an
    /// axis scrolls off).
    pub fn tick_labels(&self) -> Vec<Label> {
        if !self.labels {
            return Vec::new();
        }
        let (w, h) = self.size;
        let (ox, oy) = self.origin();
        let (tx, ty) = self.ticks();
        let fh = self.font_px;
        let clamp = |v: f64, lo: f64, hi: f64| v.max(lo).min(hi);
        let mut out = Vec::new();
        for &x in tx.major.iter().filter(|x| **x != 0.0) {
            let text = format_tick(x, tx.major_step);
            let px = self.to_page(Vec2::new(x, 0.0)).0;
            let half = self.text_width(&text) / 2.0;
            if px - half < 0.0 || px + half > w {
                continue; // would be cut by the page edge
            }
            let top = clamp(oy + LABEL_GAP, EDGE_MARGIN, h - fh - EDGE_MARGIN);
            out.push(Label {
                text,
                x: px,
                y: top + fh * 0.8,
                anchor: Anchor::Middle,
            });
        }
        for &y in ty.major.iter().filter(|y| **y != 0.0) {
            let text = format_tick(y, ty.major_step);
            let tw = self.text_width(&text);
            let right = clamp(ox - LABEL_GAP, EDGE_MARGIN + tw, w - EDGE_MARGIN);
            let py = self.to_page(Vec2::new(0.0, y)).1;
            if py - fh * 0.65 < 0.0 || py + fh * 0.35 > h {
                continue;
            }
            out.push(Label {
                text,
                x: right,
                y: clamp(py + fh * 0.35, fh, h - EDGE_MARGIN),
                anchor: Anchor::End,
            });
        }
        if (0.0..=w).contains(&ox) && (0.0..=h).contains(&oy) {
            out.push(Label {
                text: "0".to_owned(),
                x: ox - LABEL_GAP,
                y: oy + LABEL_GAP + fh * 0.8,
                anchor: Anchor::End,
            });
        }
        out
    }

    /// Legend entries: color, dashed?, label text.
    pub fn legend_entries(&self) -> Vec<(Color, Vec<f64>, String)> {
        if !self.legend {
            return Vec::new();
        }
        self.layers
            .iter()
            .filter(|l| !l.primitives.is_empty())
            .map(|l| {
                let text = l
                    .style
                    .label
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| l.name.clone());
                (l.style.color, self.dash(&l.style, LineStyle::Solid), text)
            })
            .collect()
    }

    /// Legend box geometry: x, y, width, row height.
    pub fn legend_box(&self) -> Option<(f64, f64, f64, f64)> {
        let entries = self.legend_entries();
        if entries.is_empty() {
            return None;
        }
        let widest = entries
            .iter()
            .map(|(_, _, t)| self.text_width(t))
            .fold(0.0, f64::max);
        let row = self.font_px * 1.6;
        Some((10.0, 10.0, widest + 46.0, row))
    }
}

/// Formats a number with at most `decimals` decimals, trimming zeros
/// (`-0` becomes `0`).
pub fn num(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    };
    if s == "-0" { "0".to_owned() } else { s }
}

/// Decimals needed to resolve `1/100` of a page pixel at `units_per_px`.
pub fn decimals_for(units_per_px: f64) -> usize {
    if !(units_per_px.is_finite() && units_per_px > 0.0) {
        return 6;
    }
    (-(units_per_px / 100.0).log10()).ceil().clamp(0.0, 15.0) as usize
}

/// `#rrggbb` of a color.
pub fn hex(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

/// Alpha of a color in `[0, 1]` times `k`.
pub fn alpha(c: Color, k: f64) -> f64 {
    (f64::from(c.a) / 255.0 * k).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_transform_flips_y() {
        let d = Drawing::new(Rect::new(0.0, 0.0, 2.0, 1.0), (200.0, 100.0));
        assert_eq!(d.to_page(Vec2::new(0.0, 0.0)), (0.0, 100.0));
        assert_eq!(d.to_page(Vec2::new(2.0, 1.0)), (200.0, 0.0));
        assert_eq!(d.to_page(Vec2::new(1.0, 0.5)), (100.0, 50.0));
    }

    #[test]
    fn numbers_are_trimmed() {
        assert_eq!(num(1.5000, 3), "1.5");
        assert_eq!(num(2.0, 3), "2");
        assert_eq!(num(-0.0001, 2), "0");
        assert_eq!(num(-12.25, 1), "-12.2");
        assert_eq!(decimals_for(0.01), 4);
        assert_eq!(decimals_for(10.0), 1);
        assert_eq!(hex(Color::rgb(255, 0, 16)), "#ff0010");
    }

    #[test]
    fn labels_skip_origin_ticks_and_stay_on_page() {
        let d = Drawing::new(Rect::new(-5.0, -5.0, 5.0, 5.0), (400.0, 400.0));
        let labels = d.tick_labels();
        assert!(labels.iter().any(|l| l.text == "0"));
        assert_eq!(labels.iter().filter(|l| l.text == "0").count(), 1);
        assert!(labels.iter().all(|l| l.x >= 0.0 && l.x <= 400.0));
        assert!(labels.iter().all(|l| l.y >= 0.0 && l.y <= 400.0));
        let off = Drawing::new(Rect::new(10.0, 10.0, 20.0, 20.0), (400.0, 400.0));
        assert!(
            off.tick_labels()
                .iter()
                .all(|l| l.y <= 400.0 && l.x <= 400.0)
        );
    }
}

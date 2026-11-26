//! Export settings (remembered in the preferences) and unit conversions.

use serde::{Deserialize, Serialize};

/// Output file format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    #[default]
    Svg,
    Pdf,
    Png,
    Eps,
    Ps,
    Csv,
    Tikz,
    Json,
}

impl Format {
    pub const ALL: [Format; 8] = [
        Format::Svg,
        Format::Pdf,
        Format::Png,
        Format::Eps,
        Format::Ps,
        Format::Csv,
        Format::Tikz,
        Format::Json,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Format::Svg => "SVG (vector)",
            Format::Pdf => "PDF",
            Format::Png => "PNG (raster)",
            Format::Eps => "EPS",
            Format::Ps => "PostScript",
            Format::Csv => "CSV points",
            Format::Tikz => "TikZ / PGF",
            Format::Json => "JSON points",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Format::Svg => "SVG",
            Format::Pdf => "PDF",
            Format::Png => "PNG",
            Format::Eps => "EPS",
            Format::Ps => "PS",
            Format::Csv => "CSV",
            Format::Tikz => "TikZ",
            Format::Json => "JSON",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Svg => "svg",
            Format::Pdf => "pdf",
            Format::Png => "png",
            Format::Eps => "eps",
            Format::Ps => "ps",
            Format::Csv => "csv",
            Format::Tikz => "tex",
            Format::Json => "json",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Format::Svg => "image/svg+xml",
            Format::Pdf => "application/pdf",
            Format::Png => "image/png",
            Format::Eps | Format::Ps => "application/postscript",
            Format::Csv => "text/csv",
            Format::Tikz => "text/x-tex",
            Format::Json => "application/json",
        }
    }

    /// Picture formats (page size, colors, grid); the others are data.
    pub fn is_picture(self) -> bool {
        !matches!(self, Format::Csv | Format::Json)
    }

    pub fn is_raster(self) -> bool {
        self == Format::Png
    }

    pub fn from_index(i: u32) -> Format {
        Format::ALL.get(i as usize).copied().unwrap_or_default()
    }

    pub fn index(self) -> u32 {
        Format::ALL.iter().position(|f| *f == self).unwrap_or(0) as u32
    }
}

/// What is exported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    /// Everything visible in the viewport.
    #[default]
    View,
    /// The selected nodes (and their children).
    Selected,
    /// One curve, everything of it inside the box.
    Curve,
    /// Part of one curve: an x or parameter range.
    Part,
    /// An inequality region, optionally clipped to the box.
    Region,
    /// A box of the viewport (Shift+E rubber band).
    Box,
}

impl Target {
    pub const ALL: [Target; 6] = [
        Target::View,
        Target::Selected,
        Target::Curve,
        Target::Part,
        Target::Region,
        Target::Box,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Target::View => "Whole viewport",
            Target::Selected => "Selected nodes",
            Target::Curve => "Single curve",
            Target::Part => "Part of a curve",
            Target::Region => "Inequality region",
            Target::Box => "Box region",
        }
    }

    /// Targets acting on one node picked in the dialog.
    pub fn single_node(self) -> bool {
        matches!(self, Target::Curve | Target::Part | Target::Region)
    }

    pub fn from_index(i: u32) -> Target {
        Target::ALL.get(i as usize).copied().unwrap_or_default()
    }

    pub fn index(self) -> u32 {
        Target::ALL.iter().position(|t| *t == self).unwrap_or(0) as u32
    }
}

/// Page size unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    #[default]
    Px,
    Mm,
    Cm,
    In,
    Pt,
}

impl Unit {
    pub const ALL: [Unit; 5] = [Unit::Px, Unit::Mm, Unit::Cm, Unit::In, Unit::Pt];

    pub fn suffix(self) -> &'static str {
        match self {
            Unit::Px => "px",
            Unit::Mm => "mm",
            Unit::Cm => "cm",
            Unit::In => "in",
            Unit::Pt => "pt",
        }
    }

    /// CSS pixels (1/96 in) per unit.
    pub fn css_px(self) -> f64 {
        match self {
            Unit::Px => 1.0,
            Unit::Mm => 96.0 / 25.4,
            Unit::Cm => 96.0 / 2.54,
            Unit::In => 96.0,
            Unit::Pt => 96.0 / 72.0,
        }
    }

    /// Converts `v` from `self` to `to`.
    pub fn convert(self, v: f64, to: Unit) -> f64 {
        v * self.css_px() / to.css_px()
    }

    pub fn from_index(i: u32) -> Unit {
        Unit::ALL.get(i as usize).copied().unwrap_or_default()
    }

    pub fn index(self) -> u32 {
        Unit::ALL.iter().position(|u| *u == self).unwrap_or(0) as u32
    }
}

/// Page background.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Background {
    /// The viewport background of the current theme.
    #[default]
    Theme,
    White,
    Transparent,
}

impl Background {
    pub const ALL: [Background; 3] = [
        Background::Theme,
        Background::White,
        Background::Transparent,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Background::Theme => "Theme",
            Background::White => "White (print)",
            Background::Transparent => "Transparent",
        }
    }

    pub fn from_index(i: u32) -> Background {
        Background::ALL.get(i as usize).copied().unwrap_or_default()
    }

    pub fn index(self) -> u32 {
        Background::ALL.iter().position(|b| *b == self).unwrap_or(0) as u32
    }
}

/// How a part of a curve is given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RangeKind {
    /// The curve's parameter (`x`, `y`, `t` or `θ`).
    #[default]
    Param,
    /// An x interval.
    X,
}

/// Every export option the dialog remembers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub format: Format,
    pub target: Target,
    pub unit: Unit,
    /// Page size in `unit`.
    pub width: f64,
    pub height: f64,
    /// Height follows the width and the box aspect.
    pub keep_aspect: bool,
    /// Raster resolution (PNG); vector formats use 96 px per inch.
    pub dpi: f64,
    /// Sampling density relative to the screen (1 = screen).
    pub quality: f64,
    /// Uniform samples for `t, x, y` tables.
    pub csv_samples: u32,
    /// Data exports list `t, x, y` (curves with a parameter) instead of
    /// the adaptive `x, y` points.
    pub csv_param: bool,
    pub grid: bool,
    pub axes: bool,
    pub labels: bool,
    pub legend: bool,
    pub background: Background,
    /// Multiplies every stroke width.
    pub line_scale: f64,
    /// SVG paths in world coordinates (viewBox transform) instead of page
    /// coordinates.
    pub svg_world: bool,
    /// Clip regions (and other targets) to the box fields.
    pub clip_region: bool,
    pub range_kind: RangeKind,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            format: Format::Svg,
            target: Target::View,
            unit: Unit::Px,
            width: 800.0,
            height: 600.0,
            keep_aspect: true,
            dpi: 96.0,
            quality: 2.0,
            csv_samples: 500,
            csv_param: false,
            grid: true,
            axes: true,
            labels: true,
            legend: false,
            background: Background::Theme,
            line_scale: 1.0,
            svg_world: false,
            clip_region: true,
            range_kind: RangeKind::Param,
        }
    }
}

impl ExportSettings {
    /// Clamps numbers into sane ranges.
    pub fn validate(&mut self) {
        let fix = |v: f64, lo: f64, hi: f64, d: f64| {
            if v.is_finite() { v.clamp(lo, hi) } else { d }
        };
        let d = ExportSettings::default();
        let max = Unit::Px.convert(20_000.0, self.unit);
        let min = Unit::Px.convert(8.0, self.unit);
        self.width = fix(self.width, min, max, d.width);
        self.height = fix(self.height, min, max, d.height);
        self.dpi = fix(self.dpi, 10.0, 2400.0, d.dpi);
        self.quality = fix(self.quality, 0.25, 8.0, d.quality);
        self.csv_samples = self.csv_samples.clamp(2, 1_000_000);
        self.line_scale = fix(self.line_scale, 0.1, 10.0, d.line_scale);
    }

    /// Page size in CSS pixels.
    pub fn page_px(&self) -> (f64, f64) {
        let k = self.unit.css_px();
        (self.width * k, self.height * k)
    }

    /// Size of a PNG in device pixels.
    pub fn raster_px(&self) -> (u32, u32) {
        let (w, h) = self.page_px();
        let k = self.dpi / 96.0;
        let px = |v: f64| (v * k).round().clamp(1.0, 16_384.0) as u32;
        (px(w), px(h))
    }

    /// Height for `width` keeping a box of `world_w × world_h` world units
    /// undistorted (`aspect` = vertical over horizontal screen scale).
    pub fn aspect_height(width: f64, world_w: f64, world_h: f64, aspect: f64) -> Option<f64> {
        let h = width * world_h / world_w * aspect;
        (h.is_finite() && h > 0.0).then_some(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_conversions() {
        assert!((Unit::In.convert(1.0, Unit::Mm) - 25.4).abs() < 1e-12);
        assert!((Unit::In.convert(1.0, Unit::Pt) - 72.0).abs() < 1e-12);
        assert!((Unit::Cm.convert(2.54, Unit::In) - 1.0).abs() < 1e-12);
        assert!((Unit::Mm.convert(10.0, Unit::Cm) - 1.0).abs() < 1e-12);
        assert!((Unit::Px.convert(96.0, Unit::In) - 1.0).abs() < 1e-12);
        for u in Unit::ALL {
            assert_eq!(Unit::from_index(u.index()), u);
        }
    }

    #[test]
    fn page_and_raster_sizes() {
        let s = ExportSettings {
            unit: Unit::In,
            width: 2.0,
            height: 1.0,
            dpi: 300.0,
            ..Default::default()
        };
        assert_eq!(s.page_px(), (192.0, 96.0));
        assert_eq!(s.raster_px(), (600, 300));
        let h = ExportSettings::aspect_height(100.0, 20.0, 10.0, 1.0).unwrap();
        assert_eq!(h, 50.0);
        assert!(ExportSettings::aspect_height(100.0, 0.0, 10.0, 1.0).is_none());
    }

    #[test]
    fn settings_round_trip_and_validate() {
        let mut s = ExportSettings {
            format: Format::Tikz,
            width: f64::NAN,
            quality: 100.0,
            ..Default::default()
        };
        s.validate();
        assert_eq!(s.width, 800.0);
        assert_eq!(s.quality, 8.0);
        let text = toml::to_string(&s).unwrap();
        let back: ExportSettings = toml::from_str(&text).unwrap();
        assert_eq!(back, s);
        // Missing fields fall back to defaults.
        let partial: ExportSettings = toml::from_str("format = \"Png\"").unwrap();
        assert_eq!(partial.format, Format::Png);
        assert_eq!(partial.width, 800.0);
        for f in Format::ALL {
            assert_eq!(Format::from_index(f.index()), f);
        }
    }
}

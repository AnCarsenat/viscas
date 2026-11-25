//! Theme data: every UI color token plus the colors drawn by editors
//! (viewport, console, scene tree, dependency graph). Pure logic, no GTK.
//!
//! Themes serialize to TOML. Loading merges the file over a built-in base
//! theme, so files written by older versions (missing fields) still load.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An sRGB color with 8-bit channels and alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    /// Opaque color from `0xRRGGBB`.
    pub const fn hex(rgb: u32) -> Self {
        Self {
            r: ((rgb >> 16) & 0xff) as u8,
            g: ((rgb >> 8) & 0xff) as u8,
            b: (rgb & 0xff) as u8,
            a: 255,
        }
    }

    /// Color from `0xRRGGBB` with alpha in `0..=1`.
    pub const fn hexa(rgb: u32, alpha: f64) -> Self {
        Self::hex(rgb).with_alpha(alpha)
    }

    /// The same color with alpha `alpha` (in `0..=1`, clamped).
    pub const fn with_alpha(self, alpha: f64) -> Self {
        let a = if alpha <= 0.0 {
            0.0
        } else if alpha >= 1.0 {
            1.0
        } else {
            alpha
        };
        Self {
            a: (a * 255.0 + 0.5) as u8,
            ..self
        }
    }

    /// Parses `#rgb`, `#rrggbb` or `#rrggbbaa` (the `#` is optional).
    pub fn parse(text: &str) -> Option<Self> {
        let s = text.trim();
        let s = s.strip_prefix('#').unwrap_or(s);
        if !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
        match s.len() {
            3 => {
                let n = |i: usize| u8::from_str_radix(&s[i..=i], 16).ok().map(|v| v * 17);
                Some(Self {
                    r: n(0)?,
                    g: n(1)?,
                    b: n(2)?,
                    a: 255,
                })
            }
            6 | 8 => Some(Self {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: if s.len() == 8 { byte(6)? } else { 255 },
            }),
            _ => None,
        }
    }

    /// `#rrggbb`, or `#rrggbbaa` when not opaque.
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// `#rrggbb`, ignoring alpha (for Pango markup).
    pub fn to_rgb_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// CSS value: `#rrggbb`, or `rgba(r, g, b, a)` when not opaque.
    pub fn to_css(self) -> String {
        if self.a == 255 {
            self.to_hex()
        } else {
            format!(
                "rgba({}, {}, {}, {})",
                self.r,
                self.g,
                self.b,
                trim_float(self.alpha(), 3)
            )
        }
    }

    pub fn alpha(self) -> f64 {
        f64::from(self.a) / 255.0
    }

    /// Channels as `0..=1` floats (red, green, blue, alpha), for cairo.
    pub fn rgba_f64(self) -> (f64, f64, f64, f64) {
        (
            f64::from(self.r) / 255.0,
            f64::from(self.g) / 255.0,
            f64::from(self.b) / 255.0,
            self.alpha(),
        )
    }

    /// This color composited over the opaque color `bg`.
    #[allow(dead_code, reason = "used by contrast checks")]
    pub fn over(self, bg: Color) -> Color {
        let a = self.alpha();
        let ch = |f: u8, b: u8| (f64::from(f) * a + f64::from(b) * (1.0 - a)).round() as u8;
        Color {
            r: ch(self.r, bg.r),
            g: ch(self.g, bg.g),
            b: ch(self.b, bg.b),
            a: 255,
        }
    }

    /// Linear interpolation towards `other` by `t` in `0..=1` (alpha kept).
    pub fn mix(self, other: Color, t: f64) -> Color {
        let t = t.clamp(0.0, 1.0);
        let ch = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
        Color {
            r: ch(self.r, other.r),
            g: ch(self.g, other.g),
            b: ch(self.b, other.b),
            a: self.a,
        }
    }

    /// WCAG 2 relative luminance of the opaque color.
    pub fn luminance(self) -> f64 {
        let lin = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }
}

/// WCAG 2 contrast ratio between two colors (`1.0..=21.0`). Translucent
/// colors should be composited first (see [`Color::over`]).
pub fn contrast_ratio(a: Color, b: Color) -> f64 {
    let (la, lb) = (a.luminance(), b.luminance());
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Formats `v` with at most `digits` decimals, trailing zeros removed.
pub fn trim_float(v: f64, digits: usize) -> String {
    let s = format!("{v:.digits$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Color::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid color {text:?}")))
    }
}

/// One editable color of a theme, for generic UIs and CSS generation.
pub struct Field {
    /// Field name, as in the TOML file (and the CSS token for UI colors).
    pub key: &'static str,
    /// Label shown in the Themes page.
    pub label: &'static str,
    pub get: fn(&Theme) -> Color,
    pub set: fn(&mut Theme, Color),
}

macro_rules! section {
    ($(#[$meta:meta])* $name:ident in $sec:ident { $($field:ident : $label:literal),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
        pub struct $name {
            $(pub $field: Color,)*
        }

        impl $name {
            pub const FIELDS: &'static [Field] = &[$(Field {
                key: stringify!($field),
                label: $label,
                get: |t| t.$sec.$field,
                set: |t, c| t.$sec.$field = c,
            }),*];
        }
    };
}

section! {
    /// Interface colors; each becomes a `@vc_<field>` CSS color.
    UiColors in ui {
        gap: "Area Gaps",
        bg0: "Bars",
        bg1: "Editor Background",
        bg2: "Headers",
        bg3: "Widgets",
        bg4: "Widget Hover",
        bg5: "Widget Pressed",
        header_active: "Active Header",
        border: "Outline",
        line: "Hairlines",
        fg: "Text",
        fg_strong: "Text Highlight",
        fg_dim: "Text Dim",
        fg_faint: "Text Faint",
        accent: "Accent",
        accent_fg: "Text on Accent",
        accent_text: "Accent Text",
        ok: "Success",
        danger: "Danger",
        danger_text: "Danger Text",
        tint: "Hover Tint",
        shadow: "Shadow",
        tooltip_bg: "Tooltip Background",
        tooltip_fg: "Tooltip Text",
    }
}

section! {
    /// Graph viewport colors.
    ViewportColors in viewport {
        background: "Background",
        grid_minor: "Grid Minor",
        grid_major: "Grid Major",
        axis: "Axes",
        label: "Axis Labels",
    }
}

section! {
    /// Console editor colors.
    ConsoleColors in console {
        background: "Background",
        text: "Input Text",
        prompt: "Prompt",
        result: "Result",
        error: "Error",
        hint: "Hint",
    }
}

section! {
    /// Scene tree editor colors.
    SceneTreeColors in scene_tree {
        background: "Background",
        text: "Text",
        text_dim: "Text Dim",
        selected: "Selected Row",
        hovered: "Hovered Row",
        icon: "Icons",
        upstream: "Upstream Tint",
        downstream: "Downstream Tint",
    }
}

section! {
    /// Dependency graph canvas colors.
    GraphColors in dep_graph {
        background: "Background",
        dots: "Grid Dots",
        capsule: "Node Fill",
        capsule_border: "Node Outline",
        wire: "Wires",
        port: "Ports",
        port_active: "Active Port",
    }
}

/// A complete color theme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    /// Dark themes put GTK's stock widgets (dialogs) in dark mode.
    pub dark: bool,
    pub ui: UiColors,
    pub viewport: ViewportColors,
    pub console: ConsoleColors,
    pub scene_tree: SceneTreeColors,
    pub dep_graph: GraphColors,
}

/// Theme sections in display order: title, TOML table key, fields.
pub const SECTIONS: &[(&str, &str, &[Field])] = &[
    ("User Interface", "ui", UiColors::FIELDS),
    ("Viewport", "viewport", ViewportColors::FIELDS),
    ("Console", "console", ConsoleColors::FIELDS),
    ("Scene Tree", "scene_tree", SceneTreeColors::FIELDS),
    ("Dependency Graph", "dep_graph", GraphColors::FIELDS),
];

pub const DARK_NAME: &str = "Viscas Dark";
pub const LIGHT_NAME: &str = "Viscas Light";
pub const HIGH_CONTRAST_NAME: &str = "High Contrast Dark";

const ORANGE: Color = Color::hex(0xf0883e);

/// Accent color choice applied over the theme's own accent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    /// Keep the theme's accent.
    #[default]
    Theme,
    Orange,
    Blue,
    Green,
    Purple,
    Pink,
}

impl Accent {
    pub const ALL: [Accent; 6] = [
        Accent::Theme,
        Accent::Orange,
        Accent::Blue,
        Accent::Green,
        Accent::Purple,
        Accent::Pink,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Accent::Theme => "From Theme",
            Accent::Orange => "Orange",
            Accent::Blue => "Blue",
            Accent::Green => "Green",
            Accent::Purple => "Purple",
            Accent::Pink => "Pink",
        }
    }

    pub fn color(self) -> Option<Color> {
        match self {
            Accent::Theme => None,
            Accent::Orange => Some(ORANGE),
            Accent::Blue => Some(Color::hex(0x4f9cf0)),
            Accent::Green => Some(Color::hex(0x5fbf5a)),
            Accent::Purple => Some(Color::hex(0xa77ef0)),
            Accent::Pink => Some(Color::hex(0xec6fae)),
        }
    }
}

const BLACKISH: Color = Color::hex(0x141008);
const WHITE: Color = Color::hex(0xffffff);

impl Theme {
    /// Sets the accent and derives the colors that depend on it: text on
    /// accent (black or white, whichever reads better), accent-colored text
    /// readable on the bars and editors, and the active graph port.
    pub fn set_accent(&mut self, accent: Color) {
        let accent = accent.with_alpha(1.0);
        self.ui.accent = accent;
        self.ui.accent_fg = if contrast_ratio(BLACKISH, accent) >= contrast_ratio(WHITE, accent) {
            BLACKISH
        } else {
            WHITE
        };
        let worst = |c: Color| {
            [self.ui.bg0, self.ui.bg1, self.ui.bg2]
                .into_iter()
                .map(|bg| contrast_ratio(c, bg))
                .fold(f64::INFINITY, f64::min)
        };
        let target = self.ui.fg_strong;
        let mut text = accent;
        let mut t = 0.0;
        while worst(text) < 4.6 && t < 1.0 {
            t += 0.05;
            text = accent.mix(target, t);
        }
        self.ui.accent_text = text;
        self.dep_graph.port_active = accent.with_alpha(self.dep_graph.port_active.alpha());
        // The selected tree row is an accent tint: it follows the accent.
        self.scene_tree.selected = accent.with_alpha(self.scene_tree.selected.alpha());
    }

    /// The theme with `accent` applied (unchanged for [`Accent::Theme`]).
    pub fn with_accent(&self, accent: Accent) -> Theme {
        let mut theme = self.clone();
        if let Some(color) = accent.color() {
            theme.set_accent(color);
        }
        theme
    }

    #[allow(dead_code, reason = "checked by tests; future theme editor warnings")]
    /// Text/background pairs that must stay readable (WCAG AA, 4.5:1),
    /// with translucent backgrounds composited over their base.
    pub fn text_pairs(&self) -> Vec<(String, Color, Color)> {
        let ui = &self.ui;
        let mut pairs = Vec::new();
        let mut add = |name: &str, fg: Color, bg: Color| {
            pairs.push((name.to_owned(), fg, bg));
        };
        for (bg_name, bg) in [
            ("bg0", ui.bg0),
            ("bg1", ui.bg1),
            ("bg2", ui.bg2),
            ("bg3", ui.bg3),
        ] {
            add(&format!("fg/{bg_name}"), ui.fg, bg);
            add(&format!("fg_strong/{bg_name}"), ui.fg_strong, bg);
            add(&format!("fg_dim/{bg_name}"), ui.fg_dim, bg);
        }
        for (bg_name, bg) in [("bg0", ui.bg0), ("bg1", ui.bg1), ("bg2", ui.bg2)] {
            add(&format!("accent_text/{bg_name}"), ui.accent_text, bg);
        }
        add("fg/header_active", ui.fg, ui.header_active);
        add("accent_fg/accent", ui.accent_fg, ui.accent);
        add("tooltip", ui.tooltip_fg, ui.tooltip_bg);
        let vp = &self.viewport;
        add("viewport label", vp.label, vp.background);
        let c = &self.console;
        for (name, fg) in [
            ("console text", c.text),
            ("console prompt", c.prompt),
            ("console result", c.result),
            ("console error", c.error),
        ] {
            add(name, fg, c.background);
        }
        let s = &self.scene_tree;
        add("scene text", s.text, s.background);
        add("scene text dim", s.text_dim, s.background);
        add("scene selected", s.text, s.selected.over(s.background));
        add("scene hovered", s.text, s.hovered.over(s.background));
        pairs
    }

    /// Serializes to TOML.
    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }

    /// Parses a theme file. Missing fields come from the built-in base
    /// matching the file's `dark` flag (dark when absent).
    pub fn from_toml(text: &str) -> Result<Theme, String> {
        let table: toml::Table = text.parse().map_err(|e| format!("{e}"))?;
        Theme::from_table(table)
    }

    /// Like [`Theme::from_toml`], from an already parsed table.
    pub fn from_table(table: toml::Table) -> Result<Theme, String> {
        let dark = table
            .get("dark")
            .and_then(toml::Value::as_bool)
            .unwrap_or(true);
        let base = if dark { viscas_dark() } else { viscas_light() };
        Theme::merge_over(&base, table)
    }

    /// Deserializes `table` merged over `base`: fields absent from the
    /// table keep the base value.
    pub fn merge_over(base: &Theme, table: toml::Table) -> Result<Theme, String> {
        let mut merged = toml::Table::try_from(base).map_err(|e| format!("{e}"))?;
        merge_tables(&mut merged, table);
        merged
            .try_into()
            .map_err(|e: toml::de::Error| format!("{e}"))
    }
}

/// Recursively copies `src` over `dst`.
fn merge_tables(dst: &mut toml::Table, src: toml::Table) {
    for (key, value) in src {
        match (dst.get_mut(&key), value) {
            (Some(toml::Value::Table(d)), toml::Value::Table(s)) => merge_tables(d, s),
            (_, value) => {
                dst.insert(key, value);
            }
        }
    }
}

/// The built-in themes, in display order.
pub fn builtins() -> Vec<Theme> {
    vec![viscas_dark(), viscas_light(), high_contrast_dark()]
}

/// The built-in theme called `name`.
pub fn builtin(name: &str) -> Option<Theme> {
    builtins().into_iter().find(|t| t.name == name)
}

/// The default look: dark, dense, one warm accent.
pub fn viscas_dark() -> Theme {
    let mut t = Theme {
        name: DARK_NAME.to_owned(),
        dark: true,
        ui: UiColors {
            gap: Color::hex(0x111111),
            bg0: Color::hex(0x181818),
            bg1: Color::hex(0x1f1f1f),
            bg2: Color::hex(0x272727),
            bg3: Color::hex(0x303030),
            bg4: Color::hex(0x3a3a3a),
            bg5: Color::hex(0x454545),
            header_active: Color::hex(0x2c2c2c),
            border: Color::hex(0x0b0b0b),
            line: Color::hex(0x333333),
            fg: Color::hex(0xd2d2d2),
            fg_strong: Color::hex(0xf2f2f2),
            fg_dim: Color::hex(0x9a9a9a),
            fg_faint: Color::hex(0x5f5f5f),
            accent: ORANGE,
            accent_fg: Color::hex(0x1a1006),
            accent_text: ORANGE,
            ok: Color::hex(0x7fbf6a),
            danger: Color::hex(0xb5412e),
            danger_text: Color::hex(0xff7a66),
            tint: Color::hex(0xffffff),
            shadow: Color::hex(0x000000),
            tooltip_bg: Color::hex(0x0e0e0e),
            tooltip_fg: Color::hex(0xd2d2d2),
        },
        viewport: ViewportColors {
            background: Color::hex(0x1a1a1a),
            grid_minor: Color::hexa(0xffffff, 0.035),
            grid_major: Color::hexa(0xffffff, 0.085),
            axis: Color::hexa(0x8c8c8c, 0.9),
            label: Color::hex(0x8a8a8a),
        },
        console: ConsoleColors {
            background: Color::hex(0x1f1f1f),
            text: Color::hex(0xd2d2d2),
            prompt: ORANGE,
            result: Color::hex(0x9fd18b),
            error: Color::hex(0xff7a66),
            hint: Color::hex(0x5f5f5f),
        },
        scene_tree: SceneTreeColors {
            background: Color::hex(0x1f1f1f),
            text: Color::hex(0xd2d2d2),
            text_dim: Color::hex(0x9a9a9a),
            selected: ORANGE.with_alpha(0.16),
            hovered: Color::hexa(0xffffff, 0.05),
            icon: Color::hex(0x9a9a9a),
            upstream: Color::hex(0x4fa3f0),
            downstream: Color::hex(0x8fcf5f),
        },
        dep_graph: GraphColors {
            background: Color::hex(0x191919),
            dots: Color::hexa(0xffffff, 0.07),
            capsule: Color::hexa(0xffffff, 0.025),
            capsule_border: Color::hexa(0xffffff, 0.09),
            wire: Color::hexa(0xffffff, 0.07),
            port: Color::hexa(0xffffff, 0.16),
            port_active: ORANGE.with_alpha(0.45),
        },
    };
    t.set_accent(ORANGE);
    t
}

/// Light greys in the spirit of Blender's light theme: mid-grey chrome,
/// lighter editors, near-white widgets and dark text.
pub fn viscas_light() -> Theme {
    let mut t = Theme {
        name: LIGHT_NAME.to_owned(),
        dark: false,
        ui: UiColors {
            gap: Color::hex(0x8f8f8f),
            bg0: Color::hex(0xbababa),
            bg1: Color::hex(0xd3d3d3),
            bg2: Color::hex(0xc3c3c3),
            bg3: Color::hex(0xe6e6e6),
            bg4: Color::hex(0xf2f2f2),
            bg5: Color::hex(0xb0b0b0),
            header_active: Color::hex(0xcbcbcb),
            border: Color::hex(0x7c7c7c),
            line: Color::hex(0xa6a6a6),
            fg: Color::hex(0x1a1a1a),
            fg_strong: Color::hex(0x000000),
            fg_dim: Color::hex(0x404040),
            fg_faint: Color::hex(0x6e6e6e),
            accent: ORANGE,
            accent_fg: BLACKISH,
            accent_text: ORANGE,
            ok: Color::hex(0x2f7d22),
            danger: Color::hex(0xc0392b),
            danger_text: Color::hex(0xa8231a),
            tint: Color::hex(0x000000),
            shadow: Color::hexa(0x000000, 0.4),
            tooltip_bg: Color::hex(0x2a2a2a),
            tooltip_fg: Color::hex(0xeeeeee),
        },
        viewport: ViewportColors {
            background: Color::hex(0xededed),
            grid_minor: Color::hexa(0x000000, 0.05),
            grid_major: Color::hexa(0x000000, 0.12),
            axis: Color::hexa(0x4a4a4a, 0.9),
            label: Color::hex(0x555555),
        },
        console: ConsoleColors {
            background: Color::hex(0xdedede),
            text: Color::hex(0x1a1a1a),
            prompt: Color::hex(0x9a4a0c),
            result: Color::hex(0x2a6a1c),
            error: Color::hex(0xa8231a),
            hint: Color::hex(0x6e6e6e),
        },
        scene_tree: SceneTreeColors {
            background: Color::hex(0xd3d3d3),
            text: Color::hex(0x1a1a1a),
            text_dim: Color::hex(0x404040),
            selected: ORANGE.with_alpha(0.3),
            hovered: Color::hexa(0x000000, 0.06),
            icon: Color::hex(0x404040),
            upstream: Color::hex(0x1f6fc0),
            downstream: Color::hex(0x3f8a1f),
        },
        dep_graph: GraphColors {
            background: Color::hex(0xd9d9d9),
            dots: Color::hexa(0x000000, 0.13),
            capsule: Color::hexa(0xffffff, 0.45),
            capsule_border: Color::hexa(0x000000, 0.16),
            wire: Color::hexa(0x000000, 0.18),
            port: Color::hexa(0x000000, 0.3),
            port_active: ORANGE.with_alpha(0.8),
        },
    };
    t.set_accent(ORANGE);
    t
}

/// Maximum legibility: black chrome, white text, visible outlines.
pub fn high_contrast_dark() -> Theme {
    let mut t = viscas_dark();
    t.name = HIGH_CONTRAST_NAME.to_owned();
    t.ui = UiColors {
        gap: Color::hex(0x000000),
        bg0: Color::hex(0x000000),
        bg1: Color::hex(0x0a0a0a),
        bg2: Color::hex(0x151515),
        bg3: Color::hex(0x202020),
        bg4: Color::hex(0x303030),
        bg5: Color::hex(0x404040),
        header_active: Color::hex(0x242424),
        border: Color::hex(0x7a7a7a),
        line: Color::hex(0x5c5c5c),
        fg: Color::hex(0xffffff),
        fg_strong: Color::hex(0xffffff),
        fg_dim: Color::hex(0xd8d8d8),
        fg_faint: Color::hex(0xa0a0a0),
        ok: Color::hex(0x8fe07a),
        danger: Color::hex(0xd0402a),
        danger_text: Color::hex(0xff9080),
        tooltip_bg: Color::hex(0x000000),
        tooltip_fg: Color::hex(0xffffff),
        ..t.ui
    };
    t.viewport = ViewportColors {
        background: Color::hex(0x000000),
        grid_minor: Color::hexa(0xffffff, 0.1),
        grid_major: Color::hexa(0xffffff, 0.24),
        axis: Color::hex(0xe6e6e6),
        label: Color::hex(0xe6e6e6),
    };
    t.console = ConsoleColors {
        background: Color::hex(0x0a0a0a),
        text: Color::hex(0xffffff),
        hint: Color::hex(0xa0a0a0),
        ..t.console
    };
    t.scene_tree = SceneTreeColors {
        background: Color::hex(0x0a0a0a),
        text: Color::hex(0xffffff),
        text_dim: Color::hex(0xd8d8d8),
        icon: Color::hex(0xd8d8d8),
        ..t.scene_tree
    };
    t.dep_graph = GraphColors {
        background: Color::hex(0x000000),
        dots: Color::hexa(0xffffff, 0.18),
        capsule: Color::hexa(0xffffff, 0.06),
        capsule_border: Color::hexa(0xffffff, 0.4),
        wire: Color::hexa(0xffffff, 0.3),
        port: Color::hexa(0xffffff, 0.5),
        ..t.dep_graph
    };
    t.set_accent(Color::hex(0xffa64d));
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_parse_and_format() {
        assert_eq!(Color::parse("#f0883e"), Some(Color::hex(0xf0883e)));
        assert_eq!(Color::parse("fff"), Some(Color::hex(0xffffff)));
        assert_eq!(Color::parse("#00000080").map(|c| c.a), Some(0x80));
        assert_eq!(Color::parse("#12345"), None);
        assert_eq!(Color::parse("#gggggg"), None);
        assert_eq!(Color::parse("#ffé"), None);
        assert_eq!(Color::hex(0x0b0b0b).to_hex(), "#0b0b0b");
        assert_eq!(
            Color::hexa(0xffffff, 0.5).to_css(),
            "rgba(255, 255, 255, 0.502)"
        );
        assert_eq!(Color::hex(0x123456).to_css(), "#123456");
        assert_eq!(Color::hexa(0x123456, 0.5).to_rgb_hex(), "#123456");
    }

    #[test]
    fn contrast_matches_wcag_reference_values() {
        let black = Color::hex(0);
        let white = Color::hex(0xffffff);
        assert!((contrast_ratio(black, white) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 1e-9);
        // #767676 on white is the classic 4.54:1 boundary color.
        let grey = contrast_ratio(Color::hex(0x767676), white);
        assert!((grey - 4.54).abs() < 0.01, "{grey}");
        assert_eq!(contrast_ratio(black, white), contrast_ratio(white, black));
    }

    #[test]
    fn over_composites_alpha() {
        let half_white = Color::hexa(0xffffff, 0.5);
        assert_eq!(half_white.over(Color::hex(0)), Color::hex(0x808080));
        assert_eq!(
            Color::hex(0x123456).over(Color::hex(0)),
            Color::hex(0x123456)
        );
    }

    #[test]
    fn every_builtin_theme_and_accent_is_readable() {
        for base in builtins() {
            for accent in Accent::ALL {
                let theme = base.with_accent(accent);
                for (name, fg, bg) in theme.text_pairs() {
                    let ratio = contrast_ratio(fg.over(bg), bg);
                    assert!(
                        ratio >= 4.5,
                        "{} / {accent:?}: {name} is {ratio:.2}:1 ({fg} on {bg})",
                        theme.name
                    );
                }
            }
        }
    }

    #[test]
    fn builtin_names_are_unique_and_found() {
        let names: Vec<String> = builtins().into_iter().map(|t| t.name).collect();
        for (i, a) in names.iter().enumerate() {
            assert!(builtin(a).is_some());
            assert!(!names[i + 1..].contains(a));
        }
        assert!(builtin("nope").is_none());
    }

    #[test]
    fn field_table_covers_every_field() {
        let theme = viscas_dark();
        let table = toml::Table::try_from(&theme).unwrap();
        for (_, key, fields) in SECTIONS {
            let section = table[*key].as_table().unwrap();
            assert_eq!(section.len(), fields.len(), "{key}");
            for f in *fields {
                assert!(section.contains_key(f.key), "{key}.{}", f.key);
                let mut t = theme.clone();
                (f.set)(&mut t, Color::hex(0x010203));
                assert_eq!((f.get)(&t), Color::hex(0x010203));
            }
        }
    }

    #[test]
    fn toml_round_trip() {
        for theme in builtins() {
            let text = theme.to_toml().unwrap();
            assert_eq!(Theme::from_toml(&text).unwrap(), theme);
        }
    }

    #[test]
    fn missing_fields_come_from_matching_base() {
        let t = Theme::from_toml("name = \"Mine\"\ndark = false\n[ui]\naccent = \"#ff0000\"\n")
            .unwrap();
        assert_eq!(t.name, "Mine");
        assert_eq!(t.ui.accent, Color::hex(0xff0000));
        assert_eq!(t.ui.bg1, viscas_light().ui.bg1);
        let d = Theme::from_toml("name = \"D\"").unwrap();
        assert_eq!(d.ui.bg1, viscas_dark().ui.bg1);
    }

    #[test]
    fn invalid_theme_files_are_errors_not_panics() {
        assert!(Theme::from_toml("[ui]\nbg1 = \"purple\"").is_err());
        assert!(Theme::from_toml("[ui\n").is_err());
        assert!(Theme::from_toml("ui = 3").is_err());
        assert!(Theme::from_toml("dark = \"yes\"").is_err());
    }

    #[test]
    fn set_accent_derives_readable_dependents() {
        let mut t = viscas_light();
        t.set_accent(Color::hex(0xffff00));
        assert_eq!(t.ui.accent_fg, BLACKISH);
        assert!(contrast_ratio(t.ui.accent_text, t.ui.bg1) >= 4.5);
        assert_eq!(t.dep_graph.port_active.r, 0xff);
    }
}

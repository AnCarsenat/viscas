//! Builds the application stylesheet from theme data and interface
//! preferences. Pure string processing, no GTK.

use super::model::{SceneTreeColors, Theme, UiColors, trim_float};

/// The static rules, written against the `@vc_*` colors.
pub const RULES: &str = concat!(
    include_str!("../../resources/theme.css"),
    include_str!("../../resources/scene_panels.css"),
    include_str!("../../resources/docking.css"),
);

/// Placeholder in [`RULES`] replaced by the monospace font family list.
pub const MONO_PLACEHOLDER: &str = "__MONO__";

/// Fallback monospace families after the user's choice.
const MONO_FALLBACK: &str =
    "\"JetBrains Mono\", \"JetBrainsMono Nerd Font\", \"Hack\", \"DejaVu Sans Mono\", monospace";

/// The font size [`RULES`] is written for, in px.
pub const BASE_FONT_PX: f64 = 12.0;

/// Interface options that change the stylesheet.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleOptions {
    /// Multiplies every length.
    pub ui_scale: f64,
    /// Base font size in px (before `ui_scale`).
    pub font_size: f64,
    /// Preferred monospace family; empty for the default list.
    pub mono_font: String,
    pub tooltips: bool,
}

impl Default for StyleOptions {
    fn default() -> Self {
        Self {
            ui_scale: 1.0,
            font_size: BASE_FONT_PX,
            mono_font: String::new(),
            tooltips: true,
        }
    }
}

/// `@define-color vc_<token> <value>;` for every UI color of `theme`.
pub fn palette_css(theme: &Theme) -> String {
    // The theme name is user data and deliberately not emitted.
    let mut out = String::from("/* generated palette */\n");
    for field in UiColors::FIELDS {
        out.push_str(&format!(
            "@define-color vc_{} {};\n",
            field.key,
            (field.get)(theme).to_css()
        ));
    }
    // Scene tree colors, as `@vc_tree_<field>`.
    for field in SceneTreeColors::FIELDS {
        out.push_str(&format!(
            "@define-color vc_tree_{} {};\n",
            field.key,
            (field.get)(theme).to_css()
        ));
    }
    out
}

/// CSS font-family list starting with `preferred` (quotes stripped).
pub fn mono_families(preferred: &str) -> String {
    let clean = crate::prefs::clean_font(preferred);
    if clean.is_empty() {
        MONO_FALLBACK.to_owned()
    } else {
        format!("\"{clean}\", {MONO_FALLBACK}")
    }
}

/// The complete stylesheet: palette, then the scaled rules.
pub fn stylesheet(theme: &Theme, opts: &StyleOptions) -> String {
    let font_factor = opts.font_size / BASE_FONT_PX;
    let rules = RULES.replace(MONO_PLACEHOLDER, &mono_families(&opts.mono_font));
    let mut out = palette_css(theme);
    out.push_str(&scale_px(
        &rules,
        opts.ui_scale,
        opts.ui_scale * font_factor,
    ));
    if !opts.tooltips {
        // GTK 4 has no global tooltip switch; hide their popups instead.
        out.push_str("\ntooltip, tooltip.background { opacity: 0; }\n");
    }
    out
}

/// Multiplies every `<number>px` length in `css` by `factor`, and those of
/// `font-size` declarations by `font_factor`. Comments and string
/// literals are left alone.
pub fn scale_px(css: &str, factor: f64, font_factor: f64) -> String {
    let bytes = css.as_bytes();
    let mut out = String::with_capacity(css.len());
    let mut i = 0;
    let mut in_font_size = false;
    while i < bytes.len() {
        let c = bytes[i];
        if css[i..].starts_with("/*") {
            let end = css[i + 2..].find("*/").map_or(css.len(), |p| i + 2 + p + 2);
            out.push_str(&css[i..end]);
            i = end;
            continue;
        }
        if c == b'"' || c == b'\'' {
            // String literal: copied verbatim (honouring escapes).
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != c {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(css.len());
            out.push_str(&css[i..end]);
            i = end;
            continue;
        }
        if css[i..].starts_with("font-size") {
            in_font_size = true;
        }
        if matches!(c, b';' | b'}' | b'{') {
            in_font_size = false;
        }
        let starts_number = (c.is_ascii_digit() || c == b'.')
            && !(i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'#'))
            && (c != b'.' || bytes.get(i + 1).is_some_and(u8::is_ascii_digit));
        if starts_number {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
                j += 1;
            }
            let number = &css[i..j];
            if css[j..].starts_with("px")
                && let Ok(v) = number.parse::<f64>()
            {
                let f = if in_font_size { font_factor } else { factor };
                out.push_str(&trim_float(v * f, 2));
                out.push_str("px");
                i = j + 2;
                continue;
            }
            out.push_str(number);
            i = j;
            continue;
        }
        // Copy one UTF-8 char.
        let len = css[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&css[i..i + len]);
        i += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::model::{builtins, viscas_dark};

    #[test]
    fn palette_defines_every_ui_token() {
        let css = palette_css(&viscas_dark());
        for f in UiColors::FIELDS {
            assert!(
                css.contains(&format!("@define-color vc_{} ", f.key)),
                "{}",
                f.key
            );
        }
        assert!(css.contains("@define-color vc_accent #f0883e;"));
        assert!(css.contains("@define-color vc_bg1 #1f1f1f;"));
    }

    #[test]
    fn rules_only_use_defined_tokens() {
        let palette = palette_css(&viscas_dark());
        let mut rest = RULES;
        while let Some(p) = rest.find("@vc_") {
            let tail = &rest[p + 1..];
            let end = tail
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(tail.len());
            let token = &tail[..end];
            if token == "vc_" {
                rest = &tail[end..];
                continue; // the `@vc_*` wildcard in comments
            }
            assert!(
                palette.contains(&format!("@define-color {token} ")),
                "undefined {token}"
            );
            rest = &tail[end..];
        }
    }

    #[test]
    fn stylesheet_has_no_placeholders_and_follows_theme() {
        for theme in builtins() {
            let css = stylesheet(&theme, &StyleOptions::default());
            assert!(!css.contains(MONO_PLACEHOLDER));
            assert!(css.contains(&format!("vc_bg1 {}", theme.ui.bg1.to_css())));
        }
    }

    #[test]
    fn scale_multiplies_lengths_and_fonts_separately() {
        let css = "a { min-height: 20px; padding: 1px 6px; font-size: 12px; }";
        assert_eq!(scale_px(css, 1.0, 1.0), css);
        assert_eq!(
            scale_px(css, 1.5, 2.0),
            "a { min-height: 30px; padding: 1.5px 9px; font-size: 24px; }"
        );
    }

    #[test]
    fn scale_leaves_colors_comments_and_unitless_numbers() {
        let css =
            "/* 10px */ b { color: #10a0b0; margin: 0 .5px; opacity: 0.5; x: alpha(white, 0.07); }";
        let out = scale_px(css, 2.0, 2.0);
        assert_eq!(
            out,
            "/* 10px */ b { color: #10a0b0; margin: 0 1px; opacity: 0.5; x: alpha(white, 0.07); }"
        );
    }

    #[test]
    fn options_change_output() {
        let theme = viscas_dark();
        let opts = StyleOptions {
            ui_scale: 2.0,
            font_size: 15.0,
            mono_font: "Fira \"Code\"".to_owned(),
            tooltips: false,
        };
        let css = stylesheet(&theme, &opts);
        assert!(css.contains("\"Fira Code\", \"JetBrains Mono\""));
        assert!(css.contains("font-size: 30px")); // window font: 12 * 15/12 * 2
        assert!(css.contains("tooltip, tooltip.background { opacity: 0; }"));
    }

    #[test]
    fn scale_skips_strings() {
        let css = r#"a { font-family: "Font 10px"; content: '2px\'3px'; padding: 2px; }"#;
        assert_eq!(
            scale_px(css, 2.0, 2.0),
            r#"a { font-family: "Font 10px"; content: '2px\'3px'; padding: 4px; }"#
        );
    }

    #[test]
    fn theme_name_never_reaches_css() {
        let mut theme = viscas_dark();
        theme.name = "**//} window { background: red; } /*".to_owned();
        let css = palette_css(&theme);
        assert!(!css.contains("background: red"));
        assert!(!css.contains("window"));
    }

    #[test]
    fn mono_family_is_sanitized() {
        let fam = mono_families("x\"; } window { color: red; } /*");
        assert!(fam.starts_with("\"x window color red\", "), "{fam}");
        assert!(!fam.contains('{') && !fam.contains(';') && !fam.contains('*'));
    }

    #[test]
    fn mono_family_fallback() {
        assert!(mono_families("  ").starts_with("\"JetBrains Mono\""));
        assert!(mono_families("Hack").starts_with("\"Hack\", "));
    }
}

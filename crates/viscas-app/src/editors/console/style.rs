//! Stylesheet of the Console and Expressions editors.
//!
//! Installed once as its own provider so the editors stay self-contained:
//! it defines `@vx_*` colors from the active theme (UI palette plus the
//! theme's Console section), scales like the main stylesheet and follows
//! preference changes.

use std::cell::RefCell;

use gtk::gdk;

use crate::ctx::AppCtx;
use crate::prefs::Preferences;
use crate::theme::css::{BASE_FONT_PX, MONO_PLACEHOLDER, mono_families, scale_px};
use crate::theme::model::{ConsoleColors, Theme, UiColors};

/// Static rules, written against the `@vx_*` colors.
const RULES: &str = r#"
.console { background-color: @vx_console_background; }
.console .transcript { padding: 4px 0 8px 0; }

.console-entry {
  padding: 3px 8px 5px 6px;
  border-left: 2px solid transparent;
}
.console-entry.hovered {
  background-color: alpha(@vx_ui_tint, 0.035);
  border-left-color: alpha(@vx_console_prompt, 0.7);
}
.console-entry.linked { border-left-color: @vx_ui_accent; }

.console-prompt {
  color: @vx_console_prompt;
  font-family: __MONO__;
  font-size: 12px;
  font-weight: bold;
}
.console-echo {
  color: @vx_console_text;
  font-family: __MONO__;
  font-size: 12px;
}
.console-result {
  color: @vx_console_result;
  font-size: 14px;
}
.console-approx {
  color: @vx_console_hint;
  font-family: __MONO__;
  font-size: 11px;
}
.console-arrow { color: @vx_console_hint; font-size: 12px; }
.console-tag {
  color: @vx_ui_fg_dim;
  font-family: __MONO__;
  font-size: 10px;
  padding: 0 4px;
  border-radius: 3px;
  border: 1px solid @vx_ui_line;
  background: alpha(@vx_ui_tint, 0.03);
}
.console-tag.node {
  color: @vx_ui_accent_text;
  border-color: alpha(@vx_ui_accent, 0.45);
}
.console-error { color: @vx_console_error; }
.console-info { color: @vx_ui_fg_dim; }
.console-hint { color: @vx_console_hint; font-style: italic; }

button.console-chip {
  min-height: 16px;
  padding: 0 7px;
  border-radius: 8px;
  font-family: __MONO__;
  font-size: 11px;
  color: @vx_ui_accent_text;
  background: alpha(@vx_ui_accent, 0.10);
  border: 1px solid alpha(@vx_ui_accent, 0.35);
  box-shadow: none;
}
button.console-chip:hover { background: alpha(@vx_ui_accent, 0.22); }

.console-swatch { min-width: 10px; min-height: 10px; }
button.console-node-link {
  min-height: 16px;
  padding: 0 3px;
  background: none;
  border: none;
  box-shadow: none;
  color: @vx_console_result;
  font-size: 14px;
  font-weight: 600;
}
button.console-node-link:hover {
  color: @vx_ui_accent_text;
  text-decoration: underline;
}
button.console-add {
  min-height: 16px;
  padding: 0 6px;
  border-radius: 3px;
  font-size: 11px;
  color: @vx_ui_accent_text;
  background: none;
  border: 1px dashed alpha(@vx_ui_accent, 0.45);
  box-shadow: none;
}
button.console-add:hover { background: alpha(@vx_ui_accent, 0.14); border-style: solid; }

.console-actions button {
  min-height: 16px;
  min-width: 16px;
  padding: 0 3px;
  background: none;
  border: none;
  box-shadow: none;
  color: @vx_ui_fg_dim;
}
.console-actions button:hover { color: @vx_ui_fg; background: alpha(@vx_ui_tint, 0.08); }
.console-actions button image { -gtk-icon-size: 12px; }

.console-input-bar {
  background: @vx_console_background;
  border-top: 1px solid @vx_ui_line;
  padding: 5px 8px 6px 6px;
}
.console-field {
  min-height: 28px;
  padding: 0 8px 0 6px;
  border-radius: 4px;
  background: alpha(@vx_ui_tint, 0.06);
  border: 1px solid @vx_ui_line;
}
.console-input-bar.focused .console-field {
  border-color: @vx_ui_accent;
  box-shadow: 0 0 0 1px alpha(@vx_ui_accent, 0.45);
}
.console-field .console-prompt { margin-top: 6px; }
.console-field textview.console-input { margin-top: 6px; margin-bottom: 5px; }
.console-placeholder {
  font-family: __MONO__;
  font-size: 12px;
  color: alpha(@vx_ui_fg_dim, 0.75);
  margin-top: 6px;
}
.console-examples { padding: 2px 8px 0 8px; }
textview.console-input:focus-visible { outline: none; }
textview.console-input,
textview.console-input > text {
  background: transparent;
  color: @vx_console_text;
  caret-color: @vx_console_prompt;
  font-family: __MONO__;
  font-size: 12px;
}

.console-empty { margin: 24px 16px; }
.console-empty .placeholder-title { font-size: 13px; font-weight: 600; color: @vx_ui_fg; }
.console-empty .console-hint { margin-top: 2px; }
.console-empty .console-board-hint {
  font-family: __MONO__;
  font-size: 12px;
  color: @vx_ui_accent_text;
  margin-bottom: 2px;
}
.console-empty .console-try {
  font-family: __MONO__;
  font-size: 11px;
  color: @vx_ui_fg_dim;
}

popover.completion > contents { padding: 2px; min-width: 260px; }
.completion row {
  padding: 1px 6px;
  border-radius: 3px;
  min-height: 20px;
}
.completion row:selected { background-color: alpha(@vx_ui_accent, 0.85); color: @vx_ui_accent_fg; }
.completion row:selected .completion-detail { color: alpha(@vx_ui_accent_fg, 0.8); }
.completion-text { font-family: __MONO__; font-size: 12px; }
.completion-detail { color: @vx_ui_fg_faint; font-size: 10px; }

.kind-badge {
  min-width: 14px;
  font-family: __MONO__;
  font-size: 10px;
  font-weight: bold;
  border-radius: 3px;
  padding: 0 2px;
  color: @vx_ui_fg_dim;
  background: alpha(@vx_ui_tint, 0.06);
}
.kind-badge.node { color: @vx_ui_accent_text; background: alpha(@vx_ui_accent, 0.14); }
.kind-badge.console { color: @vx_console_result; background: alpha(@vx_console_result, 0.12); }
.kind-badge.builtin { color: @vx_console_prompt; background: alpha(@vx_console_prompt, 0.12); }
.kind-badge.command { color: @vx_ui_fg; }

.expr-list { background-color: @vx_ui_bg1; }
.expr-row {
  padding: 2px 4px 2px 0;
  border-left: 3px solid transparent;
  border-bottom: 1px solid @vx_ui_line;
}
.expr-row.hovered { background-color: alpha(@vx_ui_tint, 0.08); }
.expr-row.selected {
  background-color: alpha(@vx_ui_accent, 0.13);
  border-left-color: @vx_ui_accent;
}
.expr-row.drop-before { box-shadow: inset 0 2px @vx_ui_accent; }
.expr-row.drop-after { box-shadow: inset 0 -2px @vx_ui_accent; }
.expr-row.hidden-node .expr-line { color: @vx_ui_fg_faint; }
.expr-index {
  min-width: 18px;
  color: @vx_ui_fg_faint;
  font-family: __MONO__;
  font-size: 10px;
}
.expr-handle { color: @vx_ui_fg_faint; }
.expr-handle:hover { color: @vx_ui_fg; }
.expr-row entry.expr-line {
  min-height: 20px;
  padding: 0 4px;
  border: 1px solid transparent;
  background: transparent;
  box-shadow: none;
  font-family: __MONO__;
  font-size: 12px;
}
.expr-row entry.expr-line:focus-within {
  background: @vx_ui_bg0;
  border-color: alpha(@vx_ui_accent, 0.6);
}
button.expr-swatch {
  min-width: 14px;
  min-height: 14px;
  padding: 0;
  margin: 3px 2px;
  border-radius: 7px;
  border: 1px solid alpha(@vx_ui_shadow, 0.6);
}
.expr-row button.flat { min-height: 18px; min-width: 18px; padding: 0 2px; color: @vx_ui_fg_dim; }
.expr-row button.flat:hover { color: @vx_ui_fg; }
.expr-row button.flat image { -gtk-icon-size: 12px; }
.expr-error {
  color: @vx_console_error;
  font-size: 11px;
  margin-left: 2px;
}
.expr-value { color: @vx_console_result; font-family: __MONO__; font-size: 11px; }
.expr-approx { color: @vx_console_hint; font-family: __MONO__; font-size: 11px; }
.expr-slider scale { padding: 0 4px; min-height: 16px; }
.expr-slider scale trough {
  min-height: 3px;
  border-radius: 2px;
  background-color: @vx_ui_bg4;
}
.expr-slider scale highlight { background-color: @vx_ui_accent; border-radius: 2px; }
.expr-slider scale slider {
  min-width: 10px;
  min-height: 10px;
  margin: -4px;
  border-radius: 6px;
  background: @vx_ui_fg;
  border: 1px solid @vx_ui_border;
}
.expr-slider-label { color: @vx_ui_fg_faint; font-size: 10px; }
.expr-add { padding: 3px 4px 3px 24px; }
.expr-add entry.expr-line { color: @vx_ui_fg_dim; }
.expr-add image { color: @vx_ui_fg_faint; }
.expr-empty { margin: 18px 12px 4px 12px; }
"#;

/// `@define-color` lines for every color the rules may use.
fn colors_css(theme: &Theme) -> String {
    let mut out = String::from("/* console + expressions palette */\n");
    for field in UiColors::FIELDS {
        out.push_str(&format!(
            "@define-color vx_ui_{} {};\n",
            field.key,
            (field.get)(theme).to_css()
        ));
    }
    for field in ConsoleColors::FIELDS {
        out.push_str(&format!(
            "@define-color vx_console_{} {};\n",
            field.key,
            (field.get)(theme).to_css()
        ));
    }
    out
}

/// The complete stylesheet for `prefs`.
pub fn stylesheet(prefs: &Preferences) -> String {
    let theme = prefs.effective_theme();
    let rules = RULES.replace(MONO_PLACEHOLDER, &mono_families(&prefs.interface.mono_font));
    let scale = prefs.interface.ui_scale;
    let font = scale * prefs.interface.font_size / BASE_FONT_PX;
    let mut out = colors_css(&theme);
    out.push_str(&scale_px(&rules, scale, font));
    out
}

thread_local! {
    static PROVIDER: RefCell<Option<(gtk::CssProvider, String)>> = const { RefCell::new(None) };
}

fn apply(prefs: &Preferences) {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let sheet = stylesheet(prefs);
    PROVIDER.with_borrow_mut(|slot| match slot {
        Some((_, current)) if *current == sheet => {}
        Some((provider, current)) => {
            provider.load_from_string(&sheet);
            *current = sheet;
        }
        None => {
            let provider = gtk::CssProvider::new();
            provider.connect_parsing_error(|_, section, err| {
                gtk::glib::g_warning!("viscas", "console CSS {}: {err}", section.to_str());
            });
            provider.load_from_string(&sheet);
            // One above the main theme so these rules win ties.
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            *slot = Some((provider, sheet));
        }
    });
}

/// Installs the stylesheet (once) and keeps it in sync with `ctx`'s
/// preferences.
pub fn ensure(ctx: &AppCtx) {
    let installed = PROVIDER.with_borrow(|p| p.is_some());
    ctx.preferences().with(apply);
    if !installed {
        ctx.preferences().connect_changed(|p| {
            apply(p);
            true
        });
    }
}

/// Theme color as a CSS hex string, for Pango markup.
pub fn hex(color: crate::theme::model::Color) -> String {
    crate::theme::model::Color { a: 255, ..color }.to_hex()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_used_color_is_defined() {
        let sheet = stylesheet(&Preferences::default());
        let mut rest = sheet.as_str();
        while let Some(i) = rest.find("@vx_") {
            let name: String = rest[i + 1..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            assert!(
                sheet.contains(&format!("@define-color {name} ")),
                "undefined color {name}"
            );
            rest = &rest[i + 1..];
        }
        assert!(!sheet.contains(MONO_PLACEHOLDER));
        assert!(
            !sheet.contains("@vc_"),
            "must not rely on the main provider"
        );
    }

    #[test]
    fn follows_theme_and_scale() {
        let mut prefs = Preferences::default();
        let dark = stylesheet(&prefs);
        prefs.theme = crate::theme::model::viscas_light();
        assert_ne!(stylesheet(&prefs), dark);
        prefs.interface.ui_scale = 2.0;
        assert!(stylesheet(&prefs).contains("min-height: 40px"));
    }
}

//! Applies the active theme: generates the stylesheet from theme data and
//! interface preferences and (re)loads it live, and keeps GTK's stock
//! widgets in the matching dark or light mode.

pub mod css;
pub mod model;

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::prefs::Preferences;

thread_local! {
    /// The installed provider and the stylesheet it currently holds.
    static PROVIDER: RefCell<Option<(gtk::CssProvider, String)>> = const { RefCell::new(None) };
    /// The color scheme last given to GTK.
    static LAST_DARK: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Stylesheet options taken from the preferences.
pub fn style_options(prefs: &Preferences) -> css::StyleOptions {
    css::StyleOptions {
        ui_scale: prefs.interface.ui_scale,
        font_size: prefs.interface.font_size,
        mono_font: prefs.interface.mono_font.clone(),
        tooltips: prefs.interface.tooltips,
    }
}

/// Installs or updates the theme on the default display. Cheap when
/// nothing relevant changed.
pub fn apply(prefs: &Preferences) {
    let Some(display) = gdk::Display::default() else {
        glib::g_warning!("viscas", "no display; theme not installed");
        return;
    };
    let theme = prefs.effective_theme();
    if LAST_DARK.replace(Some(theme.dark)) != Some(theme.dark) {
        set_color_scheme(&display, theme.dark);
        use_desktop_font_hinting(&display);
    }
    let sheet = css::stylesheet(&theme, &style_options(prefs));
    PROVIDER.with_borrow_mut(|slot| match slot {
        Some((_, current)) if *current == sheet => {}
        Some((provider, current)) => {
            provider.load_from_string(&sheet);
            *current = sheet;
        }
        None => {
            let provider = gtk::CssProvider::new();
            provider.connect_parsing_error(|_, section, err| {
                glib::g_warning!("viscas", "theme CSS {}: {err}", section.to_str());
            });
            provider.load_from_string(&sheet);
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            *slot = Some((provider, sheet));
        }
    });
}

/// Renders text with the desktop's hinting settings (GTK's `manual` font
/// rendering) instead of GTK 4.16+'s `automatic` mode. Unhinted glyphs at
/// our 11–12 px sizes on a 1× display lose thin strokes: `=` drew as a
/// single bar, indistinguishable from `-`, which a math app cannot afford.
fn use_desktop_font_hinting(display: &gdk::Display) {
    let settings = gtk::Settings::for_display(display);
    let manual = settings
        .find_property("gtk-font-rendering")
        .and_then(|pspec| glib::EnumClass::with_type(pspec.value_type()))
        .and_then(|class| class.to_value_by_nick("manual"));
    if let Some(value) = manual {
        settings.set_property_from_value("gtk-font-rendering", &value);
    }
}

/// Forces GTK's built-in Adwaita (so desktop themes do not leak into
/// widgets we do not restyle) in dark or light mode.
fn set_color_scheme(display: &gdk::Display, dark: bool) {
    let settings = gtk::Settings::for_display(display);
    if settings.gtk_theme_name().as_deref() != Some("Adwaita") {
        settings.set_gtk_theme_name(Some("Adwaita"));
    }
    // GTK 4.20 replaced the prefer-dark flag by a color scheme setting.
    let nick = if dark { "dark" } else { "light" };
    let scheme = settings
        .find_property("gtk-interface-color-scheme")
        .and_then(|pspec| glib::EnumClass::with_type(pspec.value_type()))
        .and_then(|class| class.to_value_by_nick(nick));
    if let Some(value) = scheme {
        settings.set_property_from_value("gtk-interface-color-scheme", &value);
    } else {
        settings.set_gtk_application_prefer_dark_theme(dark);
    }
}

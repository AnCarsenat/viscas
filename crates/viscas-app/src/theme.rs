//! Loads the Viscas CSS theme (`resources/theme.css`).

use gtk::prelude::*;
use gtk::{gdk, glib};

/// The stylesheet, embedded at compile time.
const THEME_CSS: &str = include_str!("../resources/theme.css");

/// Installs the theme on the default display: forces GTK's built-in dark
/// Adwaita as a predictable base, then layers our stylesheet on top at
/// APPLICATION priority.
pub fn install() {
    let Some(display) = gdk::Display::default() else {
        glib::g_warning!("viscas", "no display; theme not installed");
        return;
    };
    let settings = gtk::Settings::for_display(&display);
    // The desktop theme (Breeze, a custom GTK theme, ...) would otherwise
    // leak into widgets we do not restyle explicitly.
    settings.set_property("gtk-theme-name", "Adwaita");
    // GTK 4.20 replaced the prefer-dark flag by a color scheme setting.
    let dark_scheme = settings
        .find_property("gtk-interface-color-scheme")
        .and_then(|pspec| glib::EnumClass::with_type(pspec.value_type()))
        .and_then(|class| class.to_value_by_nick("dark"));
    if let Some(dark) = dark_scheme {
        settings.set_property_from_value("gtk-interface-color-scheme", &dark);
    } else {
        settings.set_property("gtk-application-prefer-dark-theme", true);
    }

    let provider = gtk::CssProvider::new();
    provider.load_from_string(THEME_CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

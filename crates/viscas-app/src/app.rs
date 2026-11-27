//! Application setup: app-level actions and dialogs (key bindings are
//! dispatched by `keymap::live`).

use gtk::gio;
use gtk::prelude::*;

use crate::ctx::AppCtx;
use crate::{prefs_window, settings, window};

pub const APP_ID: &str = "io.github.ancarsenat.Viscas";

/// GResource path of the embedded icons (hicolor layout below it).
const ICON_RESOURCE_PATH: &str = "/io/github/ancarsenat/Viscas/icons";

/// Registers the embedded app icon with the icon theme and makes it the
/// default window icon. Works wherever the binary runs; an installed
/// copy (see the README) only matters to the desktop's launcher and task
/// bar, which find the icon through the `.desktop` file.
pub fn install_icon() {
    register_resources();
    if let Some(display) = gtk::gdk::Display::default() {
        let theme = gtk::IconTheme::for_display(&display);
        theme.add_resource_path(ICON_RESOURCE_PATH);
        if !theme.has_icon(APP_ID) {
            gtk::glib::g_warning!("viscas", "the embedded app icon was not found");
        }
    }
    gtk::Window::set_default_icon_name(APP_ID);
}

/// Registers the embedded GResource bundle (once per process).
fn register_resources() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        gio::resources_register_include!("viscas.gresource").expect("embedded resources are valid");
    });
}

/// Creates the application with all actions and accelerators installed.
pub fn build() -> gtk::Application {
    let app = gtk::Application::builder().application_id(APP_ID).build();
    // One context for the application, shared by every window.
    let ctx = AppCtx::new();
    let startup_ctx = ctx.clone();
    app.connect_startup(move |app| setup(app, &startup_ctx));
    // Write a pending automatic preferences save before exiting.
    let shutdown_ctx = ctx.clone();
    app.connect_shutdown(move |_| settings::flush(&shutdown_ctx));
    app.connect_activate(move |app| {
        let window = app
            .active_window()
            .unwrap_or_else(|| window::build(app, &ctx).upcast());
        window.present();
    });
    app
}

fn setup(app: &gtk::Application, ctx: &AppCtx) {
    install_icon();
    settings::init(ctx);
    crate::intellisense::register_keys();
    crate::keymap::live::bind(ctx);
    crate::doc_ui::connect_prefs(ctx);

    // Quit goes through the window's close flow: one save prompt for the
    // shared document; Cancel aborts the whole quit.
    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &gtk::Application, _, _| match app.active_window() {
            Some(w) => {
                if WidgetExt::activate_action(&w, "win.quit-request", None).is_err() {
                    app.quit();
                }
            }
            None => app.quit(),
        })
        .build();
    let about = gio::ActionEntry::builder("about")
        .activate(|app: &gtk::Application, _, _| show_about(app))
        .build();
    let shortcuts = gio::ActionEntry::builder("shortcuts")
        .activate(|app: &gtk::Application, _, _| show_shortcuts(app))
        .build();
    let prefs_ctx = ctx.clone();
    let preferences = gio::ActionEntry::builder("preferences")
        .activate(move |app: &gtk::Application, _, _| prefs_window::present(app, &prefs_ctx, None))
        .build();
    app.add_action_entries([quit, about, shortcuts, preferences]);
}

fn show_about(app: &gtk::Application) {
    let icon = gtk::gdk::Display::default()
        .filter(|d| gtk::IconTheme::for_display(d).has_icon(APP_ID))
        .map_or("accessories-calculator", |_| APP_ID);
    let repo = env!("CARGO_PKG_REPOSITORY");
    let dialog = gtk::AboutDialog::builder()
        .program_name("Viscas")
        .logo_icon_name(icon)
        .comments("Visual computer algebra workstation")
        .version(env!("CARGO_PKG_VERSION"))
        .authors(["The Viscas contributors"])
        .website(repo)
        .license_type(gtk::License::Custom)
        .license(
            "Licensed under either of the Apache License 2.0 or the MIT license, at your option.",
        )
        .modal(true)
        .build();
    dialog.set_icon_name(Some(APP_ID));
    dialog.set_transient_for(app.active_window().as_ref());
    dialog.present();
}

/// Temporary shortcuts overview until a proper shortcuts dialog exists.
fn show_shortcuts(app: &gtk::Application) {
    let dialog = gtk::AlertDialog::builder()
        .message("Keyboard Shortcuts")
        .detail(crate::keymap::live::overview())
        .buttons(["Close"])
        .modal(true)
        .build();
    dialog.show(app.active_window().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The icon theme looks icons up as `<resource path>/<size>/apps/<id>`.
    #[test]
    fn icons_are_embedded_in_the_hicolor_layout() {
        register_resources();
        for name in [APP_ID.to_owned(), format!("{APP_ID}-symbolic")] {
            let path = format!("{ICON_RESOURCE_PATH}/scalable/apps/{name}.svg");
            let data = gio::resources_lookup_data(&path, gio::ResourceLookupFlags::NONE)
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            assert!(data.starts_with(b"<svg"), "{path}");
        }
    }
}

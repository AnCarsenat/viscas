//! Application setup: app-level actions, accelerators and dialogs.

use adw::prelude::*;
use gtk::gio;

use crate::window;

pub const APP_ID: &str = "io.github.ancarsenat.Viscas";

/// Creates the application with all actions and accelerators installed.
pub fn build() -> adw::Application {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(setup);
    app.connect_activate(|app| {
        let window = app
            .active_window()
            .unwrap_or_else(|| window::build(app).upcast());
        window.present();
    });
    app
}

fn setup(app: &adw::Application) {
    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| app.quit())
        .build();
    let about = gio::ActionEntry::builder("about")
        .activate(|app: &adw::Application, _, _| show_about(app))
        .build();
    let shortcuts = gio::ActionEntry::builder("shortcuts")
        .activate(|app: &adw::Application, _, _| show_shortcuts(app))
        .build();
    app.add_action_entries([quit, about, shortcuts]);

    app.set_accels_for_action("app.quit", &["<Control>q"]);
    app.set_accels_for_action("app.shortcuts", &["<Control>question"]);
    for (action, accels) in window::accels() {
        let accels: Vec<&str> = accels.iter().map(String::as_str).collect();
        app.set_accels_for_action(&action, &accels);
    }
}

fn show_about(app: &adw::Application) {
    let icon = gtk::gdk::Display::default()
        .filter(|d| gtk::IconTheme::for_display(d).has_icon(APP_ID))
        .map_or("accessories-calculator", |_| APP_ID);
    let repo = env!("CARGO_PKG_REPOSITORY");
    let dialog = adw::AboutDialog::builder()
        .application_name("Viscas")
        .application_icon(icon)
        .comments("Graphing calculator and computer algebra system")
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("The Viscas contributors")
        .website(repo)
        .issue_url(format!("{repo}/issues"))
        .license_type(gtk::License::Custom)
        .license(
            "Licensed under either of the Apache License 2.0 or the MIT license, at your option.",
        )
        .build();
    dialog.present(app.active_window().as_ref());
}

/// Temporary shortcuts overview until a proper shortcuts dialog exists.
fn show_shortcuts(app: &adw::Application) {
    const SHORTCUTS: &str = "\
Ctrl+Q\tQuit
F9\tToggle sidebar
Ctrl+1 … Ctrl+5\tSwitch sidebar panel
Scroll\tZoom graph at cursor
Drag\tPan graph";
    let dialog = adw::AlertDialog::builder()
        .heading("Keyboard Shortcuts")
        .body(SHORTCUTS)
        .build();
    dialog.add_response("close", "_Close");
    dialog.present(app.active_window().as_ref());
}

//! Application setup: app-level actions, accelerators and dialogs.

use gtk::gio;
use gtk::prelude::*;

use crate::{shortcuts, theme, window};

pub const APP_ID: &str = "io.github.ancarsenat.Viscas";

/// Creates the application with all actions and accelerators installed.
pub fn build() -> gtk::Application {
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_startup(setup);
    app.connect_activate(|app| {
        let window = app
            .active_window()
            .unwrap_or_else(|| window::build(app).upcast());
        window.present();
    });
    app
}

fn setup(app: &gtk::Application) {
    theme::install();

    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &gtk::Application, _, _| app.quit())
        .build();
    let about = gio::ActionEntry::builder("about")
        .activate(|app: &gtk::Application, _, _| show_about(app))
        .build();
    let shortcuts = gio::ActionEntry::builder("shortcuts")
        .activate(|app: &gtk::Application, _, _| show_shortcuts(app))
        .build();
    app.add_action_entries([quit, about, shortcuts]);

    shortcuts::install(app);
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
    dialog.set_transient_for(app.active_window().as_ref());
    dialog.present();
}

/// Temporary shortcuts overview until a proper shortcuts dialog exists.
fn show_shortcuts(app: &gtk::Application) {
    let dialog = gtk::AlertDialog::builder()
        .message("Keyboard Shortcuts")
        .detail(shortcuts::overview())
        .buttons(["Close"])
        .modal(true)
        .build();
    dialog.show(app.active_window().as_ref());
}

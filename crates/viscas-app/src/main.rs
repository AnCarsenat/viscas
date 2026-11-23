//! Viscas desktop application (plain GTK4 shell with its own theme).

extern crate gtk4 as gtk;

mod app;
mod areas;
mod commands;
mod ctx;
mod doc;
mod doc_ui;
mod editors;
mod fuzzy;
mod layout;
mod palette;
mod prefs;
mod prefs_window;
mod settings;
mod shortcuts;
mod theme;
mod viewport;
mod widgets;
mod window;
mod workspace;

use gtk::glib;
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    app::build().run()
}

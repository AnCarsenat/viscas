//! Viscas desktop application (plain GTK4 shell with its own theme).

extern crate gtk4 as gtk;

mod app;
mod areas;
mod commands;
mod compose;
mod ctx;
mod doc;
mod doc_ui;
mod docking;
mod editors;
mod export;
mod fuzzy;
mod intellisense;
mod keymap;
mod layout;
mod palette;
mod palette_sources;
mod prefs;
mod prefs_keymap;
mod prefs_window;
mod settings;
mod shortcuts;
mod status;
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

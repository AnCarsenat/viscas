//! Viscas desktop application (plain GTK4 shell with its own theme).

extern crate gtk4 as gtk;

mod app;
mod areas;
mod commands;
mod editors;
mod fuzzy;
mod layout;
mod palette;
mod shortcuts;
mod theme;
mod viewport;
mod window;
mod workspace;

use gtk::glib;
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    app::build().run()
}

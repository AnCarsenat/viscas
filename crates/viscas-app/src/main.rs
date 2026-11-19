//! Viscas desktop application (GTK4 + libadwaita shell).

extern crate gtk4 as gtk;

mod app;
mod panels;
mod viewport;
mod window;

use adw::prelude::*;
use gtk::glib;

fn main() -> glib::ExitCode {
    app::build().run()
}

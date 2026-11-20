//! Viscas desktop application (GTK4 + libadwaita shell).

extern crate gtk4 as gtk;

mod app;
#[allow(dead_code, reason = "wired into the window in the next commit")]
mod fuzzy;
#[allow(dead_code, reason = "wired into the window in the next commit")]
mod layout;
mod panels;
mod shortcuts;
mod viewport;
mod window;
#[allow(dead_code, reason = "wired into the window in the next commit")]
mod workspace;

use adw::prelude::*;
use gtk::glib;

fn main() -> glib::ExitCode {
    app::build().run()
}

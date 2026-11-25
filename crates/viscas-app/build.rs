//! Compiles the app resources (icons) into a GResource bundle that the
//! binary embeds (`gio::resources_register_include!`), so the app icon
//! works wherever the binary runs.
//!
//! Runs `glib-compile-resources`, or the program named by
//! `$GLIB_COMPILE_RESOURCES`.

use std::path::PathBuf;
use std::process::Command;

const XML: &str = "resources/viscas.gresource.xml";
const SOURCE_DIR: &str = "resources";

fn main() {
    println!("cargo:rerun-if-env-changed=GLIB_COMPILE_RESOURCES");
    println!("cargo:rerun-if-changed={XML}");
    let tool =
        std::env::var("GLIB_COMPILE_RESOURCES").unwrap_or_else(|_| "glib-compile-resources".into());
    let deps = Command::new(&tool)
        .args(["--sourcedir", SOURCE_DIR, "--generate-dependencies", XML])
        .output()
        .unwrap_or_else(|e| missing_tool(&tool, &e.to_string()));
    if !deps.status.success() {
        panic!(
            "`{tool} --generate-dependencies` failed:\n{}",
            String::from_utf8_lossy(&deps.stderr)
        );
    }
    for dep in String::from_utf8_lossy(&deps.stdout).lines() {
        println!("cargo:rerun-if-changed={dep}");
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let status = Command::new(&tool)
        .args(["--sourcedir", SOURCE_DIR, "--target"])
        .arg(out.join("viscas.gresource"))
        .arg(XML)
        .status()
        .unwrap_or_else(|e| missing_tool(&tool, &e.to_string()));
    assert!(status.success(), "`{tool}` failed to compile {XML}");
}

fn missing_tool(tool: &str, error: &str) -> ! {
    panic!(
        "\n`{tool}` could not run ({error}); it compiles the embedded app icon.\n\
         Install GLib's development tools:\n  \
         Arch:          pacman -S glib2 (glib2-devel where split)\n  \
         Debian/Ubuntu: apt install libglib2.0-dev-bin\n  \
         Fedora:        dnf install glib2-devel\n  \
         openSUSE:      zypper install glib2-tools\n  \
         macOS:         brew install glib\n\
         or set GLIB_COMPILE_RESOURCES to the binary's path.\n"
    )
}

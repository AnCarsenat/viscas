//! Compiles the app resources (icons) into a GResource bundle that the
//! binary embeds, so the app icon works wherever the binary runs.

fn main() {
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/viscas.gresource.xml",
        "viscas.gresource",
    );
}

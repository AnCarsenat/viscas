# Viscas

**VISual CAS** — a graphing calculator and computer algebra system for the desktop,
written in Rust with GTK4.

Graphing, a real CAS, a console, spreadsheets and scripting in a pro-tool UI
inspired by Blender and Godot: a scene tree of nodes, an inspector, splittable
editor areas and a command palette.

See [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md) for the full spec and roadmap.

## Build

Requires Rust (stable) and GTK ≥ 4.14 development packages.

```sh
cargo run -p viscas-app
cargo test --workspace
```

The app icon is embedded in the binary (a GResource compiled by
`build.rs`), so windows and the About dialog show it wherever you run it
from. Building needs `glib-compile-resources`, which ships with GLib.

## Install the icon and desktop entry (optional)

Task bars and launchers (KDE Plasma, GNOME) find the icon through a
`.desktop` file named after the app id `io.github.ancarsenat.Viscas`;
on Wayland the window's app id is matched to it. To install for your
user:

```sh
cargo install --path crates/viscas-app   # puts `viscas` in ~/.cargo/bin
install -Dm644 data/io.github.ancarsenat.Viscas.desktop \
  ~/.local/share/applications/io.github.ancarsenat.Viscas.desktop
install -Dm644 crates/viscas-app/resources/icons/hicolor/scalable/apps/io.github.ancarsenat.Viscas.svg \
  ~/.local/share/icons/hicolor/scalable/apps/io.github.ancarsenat.Viscas.svg
install -Dm644 crates/viscas-app/resources/icons/hicolor/scalable/apps/io.github.ancarsenat.Viscas-symbolic.svg \
  ~/.local/share/icons/hicolor/symbolic/apps/io.github.ancarsenat.Viscas-symbolic.svg
```

`Exec=viscas` needs `~/.cargo/bin` on the `PATH` of your session (or
edit the line to the full path). System-wide, use `/usr/share/…`
instead of `~/.local/share/…`. If the icon does not show up right away,
run `gtk-update-icon-cache ~/.local/share/icons/hicolor` (GNOME) or
`kbuildsycoca6` (KDE), or log out and in.

## Layout

| Crate | Role |
|-------|------|
| `viscas-core` | parser, evaluator, exact arithmetic, CAS |
| `viscas-scene` | node tree, dependency graph |
| `viscas-plot` | plot sampling, view transforms |
| `viscas-script` | Viscas script + Rhai bindings |
| `viscas-app` | GTK4 UI: area/editor shell, own theme |

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

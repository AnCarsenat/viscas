# Viscas

**VISual CAS** — a graphing calculator and computer algebra system for the desktop,
written in Rust with GTK4.

Graphing, a real CAS, a console, spreadsheets and scripting in a pro-tool UI
inspired by Blender and Godot: a scene tree of nodes, an inspector, splittable
editor areas and a command palette.

See [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md) for the full spec and roadmap.

## Build

Requires Rust (stable), GTK ≥ 4.14 and libadwaita ≥ 1.5 development packages.

```sh
cargo run -p viscas-app
cargo test --workspace
```

## Layout

| Crate | Role |
|-------|------|
| `viscas-core` | parser, evaluator, exact arithmetic, CAS |
| `viscas-scene` | node tree, dependency graph |
| `viscas-plot` | plot sampling, view transforms |
| `viscas-script` | Viscas script + Rhai bindings |
| `viscas-app` | GTK4/libadwaita UI |

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

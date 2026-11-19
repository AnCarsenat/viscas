# Viscas — Requirements (v0.1)

Viscas (VISual CAS) is a desktop graphing calculator and computer algebra system
built with Rust + GTK4. Goal: everything Desmos can do, plus a real CAS, a
qalc-grade console, spreadsheets and scripting — in a very clean, keyboard-first UI.

Status: **accepted v0.1**.

---

## 1. Core principles

1. **Everything is a node.** A document is a tree of nodes (the *scene*). Variables,
   functions, plots, points, sliders, tables, scripts, viewports, folders — all nodes.
   The only things that are *not* nodes are the fixed UI panels of the left sidebar.
2. **Reactive.** Nodes reference each other by name. Changing a node recomputes only
   its dependents (dependency DAG, cycle detection with clear error).
3. **One language.** The same expression language is used in the console, the
   expression list, table cells and (as a subset) scripts.
4. **Exact first, numeric when needed.** Results stay symbolic/exact
   (`1/3`, `√2`, `π`) until a numeric value is requested or required.
5. **Clean UI.** GNOME HIG / libadwaita look, minimal chrome, light + dark theme,
   math rendered as typeset formulas, every action reachable from the keyboard.
6. **Core is UI-independent.** Parser, CAS, evaluator, scene model and plot sampling
   live in GTK-free crates with unit tests. GTK is only the shell.

## 2. Window layout

```
┌──────────────────────────────────────────────────────────────┐
│ header bar: doc name · undo/redo · view toggles · menu       │
├───────────────┬──────────────────────────────────────────────┤
│ LEFT SIDEBAR  │                                              │
│ [tabs]        │   VIEWPORT(S)  — viewport nodes              │
│  Expressions  │   (2D graph, later 3D; can split/tab)        │
│  Console      │                                              │
│  Sheets       │                                              │
│  Scripts      │                                              │
│  Scene        │                                              │
└───────────────┴──────────────────────────────────────────────┘
```

Sidebar panels (fixed UI, not nodes):

| Panel        | Purpose |
|--------------|---------|
| Expressions  | Desmos-style ordered list of statement nodes. Typeset editing, color swatch, visibility toggle, inline slider, inline errors. |
| Console      | REPL like `qalc`, but better: history, typeset output, tab completion, `ans`, unit conversion (`to`), exact/approx toggle, results can be "pinned" into the scene as nodes. |
| Sheets       | Spreadsheet editor for table nodes: columns are lists, cells hold expressions, formulas reference other columns/nodes, columns plottable. |
| Scripts      | Code editor for script nodes: syntax highlight, run/stop, output log. |
| Scene        | Tree view of all nodes: rename, reorder, group in folders, hide, lock, inspect properties and dependencies. |

The sidebar is collapsible and resizable. Viewports fill the remaining space.

## 3. Node model

Every node has: unique id, optional name, parent, visibility, lock, style
(color, line width/style, opacity, label), and a body.

| Node kind   | Example | Notes |
|-------------|---------|-------|
| Variable    | `a = 3` | |
| Function    | `f(x) = x^2 + a` | multi-arg allowed |
| Plot        | `y = sin(x)`, `r = θ`, `(cos t, sin t)`, `x^2+y^2 < 4` | explicit, polar, parametric, implicit, inequalities, restrictions `{0<x<2}` |
| Point       | `P = (1, 2)` | draggable when defined by free variables |
| Slider      | `a = 3` with range/step/animation | a Variable with slider metadata |
| List        | `L = [1..10]` | comprehensions `[k^2 for k=[1..5]]` |
| Geometry    | `segment(A,B)`, `circle(C,r)`, `polygon(...)` | |
| Table       | sheet of columns | edited in Sheets panel |
| Script      | code body | edited in Scripts panel |
| Action/Ticker | `a -> a+1`, ticker every 100 ms | |
| Note        | text / markdown | |
| Folder      | group of nodes | |
| Viewport    | 2D graph view (bounds, grid, axes, projection) | multiple viewports per doc |
| Regression  | `y1 ~ m x1 + b` | creates fitted parameter nodes |

## 4. Expression language

- Numbers: integers, rationals, decimals, scientific (`1.2e-3`), complex (`3+2i`).
- Implicit multiplication (`2x`, `2(x+1)`, `sin 2x`), unicode input (`π`, `θ`, `√`)
  with ASCII aliases (`pi`, `theta`, `sqrt`).
- Operators: `+ - * / ^ ! %`, comparisons, `and/or/not`, `mod`.
- Piecewise: `{x < 0: -x, x}`.
- Lists, ranges `[1..10]`, `[0, 0.1 .. 1]`, indexing `L[2]`, filters `L[L > 3]`,
  comprehensions.
- Points/vectors `(x, y)`, matrices `[[1,2],[3,4]]`, member access `P.x`.
- Units (`5 km/h to m/s`), constants (`c`, `g`, `e`, `φ`…).
- Calculus syntax: `d/dx f(x)`, `f'(x)`, `∫(a,b) f(x) dx`, `∑(k=1,n) k`, `∏`.

## 5. Function library

**Desmos parity (required):**
- Trig: sin cos tan csc sec cot + inverses + hyperbolic + inverse hyperbolic; degree/radian mode.
- Elementary: exp ln log log_b sqrt nthroot abs sign floor ceil round mod lcm gcd
  nCr nPr factorial min max.
- Lists/stats: total length count mean median mode min max quantile quartile
  stdev stdevp var varp mad cov corr spearman stats sort shuffle unique join repeat.
- Distributions: normaldist tdist poissondist binomialdist uniformdist with
  pdf/cdf/random; histogram, boxplot, dotplot.
- Tests: ttest ittest (two-sample t, etc.).
- Regression with `~` (linear, polynomial, exponential, log, custom), residuals, r².
- Geometry: point line segment ray vector circle arc polygon distance midpoint
  intersection perpendicular parallel angle, transformations (translate rotate
  reflect dilate).
- Colors: rgb hsv; random; actions and tickers.
- Complex mode.

**CAS (beyond Desmos):**
- simplify expand factor collect partialfraction
- solve (equations, systems, inequalities), nsolve (numeric), roots
- derivative (symbolic, partial), integrate (symbolic + numeric), limit, series/taylor
- sum/product closed forms where possible
- matrices: det inv transpose rank eigen rref
- number theory: isprime factorint
- exact arithmetic: big integers, rationals, surds, π/e symbolic

## 6. Graphing

- 2D: explicit, implicit (marching squares), polar, parametric, inequalities
  (shaded), points, lists of curves, piecewise, discontinuity detection.
- Adaptive sampling; 60 fps pan/zoom on typical documents; sampling off the UI thread.
- Pan, zoom (scroll, pinch, box), reset, lock aspect, grid/axes options, polar grid.
- Hover trace, click curve to show point coordinates, intersections, extrema, roots.
- Draggable points that update their defining variables.
- Export viewport as PNG/SVG.
- 3D viewport: later milestone (M9).

## 7. Console

- `qalc`-style one-line evaluation with typeset output and exact + approx results.
- History (persisted), up/down recall, `ans`, `ans2`.
- Tab completion for functions, nodes, units.
- Commands: `:help f`, `:pin` (turn result into node), `:clear`, `:mode deg|rad|exact|approx`, `to <unit>`.
- Shares the scene's variables: console sees `a`, `f`, etc.

## 8. Sheets

- Table node = named columns; each column is a list usable anywhere (`T.x1`).
- Cells: values or expressions; whole-column formulas (`y1 = x1^2`).
- Plot columns as points/lines; regression on columns.
- Import/export CSV.

## 9. Scripting

- Script nodes run code with access to the scene API: read/write nodes, create
  nodes, loop, animate, define helper functions.
- Sandboxed: no filesystem/network unless explicitly allowed.
- Two layers:
  1. **Viscas language** — the expression language grows into a math + scripting
     language: `let`, `if/else`, `for`, `while`, user functions with bodies,
     `return`. Usable in console, expressions and script nodes. Math-native
     (exact values, units, lists, implicit multiplication).
  2. **Rhai** — embedded general-purpose scripting for heavier automation
     (string handling, complex control flow, maps). Script node picks language
     (`viscas` or `rhai`). Rhai gets bindings to the scene API and can call any
     Viscas function / evaluate Viscas expressions (`eval("d/dx x^3")`).

## 10. Documents

- Save/open `.viscas` files (human-readable text format, versioned).
- Undo/redo for every edit.
- Autosave + recent files.

## 11. Non-functional

- Linux first (GTK4 + libadwaita); Windows/macOS best effort.
- Startup < 1 s; UI never blocks on computation (cancellable workers).
- `cargo test` covers parser, CAS, evaluator, scene; CI runs fmt + clippy + test.

## 12. Architecture (proposed)

Cargo workspace:

| Crate           | Responsibility | Depends on GTK |
|-----------------|----------------|----------------|
| `viscas-core`   | lexer, parser, AST, values, exact arithmetic, evaluator, function library, CAS | no |
| `viscas-scene`  | node tree, dependency DAG, reactive recompute, undo, (de)serialization | no |
| `viscas-plot`   | sampling, implicit curves, view transforms → draw primitives | no |
| `viscas-script` | scripting engine + scene bindings | no |
| `viscas-app`    | GTK4/libadwaita shell: sidebar panels, viewport widget, math rendering | yes |

## 13. Milestones

| M  | Content |
|----|---------|
| M0 | Workspace, CI, app shell (header bar + sidebar tabs + empty viewport) |
| M1 | Parser + exact/numeric evaluator + console panel |
| M2 | Scene model + expressions panel (text input + typeset preview) + 2D viewport plotting `y=f(x)` |
| M2b | Structural math editor widget |
| M3 | All 2D plot kinds, sliders, points, pan/zoom/trace |
| M4 | Function library to Desmos parity (lists, stats, distributions, geometry, regression) |
| M5 | CAS: simplify/expand/factor/solve/derivative/integrate/limit/series, matrices |
| M6 | Sheets panel + table nodes + CSV |
| M7 | Scripting + actions/tickers |
| M8 | Documents: save/load, undo/redo, export; polish |
| M9 | 3D viewport |

Each milestone splits into feature branches → PR → review → squash merge (GitHub flow).

## 14. Decisions

1. Scripting: Viscas language (math + scripting) + embedded Rhai.
2. Precision: f64 for plotting; pure-Rust arbitrary precision (`dashu` / `num`)
   for console and CAS. No C dependencies.
3. 3D: later milestone (M9). 2D viewport code should not block adding it.
4. Math input: both in v1 — plain-text input with live typeset preview, and a
   Desmos-style structural editor (cursor inside fractions, exponents, roots).
   Toggle per field; both edit the same AST. Own math layout engine (Cairo/Pango).
5. Units: physical units and constants. No live currency rates (no network).
6. License: MIT OR Apache-2.0.

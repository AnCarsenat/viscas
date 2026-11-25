# Viscas — Requirements (v0.2)

Viscas (VISual CAS) is a desktop math workstation: graphing, a real CAS, a
console, spreadsheets and scripting, built with Rust + GTK4.

**Identity.** Viscas is *not* a Desmos or GeoGebra clone. It is a professional
tool in the spirit of Blender and Godot: a scene tree of nodes, an inspector,
splittable editor areas, a console as the primary input, and a dark, dense,
precise interface. Math objects are built, inspected and wired like objects in a
3D/game engine.

Status: **v0.2 — identity revision**. Supersedes the "Desmos parity" framing of v0.1.

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
5. **Pro-tool UI.** Blender-like: dark-first custom theme, dense compact widgets,
   splittable editor areas, drag-to-scrub number fields, precise typography with
   monospace accents, math rendered as typeset formulas. Every action reachable from
   the keyboard and the command palette. Own GTK CSS theme — not the stock
   libadwaita/GNOME look.
7. **Own identity.** Function names, syntax and behaviours are designed for Viscas
   (CAS + scripting first), not copied from Desmos/GeoGebra.
6. **Core is UI-independent.** Parser, CAS, evaluator, scene model and plot sampling
   live in GTK-free crates with unit tests. GTK is only the shell.

## 2. Window layout — areas and editors

The window is split into **areas**, Blender-style. Each area hosts one **editor**
and has a thin header strip with an editor-type switcher and editor-specific menus.
Areas can be split, joined, resized, swapped and maximized (Ctrl+Space).

**True docking (Blender-style):**
- Drag from an area corner into the area to split it; drag into a neighbour to
  join (the neighbour is absorbed, with a preview overlay).
- Drag an area by its header onto another area: drop zones (left/right/top/
  bottom/center) dock it there or swap.
- Tear off an area into its own floating window (drag outside, or header menu
  "Open in New Window"); dock it back the same way.
- All of this is layout-model operations, undoable for layout, saved per
  document/workspace (including floating windows).

```
┌─────────────────────────────────────────────────────────────────────┐
│ top bar: menus · workspace tabs [Graph][Algebra][Data][Script] · ^P │
├──────────────┬──────────────────────────────────────┬───────────────┤
│ Scene Tree   │                                      │ Inspector     │
│ (Godot-like) │          Viewport (2D)               │ (properties   │
│              │                                      │  of selected  │
├──────────────┤                                      │  node)        │
│ Console /    │                                      │               │
│ Expressions /├──────────────────────────────────────┤               │
│ Sheet /Script│   Dependency Graph  (optional area)  │               │
├──────────────┴──────────────────────────────────────┴───────────────┤
│ status bar: mode (exact/approx · rad/deg) · selection · eval status │
└─────────────────────────────────────────────────────────────────────┘
```

Editors (UI, not nodes):

| Editor           | Purpose |
|------------------|---------|
| Viewport         | Renders a Viewport node (2D now, 3D later). Several can be open at once. |
| Scene Tree       | Godot-style tree of all nodes: typed icons, rename, reparent by drag, folders, hide/lock toggles, filter. |
| Inspector        | Properties of the selected node(s): expression, style, ranges, metadata; compact fields, drag-to-scrub numbers, multi-edit. |
| Console          | Primary input. REPL (qalc-grade): history, typeset output, completion, `ans`, unit conversion, exact/approx; nothing typed in it goes on the board until added explicitly ("Add to board", Ctrl+Shift+Enter, `:add`). |
| Expressions      | Flat ordered list of statement nodes for quick editing (a view of the tree, not a separate model). |
| Sheet            | Spreadsheet editor for table nodes. |
| Script           | Code editor for script nodes (Viscas language or Rhai). |
| Dependency Graph | Node-graph view of how nodes depend on each other (see §2.1). |

**Workspaces** are saved area layouts (tabs in the top bar): *Graph*, *Algebra*,
*Data*, *Script*; user-defined workspaces allowed. Layout is stored per document
with a user default.

### 2.1 Signature features

1. **Dependency view.** Dependency Graph editor shows nodes as boxes with edges
   for references. Hovering/selecting any node (in any editor) highlights its
   upstream inputs and downstream dependents everywhere. Editing an edge
   (re-pointing a reference) rewrites the expression. Rendered as an
   infinite-zoom canvas with nested capsules and patch cables (see decision 8).
2. **Step-by-step CAS.** Every symbolic result (simplify, solve, derivative,
   integrate, limit…) can expand into its derivation: a sequence of rewrite
   steps with the rule applied, each step typeset and copyable.
3. **Command palette (Ctrl+P / Space).** Fuzzy search over every action, node,
   function, unit and setting. Keyboard-only workflow is a first-class goal.

## 3. Node model

Godot-style scene tree. Every node has: unique id, optional name, type (with
icon), parent, children, visibility, lock, style (color, line width/style,
opacity, label), and a body. Node properties are what the Inspector edits.

| Node kind   | Example | Notes |
|-------------|---------|-------|
| Variable    | `a := 3` | |
| Function    | `f(x) := x^2 + a` | multi-arg allowed |
| Plot        | `y = sin(x)`, `r = θ`, `(cos t, sin t)`, `x^2+y^2 < 4` | explicit, polar, parametric, implicit, inequalities, restrictions `{0<x<2}` |
| Point       | `P := (1, 2)` | draggable when defined by free variables |
| Slider      | `a := 3` with range/step/animation | a Variable with slider metadata |
| List        | `L := [1..10]` | comprehensions `[k^2 for k=[1..5]]` |
| Geometry    | `segment(A,B)`, `circle(C,r)`, `polygon(...)` | |
| Table       | sheet of columns | edited in Sheet editor |
| Script      | code body | edited in Script editor |
| Action/Ticker | `a -> a+1`, ticker every 100 ms | |
| Note        | text / markdown | |
| Folder      | group of nodes | |
| Viewport    | 2D graph view (bounds, grid, axes, projection) | multiple viewports per doc |
| Regression  | `y1 ~ m x1 + b` | creates fitted parameter nodes |

## 4. Expression language

- Definitions use `:=` (`a := 3`, `f(x) := x^2`); plain `=` always states an
  equation/relation (`y = x^2`, `x = 3`). The definition operator is a preference
  (`:=` default, or `=` for users who prefer it).

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

Own library, designed around CAS + scripting. Names and behaviours are Viscas's
own (may use namespaces, e.g. `stats.mean`, `geo.circle`); the list below is the
capability scope, not an API to copy.

**Numeric & data:**
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

**CAS (every result supports step-by-step, §2.1):**
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

- Primary input of the app. One-line evaluation with typeset output and exact + approx results.
- History (persisted), up/down recall, `ans`, `ans2`.
- Tab completion for functions, nodes, units.
- Commands: `:help f`, `:add` (alias `:pin`: turn the last result into a board node), `:plot <input>` (evaluate and add), `:clear`, `:mode deg|rad|exact|approx`, `to <unit>`.
- Add to board: typing never pins anything by default — trying expressions must not litter the scene. Every entry that can become a node (drawings such as `y = x^2`, `x^2+y^2=4`, `(1, 2)`, `r = θ`, parametric; definitions; values) shows its result with a prominent "+ Add to board" button; Ctrl+Shift+Enter and `:add` do the same (undoable). Definitions stay console-local unless a node exists. Preferences ▸ Editing can opt into auto-adding drawings and/or definitions (both off by default).
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

- Linux first (GTK4, own CSS theme); Windows/macOS best effort.
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
| `viscas-app`    | GTK4 shell: area/editor system, theme, editors, viewport widget, math rendering | yes |

## 13. Milestones

| M  | Content |
|----|---------|
| M0 | Workspace, CI, app shell (done) |
| M0b | Pro theme + area/editor system (split/join/switch), workspaces, status bar, command palette skeleton |
| M1 | Parser + exact/numeric evaluator + console editor |
| M2 | Scene model + scene tree + inspector + expressions editor (text + typeset preview) + 2D plotting |
| M2b | Structural math editor widget |
| M3 | All 2D plot kinds, sliders, points, pan/zoom/trace |
| M4 | Function library (lists, stats, distributions, geometry, regression) + dependency graph editor (infinite-zoom capsule canvas) |
| M5 | CAS with step-by-step: simplify/expand/factor/solve/derivative/integrate/limit/series, matrices |
| M6 | Sheet editor + table nodes + CSV |
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
   structural editor (cursor inside fractions, exponents, roots).
   Toggle per field; both edit the same AST. Own math layout engine (Cairo/Pango).
5. Units: physical units and constants. No live currency rates (no network).
6. License: MIT OR Apache-2.0.
7. Identity (v0.2): Blender + Godot inspired pro tool, dark-first technical look,
   own function library. Signature features: dependency view, step-by-step CAS,
   command palette.

8. "A touch of Septabee" (node-based DAW by Lost Robot): borrowed as
   - **Infinite-zoom node canvas** for the Dependency Graph editor: zoom
     continuously from the whole document down into a single node; folders are
     **nested capsules** you fly into instead of expanding in a list.
   - **Map-like navigation**: smooth fly-to (double-click a node or pick it in
     the palette to glide the camera there), minimap overview.
   - **Patch-cable wiring**: references drawn as cables between node ports;
     dragging a cable re-points a reference.
   - **Personality**: a little playful microcopy in empty states, tips and
     errors, never at the cost of clarity. The base look stays Blender-precise.

9. Definition operator `:=` (configurable to `=` in Preferences ▸ Editing).
10. Command palette: Ctrl+P, or Space when no text field has focus.

## 15. Open questions

1. Keep libadwaita as a dependency for non-visual helpers, or drop it entirely
   in favour of plain GTK4 + own theme? (Current plan: drop.)

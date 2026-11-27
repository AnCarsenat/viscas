//! Data and source exports: CSV point tables, plain JSON points and
//! TikZ/PGF (`\draw plot coordinates {…}`).

use std::fmt::Write as _;

use viscas_plot::{LineStyle, Primitive, Vec2};
use viscas_scene::NodeId;

use super::drawing::{Drawing, hex, num, num_sig};
use super::settings::Unit;

/// Uniform `(t, x, y)` samples of one node, split where undefined.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub node: NodeId,
    pub name: String,
    /// Parameter name (`x`, `y`, `t`, `θ`).
    pub param: String,
    pub pieces: Vec<Vec<[f64; 3]>>,
}

/// A CSV text field: quoted when needed, and prefixed with `'` when it
/// starts like a spreadsheet formula (`=`, `+`, `-`, `@`).
pub fn csv_field(s: &str) -> String {
    let s = if s.starts_with(['=', '+', '-', '@']) {
        format!("'{s}")
    } else {
        s.to_owned()
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// Removes repeated points and zero-width back-and-forth spikes (the
/// bridges polygon clipping leaves along the box edge) from a ring.
pub fn clean_ring(points: &[Vec2]) -> Vec<Vec2> {
    let eps = 1e-12;
    let close = |a: Vec2, b: Vec2| (a.x - b.x).abs() <= eps && (a.y - b.y).abs() <= eps;
    let mut pts: Vec<Vec2> = Vec::with_capacity(points.len());
    for &p in points.iter().filter(|q| q.is_finite()) {
        if pts.last().is_none_or(|&l| !close(l, p)) {
            pts.push(p);
        }
    }
    while pts.len() > 1 && close(pts[0], pts[pts.len() - 1]) {
        pts.pop();
    }
    loop {
        let n = pts.len();
        if n < 3 {
            return pts;
        }
        let spike = (0..n).find(|&i| {
            let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            let (u, v) = (b - a, c - b);
            let cross = u.x * v.y - u.y * v.x;
            let scale = u.length() * v.length();
            close(a, c) || (cross.abs() <= 1e-9 * scale && u.x * v.x + u.y * v.y < 0.0)
        });
        match spike {
            Some(i) => {
                pts.remove(i);
                // Drop a duplicate created by the removal.
                let n = pts.len();
                if n > 1 {
                    let j = i % n;
                    let prev = (j + n - 1) % n;
                    if close(pts[prev], pts[j]) {
                        pts.remove(j);
                    }
                }
            }
            None => return pts,
        }
    }
}

/// Finite runs of each polyline, cleaned polygons and points of a layer.
fn pieces(prims: &[Primitive]) -> Vec<(&'static str, Vec<Vec2>)> {
    let mut out = Vec::new();
    for p in prims {
        match p {
            Primitive::Polyline { line, .. } => {
                let mut pts = line.points.clone();
                if line.closed
                    && let Some(&first) = pts.first()
                {
                    pts.push(first);
                }
                // An undefined sample ends a piece.
                for run in pts.split(|q| !q.is_finite()) {
                    if !run.is_empty() {
                        out.push(("curve", run.to_vec()));
                    }
                }
            }
            Primitive::Polygon(poly) => {
                let ring = clean_ring(&poly.points);
                if ring.len() >= 3 {
                    out.push(("fill", ring));
                }
            }
            Primitive::Points(pts) => {
                let pts: Vec<Vec2> = pts.iter().copied().filter(|q| q.is_finite()).collect();
                if !pts.is_empty() {
                    out.push(("points", pts));
                }
            }
        }
    }
    out
}

/// CSV of the drawing's sampled points: `node,piece,kind,x,y`. Each
/// piece between discontinuities gets its own number.
pub fn csv_points(d: &Drawing) -> String {
    let mut s = String::from("node,piece,kind,x,y\n");
    for layer in &d.layers {
        let name = csv_field(&layer.name);
        for (i, (kind, pts)) in pieces(&layer.primitives).into_iter().enumerate() {
            for q in pts {
                let _ = writeln!(s, "{name},{i},{kind},{},{}", q.x, q.y);
            }
        }
    }
    s
}

/// CSV of parameter tables: `node,piece,param,t,x,y`.
pub fn csv_table(tables: &[Table]) -> String {
    let mut s = String::from("node,piece,param,t,x,y\n");
    for t in tables {
        let name = csv_field(&t.name);
        let param = csv_field(&t.param);
        for (i, piece) in t.pieces.iter().enumerate() {
            for [tt, x, y] in piece {
                let _ = writeln!(s, "{name},{i},{param},{tt},{x},{y}");
            }
        }
    }
    s
}

/// Plain JSON: world box and, per node, its pieces as `[x, y]` arrays
/// (plus `t` tables when given).
pub fn json(d: &Drawing, tables: &[Table]) -> String {
    use serde_json::{Value, json};
    let pt = |q: &Vec2| json!([q.x, q.y]);
    let nodes: Vec<Value> = d
        .layers
        .iter()
        .map(|l| {
            let mut curves = Vec::new();
            let mut fills = Vec::new();
            let mut points = Vec::new();
            for (kind, pts) in pieces(&l.primitives) {
                let arr: Vec<Value> = pts.iter().map(pt).collect();
                match kind {
                    "curve" => curves.push(Value::Array(arr)),
                    "fill" => fills.push(Value::Array(arr)),
                    _ => points.extend(arr),
                }
            }
            let mut v = json!({
                "name": l.name,
                "kind": l.kind.as_str(),
                "color": hex(l.style.color),
                "curves": curves,
            });
            if !fills.is_empty() {
                v["fills"] = Value::Array(fills);
            }
            if !points.is_empty() {
                v["points"] = Value::Array(points);
            }
            if let Some(e) = &l.error {
                v["error"] = json!(e);
            }
            if l.truncated {
                v["truncated"] = json!(true);
            }
            if let Some(t) = tables.iter().find(|t| t.node == l.node) {
                v["param"] = json!(t.param);
                v["samples"] = json!(t.pieces);
            }
            v
        })
        .collect();
    let w = &d.world;
    let doc = json!({
        "generator": "Viscas",
        "world": {"x_min": w.x_min, "x_max": w.x_max, "y_min": w.y_min, "y_max": w.y_max},
        "nodes": nodes,
    });
    serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n"
}

/// Text for a TeX comment line.
fn tex_comment(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Text for a TeX label (math mode).
fn tex_label(s: &str) -> String {
    s.replace('−', "-")
}

/// TikZ picture of the drawing. Coordinates are page centimetres with y
/// up (origin at the box's lower-left corner), so numbers stay small and
/// TeX never overflows, whatever the world box; the world mapping is in a
/// comment.
pub fn tikz(d: &Drawing) -> String {
    let w = &d.world;
    let (page_w, page_h) = (
        Unit::Px.convert(d.size.0, Unit::Cm),
        Unit::Px.convert(d.size.1, Unit::Cm),
    );
    let (kx, ky) = (page_w / w.width(), page_h / w.height());
    let to_cm = |q: Vec2| ((q.x - w.x_min) * kx, (q.y - w.y_min) * ky);
    // 1/1000 mm resolution on the page.
    let c = |v: f64| num(v, 4);
    let coord = |q: &Vec2| {
        let (x, y) = to_cm(*q);
        (x.is_finite() && y.is_finite()).then(|| format!("({},{})", c(x), c(y)))
    };
    let mut s = String::new();
    let _ = writeln!(s, "% Generated by Viscas — needs \\usepackage{{tikz}}");
    let _ = writeln!(
        s,
        "% World box x ∈ [{}, {}], y ∈ [{}, {}]; 1 unit = {} cm (x), {} cm (y).",
        num_sig(w.x_min, 10),
        num_sig(w.x_max, 10),
        num_sig(w.y_min, 10),
        num_sig(w.y_max, 10),
        num_sig(kx, 8),
        num_sig(ky, 8)
    );
    for note in d.layers.iter().filter_map(|l| l.note()) {
        let _ = writeln!(s, "% {}", tex_comment(&note));
    }
    let _ = writeln!(s, "\\begin{{tikzpicture}}[x=1cm,y=1cm]");
    for (i, l) in d.layers.iter().enumerate() {
        let col = l.style.color;
        let _ = writeln!(
            s,
            "\\definecolor{{viscas{}}}{{HTML}}{{{:02X}{:02X}{:02X}}} % {}",
            i,
            col.r,
            col.g,
            col.b,
            tex_comment(&l.name)
        );
    }
    let corners = format!("(0,0) rectangle ({},{})", c(page_w), c(page_h));
    if let Some(bg) = d.background
        && bg != viscas_scene::Color::rgb(255, 255, 255)
    {
        let _ = writeln!(
            s,
            "\\fill[fill={{rgb,255:red,{};green,{};blue,{}}}] {corners};",
            bg.r, bg.g, bg.b
        );
    }
    let _ = writeln!(s, "\\begin{{scope}}");
    let _ = writeln!(s, "\\clip {corners};");
    let (tx, ty) = d.ticks();
    if d.grid {
        let mut grid = |color: &str, xs: &[f64], ys: &[f64]| {
            for &x in xs {
                let px = (x - w.x_min) * kx;
                let _ = writeln!(
                    s,
                    "\\draw[{color}] ({},0) -- ({},{});",
                    c(px),
                    c(px),
                    c(page_h)
                );
            }
            for &y in ys {
                let py = (y - w.y_min) * ky;
                let _ = writeln!(
                    s,
                    "\\draw[{color}] (0,{}) -- ({},{});",
                    c(py),
                    c(page_w),
                    c(py)
                );
            }
        };
        if d.minor_grid {
            grid("black!8, very thin", &tx.minor, &ty.minor);
        }
        grid("black!20, thin", &tx.major, &ty.major);
    }
    let (ox, oy) = to_cm(Vec2::new(0.0, 0.0));
    if d.axes {
        if (0.0..=page_h).contains(&oy) {
            let _ = writeln!(
                s,
                "\\draw[black!70] (0,{}) -- ({},{});",
                c(oy),
                c(page_w),
                c(oy)
            );
        }
        if (0.0..=page_w).contains(&ox) {
            let _ = writeln!(
                s,
                "\\draw[black!70] ({},0) -- ({},{});",
                c(ox),
                c(ox),
                c(page_h)
            );
        }
    }
    for (i, l) in d.layers.iter().enumerate() {
        let st = &l.style;
        let width_pt = Unit::Px.convert(d.stroke_width(st), Unit::Pt);
        for p in l.primitives.iter() {
            match p {
                Primitive::Polygon(poly) => {
                    let pts: Vec<String> =
                        clean_ring(&poly.points).iter().filter_map(coord).collect();
                    if pts.len() >= 3 {
                        let _ = writeln!(
                            s,
                            "\\fill[viscas{i}, fill opacity={}] plot coordinates {{{}}} -- cycle;",
                            num(st.fill_opacity * st.opacity, 3),
                            pts.join(" ")
                        );
                    }
                }
                Primitive::Polyline { line, style } => {
                    let dashed = match st.line_style {
                        viscas_scene::LineStyle::Dashed => ", dashed",
                        viscas_scene::LineStyle::Dotted => ", dotted",
                        viscas_scene::LineStyle::Solid if *style == LineStyle::Dashed => ", dashed",
                        viscas_scene::LineStyle::Solid => "",
                    };
                    let opacity = if st.opacity < 0.999 {
                        format!(", opacity={}", num(st.opacity, 3))
                    } else {
                        String::new()
                    };
                    let mapped: Vec<Option<String>> = line.points.iter().map(coord).collect();
                    let runs: Vec<&[Option<String>]> = mapped.split(Option::is_none).collect();
                    let whole = runs.len() == 1;
                    for run in runs {
                        if run.len() < 2 {
                            continue;
                        }
                        let pts: Vec<&str> = run.iter().flatten().map(String::as_str).collect();
                        let cycle = if line.closed && whole {
                            " -- cycle"
                        } else {
                            ""
                        };
                        let _ = writeln!(
                            s,
                            "\\draw[viscas{i}, line width={}pt, line cap=round, line join=round{dashed}{opacity}] plot coordinates {{{}}}{cycle};",
                            num(width_pt, 3),
                            pts.join(" ")
                        );
                    }
                }
                Primitive::Points(pts) => {
                    let r = Unit::Px.convert(d.point_radius(st), Unit::Pt);
                    for q in pts.iter().filter_map(coord) {
                        let _ =
                            writeln!(s, "\\fill[viscas{i}] {q} circle[radius={}pt];", num(r, 3));
                    }
                }
            }
        }
    }
    let _ = writeln!(s, "\\end{{scope}}");
    if d.labels {
        if (0.0..=page_h).contains(&oy) {
            for &x in tx.major.iter().filter(|x| **x != 0.0) {
                let px = (x - w.x_min) * kx;
                let _ = writeln!(
                    s,
                    "\\node[below, font=\\tiny] at ({},{}) {{${}$}};",
                    c(px),
                    c(oy),
                    tex_label(&crate::viewport::view::format_tick(x, tx.major_step))
                );
            }
        }
        if (0.0..=page_w).contains(&ox) {
            for &y in ty.major.iter().filter(|y| **y != 0.0) {
                let py = (y - w.y_min) * ky;
                let _ = writeln!(
                    s,
                    "\\node[left, font=\\tiny] at ({},{}) {{${}$}};",
                    c(ox),
                    c(py),
                    tex_label(&crate::viewport::view::format_tick(y, ty.major_step))
                );
            }
        }
    }
    let _ = writeln!(s, "\\end{{tikzpicture}}");
    s
}

#[cfg(test)]
mod tests {
    use viscas_plot::{Polygon, Polyline, Rect};

    use super::*;
    use crate::export::svg::tests::layer;

    fn drawing() -> Drawing {
        let mut d = Drawing::new(
            Rect::new(0.0, 0.0, 2.0, 1.0),
            (Unit::Cm.css_px() * 4.0, Unit::Cm.css_px() * 2.0),
        );
        d.layers.push(layer(vec![
            Primitive::Polyline {
                line: Polyline::open(vec![Vec2::new(0.0, 0.0), Vec2::new(0.5, 0.25)]),
                style: Default::default(),
            },
            Primitive::Polyline {
                line: Polyline::open(vec![
                    Vec2::new(1.0, 0.5),
                    Vec2::new(1.25, 0.75),
                    Vec2::new(1.5, f64::NAN),
                    Vec2::new(2.0, 1.0),
                ]),
                style: LineStyle::Dashed,
            },
        ]));
        d.layers[0].name = "f, g".to_owned();
        d
    }

    #[test]
    fn csv_lists_pieces_and_quotes_names() {
        let csv = csv_points(&drawing());
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "node,piece,kind,x,y");
        assert_eq!(lines[1], "\"f, g\",0,curve,0,0");
        assert_eq!(lines[2], "\"f, g\",0,curve,0.5,0.25");
        // The undefined sample splits the second polyline.
        assert_eq!(lines[3], "\"f, g\",1,curve,1,0.5");
        assert_eq!(lines[4], "\"f, g\",1,curve,1.25,0.75");
        assert_eq!(lines[5], "\"f, g\",2,curve,2,1");
        assert_eq!(lines.len(), 6);
        let t = csv_table(&[Table {
            node: NodeId(1),
            name: "y".into(),
            param: "x".into(),
            pieces: vec![vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]],
        }]);
        assert_eq!(t, "node,piece,param,t,x,y\ny,0,x,0,0,0\ny,0,x,1,1,1\n");
    }

    #[test]
    fn csv_fields_never_start_formulas() {
        assert_eq!(csv_field("=cmd|' /C calc'!A0"), "'=cmd|' /C calc'!A0");
        assert_eq!(csv_field("+1"), "'+1");
        assert_eq!(csv_field("-x, y"), "\"'-x, y\"");
        assert_eq!(csv_field("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(csv_field("Plot1"), "Plot1");
    }

    #[test]
    fn clip_bridges_are_removed() {
        // A square with a zero-width bridge along its bottom edge.
        let ring = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(3.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(0.0, 2.0),
        ];
        let clean = clean_ring(&ring);
        assert_eq!(
            clean,
            vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(2.0, 0.0),
                Vec2::new(2.0, 2.0),
                Vec2::new(0.0, 2.0)
            ]
        );
        assert_eq!(
            Polygon::new(clean).area(),
            4.0,
            "area unchanged by cleaning"
        );
    }

    #[test]
    fn tikz_draws_plot_coordinates_in_page_cm() {
        let mut d = drawing();
        d.grid = false;
        d.labels = false;
        let tex = tikz(&d);
        assert!(tex.contains("\\begin{tikzpicture}[x=1cm,y=1cm]"), "{tex}");
        assert!(tex.contains("1 unit = 2 cm (x), 2 cm (y)"), "{tex}");
        assert!(tex.contains("\\definecolor{viscas0}{HTML}{2080FF}"));
        assert!(tex.contains("plot coordinates {(0,0) (1,0.5)};"), "{tex}");
        assert!(
            tex.contains(", dashed] plot coordinates {(2,1) (2.5,1.5)};"),
            "{tex}"
        );
        assert!(tex.contains("\\clip (0,0) rectangle (4,2);"));
        assert!(tex.trim_end().ends_with("\\end{tikzpicture}"));
        assert_eq!(tex.matches('{').count(), tex.matches('}').count());
    }

    #[test]
    fn tikz_stays_small_for_huge_and_tiny_worlds() {
        for world in [
            Rect::new(-3e9, -2e9, 3e9, 2e9),
            Rect::new(1.0, 1.0, 1.0 + 3e-12, 1.0 + 2e-12),
        ] {
            let mut d = Drawing::new(world, (600.0, 400.0));
            let (a, b) = (world.center(), Vec2::new(world.x_max, world.y_max));
            d.layers.push(layer(vec![Primitive::Polyline {
                line: Polyline::open(vec![a, b]),
                style: Default::default(),
            }]));
            let tex = tikz(&d);
            assert!(!tex.contains("e-") && !tex.contains("e+") && !tex.contains("NaN"));
            // Page coordinates: the curve ends at the top-right corner.
            let w = num(Unit::Px.convert(600.0, Unit::Cm), 4);
            let h = num(Unit::Px.convert(400.0, Unit::Cm), 4);
            assert!(tex.contains(&format!("({w},{h})}}")), "{tex}");
            assert!(!tex.contains("1 unit = 0 cm"));
        }
    }

    #[test]
    fn json_is_parseable_points() {
        let mut d = drawing();
        d.layers[0].truncated = true;
        let text = json(&d, &[]);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["generator"], "Viscas");
        let curves = v["nodes"][0]["curves"].as_array().unwrap();
        assert_eq!(curves.len(), 3);
        assert_eq!(curves[0][1], serde_json::json!([0.5, 0.25]));
        assert_eq!(v["world"]["x_max"], 2.0);
        assert_eq!(v["nodes"][0]["truncated"], true);
    }
}

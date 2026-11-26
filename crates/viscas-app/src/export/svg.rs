//! SVG writer: real paths for curves and fills, stroke widths and dashes,
//! colors, optional grid/axes/labels/legend, text as text.

use std::fmt::Write as _;

use viscas_plot::{Primitive, Vec2};

use super::drawing::{Drawing, alpha, decimals_for, hex, num, num_sig};
use super::settings::Unit;

/// SVG output options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvgOptions {
    /// Unit of the `width`/`height` attributes (the viewBox is in CSS px).
    pub unit: Unit,
    /// Paths in world coordinates under a viewBox transform (strokes keep
    /// their page width through `vector-effect`).
    pub world_coords: bool,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            unit: Unit::Px,
            world_coords: false,
        }
    }
}

/// Escapes text for XML content and attribute values.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// Text safe inside an XML comment (no `--`).
fn comment(s: &str) -> String {
    let mut t = s.replace("--", "- -");
    if t.ends_with('-') {
        t.push(' ');
    }
    t
}

/// Page-coordinate decimals (1/1000 px).
const PAGE_DEC: usize = 3;

/// Writes `d` as a standalone SVG document.
pub fn write(d: &Drawing, opts: &SvgOptions) -> String {
    let (w, h) = d.size;
    let mut s = String::new();
    let size = |v: f64| num(Unit::Px.convert(v, opts.unit), 4);
    let _ = writeln!(s, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" version="1.1" width="{}{u}" height="{}{u}" viewBox="0 0 {} {}">"#,
        size(w),
        size(h),
        num(w, PAGE_DEC),
        num(h, PAGE_DEC),
        u = opts.unit.suffix(),
    );
    let wr = &d.world;
    let _ = writeln!(
        s,
        r#"<title>Viscas export</title><desc>x ∈ [{}, {}], y ∈ [{}, {}]</desc>"#,
        wr.x_min, wr.x_max, wr.y_min, wr.y_max
    );
    if let Some(bg) = d.background {
        let _ = writeln!(
            s,
            r#"<rect id="background" x="0" y="0" width="{}" height="{}" fill="{}"{}/>"#,
            num(w, PAGE_DEC),
            num(h, PAGE_DEC),
            hex(bg),
            opacity_attr("fill-opacity", alpha(bg, 1.0)),
        );
    }
    let _ = writeln!(
        s,
        r#"<clipPath id="page"><rect x="0" y="0" width="{}" height="{}"/></clipPath>"#,
        num(w, PAGE_DEC),
        num(h, PAGE_DEC)
    );
    write_grid(&mut s, d);
    write_plots(&mut s, d, opts);
    write_labels(&mut s, d);
    write_legend(&mut s, d);
    s.push_str("</svg>\n");
    s
}

fn opacity_attr(name: &str, a: f64) -> String {
    if a >= 0.999 {
        String::new()
    } else {
        format!(r#" {name}="{}""#, num(a, 3))
    }
}

fn write_grid(s: &mut String, d: &Drawing) {
    let (w, h) = d.size;
    let (tx, ty) = d.ticks();
    let line_set = |s: &mut String, id: &str, color, xs: &[f64], ys: &[f64]| {
        if xs.is_empty() && ys.is_empty() {
            return;
        }
        let mut path = String::new();
        for &x in xs {
            let px = d.to_page(Vec2::new(x, 0.0)).0;
            let _ = write!(path, "M{} 0V{}", num(px, PAGE_DEC), num(h, PAGE_DEC));
        }
        for &y in ys {
            let py = d.to_page(Vec2::new(0.0, y)).1;
            let _ = write!(path, "M0 {}H{}", num(py, PAGE_DEC), num(w, PAGE_DEC));
        }
        let _ = writeln!(
            s,
            r#"<path id="{id}" d="{path}" fill="none" stroke="{}" stroke-width="{}"{}/>"#,
            hex(color),
            num(d.grid_width, 3),
            opacity_attr("stroke-opacity", alpha(color, 1.0)),
        );
    };
    if d.grid {
        if d.minor_grid {
            line_set(s, "grid-minor", d.colors.minor, &tx.minor, &ty.minor);
        }
        let xs: Vec<f64> = tx.major.iter().copied().filter(|x| *x != 0.0).collect();
        let ys: Vec<f64> = ty.major.iter().copied().filter(|y| *y != 0.0).collect();
        line_set(s, "grid-major", d.colors.major, &xs, &ys);
    }
    if d.axes {
        let (ox, oy) = d.origin();
        let mut path = String::new();
        if (0.0..=w).contains(&ox) {
            let _ = write!(path, "M{} 0V{}", num(ox, PAGE_DEC), num(h, PAGE_DEC));
        }
        if (0.0..=h).contains(&oy) {
            let _ = write!(path, "M0 {}H{}", num(oy, PAGE_DEC), num(w, PAGE_DEC));
        }
        if !path.is_empty() {
            let _ = writeln!(
                s,
                r#"<path id="axes" d="{path}" fill="none" stroke="{}" stroke-width="{}"/>"#,
                hex(d.colors.axis),
                num(d.grid_width, 3)
            );
        }
    }
}

/// Path data of a point list (`M x y L x y …`, `Z` when closed). A
/// non-finite point starts a new subpath, so the data never holds NaN or
/// infinities; subpaths of a single point are dropped.
fn path_data(pts: &[(f64, f64)], dec: usize, closed: bool) -> String {
    let mut out = String::with_capacity(pts.len() * 16);
    let mut run: Vec<(f64, f64)> = Vec::new();
    let mut runs = 0;
    let mut flush = |run: &mut Vec<(f64, f64)>, out: &mut String| {
        if run.len() >= 2 {
            for (i, (x, y)) in run.iter().enumerate() {
                out.push(if i == 0 { 'M' } else { 'L' });
                let _ = write!(out, "{} {}", num(*x, dec), num(*y, dec));
            }
            runs += 1;
        }
        run.clear();
    };
    for &(x, y) in pts {
        if x.is_finite() && y.is_finite() {
            run.push((x, y));
        } else {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    if closed && runs == 1 {
        out.push('Z');
    }
    out
}

/// World point → SVG coordinates.
type PointMap<'a> = dyn Fn(Vec2) -> (f64, f64) + 'a;

fn write_plots(s: &mut String, d: &Drawing, opts: &SvgOptions) {
    let (dec, map): (usize, Box<PointMap>) = if opts.world_coords {
        let upp = (1.0 / d.sx()).min(1.0 / d.sy());
        (decimals_for(upp), Box::new(|p: Vec2| (p.x, p.y)))
    } else {
        (PAGE_DEC, Box::new(|p: Vec2| d.to_page(p)))
    };
    // The clip lives on an untransformed group so it stays in page units;
    // the world transform goes on the inner group.
    s.push_str("<g id=\"plots\" clip-path=\"url(#page)\">\n");
    if opts.world_coords {
        // Strokes and dashes use `vector-effect="non-scaling-stroke"`: per
        // SVG 2 both are then measured in page units. Renderers without
        // vector-effect support draw them scaled by the world transform.
        let (sx, sy) = (d.sx(), d.sy());
        let _ = writeln!(
            s,
            r#"<g id="world" transform="matrix({} 0 0 {} {} {})">"#,
            num_sig(sx, 12),
            num_sig(-sy, 12),
            num_sig(-d.world.x_min * sx, 12),
            num_sig(d.world.y_max * sy, 12)
        );
    } else {
        s.push_str("<g id=\"page-coords\">\n");
    }
    let nse = if opts.world_coords {
        r#" vector-effect="non-scaling-stroke""#
    } else {
        ""
    };
    for (i, layer) in d.layers.iter().enumerate() {
        let st = &layer.style;
        if let Some(note) = layer.note() {
            let _ = writeln!(s, "<!-- {} -->", comment(&note));
        }
        let _ = writeln!(
            s,
            r#"<g id="layer-{i}" data-name="{}" data-kind="{}">"#,
            escape(&layer.name),
            layer.kind
        );
        // Fills first (one path: cells never overlap, so nonzero fills each once).
        let mut fill = String::new();
        for p in layer.primitives.iter() {
            if let Primitive::Polygon(poly) = p {
                let pts: Vec<(f64, f64)> = poly
                    .points
                    .iter()
                    .filter(|q| q.is_finite())
                    .map(|q| map(*q))
                    .collect();
                if pts.len() >= 3 && pts.iter().all(|(x, y)| x.is_finite() && y.is_finite()) {
                    fill.push_str(&path_data(&pts, dec, true));
                }
            }
        }
        if !fill.is_empty() {
            let _ = writeln!(
                s,
                r#"<path class="fill" d="{fill}" fill="{}" fill-opacity="{}" stroke="none"/>"#,
                hex(st.color),
                num(alpha(st.color, st.fill_opacity * st.opacity), 3)
            );
        }
        for p in layer.primitives.iter() {
            if let Primitive::Polyline { line, style } = p {
                // Undefined samples split the path (path_data).
                let pts: Vec<(f64, f64)> = line.points.iter().map(|q| map(*q)).collect();
                let data = path_data(&pts, dec, line.closed);
                if data.is_empty() {
                    continue;
                }
                let dash = d.dash(st, *style);
                let dash_attr = if dash.is_empty() {
                    String::new()
                } else {
                    let list: Vec<String> = dash.iter().map(|v| num(*v, 3)).collect();
                    format!(r#" stroke-dasharray="{}""#, list.join(" "))
                };
                let _ = writeln!(
                    s,
                    r#"<path class="curve" d="{}" fill="none" stroke="{}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round"{}{}{nse}/>"#,
                    data,
                    hex(st.color),
                    num(d.stroke_width(st), 3),
                    opacity_attr("stroke-opacity", alpha(st.color, st.opacity)),
                    dash_attr,
                );
            }
        }
        s.push_str("</g>\n");
    }
    s.push_str("</g>\n</g>\n");
    // Points in page coordinates (circles stay round in world mode).
    for layer in &d.layers {
        let st = &layer.style;
        let r = d.point_radius(st);
        let mut dots = String::new();
        for p in layer.primitives.iter() {
            if let Primitive::Points(pts) = p {
                for q in pts.iter().filter(|q| q.is_finite()) {
                    let (x, y) = d.to_page(*q);
                    if !(x.is_finite() && y.is_finite()) {
                        continue;
                    }
                    let _ = write!(
                        dots,
                        r#"<circle cx="{}" cy="{}" r="{}"/>"#,
                        num(x, PAGE_DEC),
                        num(y, PAGE_DEC),
                        num(r, 3)
                    );
                }
            }
        }
        if !dots.is_empty() {
            let _ = writeln!(
                s,
                r#"<g class="points" data-name="{}" fill="{}"{} clip-path="url(#page)">{dots}</g>"#,
                escape(&layer.name),
                hex(st.color),
                opacity_attr("fill-opacity", alpha(st.color, st.opacity)),
            );
        }
    }
}

fn write_labels(s: &mut String, d: &Drawing) {
    let labels = d.tick_labels();
    if labels.is_empty() {
        return;
    }
    let _ = writeln!(
        s,
        r#"<g id="tick-labels" font-family="{}" font-size="{}" fill="{}">"#,
        escape(&d.font_family),
        num(d.font_px, 2),
        hex(d.colors.label)
    );
    for l in labels {
        let _ = writeln!(
            s,
            r#"<text x="{}" y="{}" text-anchor="{}">{}</text>"#,
            num(l.x, 2),
            num(l.y, 2),
            l.anchor.svg(),
            escape(&l.text)
        );
    }
    s.push_str("</g>\n");
}

fn write_legend(s: &mut String, d: &Drawing) {
    let Some((x, y, w, row)) = d.legend_box() else {
        return;
    };
    let entries = d.legend_entries();
    let bg = d
        .background
        .unwrap_or(viscas_scene::Color::rgb(255, 255, 255));
    let _ = writeln!(
        s,
        r#"<g id="legend" font-family="{}" font-size="{}">"#,
        escape(&d.font_family),
        num(d.font_px, 2)
    );
    let _ = writeln!(
        s,
        r#"<rect x="{}" y="{}" width="{}" height="{}" rx="3" fill="{}" fill-opacity="0.85" stroke="{}" stroke-width="1"/>"#,
        num(x, 2),
        num(y, 2),
        num(w, 2),
        num(row * entries.len() as f64 + 8.0, 2),
        hex(bg),
        hex(d.colors.major)
    );
    for (i, (color, dash, text)) in entries.iter().enumerate() {
        let cy = y + 4.0 + row * (i as f64 + 0.5);
        let dash_attr = if dash.is_empty() {
            String::new()
        } else {
            let list: Vec<String> = dash.iter().map(|v| num(*v, 3)).collect();
            format!(r#" stroke-dasharray="{}""#, list.join(" "))
        };
        let _ = writeln!(
            s,
            r#"<path d="M{} {}H{}" stroke="{}" stroke-width="{}" stroke-linecap="round"{dash_attr}/>"#,
            num(x + 8.0, 2),
            num(cy, 2),
            num(x + 30.0, 2),
            hex(*color),
            num(2.0 * d.line_scale, 2)
        );
        let _ = writeln!(
            s,
            r#"<text x="{}" y="{}" fill="{}">{}</text>"#,
            num(x + 38.0, 2),
            num(cy + d.font_px * 0.35, 2),
            hex(d.colors.label),
            escape(text)
        );
    }
    s.push_str("</g>\n");
}

#[cfg(test)]
pub mod tests {
    use std::sync::Arc;

    use viscas_engine::{PlotKind, PlotStyle};
    use viscas_plot::{Polygon, Polyline, Rect};
    use viscas_scene::Color;

    use super::*;
    use crate::export::drawing::DrawLayer;

    /// Minimal XML well-formedness check: balanced tags, quoted
    /// attributes, escaped text. Returns the element names in order.
    pub fn check_xml(doc: &str) -> Result<Vec<String>, String> {
        let mut stack: Vec<String> = Vec::new();
        let mut names = Vec::new();
        let b = doc.as_bytes();
        let mut i = 0;
        let mut roots = 0;
        while i < b.len() {
            if doc[i..].starts_with("<!--") {
                let end = doc[i..].find("-->").ok_or("unterminated comment")? + i;
                if doc[i + 4..end].contains("--") {
                    return Err("-- inside a comment".into());
                }
                i = end + 3;
                continue;
            }
            if b[i] == b'<' {
                let end = doc[i..].find('>').ok_or("unterminated tag")? + i;
                let tag = &doc[i + 1..end];
                if let Some(rest) = tag.strip_prefix('?') {
                    if !rest.ends_with('?') {
                        return Err("bad declaration".into());
                    }
                } else if let Some(name) = tag.strip_prefix('/') {
                    let open = stack.pop().ok_or("close without open")?;
                    if open != name.trim() {
                        return Err(format!("</{name}> closes <{open}>"));
                    }
                } else {
                    let self_closing = tag.ends_with('/');
                    let body = tag.trim_end_matches('/');
                    let name: String = body.chars().take_while(|c| !c.is_whitespace()).collect();
                    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '-') {
                        return Err(format!("bad element name {name:?}"));
                    }
                    check_attrs(&body[name.len()..])?;
                    if stack.is_empty() {
                        roots += 1;
                    }
                    names.push(name.clone());
                    if !self_closing {
                        stack.push(name);
                    }
                }
                i = end + 1;
            } else {
                let end = doc[i..].find('<').map_or(b.len(), |e| e + i);
                let text = &doc[i..end];
                if text.contains('>') {
                    return Err("unescaped > in text".into());
                }
                check_entities(text)?;
                if stack.is_empty() && !text.trim().is_empty() {
                    return Err("text outside the root".into());
                }
                i = end;
            }
        }
        if !stack.is_empty() {
            return Err(format!("unclosed {stack:?}"));
        }
        if roots != 1 {
            return Err(format!("{roots} root elements"));
        }
        Ok(names)
    }

    fn check_entities(text: &str) -> Result<(), String> {
        for (i, _) in text.match_indices('&') {
            let rest = &text[i..];
            let ok = ["&amp;", "&lt;", "&gt;", "&quot;", "&apos;"]
                .iter()
                .any(|e| rest.starts_with(e));
            if !ok {
                return Err(format!("bad entity near {:?}", &rest[..rest.len().min(8)]));
            }
        }
        Ok(())
    }

    fn check_attrs(mut s: &str) -> Result<(), String> {
        let mut seen = Vec::new();
        loop {
            s = s.trim_start();
            if s.is_empty() {
                return Ok(());
            }
            let eq = s
                .find('=')
                .ok_or(format!("attribute without value: {s:?}"))?;
            let name = s[..eq].trim();
            if seen.contains(&name) {
                return Err(format!("duplicate attribute {name}"));
            }
            seen.push(name);
            let rest = &s[eq + 1..];
            if !rest.starts_with('"') {
                return Err(format!("unquoted attribute {name}"));
            }
            let close = rest[1..].find('"').ok_or("unterminated attribute")? + 1;
            let value = &rest[1..close];
            if value.contains('<') {
                return Err("< in attribute".into());
            }
            check_entities(value)?;
            s = &rest[close + 1..];
        }
    }

    /// Every `d` attribute of `<path class="curve">` elements, parsed.
    pub fn curve_paths(svg: &str) -> Vec<Vec<(f64, f64)>> {
        svg.lines()
            .filter(|l| l.contains(r#"class="curve""#))
            .map(|l| {
                let d = l.split(r#" d=""#).nth(1).unwrap();
                let d = &d[..d.find('"').unwrap()];
                d.trim_end_matches('Z')
                    .split(['M', 'L'])
                    .filter(|p| !p.is_empty())
                    .map(|p| {
                        let mut it = p.split(' ').map(|v| v.parse::<f64>().unwrap());
                        (it.next().unwrap(), it.next().unwrap())
                    })
                    .collect()
            })
            .collect()
    }

    pub fn layer(prims: Vec<Primitive>) -> DrawLayer {
        DrawLayer {
            node: viscas_scene::NodeId(1),
            truncated: false,
            error: None,
            name: "f <1> & \"g\"".to_owned(),
            kind: PlotKind::ExplicitY,
            style: PlotStyle::from_style(&Default::default(), Color::rgb(0x20, 0x80, 0xff)),
            primitives: Arc::new(prims),
        }
    }

    fn identity() -> Drawing {
        let mut d = Drawing::new(Rect::new(-0.5, -0.5, 1.5, 1.5), (200.0, 200.0));
        d.legend = true;
        let pts = (0..=10).map(|i| Vec2::new(i as f64 / 10.0, i as f64 / 10.0));
        d.layers.push(layer(vec![Primitive::Polyline {
            line: Polyline::open(pts.collect()),
            style: Default::default(),
        }]));
        d
    }

    #[test]
    fn svg_is_well_formed_with_all_parts() {
        let mut d = identity();
        d.layers.push(layer(vec![
            Primitive::Polygon(Polygon::new(vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
            ])),
            Primitive::Points(vec![Vec2::new(0.5, 0.5)]),
        ]));
        for world in [false, true] {
            let svg = write(
                &d,
                &SvgOptions {
                    unit: Unit::Mm,
                    world_coords: world,
                },
            );
            let names = check_xml(&svg).unwrap_or_else(|e| panic!("{e}\n{svg}"));
            assert_eq!(names[0], "svg");
            for n in ["path", "circle", "text", "rect", "clipPath", "g"] {
                assert!(names.iter().any(|x| x == n), "missing <{n}>");
            }
            assert!(svg.contains("&lt;1&gt; &amp; &quot;g&quot;"));
            assert!(svg.contains(r#"width="52.9167mm""#));
        }
    }

    #[test]
    fn identity_is_a_straight_path_matching_the_samples() {
        let d = identity();
        let svg = write(&d, &SvgOptions::default());
        let paths = curve_paths(&svg);
        assert_eq!(paths.len(), 1);
        let p = &paths[0];
        assert_eq!(p.len(), 11);
        // World (0,0) → page (50,150); (1,1) → (150,50); all on x + y = 200.
        assert_eq!(p[0], (50.0, 150.0));
        assert_eq!(p[10], (150.0, 50.0));
        assert!(p.iter().all(|(x, y)| (x + y - 200.0).abs() < 1e-9));
        // World coordinates: path data equals the samples.
        let svg = write(
            &d,
            &SvgOptions {
                world_coords: true,
                ..Default::default()
            },
        );
        let p = &curve_paths(&svg)[0];
        for (i, (x, y)) in p.iter().enumerate() {
            assert!((x - i as f64 / 10.0).abs() < 1e-9 && (y - x).abs() < 1e-9);
        }
        assert!(svg.contains("non-scaling-stroke"));
        assert!(svg.contains(r#"transform="matrix(100 0 0 -100 50 150)""#));
    }

    #[test]
    fn undefined_samples_split_paths_without_nan() {
        let mut d = identity();
        d.layers[0].truncated = true;
        d.layers[0].name = "a--b".into();
        d.layers.push(layer(vec![Primitive::Polyline {
            line: Polyline::open(vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(0.1, 0.1),
                Vec2::new(0.2, f64::NAN),
                Vec2::new(0.3, 0.3),
                Vec2::new(f64::INFINITY, 0.4),
                Vec2::new(0.5, 0.5),
                Vec2::new(0.6, 0.6),
            ]),
            style: Default::default(),
        }]));
        for world in [false, true] {
            let svg = write(
                &d,
                &SvgOptions {
                    world_coords: world,
                    ..Default::default()
                },
            );
            check_xml(&svg).unwrap();
            assert!(!svg.contains("NaN") && !svg.contains("inf"));
            assert!(svg.contains("<!-- a- -b: sampling budget reached"));
            let line = svg
                .lines()
                .filter(|l| l.contains(r#"class="curve""#))
                .nth(1)
                .unwrap();
            // Two runs of ≥ 2 points; the lone (0.3, 0.3) is dropped.
            assert_eq!(line.matches('M').count(), 2, "{line}");
        }
    }

    #[test]
    fn world_mode_clips_in_page_units() {
        let svg = write(
            &identity(),
            &SvgOptions {
                world_coords: true,
                ..Default::default()
            },
        );
        let plots = svg.lines().find(|l| l.contains(r#"id="plots""#)).unwrap();
        assert!(plots.contains("clip-path") && !plots.contains("transform"));
        let world = svg.lines().find(|l| l.contains(r#"id="world""#)).unwrap();
        assert!(world.contains("transform") && !world.contains("clip-path"));
    }

    #[test]
    fn styles_become_attributes() {
        let mut d = identity();
        d.background = None;
        d.grid = false;
        d.axes = false;
        d.labels = false;
        d.legend = false;
        d.line_scale = 2.0;
        d.layers[0].style.line_style = viscas_scene::LineStyle::Dashed;
        d.layers[0].style.opacity = 0.5;
        let svg = write(&d, &SvgOptions::default());
        assert!(!svg.contains(r#"id="background""#));
        assert!(!svg.contains("grid-major") && !svg.contains("<text"));
        assert!(svg.contains(r#"stroke-dasharray="14 10""#));
        assert!(svg.contains(r#"stroke-opacity="0.5""#));
        assert!(svg.contains(r##"stroke="#2080ff""##));
        let width = d.layers[0].style.line_width * 2.0;
        assert!(svg.contains(&format!(r#"stroke-width="{}""#, num(width, 3))));
    }
}

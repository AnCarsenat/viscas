//! Turning an export request into bytes: [`prepare`] snapshots the scene
//! on the UI thread, [`produce`] re-samples and writes on a worker thread.

use std::collections::HashMap;

use viscas_engine::export::{
    ExportItem, ExportJob, ExportRange, param_name, param_range, sample_table,
};
use viscas_engine::{Engine, PlotShape};
use viscas_plot::Rect;
use viscas_scene::{Color, NodeId};

use super::drawing::{DrawLayer, Drawing, GridColors};
use super::raster::{self, Vector};
use super::settings::{Background, ExportSettings, Format};
use super::svg::{self, SvgOptions};
use super::text::{self, Table};
use crate::prefs::Preferences;

/// What to export and how.
#[derive(Clone, Debug)]
pub struct Request {
    pub settings: ExportSettings,
    /// World box of the page.
    pub world: Rect,
    /// Only these nodes (and their descendants); `None` = every visible plot.
    pub nodes: Option<Vec<NodeId>>,
    pub range: ExportRange,
    /// Clip geometry to this box (besides the page).
    pub clip: Option<Rect>,
}

/// A request snapshotted from the scene, ready for a worker thread.
pub struct Prepared {
    job: ExportJob,
    names: HashMap<NodeId, String>,
    base: Drawing,
    settings: ExportSettings,
    range: ExportRange,
}

impl Prepared {
    pub fn cancel_token(&self) -> viscas_plot::CancelToken {
        self.job.cancel_token()
    }

    pub fn is_empty(&self) -> bool {
        self.job.items.is_empty()
    }
}

fn theme_color(c: crate::theme::model::Color) -> Color {
    Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

/// Is `id` or one of its ancestors in `set`?
fn covered(engine: &Engine, id: NodeId, set: &[NodeId]) -> bool {
    let mut cur = Some(id);
    while let Some(c) = cur {
        if set.contains(&c) {
            return true;
        }
        cur = engine.scene().get(c).and_then(|n| n.parent);
    }
    false
}

/// The base drawing (everything but the layers) for `settings`.
pub fn base_drawing(prefs: &Preferences, settings: &ExportSettings, world: Rect) -> Drawing {
    let theme = prefs.effective_theme();
    let vp = &theme.viewport;
    let mut d = Drawing::new(world, settings.page_px());
    let (background, colors) = match settings.background {
        Background::Theme => (
            Some(theme_color(vp.background)),
            GridColors {
                minor: theme_color(vp.grid_minor),
                major: theme_color(vp.grid_major),
                axis: theme_color(vp.axis),
                label: theme_color(vp.label),
            },
        ),
        Background::White => (Some(Color::rgb(255, 255, 255)), GridColors::default()),
        Background::Transparent => (None, GridColors::default()),
    };
    d.background = background;
    d.colors = colors;
    d.grid = settings.grid;
    d.minor_grid = prefs.viewport.minor_grid;
    d.axes = settings.axes;
    d.labels = settings.labels;
    d.legend = settings.legend;
    d.grid_width = prefs.viewport.line_width;
    d.line_scale = settings.line_scale;
    let mono = prefs.interface.mono_font.trim();
    d.font_family = if mono.is_empty() {
        "DejaVu Sans Mono, monospace".to_owned()
    } else {
        format!("{mono}, DejaVu Sans Mono, monospace")
    };
    d
}

/// Snapshots `req` from the engine (UI thread).
pub fn prepare(engine: &Engine, prefs: &Preferences, req: &Request) -> Result<Prepared, String> {
    let mut settings = req.settings.clone();
    settings.validate();
    if !(req.world.width() > 0.0 && req.world.height() > 0.0) {
        return Err("the export box is empty".to_owned());
    }
    let mut names = HashMap::new();
    let items: Vec<ExportItem> = engine
        .plot_items()
        .into_iter()
        .filter(|it| {
            req.nodes
                .as_ref()
                .is_none_or(|set| covered(engine, it.node, set))
        })
        .map(|it| {
            names.insert(
                it.node,
                crate::editors::scene_tree::model::display_name(engine.scene(), it.node),
            );
            ExportItem::new(it)
                .with_range(req.range)
                .with_clip(req.clip)
        })
        .collect();
    // Draw order as on screen: fills, curves, points.
    let mut items = items;
    items.sort_by_key(|it| it.item.plot.kind.draw_rank());
    let job = ExportJob::new(items, req.world, settings.page_px(), settings.quality);
    Ok(Prepared {
        job,
        names,
        base: base_drawing(prefs, &settings, req.world),
        settings,
        range: req.range,
    })
}

/// Parameter tables of the job's curves.
fn tables(p: &Prepared) -> Vec<Table> {
    p.job
        .items
        .iter()
        .filter_map(|it| {
            let shape: &PlotShape = &it.item.plot.shape;
            let param = param_name(shape)?;
            let range = match p.range {
                ExportRange::Param(a, b) => (a.min(b), a.max(b)),
                ExportRange::X(a, b) if matches!(shape, PlotShape::ExplicitY(_)) => {
                    (a.min(b), a.max(b))
                }
                _ => param_range(shape, &p.job.world)?,
            };
            let pieces = sample_table(shape, range, p.settings.csv_samples as usize)?;
            Some(Table {
                name: p.names.get(&it.item.node).cloned().unwrap_or_default(),
                param: param.to_owned(),
                pieces,
            })
        })
        .collect()
}

/// Renders `p` (worker thread); `progress(done, total)` follows sampling.
pub fn produce(p: Prepared, progress: impl FnMut(usize, usize)) -> Result<Vec<u8>, String> {
    let layers = p
        .job
        .run(progress)
        .map_err(|_| "export cancelled".to_owned())?;
    let mut d = p.base.clone();
    d.layers = layers
        .into_iter()
        .filter(|l| l.error.is_none())
        .map(|l| DrawLayer {
            name: p.names.get(&l.node_id).cloned().unwrap_or_default(),
            kind: l.kind,
            style: l.style,
            primitives: l.primitives,
        })
        .collect();
    let s = &p.settings;
    let bytes = match s.format {
        Format::Svg => svg::write(
            &d,
            &SvgOptions {
                unit: s.unit,
                world_coords: s.svg_world,
            },
        )
        .into_bytes(),
        Format::Png => raster::png(&d, s.raster_px())?,
        Format::Pdf => raster::vector(&d, Vector::Pdf)?,
        Format::Ps => raster::vector(&d, Vector::Ps)?,
        Format::Eps => raster::vector(&d, Vector::Eps)?,
        Format::Tikz => text::tikz(&d).into_bytes(),
        Format::Csv if s.csv_param => text::csv_table(&tables(&p)).into_bytes(),
        Format::Csv => text::csv_points(&d).into_bytes(),
        Format::Json => {
            let t = if s.csv_param { tables(&p) } else { Vec::new() };
            text::json(&d, &t).into_bytes()
        }
    };
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::settings::Target;
    use crate::export::svg::tests::{check_xml, curve_paths};

    fn engine_with(lines: &[&str]) -> (Engine, Vec<NodeId>) {
        let mut e = Engine::with_workers(1);
        let ids = lines.iter().map(|l| e.add_line(None, l).unwrap()).collect();
        e.update();
        (e, ids)
    }

    fn request(format: Format, world: Rect) -> Request {
        Request {
            settings: ExportSettings {
                format,
                target: Target::View,
                width: 200.0,
                height: 200.0,
                quality: 1.0,
                ..Default::default()
            },
            world,
            nodes: None,
            range: ExportRange::Full,
            clip: None,
        }
    }

    #[test]
    fn exports_a_scene_curve_part_as_svg() {
        let (e, ids) = engine_with(&["y = x", "y = x^2"]);
        let mut req = request(Format::Svg, Rect::new(-1.0, -1.0, 3.0, 3.0));
        req.nodes = Some(vec![ids[0]]);
        req.range = ExportRange::Param(0.0, 1.0);
        req.settings.svg_world = true;
        let p = prepare(&e, &Preferences::default(), &req).unwrap();
        let svg = String::from_utf8(produce(p, |_, _| {}).unwrap()).unwrap();
        check_xml(&svg).unwrap();
        let paths = curve_paths(&svg);
        assert_eq!(paths.len(), 1, "only the selected node");
        let p = &paths[0];
        assert_eq!(p[0], (0.0, 0.0));
        assert_eq!(*p.last().unwrap(), (1.0, 1.0));
        assert!(p.iter().all(|(x, y)| (x - y).abs() < 1e-9));
    }

    #[test]
    fn every_format_produces_output() {
        let (e, _) = engine_with(&["y = sin(x)", "y < x", "(1, 2)"]);
        for f in Format::ALL {
            let mut req = request(f, Rect::new(-4.0, -4.0, 4.0, 4.0));
            req.settings.csv_param = f == Format::Json;
            let p = prepare(&e, &Preferences::default(), &req).unwrap();
            let out = produce(p, |_, _| {}).unwrap();
            assert!(out.len() > 50, "{f:?} output too small");
        }
    }

    #[test]
    fn csv_parameter_table_uses_the_range() {
        let (e, ids) = engine_with(&["y = 2 x"]);
        let mut req = request(Format::Csv, Rect::new(-4.0, -4.0, 4.0, 4.0));
        req.nodes = Some(ids);
        req.range = ExportRange::Param(0.0, 1.0);
        req.settings.csv_param = true;
        req.settings.csv_samples = 3;
        let p = prepare(&e, &Preferences::default(), &req).unwrap();
        let csv = String::from_utf8(produce(p, |_, _| {}).unwrap()).unwrap();
        let rows: Vec<&str> = csv.lines().skip(1).collect();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].ends_with(",0,x,0,0,0"), "{}", rows[0]);
        assert!(rows[1].ends_with(",0,x,0.5,0.5,1"), "{}", rows[1]);
        assert!(rows[2].ends_with(",0,x,1,1,2"), "{}", rows[2]);
    }

    #[test]
    fn progress_reports_each_item() {
        let (e, _) = engine_with(&["y = x", "y = -x", "y = 1"]);
        let p = prepare(
            &e,
            &Preferences::default(),
            &request(Format::Png, Rect::new(-1.0, -1.0, 1.0, 1.0)),
        )
        .unwrap();
        let mut seen = Vec::new();
        produce(p, |d, t| seen.push((d, t))).unwrap();
        assert_eq!(seen, vec![(1, 3), (2, 3), (3, 3)]);
    }
}

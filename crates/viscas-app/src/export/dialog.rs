//! The export dialog: format, target, range, page size, sampling and
//! decoration options with a live preview. Settings are remembered in the
//! preferences.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use viscas_engine::PlotKind;
use viscas_engine::export::{ExportRange, param_name};
use viscas_plot::{CancelToken, Rect};
use viscas_scene::NodeId;

use super::job::{self, Request};
use super::pick;
use super::settings::{Background, ExportSettings, Format, RangeKind, Target, Unit};
use super::{Dest, Preset};
use crate::ctx::AppCtx;
use crate::widgets::row::{check_row, property_row};

/// Largest preview size in pixels.
const PREVIEW: (f64, f64) = (300.0, 220.0);
/// Delay coalescing preview renders while settings change.
const PREVIEW_DELAY: Duration = Duration::from_millis(180);
/// Quality steps offered.
const QUALITIES: [(f64, &str); 4] = [
    (1.0, "Screen (1×)"),
    (2.0, "Fine (2×)"),
    (4.0, "Ultra (4×)"),
    (8.0, "Extreme (8×)"),
];

/// Parses a number or a constant expression (`pi/2`, `2τ`, `1e-3`).
pub fn parse_number(text: &str) -> Option<f64> {
    use viscas_core::eval::{Env, eval_expr};
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(v) = t.replace('−', "-").parse::<f64>() {
        return v.is_finite().then_some(v);
    }
    let expr = viscas_core::syntax::parse_expr(t).ok()?;
    let v = eval_expr(&expr, &Env::new(), &Default::default()).ok()?;
    let n = v.as_number()?;
    n.is_real().then(|| v.to_f64()).filter(|v| v.is_finite())
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_owned();
    if s == "-0" { "0".to_owned() } else { s }
}

struct Widgets {
    window: gtk::Window,
    format: gtk::DropDown,
    target: gtk::DropDown,
    node: gtk::DropDown,
    node_row: gtk::Box,
    range_kind: gtk::DropDown,
    range_row: gtk::Box,
    range_from: gtk::Entry,
    range_to: gtk::Entry,
    range_fields: gtk::Box,
    pick: gtk::ToggleButton,
    from_view: gtk::Button,
    x_min: gtk::Entry,
    x_max: gtk::Entry,
    y_min: gtk::Entry,
    y_max: gtk::Entry,
    clip_row: gtk::Box,
    clip: gtk::CheckButton,
    width: gtk::SpinButton,
    height: gtk::SpinButton,
    unit: gtk::DropDown,
    keep_aspect: gtk::CheckButton,
    dpi: gtk::SpinButton,
    dpi_row: gtk::Box,
    quality: gtk::DropDown,
    csv_param_row: gtk::Box,
    csv_param: gtk::CheckButton,
    csv_samples: gtk::SpinButton,
    csv_samples_row: gtk::Box,
    picture_box: gtk::Box,
    grid: gtk::CheckButton,
    axes: gtk::CheckButton,
    labels: gtk::CheckButton,
    legend: gtk::CheckButton,
    background: gtk::DropDown,
    line_scale: gtk::SpinButton,
    svg_world_row: gtk::Box,
    svg_world: gtk::CheckButton,
    preview: gtk::Picture,
    info: gtk::Label,
    error: gtk::Label,
    copy: gtk::Button,
    export: gtk::Button,
}

struct Inner {
    ctx: AppCtx,
    w: Widgets,
    nodes: RefCell<Vec<(NodeId, String, PlotKind)>>,
    /// y/x scale ratio of the source viewport.
    aspect: Cell<f64>,
    /// Programmatic updates in progress: ignore change signals.
    syncing: Cell<bool>,
    preview_queued: Cell<bool>,
    preview_token: RefCell<Option<CancelToken>>,
    previous_unit: Cell<Unit>,
}

thread_local! {
    static OPEN: RefCell<Weak<Inner>> = const { RefCell::new(Weak::new()) };
}

/// The export dialog (one at a time; opening again re-presents it).
pub struct ExportDialog;

fn dropdown(items: &[&str]) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(items);
    d.set_hexpand(true);
    d
}

fn entry(width: i32) -> gtk::Entry {
    gtk::Entry::builder()
        .width_chars(width)
        .hexpand(true)
        .css_classes(["mono"])
        .build()
}

fn spin(lo: f64, hi: f64, step: f64, digits: u32) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(lo, hi, step);
    s.set_digits(digits);
    s.set_hexpand(true);
    s
}

fn section(title: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .css_classes(["heading", "dim"])
        .margin_top(6)
        .build()
}

fn pair(a: &impl IsA<gtk::Widget>, b: &impl IsA<gtk::Widget>) -> gtk::Box {
    let bx = gtk::Box::builder().spacing(4).hexpand(true).build();
    bx.append(a);
    bx.append(b);
    bx
}

impl ExportDialog {
    /// Shows the dialog configured by `preset`.
    pub fn present(ctx: &AppCtx, preset: Preset) {
        if let Some(inner) = OPEN.with_borrow(Weak::upgrade) {
            inner.apply_preset(&preset);
            inner.w.window.present();
            return;
        }
        let inner = Rc::new(Inner::build(ctx));
        OPEN.with_borrow_mut(|o| *o = Rc::downgrade(&inner));
        inner.connect(&inner);
        inner.apply_preset(&preset);
        inner.w.window.present();
        // The window keeps the state alive.
        let keep = inner.clone();
        inner.w.window.connect_destroy(move |_| {
            pick::stop();
            if let Some(t) = keep.preview_token.borrow_mut().take() {
                t.cancel();
            }
        });
    }
}

impl Inner {
    fn build(ctx: &AppCtx) -> Inner {
        let app = gio::Application::default().and_downcast::<gtk::Application>();
        let parent = app.as_ref().and_then(|a| a.active_window());
        let window = gtk::Window::builder()
            .title("Export")
            .default_width(760)
            .default_height(660)
            .css_classes(["export-dialog"])
            .build();
        if let Some(p) = &parent {
            window.set_transient_for(Some(p));
        }
        if let Some(app) = &app {
            window.set_application(Some(app));
        }

        let formats: Vec<&str> = Format::ALL.iter().map(|f| f.title()).collect();
        let targets: Vec<&str> = Target::ALL.iter().map(|t| t.title()).collect();
        let units: Vec<&str> = Unit::ALL.iter().map(|u| u.suffix()).collect();
        let backgrounds: Vec<&str> = Background::ALL.iter().map(|b| b.title()).collect();
        let qualities: Vec<&str> = QUALITIES.iter().map(|(_, t)| *t).collect();

        let format = dropdown(&formats);
        let target = dropdown(&targets);
        let node = dropdown(&[]);
        let range_kind = dropdown(&["Parameter", "x interval"]);
        let range_from = entry(8);
        let range_to = entry(8);
        let pick = gtk::ToggleButton::builder()
            .label("Pick")
            .tooltip_text("Drag the two handles along the curve in the viewport")
            .build();
        let x_min = entry(7);
        let x_max = entry(7);
        let y_min = entry(7);
        let y_max = entry(7);
        let (clip_row, clip) = check_row("Clip to box", true);
        let width = spin(1.0, 20_000.0, 1.0, 2);
        let height = spin(1.0, 20_000.0, 1.0, 2);
        let unit = dropdown(&units);
        unit.set_hexpand(false);
        let (aspect_row, keep_aspect) = check_row("Keep box aspect", true);
        let dpi = spin(10.0, 2400.0, 1.0, 0);
        let quality = dropdown(&qualities);
        let (csv_param_row, csv_param) = check_row("Table of t, x, y (uniform samples)", false);
        let csv_samples = spin(2.0, 1_000_000.0, 10.0, 0);
        let (grid_row, grid) = check_row("Grid", true);
        let (axes_row, axes) = check_row("Axes", true);
        let (labels_row, labels) = check_row("Tick labels", true);
        let (legend_row, legend) = check_row("Legend", false);
        let background = dropdown(&backgrounds);
        let line_scale = spin(0.1, 10.0, 0.1, 2);
        let (svg_world_row, svg_world) = check_row("SVG paths in world units", false);

        let form = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_start(12)
            .margin_end(12)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        form.append(&section("Output"));
        form.append(&property_row("Format", &format));
        form.append(&section("What"));
        form.append(&property_row("Export", &target));
        let node_row = property_row("Node", &node);
        form.append(&node_row);
        let range_row = property_row("Range", &range_kind);
        form.append(&range_row);
        let range_fields = gtk::Box::builder().spacing(4).build();
        range_fields.append(&range_from);
        range_fields.append(&gtk::Label::new(Some("to")));
        range_fields.append(&range_to);
        range_fields.append(&pick);
        let range_fields = property_row("", &range_fields);
        form.append(&range_fields);
        let from_view = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Reset the box to the viewport")
            .css_classes(["flat"])
            .build();
        let xs = pair(&x_min, &x_max);
        xs.append(&from_view);
        form.append(&property_row("Box x", &xs));
        form.append(&property_row("Box y", &pair(&y_min, &y_max)));
        form.append(&clip_row);
        form.append(&section("Page"));
        let size = gtk::Box::builder().spacing(4).build();
        size.append(&width);
        size.append(&gtk::Label::new(Some("×")));
        size.append(&height);
        size.append(&unit);
        form.append(&property_row("Size", &size));
        form.append(&aspect_row);
        let dpi_row = property_row("DPI", &dpi);
        form.append(&dpi_row);
        form.append(&section("Sampling"));
        form.append(&property_row("Quality", &quality));
        form.append(&csv_param_row);
        let csv_samples_row = property_row("Samples", &csv_samples);
        form.append(&csv_samples_row);
        form.append(&section("Appearance"));
        let picture_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        let checks = gtk::Grid::builder().column_spacing(12).build();
        checks.attach(&grid_row, 0, 0, 1, 1);
        checks.attach(&axes_row, 1, 0, 1, 1);
        checks.attach(&labels_row, 0, 1, 1, 1);
        checks.attach(&legend_row, 1, 1, 1, 1);
        picture_box.append(&checks);
        picture_box.append(&property_row("Background", &background));
        picture_box.append(&property_row("Line scale", &line_scale));
        picture_box.append(&svg_world_row);
        form.append(&picture_box);

        let scroller = gtk::ScrolledWindow::builder()
            .child(&form)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .width_request(400)
            .build();

        let preview = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .width_request(PREVIEW.0 as i32)
            .height_request(PREVIEW.1 as i32)
            .css_classes(["export-preview"])
            .build();
        let frame = gtk::Frame::builder().child(&preview).build();
        let info = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim", "mono"])
            .build();
        let error = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["error"])
            .visible(false)
            .build();
        let side = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_start(6)
            .margin_end(12)
            .margin_top(12)
            .hexpand(true)
            .build();
        side.append(&section("Preview"));
        side.append(&frame);
        side.append(&info);
        side.append(&error);

        let body = gtk::Box::builder().vexpand(true).build();
        body.append(&scroller);
        body.append(&side);

        let copy = gtk::Button::builder()
            .label("Copy")
            .tooltip_text("Copy to the clipboard in the chosen format")
            .build();
        let cancel = gtk::Button::builder().label("Cancel").build();
        let export = gtk::Button::builder()
            .label("Export…")
            .css_classes(["suggested-action"])
            .build();
        let actions = gtk::Box::builder()
            .spacing(6)
            .margin_start(12)
            .margin_end(12)
            .margin_top(8)
            .margin_bottom(12)
            .build();
        actions.append(&copy);
        actions.append(&gtk::Box::builder().hexpand(true).build());
        actions.append(&cancel);
        actions.append(&export);
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        root.append(&body);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&actions);
        window.set_child(Some(&root));
        window.set_default_widget(Some(&export));
        let win = window.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(w) = win.upgrade() {
                w.close();
            }
        });
        let keys = gtk::EventControllerKey::new();
        let win = window.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape
                && let Some(w) = win.upgrade()
            {
                w.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(keys);

        let inner = Inner {
            ctx: ctx.clone(),
            w: Widgets {
                window,
                format,
                target,
                node,
                node_row,
                range_kind,
                range_row,
                range_from,
                range_to,
                range_fields,
                pick,
                from_view,
                x_min,
                x_max,
                y_min,
                y_max,
                clip_row,
                clip,
                width,
                height,
                unit,
                keep_aspect,
                dpi,
                dpi_row,
                quality,
                csv_param_row,
                csv_param,
                csv_samples,
                csv_samples_row,
                picture_box,
                grid,
                axes,
                labels,
                legend,
                background,
                line_scale,
                svg_world_row,
                svg_world,
                preview,
                info,
                error,
                copy,
                export,
            },
            nodes: RefCell::default(),
            aspect: Cell::new(1.0),
            syncing: Cell::new(false),
            preview_queued: Cell::new(false),
            preview_token: RefCell::default(),
            previous_unit: Cell::new(Unit::Px),
        };
        inner.load_settings(&ctx.preferences().with(|p| p.export.clone()));
        inner
    }

    /// Wires every widget to [`Inner::changed`].
    fn connect(&self, rc: &Rc<Inner>) {
        let w = &self.w;
        let weak = Rc::downgrade(rc);
        let on = move || {
            let weak = weak.clone();
            move || {
                if let Some(i) = weak.upgrade()
                    && !i.syncing.get()
                {
                    i.changed();
                }
            }
        };
        for d in [&w.format, &w.range_kind, &w.unit, &w.quality, &w.background] {
            let f = on();
            d.connect_selected_notify(move |_| f());
        }
        {
            let weak = Rc::downgrade(rc);
            w.unit.connect_selected_notify(move |_| {
                if let Some(i) = weak.upgrade() {
                    i.unit_changed();
                }
            });
        }
        {
            let weak = Rc::downgrade(rc);
            w.target.connect_selected_notify(move |_| {
                if let Some(i) = weak.upgrade()
                    && !i.syncing.get()
                {
                    i.target_changed();
                }
            });
            let weak = Rc::downgrade(rc);
            w.node.connect_selected_notify(move |_| {
                if let Some(i) = weak.upgrade()
                    && !i.syncing.get()
                {
                    i.prefill_range();
                    i.changed();
                }
            });
        }
        for e in [
            &w.range_from,
            &w.range_to,
            &w.x_min,
            &w.x_max,
            &w.y_min,
            &w.y_max,
        ] {
            let f = on();
            e.connect_changed(move |_| f());
        }
        for c in [
            &w.clip,
            &w.keep_aspect,
            &w.csv_param,
            &w.grid,
            &w.axes,
            &w.labels,
            &w.legend,
            &w.svg_world,
        ] {
            let f = on();
            c.connect_toggled(move |_| f());
        }
        for s in [&w.width, &w.height, &w.dpi, &w.csv_samples, &w.line_scale] {
            let f = on();
            s.connect_value_changed(move |_| f());
        }
        let weak = Rc::downgrade(rc);
        w.pick.connect_toggled(move |b| {
            if let Some(i) = weak.upgrade() {
                i.toggle_pick(b.is_active());
            }
        });
        let weak = Rc::downgrade(rc);
        w.export.connect_clicked(move |_| {
            if let Some(i) = weak.upgrade() {
                i.export_to_file();
            }
        });
        let weak = Rc::downgrade(rc);
        w.copy.connect_clicked(move |_| {
            if let Some(i) = weak.upgrade() {
                i.copy();
            }
        });
        let weak = Rc::downgrade(rc);
        w.from_view.connect_clicked(move |_| {
            if let Some(i) = weak.upgrade() {
                let (world, aspect) = super::view_box();
                i.aspect.set(aspect);
                i.set_world(world);
                i.changed();
            }
        });
        let weak = Rc::downgrade(rc);
        w.window.connect_close_request(move |_| {
            if let Some(i) = weak.upgrade() {
                i.remember();
            }
            glib::Propagation::Proceed
        });
    }

    fn load_settings(&self, s: &ExportSettings) {
        let w = &self.w;
        self.syncing.set(true);
        w.format.set_selected(s.format.index());
        w.target.set_selected(s.target.index());
        w.range_kind.set_selected(match s.range_kind {
            RangeKind::Param => 0,
            RangeKind::X => 1,
        });
        w.clip.set_active(s.clip_region);
        w.unit.set_selected(s.unit.index());
        self.previous_unit.set(s.unit);
        w.width.set_value(s.width);
        w.height.set_value(s.height);
        w.keep_aspect.set_active(s.keep_aspect);
        w.dpi.set_value(s.dpi);
        let q = QUALITIES
            .iter()
            .position(|(v, _)| (*v - s.quality).abs() < 1e-9)
            .unwrap_or(1);
        w.quality.set_selected(q as u32);
        w.csv_param.set_active(s.csv_param);
        w.csv_samples.set_value(f64::from(s.csv_samples));
        w.grid.set_active(s.grid);
        w.axes.set_active(s.axes);
        w.labels.set_active(s.labels);
        w.legend.set_active(s.legend);
        w.background.set_selected(s.background.index());
        w.line_scale.set_value(s.line_scale);
        w.svg_world.set_active(s.svg_world);
        self.syncing.set(false);
    }

    /// The settings shown in the widgets.
    fn settings(&self) -> ExportSettings {
        let w = &self.w;
        let mut s = ExportSettings {
            format: Format::from_index(w.format.selected()),
            target: Target::from_index(w.target.selected()),
            unit: Unit::from_index(w.unit.selected()),
            width: w.width.value(),
            height: w.height.value(),
            keep_aspect: w.keep_aspect.is_active(),
            dpi: w.dpi.value(),
            quality: QUALITIES
                .get(w.quality.selected() as usize)
                .map_or(2.0, |(v, _)| *v),
            csv_samples: w.csv_samples.value() as u32,
            csv_param: w.csv_param.is_active(),
            grid: w.grid.is_active(),
            axes: w.axes.is_active(),
            labels: w.labels.is_active(),
            legend: w.legend.is_active(),
            background: Background::from_index(w.background.selected()),
            line_scale: w.line_scale.value(),
            svg_world: w.svg_world.is_active(),
            clip_region: w.clip.is_active(),
            range_kind: if w.range_kind.selected() == 1 {
                RangeKind::X
            } else {
                RangeKind::Param
            },
        };
        s.validate();
        s
    }

    fn world(&self) -> Option<Rect> {
        let w = &self.w;
        let v = |e: &gtk::Entry| parse_number(&e.text());
        let (x0, x1, y0, y1) = (v(&w.x_min)?, v(&w.x_max)?, v(&w.y_min)?, v(&w.y_max)?);
        (x0 < x1 && y0 < y1).then(|| Rect::new(x0, y0, x1, y1))
    }

    fn set_world(&self, r: Rect) {
        let w = &self.w;
        self.syncing.set(true);
        w.x_min.set_text(&fmt(r.x_min));
        w.x_max.set_text(&fmt(r.x_max));
        w.y_min.set_text(&fmt(r.y_min));
        w.y_max.set_text(&fmt(r.y_max));
        self.syncing.set(false);
    }

    fn selected_node(&self) -> Option<(NodeId, PlotKind)> {
        let nodes = self.nodes.borrow();
        nodes
            .get(self.w.node.selected() as usize)
            .map(|(id, _, k)| (*id, *k))
    }

    fn apply_preset(&self, preset: &Preset) {
        let ctx = &self.ctx;
        let nodes = super::plot_nodes(ctx);
        let names: Vec<&str> = nodes.iter().map(|(_, n, _)| n.as_str()).collect();
        self.syncing.set(true);
        self.w.node.set_model(Some(&gtk::StringList::new(&names)));
        let pos = preset
            .node
            .and_then(|n| nodes.iter().position(|(id, _, _)| *id == n))
            .unwrap_or(0);
        *self.nodes.borrow_mut() = nodes;
        self.w.node.set_selected(pos as u32);
        if let Some(f) = preset.format {
            self.w.format.set_selected(f.index());
        }
        if let Some(t) = preset.target {
            self.w.target.set_selected(t.index());
        }
        self.syncing.set(false);
        let (view, aspect) = super::view_box();
        self.aspect.set(aspect);
        self.set_world(preset.world.unwrap_or(view));
        self.prefill_range();
        self.changed();
    }

    fn target_changed(&self) {
        let t = Target::from_index(self.w.target.selected());
        if t == Target::Region {
            // Prefer an inequality node for regions.
            let nodes = self.nodes.borrow();
            if let Some(i) = nodes
                .iter()
                .position(|(_, _, k)| matches!(k, PlotKind::Inequality | PlotKind::PolarInequality))
                && !self.selected_node().is_some_and(|(_, k)| {
                    matches!(k, PlotKind::Inequality | PlotKind::PolarInequality)
                })
            {
                drop(nodes);
                self.syncing.set(true);
                self.w.node.set_selected(i as u32);
                self.syncing.set(false);
            }
        }
        if t != Target::Part {
            self.w.pick.set_active(false);
        }
        self.prefill_range();
        self.changed();
    }

    /// Fills the range fields with the node's natural parameter range.
    fn prefill_range(&self) {
        let Some((node, _)) = self.selected_node() else {
            return;
        };
        let world = self.world().unwrap_or_else(|| super::view_box().0);
        let r = match self.settings().range_kind {
            RangeKind::Param => super::node_param_range(&self.ctx, node, &world),
            RangeKind::X => None,
        }
        .unwrap_or((world.x_min, world.x_max));
        // Explicit curves span the whole view: start with its middle half
        // so both handles are on screen.
        let explicit = self
            .selected_node()
            .is_some_and(|(_, k)| matches!(k, PlotKind::ExplicitY | PlotKind::ExplicitX));
        let r = if explicit && r.1 > r.0 {
            let q = (r.1 - r.0) / 4.0;
            let nice = |v: f64| {
                let step = viscas_plot::nice_step(q / 5.0);
                (v / step).round() * step
            };
            (nice(r.0 + q), nice(r.1 - q))
        } else {
            r
        };
        self.syncing.set(true);
        self.w.range_from.set_text(&fmt(r.0));
        self.w.range_to.set_text(&fmt(r.1));
        self.syncing.set(false);
        pick::set_range(r.0, r.1);
    }

    fn unit_changed(&self) {
        let new = Unit::from_index(self.w.unit.selected());
        let old = self.previous_unit.replace(new);
        if old == new {
            return;
        }
        let w = &self.w;
        self.syncing.set(true);
        w.width.set_value(old.convert(w.width.value(), new));
        w.height.set_value(old.convert(w.height.value(), new));
        self.syncing.set(false);
        self.changed();
    }

    fn range(&self) -> Option<(f64, f64)> {
        let a = parse_number(&self.w.range_from.text())?;
        let b = parse_number(&self.w.range_to.text())?;
        (a != b).then_some((a.min(b), a.max(b)))
    }

    /// The request of the current widgets, or a message saying what is
    /// missing.
    fn request(&self) -> Result<Request, String> {
        let mut settings = self.settings();
        let world = self.world().ok_or("the box needs min < max on both axes")?;
        if settings.keep_aspect
            && let Some(h) = ExportSettings::aspect_height(
                settings.width,
                world.width(),
                world.height(),
                self.aspect.get(),
            )
        {
            settings.height = h;
        }
        let (nodes, range, clip) = match settings.target {
            Target::View | Target::Box => (None, ExportRange::Full, None),
            Target::Selected => {
                let sel = self.ctx.selection().selected();
                if sel.is_empty() {
                    return Err("nothing selected".to_owned());
                }
                (Some(sel), ExportRange::Full, None)
            }
            Target::Curve => {
                let (n, _) = self.selected_node().ok_or("no plot to export")?;
                (Some(vec![n]), ExportRange::Full, None)
            }
            Target::Part => {
                let (n, _) = self.selected_node().ok_or("no plot to export")?;
                let (a, b) = self
                    .range()
                    .ok_or("the range needs two different numbers")?;
                let r = match settings.range_kind {
                    RangeKind::Param => ExportRange::Param(a, b),
                    RangeKind::X => ExportRange::X(a, b),
                };
                (Some(vec![n]), r, None)
            }
            Target::Region => {
                let (n, _) = self.selected_node().ok_or("no region to export")?;
                let clip = settings.clip_region.then_some(world);
                (Some(vec![n]), ExportRange::Full, clip)
            }
        };
        Ok(Request {
            settings,
            world,
            nodes,
            range,
            clip,
        })
    }

    /// Updates visibility, the computed height, the info line and the
    /// preview after any change.
    fn changed(&self) {
        let w = &self.w;
        let s = self.settings();
        let single = s.target.single_node();
        w.node_row.set_visible(single);
        let part = s.target == Target::Part;
        w.range_row.set_visible(part);
        w.range_fields.set_visible(part);
        w.clip_row.set_visible(s.target == Target::Region);
        w.dpi_row.set_visible(s.format.is_raster());
        let data = !s.format.is_picture();
        w.csv_param_row.set_visible(data);
        w.csv_samples_row.set_visible(data && s.csv_param);
        w.picture_box.set_sensitive(!data);
        w.svg_world_row.set_visible(s.format == Format::Svg);
        w.height.set_sensitive(!s.keep_aspect);
        // Picking needs a curve with a parameter.
        let pickable =
            part && self.selected_node().is_some_and(|(n, _)| {
                self.ctx
                    .engine()
                    .with(|e| e.plot_of(n).is_some_and(|p| param_name(&p.shape).is_some()))
            }) && (s.range_kind == RangeKind::Param
                || self
                    .selected_node()
                    .is_some_and(|(_, k)| k == PlotKind::ExplicitY));
        w.pick.set_sensitive(pickable);
        if !pickable && w.pick.is_active() {
            w.pick.set_active(false);
        }
        if let Some((a, b)) = self.range()
            && pick::is_active()
            && !pick::dragging()
        {
            pick::set_range(a, b);
        }
        match self.request() {
            Ok(req) => {
                if s.keep_aspect {
                    self.syncing.set(true);
                    w.height.set_value(req.settings.height);
                    self.syncing.set(false);
                }
                let (pw, ph) = req.settings.page_px();
                let mut info = format!(
                    "{} × {} {} · {}",
                    fmt((req.settings.width * 100.0).round() / 100.0),
                    fmt((req.settings.height * 100.0).round() / 100.0),
                    req.settings.unit.suffix(),
                    req.settings.format.short()
                );
                if req.settings.format.is_raster() {
                    let (rw, rh) = req.settings.raster_px();
                    info.push_str(&format!(" · {rw} × {rh} px"));
                } else if req.settings.unit != Unit::Px {
                    info.push_str(&format!(" · {:.0} × {:.0} css px", pw, ph));
                }
                w.info.set_label(&info);
                w.error.set_visible(false);
                w.export.set_sensitive(true);
                w.copy.set_sensitive(true);
                self.queue_preview();
            }
            Err(e) => {
                w.error.set_label(&e);
                w.error.set_visible(true);
                w.export.set_sensitive(false);
                w.copy.set_sensitive(false);
            }
        }
    }

    fn toggle_pick(&self, on: bool) {
        if !on {
            pick::stop();
            return;
        }
        let Some((node, _)) = self.selected_node() else {
            return;
        };
        let Some((shape, color)) = self
            .ctx
            .engine()
            .with(|e| Some((e.plot_of(node)?.shape.clone(), e.node_color(node)?)))
        else {
            return;
        };
        let (t0, t1) = self.range().unwrap_or((0.0, 1.0));
        let domain = match &shape {
            viscas_engine::PlotShape::Parametric { range, .. }
            | viscas_engine::PlotShape::Polar { range, .. } => Some(*range),
            _ => None,
        };
        let from = self.w.range_from.downgrade();
        let to = self.w.range_to.downgrade();
        pick::start(pick::Pick {
            shape,
            color,
            domain,
            t: [t0, t1],
            on_change: Rc::new(move |a, b| {
                if let (Some(f), Some(t)) = (from.upgrade(), to.upgrade()) {
                    f.set_text(&fmt(a));
                    t.set_text(&fmt(b));
                }
            }),
        });
    }

    fn queue_preview(&self) {
        if self.preview_queued.replace(true) {
            return;
        }
        let weak = OPEN.with_borrow(Weak::clone);
        glib::timeout_add_local_once(PREVIEW_DELAY, move || {
            if let Some(i) = weak.upgrade() {
                i.preview_queued.set(false);
                i.render_preview();
            }
        });
    }

    /// Renders a small PNG of the current request off the UI thread.
    fn render_preview(&self) {
        let Ok(mut req) = self.request() else {
            return;
        };
        let s = &mut req.settings;
        let (pw, ph) = s.page_px();
        let k = (PREVIEW.0 / pw).min(PREVIEW.1 / ph);
        let data = !s.format.is_picture();
        s.format = Format::Png;
        // Same look, scaled: the PNG gets the page at preview resolution.
        s.dpi = 96.0 * k * 2.0;
        s.quality = s.quality.min(1.0);
        if data {
            s.grid = true;
            s.axes = true;
        }
        let prepared = {
            let prefs = self.ctx.preferences().get();
            self.ctx.engine().with(|e| job::prepare(e, &prefs, &req))
        };
        let Ok(prepared) = prepared else {
            return;
        };
        if let Some(t) = self
            .preview_token
            .borrow_mut()
            .replace(prepared.cancel_token())
        {
            t.cancel();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("viscas-export-preview".into())
            .spawn(move || {
                let _ = tx.send(job::produce(prepared, |_, _| {}));
            })
            .ok();
        let picture = self.w.preview.downgrade();
        glib::timeout_add_local(Duration::from_millis(30), move || match rx.try_recv() {
            Ok(Ok(bytes)) => {
                if let Some(p) = picture.upgrade()
                    && let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes))
                {
                    p.set_paintable(Some(&tex));
                }
                glib::ControlFlow::Break
            }
            Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        });
    }

    /// Saves the settings in the preferences.
    fn remember(&self) {
        let s = self.settings();
        self.ctx.preferences().update(|p| {
            if p.export != s {
                p.export = s;
            }
        });
    }

    fn copy(&self) {
        let Ok(req) = self.request() else {
            return;
        };
        self.remember();
        super::run(&self.ctx, req, Dest::Clipboard, |_| {});
    }

    fn export_to_file(&self) {
        let Ok(req) = self.request() else {
            return;
        };
        self.remember();
        let format = req.settings.format;
        let name = match req.settings.target {
            Target::View | Target::Box => "viewport".to_owned(),
            _ => self
                .selected_node()
                .and_then(|(n, _)| {
                    self.nodes
                        .borrow()
                        .iter()
                        .find(|(id, _, _)| *id == n)
                        .map(|(_, name, _)| name.clone())
                })
                .unwrap_or_else(|| "export".to_owned()),
        };
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(format.title()));
        filter.add_suffix(format.extension());
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(format!("Export {}", format.short()))
            .modal(true)
            .initial_name(format!("{safe}.{}", format.extension()))
            .filters(&filters)
            .default_filter(&filter)
            .build();
        let ctx = self.ctx.clone();
        let window = self.w.window.downgrade();
        dialog.save(Some(&self.w.window), gio::Cancellable::NONE, move |res| {
            let Ok(file) = res else {
                return;
            };
            let Some(mut path) = file.path() else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension(format.extension());
            }
            super::run(&ctx, req, Dest::File(path), move |r| {
                if r.is_ok()
                    && let Some(w) = window.upgrade()
                {
                    w.close();
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::parse_number;

    #[test]
    fn numbers_and_constant_expressions() {
        assert_eq!(parse_number(" 1.5 "), Some(1.5));
        assert_eq!(parse_number("−2"), Some(-2.0));
        assert!((parse_number("pi/2").unwrap() - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("x + 1"), None);
    }
}

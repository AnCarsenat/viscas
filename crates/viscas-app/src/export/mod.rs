//! Export of the viewport, nodes, curves, parts of curves and regions as
//! SVG, PDF, PNG, EPS/PS, CSV, TikZ and JSON (files or clipboard).
//!
//! The dialog ([`dialog`]) builds a [`job::Request`]; [`run`] snapshots it
//! on the UI thread and re-samples/writes on a worker thread, with
//! progress and a cancel button in the status bar.

pub mod dialog;
pub mod drawing;
pub mod job;
pub mod pick;
pub mod raster;
pub mod settings;
pub mod svg;
pub mod text;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib};
use viscas_engine::export::{ExportRange, param_range};
use viscas_plot::{CancelToken, Rect};
use viscas_scene::NodeId;

use self::job::Request;
use self::settings::{ExportSettings, Format, Target};
use crate::commands::{Command, CommandProvider};
use crate::ctx::{AppCtx, WeakCtx};

/// Where finished bytes go.
#[derive(Clone, Debug)]
pub enum Dest {
    File(PathBuf),
    Clipboard,
}

enum Msg {
    Progress(usize, usize),
    Done(Result<Vec<u8>, String>),
}

/// Status-bar widgets showing the running export.
struct Status {
    root: gtk::Box,
    label: gtk::Label,
    bar: gtk::ProgressBar,
    cancel: gtk::Button,
}

thread_local! {
    static STATUS: RefCell<Option<Status>> = const { RefCell::new(None) };
    /// Number and token of the running export (one at a time; a new one
    /// cancels it).
    static RUNNING: RefCell<Option<(u64, CancelToken)>> = const { RefCell::new(None) };
    static NEXT_RUN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Hides the status message after a delay.
    static HIDE: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
}

/// The status-bar area of exports (hidden while idle).
pub fn status_widget() -> gtk::Widget {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .visible(false)
        .margin_end(10)
        .css_classes(["export-status"])
        .build();
    let label = gtk::Label::builder().css_classes(["mono", "dim"]).build();
    let bar = gtk::ProgressBar::builder()
        .valign(gtk::Align::Center)
        .width_request(90)
        .build();
    let cancel = gtk::Button::builder()
        .icon_name("process-stop-symbolic")
        .tooltip_text("Cancel the export")
        .css_classes(["flat", "area-tool"])
        .build();
    cancel.connect_clicked(|_| cancel_running());
    root.append(&label);
    root.append(&bar);
    root.append(&cancel);
    STATUS.with_borrow_mut(|s| {
        *s = Some(Status {
            root: root.clone(),
            label,
            bar,
            cancel,
        });
    });
    root.upcast()
}

/// Cancels the running export, if any.
pub fn cancel_running() {
    if let Some((_, t)) = RUNNING.with_borrow_mut(Option::take) {
        t.cancel();
    }
}

fn show_status(text: &str, fraction: Option<f64>, busy: bool) {
    STATUS.with_borrow(|s| {
        let Some(s) = s else {
            return;
        };
        if let Some(id) = HIDE.with_borrow_mut(Option::take) {
            id.remove();
        }
        s.root.set_visible(true);
        s.label.set_label(text);
        s.bar.set_visible(fraction.is_some());
        if let Some(f) = fraction {
            s.bar.set_fraction(f.clamp(0.0, 1.0));
        }
        s.cancel.set_visible(busy);
        if !busy {
            let id = glib::timeout_add_local_once(Duration::from_secs(5), || {
                HIDE.with_borrow_mut(|h| *h = None);
                STATUS.with_borrow(|s| {
                    if let Some(s) = s {
                        s.root.set_visible(false);
                    }
                });
            });
            HIDE.with_borrow_mut(|h| *h = Some(id));
        }
    });
}

/// Runs `req` in the background, delivering the bytes to `dest`.
/// `done` gets the outcome on the UI thread.
pub fn run(
    ctx: &AppCtx,
    req: Request,
    dest: Dest,
    done: impl FnOnce(Result<(), String>) + 'static,
) {
    let prepared = {
        let prefs = ctx.preferences().get();
        ctx.engine().with(|e| job::prepare(e, &prefs, &req))
    };
    let prepared = match prepared {
        Ok(p) if p.is_empty() => Err("nothing to export: no visible plot matches".to_owned()),
        other => other,
    };
    let prepared = match prepared {
        Ok(p) => p,
        Err(e) => {
            show_status(&format!("export failed: {e}"), None, false);
            done(Err(e));
            return;
        }
    };
    cancel_running();
    let run_id = NEXT_RUN.get() + 1;
    NEXT_RUN.set(run_id);
    RUNNING.with_borrow_mut(|r| *r = Some((run_id, prepared.cancel_token())));
    let format = req.settings.format;
    show_status(&format!("exporting {}…", format.short()), Some(0.0), true);
    let (tx, rx) = mpsc::channel::<Msg>();
    let file = match &dest {
        Dest::File(p) => Some(p.clone()),
        Dest::Clipboard => None,
    };
    std::thread::Builder::new()
        .name("viscas-export".into())
        .spawn(move || {
            let progress_tx = tx.clone();
            let result = job::produce(prepared, |d, t| {
                let _ = progress_tx.send(Msg::Progress(d, t));
            });
            // Files are written here too, off the UI thread.
            let result = match (result, &file) {
                (Ok(bytes), Some(path)) => std::fs::write(path, &bytes)
                    .map(|_| Vec::new())
                    .map_err(|e| format!("cannot write {}: {e}", path.display())),
                (r, _) => r,
            };
            let _ = tx.send(Msg::Done(result));
        })
        .ok();
    let mut done = Some(done);
    glib::timeout_add_local(Duration::from_millis(40), move || {
        loop {
            match rx.try_recv() {
                Ok(Msg::Progress(d, t)) => {
                    let f = d as f64 / t.max(1) as f64;
                    show_status(
                        &format!("exporting {}… {d}/{t}", format.short()),
                        Some(f),
                        true,
                    );
                }
                Ok(Msg::Done(result)) => {
                    RUNNING.with_borrow_mut(|r| {
                        if r.as_ref().is_some_and(|(id, _)| *id == run_id) {
                            *r = None;
                        }
                    });
                    let outcome = result.and_then(|bytes| match &dest {
                        Dest::File(path) => {
                            let name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            show_status(&format!("exported {name}"), None, false);
                            Ok(())
                        }
                        Dest::Clipboard => {
                            to_clipboard(format, bytes)?;
                            show_status(
                                &format!("copied {} to the clipboard", format.short()),
                                None,
                                false,
                            );
                            Ok(())
                        }
                    });
                    if let Err(e) = &outcome {
                        show_status(e, None, false);
                    }
                    if let Some(f) = done.take() {
                        f(outcome);
                    }
                    return glib::ControlFlow::Break;
                }
                Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    show_status("export failed (worker stopped)", None, false);
                    if let Some(f) = done.take() {
                        f(Err("worker stopped".into()));
                    }
                    return glib::ControlFlow::Break;
                }
            }
        }
    });
}

/// Puts export bytes on the clipboard: SVG as image and text, PNG as a
/// texture, PDF/PS as their MIME type, data formats as text.
fn to_clipboard(format: Format, bytes: Vec<u8>) -> Result<(), String> {
    let display = gdk::Display::default().ok_or("no display")?;
    let clipboard = display.clipboard();
    match format {
        Format::Png => {
            let tex = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes))
                .map_err(|e| e.to_string())?;
            clipboard.set_texture(&tex);
        }
        Format::Svg => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let svg =
                gdk::ContentProvider::for_bytes(format.mime(), &glib::Bytes::from_owned(bytes));
            let plain = gdk::ContentProvider::for_value(&text.to_value());
            clipboard
                .set_content(Some(&gdk::ContentProvider::new_union(&[svg, plain])))
                .map_err(|e| e.to_string())?;
        }
        Format::Pdf | Format::Eps | Format::Ps => {
            let p = gdk::ContentProvider::for_bytes(format.mime(), &glib::Bytes::from_owned(bytes));
            clipboard.set_content(Some(&p)).map_err(|e| e.to_string())?;
        }
        Format::Csv | Format::Tikz | Format::Json => {
            clipboard.set_text(&String::from_utf8_lossy(&bytes));
        }
    }
    Ok(())
}

/// World box and y/x scale ratio of the active viewport (a default box
/// when there is none).
pub fn view_box() -> (Rect, f64) {
    match crate::viewport::active() {
        Some(vp) => {
            let v = vp.view();
            let b = v.bounds();
            (Rect::new(b.x_min, b.y_min, b.x_max, b.y_max), v.aspect)
        }
        None => (Rect::new(-10.0, -7.5, 10.0, 7.5), 1.0),
    }
}

/// The plot nodes of the scene: id and display name, in scene order.
pub fn plot_nodes(ctx: &AppCtx) -> Vec<(NodeId, String, viscas_engine::PlotKind)> {
    ctx.engine().with(|e| {
        e.plot_items()
            .into_iter()
            .map(|it| {
                (
                    it.node,
                    crate::editors::scene_tree::model::display_name(e.scene(), it.node),
                    it.plot.kind,
                )
            })
            .collect()
    })
}

/// The selected plot node, if any (last selected wins).
fn selected_plot(ctx: &AppCtx) -> Option<NodeId> {
    let nodes = plot_nodes(ctx);
    ctx.selection()
        .selected()
        .into_iter()
        .rev()
        .find(|id| nodes.iter().any(|(n, _, _)| n == id))
}

/// A request built from the remembered settings for `target` (quick
/// exports and copies). `None` when the target needs a selection.
pub fn quick_request(ctx: &AppCtx, format: Format, target: Target) -> Option<Request> {
    let mut settings = ctx.preferences().with(|p| p.export.clone());
    settings.format = format;
    settings.target = target;
    let (world, aspect) = view_box();
    if settings.keep_aspect
        && let Some(h) =
            ExportSettings::aspect_height(settings.width, world.width(), world.height(), aspect)
    {
        settings.height = h;
    }
    let nodes = match target {
        Target::View | Target::Box => None,
        _ => {
            let sel = ctx.selection().selected();
            if sel.is_empty() {
                return None;
            }
            Some(sel)
        }
    };
    Some(Request {
        settings,
        world,
        nodes,
        range: ExportRange::Full,
        clip: None,
    })
}

/// Copies the view (or the selection) to the clipboard.
pub fn copy(ctx: &AppCtx, format: Format, target: Target) {
    match quick_request(ctx, format, target) {
        Some(req) => run(ctx, req, Dest::Clipboard, |_| {}),
        None => show_status("select a node to copy first", None, false),
    }
}

/// How the dialog opens.
#[derive(Clone, Debug, Default)]
pub struct Preset {
    pub format: Option<Format>,
    pub target: Option<Target>,
    /// World box (e.g. from the Shift+E rubber band).
    pub world: Option<Rect>,
    pub node: Option<NodeId>,
}

/// Opens the export dialog (File ▸ Export…, Ctrl+Shift+E).
pub fn open(ctx: &AppCtx, preset: Preset) {
    let mut preset = preset;
    if preset.target.is_none() {
        // A selected curve exports itself; otherwise the whole view.
        preset.target = Some(if ctx.selection().selected().is_empty() {
            Target::View
        } else {
            Target::Selected
        });
    }
    if preset.node.is_none() {
        preset.node = selected_plot(ctx);
    }
    dialog::ExportDialog::present(ctx, preset);
}

/// Opens the dialog for the box `world` (Shift+E rubber band).
pub fn open_box(ctx: &AppCtx, world: Rect) {
    open(
        ctx,
        Preset {
            target: Some(Target::Box),
            world: Some(world),
            ..Default::default()
        },
    );
}

/// Natural parameter range of `node` in `world` (part-of-curve fields).
pub fn node_param_range(ctx: &AppCtx, node: NodeId, world: &Rect) -> Option<(f64, f64)> {
    ctx.engine()
        .with(|e| e.plot_of(node).and_then(|p| param_range(&p.shape, world)))
}

/// Palette commands for exports (they also show in the keymap).
pub struct ExportCommands(WeakCtx);

impl ExportCommands {
    pub fn new(ctx: &AppCtx) -> Self {
        Self(ctx.downgrade())
    }
}

impl CommandProvider for ExportCommands {
    fn commands(&self) -> Vec<Command> {
        let Some(ctx) = self.0.upgrade() else {
            return Vec::new();
        };
        let has_sel = !ctx.selection().selected().is_empty();
        let mut out = Vec::new();
        let mut open_cmd = |id: &str, title: &str, format: Format, target: Target| {
            let weak = ctx.downgrade();
            out.push(Command {
                id: id.to_owned(),
                title: title.to_owned(),
                category: "Export",
                shortcut: None,
                run: Rc::new(move || {
                    if let Some(ctx) = weak.upgrade() {
                        open(
                            &ctx,
                            Preset {
                                format: Some(format),
                                target: Some(target),
                                ..Default::default()
                            },
                        );
                    }
                }),
            });
        };
        open_cmd(
            "export.view-svg",
            "Export View as SVG…",
            Format::Svg,
            Target::View,
        );
        open_cmd(
            "export.view-png",
            "Export View as PNG…",
            Format::Png,
            Target::View,
        );
        open_cmd(
            "export.view-pdf",
            "Export View as PDF…",
            Format::Pdf,
            Target::View,
        );
        open_cmd(
            "export.view-tikz",
            "Export View as TikZ…",
            Format::Tikz,
            Target::View,
        );
        if has_sel {
            open_cmd(
                "export.selected-svg",
                "Export Selected as SVG…",
                Format::Svg,
                Target::Selected,
            );
            open_cmd(
                "export.selected-pdf",
                "Export Selected as PDF…",
                Format::Pdf,
                Target::Selected,
            );
            open_cmd(
                "export.selected-csv",
                "Export Selected as CSV…",
                Format::Csv,
                Target::Selected,
            );
            open_cmd(
                "export.selected-tikz",
                "Export Selected as TikZ…",
                Format::Tikz,
                Target::Selected,
            );
            open_cmd(
                "export.selected-json",
                "Export Selected as JSON…",
                Format::Json,
                Target::Selected,
            );
            open_cmd(
                "export.part",
                "Export Part of Curve…",
                Format::Svg,
                Target::Part,
            );
        }
        let mut copy_cmd = |id: &str, title: &str, format: Format, target: Target| {
            let weak = ctx.downgrade();
            out.push(Command {
                id: id.to_owned(),
                title: title.to_owned(),
                category: "Export",
                shortcut: None,
                run: Rc::new(move || {
                    if let Some(ctx) = weak.upgrade() {
                        copy(&ctx, format, target);
                    }
                }),
            });
        };
        copy_cmd(
            "export.copy-view-svg",
            "Copy View as SVG",
            Format::Svg,
            Target::View,
        );
        copy_cmd(
            "export.copy-view-png",
            "Copy View as PNG",
            Format::Png,
            Target::View,
        );
        if has_sel {
            copy_cmd(
                "export.copy-selected-svg",
                "Copy Selected as SVG",
                Format::Svg,
                Target::Selected,
            );
            copy_cmd(
                "export.copy-selected-png",
                "Copy Selected as PNG",
                Format::Png,
                Target::Selected,
            );
        }
        let weak = ctx.downgrade();
        out.push(Command {
            id: "export.box".to_owned(),
            title: "Export Box Region… (drag in the viewport)".to_owned(),
            category: "Export",
            shortcut: Some("Shift+E".to_owned()),
            run: Rc::new(move || {
                if weak.upgrade().is_some() {
                    pick::arm_box();
                }
            }),
        });
        out
    }
}

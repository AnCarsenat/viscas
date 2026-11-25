//! Editor widgets hosted by areas (REQUIREMENTS §2).
//!
//! The Viewport, Scene Tree, Inspector, Console and Expressions editors are
//! functional; every other editor shows an empty state describing what it
//! will do and in which milestone.

pub mod console;
pub mod expressions;
pub mod inspector;
pub mod scene_tree;

use std::any::Any;
use std::f64::consts::PI;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, gio};

use crate::ctx::AppCtx;
use crate::layout::{EditorKind, Layout};
use crate::prefs::Preferences;
use crate::theme::model::{Color, GraphColors};
use crate::viewport::Viewport;
use crate::workspace::Workspace;

/// Editors that only show an empty state so far.
pub fn is_unfinished(kind: EditorKind) -> bool {
    matches!(
        kind,
        EditorKind::Sheet | EditorKind::Script | EditorKind::DependencyGraph
    )
}

/// Workspaces built around unfinished editors.
pub fn is_unfinished_workspace(ws: Workspace) -> bool {
    matches!(ws, Workspace::Data | Workspace::Script)
}

/// Whether `kind` is offered in tabs, menus and the palette.
pub fn editor_shown(kind: EditorKind, prefs: &Preferences) -> bool {
    prefs.interface.show_unfinished || !is_unfinished(kind)
}

/// Whether the workspace tab is offered.
pub fn workspace_shown(ws: Workspace, prefs: &Preferences) -> bool {
    prefs.interface.show_unfinished || !is_unfinished_workspace(ws)
}

/// The built editor shown instead of an unfinished one.
pub fn substitute(kind: EditorKind) -> EditorKind {
    match kind {
        EditorKind::Sheet => EditorKind::Expressions,
        EditorKind::Script => EditorKind::Console,
        EditorKind::DependencyGraph => EditorKind::Viewport,
        other => other,
    }
}

/// The layout as displayed: a copy with unfinished editors swapped for
/// built ones unless `show_unfinished` (the stored layout never changes).
pub fn displayed_layout(layout: &Layout, show_unfinished: bool) -> Layout {
    let mut shown = layout.clone();
    if !show_unfinished {
        replace_unfinished(&mut shown);
    }
    shown
}

/// Swaps unfinished editors of `layout` for built ones; returns whether
/// anything changed.
fn replace_unfinished(layout: &mut Layout) -> bool {
    let mut changed = false;
    for (id, kind) in layout.leaves() {
        if is_unfinished(kind) {
            changed |= layout.set_editor(id, substitute(kind));
        }
    }
    changed
}

/// Keeps `widget` visible only while `shown(preferences)` holds.
pub fn bind_shown(
    widget: &impl IsA<gtk::Widget>,
    ctx: &AppCtx,
    shown: impl Fn(&Preferences) -> bool + 'static,
) {
    let widget = widget.as_ref().clone();
    widget.set_visible(ctx.preferences().with(&shown));
    ctx.preferences()
        .connect_changed_for(widget.downgrade(), move |w, p| w.set_visible(shown(p)));
}

/// Candidate symbolic icons, most preferred first. Icon themes differ
/// (Adwaita, Breeze, ...), so several names are tried.
fn icon_candidates(kind: EditorKind) -> &'static [&'static str] {
    match kind {
        EditorKind::Viewport => &[
            "office-chart-line-symbolic",
            "labplot-xy-curve-symbolic",
            "x-office-drawing-symbolic",
        ],
        EditorKind::SceneTree => &["view-list-tree-symbolic", "view-list-bullet-symbolic"],
        EditorKind::Inspector => &["document-properties-symbolic", "view-list-details-symbolic"],
        EditorKind::Console => &["utilities-terminal-symbolic"],
        EditorKind::Expressions => &[
            "view-list-ordered-symbolic",
            "format-list-ordered-symbolic",
            "view-list-symbolic",
        ],
        EditorKind::Sheet => &["x-office-spreadsheet-symbolic", "view-grid-symbolic"],
        EditorKind::Script => &[
            "format-text-code-symbolic",
            "code-context-symbolic",
            "text-x-script-symbolic",
        ],
        EditorKind::DependencyGraph => &[
            "distribute-graph-directed-symbolic",
            "network-workgroup-symbolic",
            "object-group-symbolic",
        ],
    }
}

/// Symbolic icon for an editor, picked from the current icon theme.
pub fn icon_name(kind: EditorKind) -> &'static str {
    pick_icon(icon_candidates(kind))
}

/// The first icon of `candidates` present in the icon theme (or the first
/// candidate when none is, so GTK shows its "missing" icon).
pub fn pick_icon(candidates: &'static [&'static str]) -> &'static str {
    let theme = gtk::gdk::Display::default().map(|d| gtk::IconTheme::for_display(&d));
    candidates
        .iter()
        .find(|name| theme.as_ref().is_some_and(|t| t.has_icon(name)))
        .or(candidates.first())
        .copied()
        .unwrap_or("image-missing")
}

/// An editor instance: its body widget and, for viewports, the handle.
pub struct Editor {
    pub kind: EditorKind,
    pub widget: gtk::Widget,
    pub viewport: Option<Viewport>,
    /// Builds editor-specific header widgets (fresh for every header).
    header: Option<Rc<dyn Fn() -> Vec<gtk::Widget>>>,
    /// Keeps the editor's controller alive as long as the editor.
    _state: Option<Rc<dyn Any>>,
}

impl Editor {
    /// Creates the editor; it follows the theme and preferences in `ctx`.
    pub fn new(kind: EditorKind, ctx: &AppCtx) -> Self {
        match kind {
            EditorKind::Viewport => {
                let viewport = Viewport::new(ctx);
                Self {
                    kind,
                    widget: viewport.widget().clone(),
                    viewport: Some(viewport),
                    header: None,
                    _state: None,
                }
            }
            EditorKind::Console => {
                let console = console::Console::new(ctx);
                let widget = console.widget();
                Self {
                    kind,
                    widget,
                    viewport: None,
                    header: Some(Rc::new(move || console.header_content())),
                    _state: None,
                }
            }
            EditorKind::Expressions => {
                let list = expressions::Expressions::new(ctx);
                let widget = list.widget();
                Self {
                    kind,
                    widget,
                    viewport: None,
                    header: Some(Rc::new(move || list.header_content())),
                    _state: None,
                }
            }
            EditorKind::SceneTree => {
                let tree = scene_tree::SceneTree::new(ctx);
                Self {
                    kind,
                    widget: tree.widget().clone(),
                    viewport: None,
                    header: None,
                    _state: Some(Rc::new(tree)),
                }
            }
            EditorKind::Inspector => {
                let inspector = inspector::Inspector::new(ctx);
                Self {
                    kind,
                    widget: inspector.widget().clone(),
                    viewport: None,
                    header: None,
                    _state: Some(Rc::new(inspector)),
                }
            }
            EditorKind::DependencyGraph => Self {
                kind,
                widget: canvas_placeholder(kind, ctx),
                viewport: None,
                header: None,
                _state: None,
            },
            _ => Self {
                kind,
                widget: placeholder(kind, ctx),
                viewport: None,
                header: None,
                _state: None,
            },
        }
    }

    /// Editor-specific widgets for the area header (menus, tools).
    pub fn header_content(&self) -> Vec<gtk::Widget> {
        if let Some(viewport) = &self.viewport {
            return viewport.header_widgets();
        }
        let mut widgets: Vec<gtk::Widget> = header_menus(self.kind)
            .iter()
            .map(|(title, items)| header_menu(title, items).upcast())
            .collect();
        if let Some(header) = &self.header {
            widgets.extend(header());
        }
        widgets
    }
}

/// Header menus per editor: title and (placeholder) item labels. Items have
/// no action yet, so they render insensitive.
fn header_menus(kind: EditorKind) -> &'static [(&'static str, &'static [&'static str])] {
    match kind {
        EditorKind::Viewport => &[], // built by the viewport itself
        // The scene tree and inspector have their own toolbars.
        EditorKind::SceneTree | EditorKind::Inspector => &[],
        // Console and Expressions put their tools in the header directly.
        EditorKind::Console | EditorKind::Expressions => &[],
        // Unfinished editors get their menus with their features (no
        // dead, insensitive menus meanwhile).
        EditorKind::Sheet | EditorKind::Script | EditorKind::DependencyGraph => &[],
    }
}

fn header_menu(title: &str, items: &[&str]) -> gtk::MenuButton {
    let menu = gio::Menu::new();
    for item in items {
        // An unregistered action makes the item insensitive.
        menu.append(Some(item), Some("win.not-implemented"));
    }
    gtk::MenuButton::builder()
        .label(title)
        .menu_model(&menu)
        .always_show_arrow(false)
        .css_classes(["header-menu", "flat"])
        .build()
}

/// The text block of an empty state: icon, name, description, quip (when
/// playful microcopy is on) and the milestone that will implement it.
fn empty_state(kind: EditorKind, ctx: &AppCtx) -> gtk::Box {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .margin_start(16)
        .margin_end(16)
        .build();
    let icon = gtk::Image::builder()
        .icon_name(icon_name(kind))
        .css_classes(["placeholder-icon"])
        .build();
    let title = gtk::Label::builder()
        .label(kind.title())
        .css_classes(["placeholder-title"])
        .build();
    let desc = gtk::Label::builder()
        .label(kind.description())
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(46)
        .css_classes(["placeholder-desc"])
        .build();
    let quip = gtk::Label::builder()
        .label(kind.quip())
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes(["placeholder-quip"])
        .visible(ctx.preferences().with(|p| p.interface.playful))
        .build();
    let weak_quip = quip.downgrade();
    ctx.preferences()
        .connect_changed_for(weak_quip, |quip, p| quip.set_visible(p.interface.playful));
    let milestone = gtk::Label::builder()
        .label("Coming in a later version")
        .tooltip_text(format!("Planned for milestone {}", kind.milestone()))
        .halign(gtk::Align::Center)
        .css_classes(["placeholder-quip", "coming-later"])
        .build();
    column.append(&icon);
    column.append(&title);
    column.append(&desc);
    column.append(&quip);
    column.append(&milestone);
    column
}

fn placeholder(kind: EditorKind, ctx: &AppCtx) -> gtk::Widget {
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .hexpand(true)
        .vexpand(true)
        .css_classes(["placeholder"])
        .child(&empty_state(kind, ctx))
        .build();
    scrolled.upcast()
}

/// Dependency Graph empty state: a hint of the infinite node canvas (dot
/// grid and a few capsules wired together) behind the text.
fn canvas_placeholder(kind: EditorKind, ctx: &AppCtx) -> gtk::Widget {
    let canvas = gtk::DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .build();
    let draw_ctx = ctx.downgrade();
    canvas.set_draw_func(move |_, cr, w, h| {
        if let Some(ctx) = draw_ctx.upgrade() {
            let colors = ctx.preferences().with(|p| p.effective_theme().dep_graph);
            draw_canvas_hint(cr, f64::from(w), f64::from(h), &colors);
        }
    });
    let weak_canvas = canvas.downgrade();
    ctx.preferences()
        .connect_changed_for(weak_canvas, |canvas, _| canvas.queue_draw());
    let overlay = gtk::Overlay::builder()
        .child(&canvas)
        .css_classes(["placeholder"])
        .build();
    let text = empty_state(kind, ctx);
    text.set_can_target(false);
    overlay.add_overlay(&text);
    overlay.upcast()
}

/// Sets `color` as the cairo source.
fn set(cr: &cairo::Context, color: Color) {
    let (r, g, b, a) = color.rgba_f64();
    cr.set_source_rgba(r, g, b, a);
}

fn draw_canvas_hint(cr: &cairo::Context, w: f64, h: f64, colors: &GraphColors) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    set(cr, colors.background);
    let _ = cr.paint();

    // Dot grid.
    const STEP: f64 = 18.0;
    set(cr, colors.dots);
    let mut y = (h / 2.0) % STEP;
    while y < h {
        let mut x = (w / 2.0) % STEP;
        while x < w {
            cr.rectangle(x.floor(), y.floor(), 1.0, 1.0);
            x += STEP;
        }
        y += STEP;
    }
    let _ = cr.fill();

    // Faint capsules around the text, as if nodes were drifting by.
    let (cx, cy) = (w / 2.0, h / 2.0);
    let capsules = [
        (cx - 300.0, cy - 150.0, 120.0),
        (cx + 190.0, cy - 120.0, 104.0),
        (cx - 250.0, cy + 110.0, 96.0),
        (cx + 160.0, cy + 130.0, 128.0),
    ];
    cr.set_line_width(1.0);
    set(cr, colors.wire);
    let wires = [(0, 1), (2, 3), (0, 3)];
    for (from, to) in wires {
        let (ax, ay, aw) = capsules[from];
        let (bx, by, _) = capsules[to];
        let (sx, sy) = (ax + aw, ay + 11.0);
        let (ex, ey) = (bx, by + 11.0);
        let dx = ((ex - sx).abs() / 2.0).max(40.0);
        cr.move_to(sx, sy);
        cr.curve_to(sx + dx, sy, ex - dx, ey, ex, ey);
        let _ = cr.stroke();
    }
    for (x, y, width) in capsules {
        capsule(cr, x + 0.5, y + 0.5, width, 22.0);
        set(cr, colors.capsule);
        let _ = cr.fill_preserve();
        set(cr, colors.capsule_border);
        let _ = cr.stroke();
        // Ports, the right one in the accent color.
        set(cr, colors.port);
        cr.arc(x, y + 11.0, 2.5, 0.0, 2.0 * PI);
        let _ = cr.fill();
        set(cr, colors.port_active);
        cr.arc(x + width, y + 11.0, 2.5, 0.0, 2.0 * PI);
        let _ = cr.fill();
    }
}

/// Adds a pill-shaped path (fully rounded ends) to `cr`.
fn capsule(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    let r = h / 2.0;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, PI / 2.0);
    cr.arc(x + r, y + r, r, PI / 2.0, 3.0 * PI / 2.0);
    cr.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::IdGen;

    #[test]
    fn unfinished_editors_are_swapped_for_built_ones() {
        for ws in Workspace::ALL {
            let mut layout = ws.default_layout(&mut IdGen::default());
            replace_unfinished(&mut layout);
            assert!(
                layout.leaves().iter().all(|(_, k)| !is_unfinished(*k)),
                "{ws:?}"
            );
            assert!(!replace_unfinished(&mut layout), "idempotent");
        }
        // View-only: the stored layout (what documents save) keeps its
        // Sheet area; only the displayed copy swaps it.
        let stored = Workspace::Data.default_layout(&mut IdGen::default());
        let before = serde_json::to_value(&stored).unwrap();
        let shown = displayed_layout(&stored, false);
        assert!(shown.leaves().iter().all(|(_, k)| !is_unfinished(*k)));
        assert_eq!(serde_json::to_value(&stored).unwrap(), before);
        assert!(stored.leaves().iter().any(|(_, k)| *k == EditorKind::Sheet));
        assert_eq!(displayed_layout(&stored, true), stored);
        let mut prefs = Preferences::default();
        assert!(!prefs.interface.show_unfinished, "off by default");
        assert!(!workspace_shown(Workspace::Data, &prefs));
        assert!(editor_shown(EditorKind::Console, &prefs));
        prefs.interface.show_unfinished = true;
        assert!(editor_shown(EditorKind::Sheet, &prefs));
    }
}

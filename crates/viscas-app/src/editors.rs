//! Editor widgets hosted by areas (REQUIREMENTS §2).
//!
//! Only the Viewport is functional so far; every other editor shows an
//! empty state describing what it will do and in which milestone.

use std::f64::consts::PI;

use gtk::prelude::*;
use gtk::{cairo, gio};

use crate::layout::EditorKind;
use crate::viewport::Viewport;

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
}

impl Editor {
    pub fn new(kind: EditorKind) -> Self {
        match kind {
            EditorKind::Viewport => {
                let viewport = Viewport::new();
                Self {
                    kind,
                    widget: viewport.widget().clone(),
                    viewport: Some(viewport),
                }
            }
            EditorKind::DependencyGraph => Self {
                kind,
                widget: canvas_placeholder(kind),
                viewport: None,
            },
            _ => Self {
                kind,
                widget: placeholder(kind),
                viewport: None,
            },
        }
    }

    /// Editor-specific widgets for the area header (menus, tools).
    pub fn header_content(&self) -> Vec<gtk::Widget> {
        let mut widgets: Vec<gtk::Widget> = header_menus(self.kind)
            .iter()
            .map(|(title, items)| header_menu(title, items).upcast())
            .collect();
        if let Some(viewport) = &self.viewport {
            let reset = gtk::Button::builder()
                .icon_name(pick_icon(&[
                    "zoom-original-symbolic",
                    "view-refresh-symbolic",
                ]))
                .tooltip_text("Reset View")
                .css_classes(["area-tool"])
                .build();
            let vp = viewport.clone();
            reset.connect_clicked(move |_| vp.reset_view());
            widgets.push(reset.upcast());
        }
        widgets
    }
}

/// Header menus per editor: title and (placeholder) item labels. Items have
/// no action yet, so they render insensitive.
fn header_menus(kind: EditorKind) -> &'static [(&'static str, &'static [&'static str])] {
    match kind {
        EditorKind::Viewport => &[
            ("View", &["Frame All", "Lock Aspect", "Polar Grid"]),
            ("Add", &["Function", "Point", "Slider"]),
        ],
        EditorKind::SceneTree => &[("Node", &["New Folder", "Rename", "Delete"])],
        EditorKind::Inspector => &[("Edit", &["Copy Style", "Paste Style"])],
        EditorKind::Console => &[("Session", &["Clear", "Copy Result", "Pin as Node"])],
        EditorKind::Expressions => &[("Edit", &["New Expression", "Sort"])],
        EditorKind::Sheet => &[("Table", &["Add Column", "Import CSV", "Export CSV"])],
        EditorKind::Script => &[("Script", &["Run", "Stop"])],
        EditorKind::DependencyGraph => {
            &[("View", &["Frame All", "Show Upstream", "Show Downstream"])]
        }
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

/// The text block of an empty state: icon, name, description, quip and the
/// milestone that will implement the editor.
fn empty_state(kind: EditorKind) -> gtk::Box {
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
        .build();
    let milestone = gtk::Label::builder()
        .label(format!("planned · {}", kind.milestone()))
        .halign(gtk::Align::Center)
        .css_classes(["milestone", "mono"])
        .build();
    column.append(&icon);
    column.append(&title);
    column.append(&desc);
    column.append(&quip);
    column.append(&milestone);
    column
}

fn placeholder(kind: EditorKind) -> gtk::Widget {
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .hexpand(true)
        .vexpand(true)
        .css_classes(["placeholder"])
        .child(&empty_state(kind))
        .build();
    scrolled.upcast()
}

/// Dependency Graph empty state: a hint of the infinite node canvas (dot
/// grid and a few capsules wired together) behind the text.
fn canvas_placeholder(kind: EditorKind) -> gtk::Widget {
    let canvas = gtk::DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .build();
    canvas.set_draw_func(|_, cr, w, h| draw_canvas_hint(cr, f64::from(w), f64::from(h)));
    let overlay = gtk::Overlay::builder()
        .child(&canvas)
        .css_classes(["placeholder"])
        .build();
    let text = empty_state(kind);
    text.set_can_target(false);
    overlay.add_overlay(&text);
    overlay.upcast()
}

fn draw_canvas_hint(cr: &cairo::Context, w: f64, h: f64) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    cr.set_source_rgb(0.098, 0.098, 0.098);
    let _ = cr.paint();

    // Dot grid.
    const STEP: f64 = 18.0;
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.07);
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
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.07);
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
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.025);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.09);
        let _ = cr.stroke();
        // Ports, the right one in the accent color.
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.16);
        cr.arc(x, y + 11.0, 2.5, 0.0, 2.0 * PI);
        let _ = cr.fill();
        cr.set_source_rgba(0.941, 0.533, 0.243, 0.45);
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

//! Area system widgets: builds a tree of `gtk::Paned` from the current
//! workspace's [`Layout`] and keeps it in sync with the model.
//!
//! The model is the source of truth. Structural edits (split, close, change
//! editor, maximize, switch workspace) edit the model and rebuild the widget
//! tree; editor widgets are cached per area so they keep their state (e.g. a
//! viewport's pan and zoom) across rebuilds. Divider drags only write the
//! split ratio back into the model.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gtk::glib;
use gtk::prelude::*;

use crate::editors::{self, Editor, pick_icon};
use crate::layout::{AreaId, Branch, Dir, EditorKind, Layout, Node, Path};
use crate::workspace::Workspace;

/// Callback run after workspace, layout or active-area changes.
type Listener = Box<dyn Fn(&Areas)>;

/// Handle to the area system. Cloning is cheap.
#[derive(Clone)]
pub struct Areas {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::Box,
    workspace: Cell<Workspace>,
    layouts: RefCell<HashMap<Workspace, Layout>>,
    editors: RefCell<HashMap<(Workspace, AreaId), Editor>>,
    /// Area widgets of the current tree, to toggle the `active` class.
    area_widgets: RefCell<HashMap<AreaId, gtk::Box>>,
    /// The area keyboard commands apply to (last hovered or clicked).
    active: Cell<Option<AreaId>>,
    rebuild_pending: Cell<bool>,
    listeners: RefCell<Vec<Listener>>,
}

impl Areas {
    pub fn new(workspace: Workspace) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .css_classes(["area-root"])
            .build();
        let areas = Self {
            inner: Rc::new(Inner {
                root,
                workspace: Cell::new(workspace),
                layouts: RefCell::default(),
                editors: RefCell::default(),
                area_widgets: RefCell::default(),
                active: Cell::new(None),
                rebuild_pending: Cell::new(false),
                listeners: RefCell::default(),
            }),
        };
        areas.rebuild();
        areas
    }

    fn downgrade(&self) -> WeakAreas {
        WeakAreas(Rc::downgrade(&self.inner))
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.inner.root.upcast_ref()
    }

    pub fn workspace(&self) -> Workspace {
        self.inner.workspace.get()
    }

    /// Calls `f` after every change of workspace, layout or active area.
    pub fn connect_changed(&self, f: impl Fn(&Areas) + 'static) {
        self.inner.listeners.borrow_mut().push(Box::new(f));
    }

    fn notify(&self) {
        for f in self.inner.listeners.borrow().iter() {
            f(self);
        }
    }

    /// Runs `f` on the current workspace's layout, creating it on first use.
    fn with_layout<R>(&self, f: impl FnOnce(&mut Layout) -> R) -> R {
        let ws = self.workspace();
        let mut layouts = self.inner.layouts.borrow_mut();
        f(layouts.entry(ws).or_insert_with(|| ws.default_layout()))
    }

    /// A copy of the current workspace's layout.
    pub fn layout(&self) -> Layout {
        self.with_layout(|l| l.clone())
    }

    /// The area commands apply to: the last hovered/clicked one if it is
    /// still part of the layout, else the first viewport, else the first area.
    pub fn active_area(&self) -> Option<AreaId> {
        self.with_layout(|l| {
            let leaves = l.leaves();
            self.inner
                .active
                .get()
                .filter(|id| l.contains(*id))
                .or_else(|| {
                    leaves
                        .iter()
                        .find(|(_, e)| *e == EditorKind::Viewport)
                        .map(|(id, _)| *id)
                })
                .or_else(|| leaves.first().map(|(id, _)| *id))
        })
    }

    pub fn active_editor(&self) -> Option<EditorKind> {
        let id = self.active_area()?;
        self.with_layout(|l| l.editor(id))
    }

    pub fn set_workspace(&self, workspace: Workspace) {
        if workspace == self.workspace() {
            return;
        }
        self.inner.workspace.set(workspace);
        self.inner.active.set(None);
        self.schedule_rebuild();
    }

    /// Splits `id` (or the active area) along `dir`.
    pub fn split(&self, id: Option<AreaId>, dir: Dir) {
        let Some(id) = id.or_else(|| self.active_area()) else {
            return;
        };
        if let Some(new) = self.with_layout(|l| l.split(id, dir)) {
            self.inner.active.set(Some(new));
            self.schedule_rebuild();
        }
    }

    /// Closes `id` (or the active area); its sibling takes the space.
    pub fn close(&self, id: Option<AreaId>) {
        let Some(id) = id.or_else(|| self.active_area()) else {
            return;
        };
        if self.with_layout(|l| l.close(id)) {
            self.inner
                .editors
                .borrow_mut()
                .remove(&(self.workspace(), id));
            self.schedule_rebuild();
        }
    }

    /// Maximizes `id` (or the active area), or restores the layout.
    pub fn toggle_maximize(&self, id: Option<AreaId>) {
        let Some(id) = id.or_else(|| self.active_area()) else {
            return;
        };
        if self.with_layout(|l| l.toggle_maximize(id)) {
            self.schedule_rebuild();
        }
    }

    /// Shows `kind` in `id` (or the active area).
    pub fn set_editor(&self, id: Option<AreaId>, kind: EditorKind) {
        let Some(id) = id.or_else(|| self.active_area()) else {
            return;
        };
        if self.with_layout(|l| l.editor(id) != Some(kind) && l.set_editor(id, kind)) {
            self.schedule_rebuild();
        }
    }

    /// Rebuilds on idle. Structural edits are often triggered by widgets
    /// inside the tree, which must not be destroyed while handling an event.
    fn schedule_rebuild(&self) {
        if self.inner.rebuild_pending.replace(true) {
            return;
        }
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(areas) = weak.upgrade() {
                areas.inner.rebuild_pending.set(false);
                areas.rebuild();
            }
        });
    }

    /// Replaces the widget tree with one built from the current layout.
    fn rebuild(&self) {
        let root = &self.inner.root;
        while let Some(child) = root.first_child() {
            root.remove(&child);
        }
        self.inner.area_widgets.borrow_mut().clear();

        let layout = self.layout();
        let tree = self.build_node(&layout, layout.visible_root(), Vec::new());
        root.append(&tree);

        // Drop cached editors of areas that no longer exist.
        let ws = self.workspace();
        self.inner
            .editors
            .borrow_mut()
            .retain(|(w, id), _| *w != ws || layout.contains(*id));

        self.refresh_active_class();
        self.notify();
    }

    fn build_node(&self, layout: &Layout, node: &Node, path: Path) -> gtk::Widget {
        match node {
            Node::Leaf { id, editor } => self.build_area(layout, *id, *editor),
            Node::Split { dir, a, b, .. } => {
                let mut path_a = path.clone();
                path_a.push(Branch::A);
                let mut path_b = path.clone();
                path_b.push(Branch::B);
                let start = self.build_node(layout, a, path_a);
                let end = self.build_node(layout, b, path_b);
                self.build_paned(*dir, &start, &end, path)
            }
        }
    }

    /// A paned whose divider position follows the model ratio at `path`
    /// and writes user drags back into it.
    fn build_paned(
        &self,
        dir: Dir,
        start: &gtk::Widget,
        end: &gtk::Widget,
        path: Path,
    ) -> gtk::Widget {
        let orientation = match dir {
            Dir::Horizontal => gtk::Orientation::Horizontal,
            Dir::Vertical => gtk::Orientation::Vertical,
        };
        let paned = gtk::Paned::builder()
            .orientation(orientation)
            .start_child(start)
            .end_child(end)
            .resize_start_child(true)
            .resize_end_child(true)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .wide_handle(false)
            .build();

        let path = Rc::new(path);
        // Divider position we last set ourselves (so its notification is not
        // mistaken for a drag), and the max position at that time: GTK
        // reports the new position before the new max on resizes, so
        // position changes while the max differs are resizes, not drags.
        let applied = Rc::new(Cell::new(None::<i32>));
        let last_max = Rc::new(Cell::new(-1));

        let weak = self.downgrade();
        paned.connect_max_position_notify(glib::clone!(
            #[strong]
            path,
            #[strong]
            applied,
            #[strong]
            last_max,
            move |paned| {
                let max = paned.max_position();
                last_max.set(max);
                let (Some(areas), Some(span)) = (weak.upgrade(), paned_span(paned)) else {
                    return;
                };
                let ratio = areas.with_layout(|l| l.ratio(&path)).unwrap_or(0.5);
                let pos = ((ratio * span).round() as i32).clamp(paned.min_position(), max);
                applied.set(Some(pos));
                paned.set_position(pos);
            }
        ));
        let weak = self.downgrade();
        paned.connect_position_notify(move |paned| {
            let pos = paned.position();
            let Some(last) = applied.get() else {
                return; // GTK's initial placement, before the model applies
            };
            if pos == last || paned.max_position() != last_max.get() {
                return;
            }
            if let (Some(areas), Some(span)) = (weak.upgrade(), paned_span(paned)) {
                applied.set(Some(pos));
                areas.with_layout(|l| l.set_ratio(&path, f64::from(pos) / span));
            }
        });
        paned.upcast()
    }

    fn build_area(&self, layout: &Layout, id: AreaId, kind: EditorKind) -> gtk::Widget {
        let ws = self.workspace();
        let mut editors = self.inner.editors.borrow_mut();
        let editor = editors
            .entry((ws, id))
            .and_modify(|e| {
                if e.kind != kind {
                    *e = Editor::new(kind);
                }
            })
            .or_insert_with(|| Editor::new(kind));
        detach(&editor.widget);

        let maximized = layout.maximized() == Some(id);
        let only_one = layout.leaves().len() == 1;

        let area = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["area"])
            .build();
        if maximized {
            area.add_css_class("maximized");
        }
        area.append(&self.build_header(editor, id, maximized, only_one));
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["area-body"])
            .build();
        editor.widget.set_hexpand(true);
        editor.widget.set_vexpand(true);
        body.append(&editor.widget);
        area.append(&body);

        self.track_active(&area, id);
        self.inner
            .area_widgets
            .borrow_mut()
            .insert(id, area.clone());
        area.upcast()
    }

    fn build_header(
        &self,
        editor: &Editor,
        id: AreaId,
        maximized: bool,
        only_one: bool,
    ) -> gtk::Widget {
        // Left part: editor switch and editor menus; scrolls when the area
        // is narrow instead of forcing a large minimum width.
        let menus = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(2)
            .build();
        menus.append(&self.editor_switch(editor.kind, id));
        menus.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        for widget in editor.header_content() {
            menus.append(&widget);
        }
        let scroller = gtk::ScrolledWindow::builder()
            .child(&menus)
            .hexpand(true)
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .build();

        // Right part: area tools, always visible.
        let tools = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .build();
        let tool = |icons: &'static [&'static str], tip: &str| {
            gtk::Button::builder()
                .icon_name(pick_icon(icons))
                .tooltip_text(tip)
                .css_classes(["area-tool"])
                .build()
        };
        let split_h = tool(
            &["view-split-left-right-symbolic", "view-dual-symbolic"],
            "Split Area Horizontally",
        );
        let split_v = tool(
            &["view-split-top-bottom-symbolic", "view-dual-symbolic"],
            "Split Area Vertically",
        );
        let maximize = if maximized {
            tool(&["view-restore-symbolic"], "Restore Layout (Ctrl+Space)")
        } else {
            tool(
                &["view-fullscreen-symbolic", "window-maximize-symbolic"],
                "Maximize Area (Ctrl+Space)",
            )
        };
        let close = tool(
            &["window-close-symbolic"],
            "Close Area (join into neighbour)",
        );
        close.add_css_class("close");
        close.set_sensitive(!only_one);

        let weak = self.downgrade();
        let act = move |f: fn(&Areas, AreaId)| {
            let weak = weak.clone();
            move |_: &gtk::Button| {
                if let Some(areas) = weak.upgrade() {
                    f(&areas, id);
                }
            }
        };
        split_h.connect_clicked(act(|a, id| a.split(Some(id), Dir::Horizontal)));
        split_v.connect_clicked(act(|a, id| a.split(Some(id), Dir::Vertical)));
        maximize.connect_clicked(act(|a, id| a.toggle_maximize(Some(id))));
        close.connect_clicked(act(|a, id| a.close(Some(id))));
        for button in [&split_h, &split_v, &maximize, &close] {
            tools.append(button);
        }

        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .css_classes(["area-header"])
            .build();
        header.append(&scroller);
        header.append(&tools);
        header.upcast()
    }

    /// Editor-type dropdown: current editor's icon and name, opening a list
    /// of all editors.
    fn editor_switch(&self, current: EditorKind, id: AreaId) -> gtk::MenuButton {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(5)
            .build();
        content.append(
            &gtk::Image::builder()
                .icon_name(editors::icon_name(current))
                .css_classes(["editor-icon"])
                .build(),
        );
        content.append(
            &gtk::Label::builder()
                .label(current.title())
                .css_classes(["editor-title"])
                .build(),
        );

        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(1)
            .css_classes(["editor-menu"])
            .build();
        list.append(
            &gtk::Label::builder()
                .label("EDITOR TYPE")
                .xalign(0.0)
                .css_classes(["menu-heading"])
                .build(),
        );
        let popover = gtk::Popover::builder()
            .child(&list)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .build();
        for kind in EditorKind::ALL {
            let row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .build();
            row.append(&gtk::Image::from_icon_name(editors::icon_name(kind)));
            row.append(
                &gtk::Label::builder()
                    .label(kind.title())
                    .xalign(0.0)
                    .build(),
            );
            let button = gtk::Button::builder()
                .child(&row)
                .tooltip_text(kind.description())
                .build();
            if kind == current {
                button.add_css_class("current");
            }
            let weak = self.downgrade();
            button.connect_clicked(glib::clone!(
                #[weak]
                popover,
                move |_| {
                    popover.popdown();
                    if let Some(areas) = weak.upgrade() {
                        areas.set_editor(Some(id), kind);
                    }
                }
            ));
            list.append(&button);
        }

        gtk::MenuButton::builder()
            .child(&content)
            .popover(&popover)
            .always_show_arrow(true)
            .tooltip_text("Editor Type")
            .css_classes(["editor-switch"])
            .build()
    }

    /// Makes `id` the active area when the pointer enters or clicks it.
    fn track_active(&self, area: &gtk::Box, id: AreaId) {
        let motion = gtk::EventControllerMotion::new();
        let weak = self.downgrade();
        motion.connect_enter(move |_, _, _| {
            if let Some(areas) = weak.upgrade() {
                areas.set_active(id);
            }
        });
        area.add_controller(motion);

        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        click.connect_pressed(move |_, _, _, _| {
            if let Some(areas) = weak.upgrade() {
                areas.set_active(id);
            }
        });
        area.add_controller(click);
    }

    fn set_active(&self, id: AreaId) {
        if self.inner.active.replace(Some(id)) != Some(id) {
            self.refresh_active_class();
            self.notify();
        }
    }

    fn refresh_active_class(&self) {
        let active = self.active_area();
        for (id, widget) in self.inner.area_widgets.borrow().iter() {
            if Some(*id) == active {
                widget.add_css_class("active");
            } else {
                widget.remove_css_class("active");
            }
        }
    }
}

#[derive(Clone)]
struct WeakAreas(Weak<Inner>);

impl WeakAreas {
    fn upgrade(&self) -> Option<Areas> {
        self.0.upgrade().map(|inner| Areas { inner })
    }
}

/// Removes a cached editor widget from its previous area body, if any.
fn detach(widget: &gtk::Widget) {
    if let Some(parent) = widget.parent() {
        match parent.downcast::<gtk::Box>() {
            Ok(body) => body.remove(widget),
            Err(_) => widget.unparent(),
        }
    }
}

/// Space shared by a paned's two children along its orientation (its size
/// minus the divider), or `None` before it is allocated.
fn paned_span(paned: &gtk::Paned) -> Option<f64> {
    let horizontal = paned.orientation() == gtk::Orientation::Horizontal;
    let extent = |w: &gtk::Widget| if horizontal { w.width() } else { w.height() };
    let mut handle = 0;
    let mut child = paned.first_child();
    while let Some(c) = child {
        if c.css_name() == "separator" {
            handle = extent(&c);
        }
        child = c.next_sibling();
    }
    let span = extent(paned.upcast_ref()) - handle;
    (span > 0).then_some(f64::from(span))
}

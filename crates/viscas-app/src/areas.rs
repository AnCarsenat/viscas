//! Area system widgets: builds a tree of `gtk::Paned` from the current
//! workspace's [`Layout`] and keeps it in sync with the model.
//!
//! The model is the source of truth. Structural edits (split, close, swap,
//! change editor, maximize, switch workspace) edit the model and rebuild the
//! widget tree on idle. Editor widgets are cached by area id, which is
//! globally unique (one [`IdGen`] for all workspaces), so they keep their
//! state (e.g. a viewport's pan and zoom) across rebuilds and swaps. Divider
//! drags write the ratio back into the model, addressed by stable split id.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gtk::glib;
use gtk::prelude::*;

use crate::commands::{Command, CommandProvider};
use crate::ctx::AppCtx;
use crate::editors::{self, Editor, pick_icon};
use crate::layout::{AreaId, Dir, EditorKind, IdGen, Layout, Node, SplitId};
use crate::workspace::Workspace;

/// Callback run after workspace, layout or active-area changes.
///
/// Listeners receive the [`Areas`] handle as argument and must not capture
/// an `Areas` strongly themselves: `Areas` owns its listeners, so that
/// would form a reference cycle. Capture widgets or weak handles instead.
type Listener = Rc<dyn Fn(&Areas)>;

/// Handle to the area system. Cloning is cheap.
#[derive(Clone)]
pub struct Areas {
    inner: Rc<Inner>,
}

struct Inner {
    ctx: AppCtx,
    root: gtk::Box,
    workspace: Cell<Workspace>,
    /// Id source shared by every workspace's layout.
    ids: RefCell<IdGen>,
    layouts: RefCell<HashMap<Workspace, Layout>>,
    editors: RefCell<HashMap<AreaId, Editor>>,
    /// Area widgets of the current tree, to toggle the `active` class.
    area_widgets: RefCell<HashMap<AreaId, gtk::Box>>,
    /// The area keyboard commands apply to (last hovered or clicked).
    active: Cell<Option<AreaId>>,
    rebuild_pending: Cell<bool>,
    listeners: RefCell<Vec<Listener>>,
}

impl Areas {
    pub fn new(ctx: &AppCtx, workspace: Workspace) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .css_classes(["area-root"])
            .build();
        let areas = Self {
            inner: Rc::new(Inner {
                ctx: ctx.clone(),
                root,
                workspace: Cell::new(workspace),
                ids: RefCell::default(),
                layouts: RefCell::default(),
                editors: RefCell::default(),
                area_widgets: RefCell::default(),
                active: Cell::new(None),
                rebuild_pending: Cell::new(false),
                listeners: RefCell::default(),
            }),
        };
        // "Show unfinished editors" off: hidden editors and workspaces are
        // swapped out on the next rebuild.
        let last = Cell::new(ctx.preferences().with(|p| p.interface.show_unfinished));
        ctx.preferences()
            .connect_changed_for(Rc::downgrade(&areas.inner), move |inner, p| {
                if last.replace(p.interface.show_unfinished) != p.interface.show_unfinished {
                    Areas { inner }.schedule_rebuild();
                }
            });
        areas.rebuild();
        areas
    }

    /// Hook for "Show unfinished editors": while off, an unfinished
    /// workspace falls back to Graph and unfinished editors in any layout
    /// are swapped for built ones (see [`editors::replace_unfinished`]).
    fn hide_unfinished(&self) {
        if self
            .inner
            .ctx
            .preferences()
            .with(|p| p.interface.show_unfinished)
        {
            return;
        }
        if editors::is_unfinished_workspace(self.workspace()) {
            self.inner.workspace.set(Workspace::Graph);
            self.inner.active.set(None);
        }
        for layout in self.inner.layouts.borrow_mut().values_mut() {
            editors::replace_unfinished(layout);
        }
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
    /// See [`Listener`] for the capture rules.
    pub fn connect_changed(&self, f: impl Fn(&Areas) + 'static) {
        self.inner.listeners.borrow_mut().push(Rc::new(f));
    }

    fn notify(&self) {
        // Clone first: a listener may connect listeners or edit the layout.
        let listeners = self.inner.listeners.borrow().clone();
        for f in listeners {
            f(self);
        }
    }

    /// Runs `f` on the current workspace's layout and the shared id source,
    /// creating the layout on first use.
    fn with_layout<R>(&self, f: impl FnOnce(&mut Layout, &mut IdGen) -> R) -> R {
        let ws = self.workspace();
        let mut ids = self.inner.ids.borrow_mut();
        let mut layouts = self.inner.layouts.borrow_mut();
        let layout = layouts
            .entry(ws)
            .or_insert_with(|| ws.default_layout(&mut ids));
        f(layout, &mut ids)
    }

    /// A copy of the current workspace's layout.
    pub fn layout(&self) -> Layout {
        self.with_layout(|l, _| l.clone())
    }

    /// The area commands apply to: the maximized area if there is one, else
    /// the last hovered/clicked area if it still exists, else the first
    /// viewport, else the first area.
    pub fn active_area(&self) -> Option<AreaId> {
        self.with_layout(|l, _| {
            if let Some(id) = l.maximized() {
                return Some(id);
            }
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
        self.with_layout(|l, _| l.editor(id))
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
        if let Some(new) = self.with_layout(|l, ids| l.split(id, dir, ids)) {
            self.inner.active.set(Some(new));
            self.schedule_rebuild();
        }
    }

    /// Closes `id` (or the active area); its sibling takes the space.
    pub fn close(&self, id: Option<AreaId>) {
        let Some(id) = id.or_else(|| self.active_area()) else {
            return;
        };
        if self.with_layout(|l, _| l.close(id)) {
            self.schedule_rebuild();
        }
    }

    /// Swaps `id` (or the active area) with `other`.
    pub fn swap(&self, id: Option<AreaId>, other: AreaId) {
        let Some(id) = id.or_else(|| self.active_area()) else {
            return;
        };
        if self.with_layout(|l, _| l.swap(id, other)) {
            self.schedule_rebuild();
        }
    }

    /// Maximizes `id` (or the active area), or restores the layout.
    pub fn toggle_maximize(&self, id: Option<AreaId>) {
        let Some(id) = id.or_else(|| self.active_area()) else {
            return;
        };
        if self.with_layout(|l, _| l.toggle_maximize(id)) {
            self.schedule_rebuild();
        }
    }

    /// Shows `kind` in `id` (or the active area).
    pub fn set_editor(&self, id: Option<AreaId>, kind: EditorKind) {
        let Some(id) = id.or_else(|| self.active_area()) else {
            return;
        };
        if self.with_layout(|l, _| l.editor(id) != Some(kind) && l.set_editor(id, kind)) {
            self.schedule_rebuild();
        }
    }

    /// The layout of every workspace plus the current workspace, as the
    /// document's layout blob.
    pub fn to_blob(&self) -> serde_json::Value {
        // Make sure the current workspace has a layout entry.
        self.with_layout(|_, _| ());
        let layouts = self.inner.layouts.borrow();
        let blob = LayoutBlob {
            version: LayoutBlob::VERSION,
            workspace: self.workspace().id().to_owned(),
            layouts: layouts
                .iter()
                .map(|(ws, l)| (ws.id().to_owned(), l.clone()))
                .collect(),
        };
        serde_json::to_value(blob).unwrap_or(serde_json::Value::Null)
    }

    /// Restores a blob written by [`Areas::to_blob`]. Unknown workspaces
    /// are skipped; returns `false` (and changes nothing) when the blob is
    /// not a valid layout.
    pub fn restore_blob(&self, value: &serde_json::Value) -> bool {
        let Ok(blob) = serde_json::from_value::<LayoutBlob>(value.clone()) else {
            return false;
        };
        let Some((workspace, layouts)) = blob.into_layouts() else {
            return false;
        };
        let mut ids = IdGen::default();
        for l in layouts.values() {
            ids.reserve(l);
        }
        *self.inner.ids.borrow_mut() = ids;
        *self.inner.layouts.borrow_mut() = layouts;
        // Area ids restart: cached editors would be matched to the wrong
        // areas. Dispose them outside the borrow.
        let dropped: Vec<Editor> = self
            .inner
            .editors
            .borrow_mut()
            .drain()
            .map(|(_, e)| e)
            .collect();
        drop(dropped);
        self.inner.workspace.set(workspace);
        self.inner.active.set(None);
        self.schedule_rebuild();
        true
    }

    /// Back to the default layouts, in the Graph workspace (new document,
    /// or a document without a usable layout).
    pub fn reset_default(&self) {
        *self.inner.ids.borrow_mut() = IdGen::default();
        self.inner.layouts.borrow_mut().clear();
        let dropped: Vec<Editor> = self
            .inner
            .editors
            .borrow_mut()
            .drain()
            .map(|(_, e)| e)
            .collect();
        drop(dropped);
        self.inner.workspace.set(Workspace::Graph);
        self.inner.active.set(None);
        self.schedule_rebuild();
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
        self.hide_unfinished();
        let root = &self.inner.root;
        while let Some(child) = root.first_child() {
            root.remove(&child);
        }
        self.inner.area_widgets.borrow_mut().clear();

        let layout = self.layout();
        let tree = self.build_node(&layout, layout.visible_root());
        root.append(&tree);

        // Drop cached editors of areas that exist in no workspace anymore.
        let live: Vec<AreaId> = self
            .inner
            .layouts
            .borrow()
            .values()
            .flat_map(|l| l.leaves().into_iter().map(|(id, _)| id))
            .collect();
        let dropped: Vec<Editor> = {
            let mut editors = self.inner.editors.borrow_mut();
            let dead: Vec<AreaId> = editors
                .keys()
                .filter(|id| !live.contains(id))
                .copied()
                .collect();
            dead.iter().filter_map(|id| editors.remove(id)).collect()
        };
        // Widgets are disposed outside the borrow, in case disposal runs
        // handlers that look at the cache.
        drop(dropped);

        self.refresh_active_class();
        self.notify();
    }

    fn build_node(&self, layout: &Layout, node: &Node) -> gtk::Widget {
        match node {
            Node::Leaf { id, editor } => self.build_area(layout, *id, *editor),
            Node::Split { id, dir, a, b, .. } => {
                let start = self.build_node(layout, a);
                let end = self.build_node(layout, b);
                self.build_paned(*dir, &start, &end, *id)
            }
        }
    }

    /// A paned whose divider follows the model ratio of split `split` and
    /// writes user drags back into it.
    fn build_paned(
        &self,
        dir: Dir,
        start: &gtk::Widget,
        end: &gtk::Widget,
        split: SplitId,
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

        // Divider position we last set ourselves (so its notification is not
        // mistaken for a drag), and the max position at that time: GTK
        // reports the new position before the new max on resizes, so
        // position changes while the max differs are resizes, not drags.
        let applied = Rc::new(Cell::new(None::<i32>));
        let last_max = Rc::new(Cell::new(-1));

        let weak = self.downgrade();
        paned.connect_max_position_notify(glib::clone!(
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
                let ratio = areas.with_layout(|l, _| l.ratio(split)).unwrap_or(0.5);
                // Not `Ord::clamp`: min may exceed max on tiny allocations.
                let pos = ((ratio * span).round() as i32)
                    .max(paned.min_position())
                    .min(max);
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
            let (Some(areas), Some(span)) = (weak.upgrade(), paned_span(paned)) else {
                return;
            };
            // This paned is about to be replaced; its state is stale.
            if areas.inner.rebuild_pending.get() {
                return;
            }
            applied.set(Some(pos));
            areas.with_layout(|l, _| l.set_ratio(split, f64::from(pos) / span));
        });
        paned.upcast()
    }

    fn build_area(&self, layout: &Layout, id: AreaId, kind: EditorKind) -> gtk::Widget {
        // Take the editor out of the cache so no borrow is held while
        // creating editors or building the header (which may re-enter).
        let cached = self.inner.editors.borrow_mut().remove(&id);
        let editor = match cached {
            Some(e) if e.kind == kind => e,
            _ => Editor::new(kind, &self.inner.ctx),
        };
        detach(&editor.widget);

        let maximized = layout.maximized() == Some(id);
        let area = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["area"])
            .build();
        if maximized {
            area.add_css_class("maximized");
        }
        area.append(&self.build_header(layout, &editor, id, maximized));
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
        self.inner.editors.borrow_mut().insert(id, editor);
        area.upcast()
    }

    fn build_header(
        &self,
        layout: &Layout,
        editor: &Editor,
        id: AreaId,
        maximized: bool,
    ) -> gtk::Widget {
        // Left part: editor switch and editor menus; scrolls when the area
        // is narrow instead of forcing a large minimum width.
        let menus = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(2)
            .build();
        menus.append(&self.editor_switch(layout, editor.kind, id, maximized));
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
        close.set_sensitive(layout.leaves().len() > 1);

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
    /// of all editors and, below, the other areas to swap places with.
    fn editor_switch(
        &self,
        layout: &Layout,
        current: EditorKind,
        id: AreaId,
        maximized: bool,
    ) -> gtk::MenuButton {
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
        let popover = gtk::Popover::builder()
            .child(&list)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .build();
        let heading = |text: &str| {
            gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .css_classes(["menu-heading"])
                .build()
        };
        let weak = self.downgrade();
        let item = |icon: &str, label: &str, tip: &str, run: Box<dyn Fn(&Areas)>| {
            let row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .build();
            row.append(&gtk::Image::from_icon_name(icon));
            row.append(&gtk::Label::builder().label(label).xalign(0.0).build());
            let button = gtk::Button::builder().child(&row).tooltip_text(tip).build();
            let weak = weak.clone();
            button.connect_clicked(glib::clone!(
                #[weak]
                popover,
                move |_| {
                    popover.popdown();
                    if let Some(areas) = weak.upgrade() {
                        run(&areas);
                    }
                }
            ));
            button
        };

        list.append(&heading("EDITOR TYPE"));
        let prefs = self.inner.ctx.preferences().get();
        for kind in EditorKind::ALL {
            if !editors::editor_shown(kind, &prefs) && kind != current {
                continue;
            }
            let button = item(
                editors::icon_name(kind),
                kind.title(),
                kind.description(),
                Box::new(move |a| a.set_editor(Some(id), kind)),
            );
            if kind == current {
                button.add_css_class("current");
            }
            list.append(&button);
        }

        // Swapping needs the other areas on screen.
        let others: Vec<(usize, AreaId, EditorKind)> = layout
            .leaves()
            .into_iter()
            .enumerate()
            .filter(|(_, (other, _))| *other != id)
            .map(|(pos, (other, kind))| (pos + 1, other, kind))
            .collect();
        if !maximized && !others.is_empty() {
            list.append(&heading("SWAP WITH"));
            for (pos, other, kind) in others {
                list.append(&item(
                    editors::icon_name(kind),
                    &format!("{}  ·  area {pos}", kind.title()),
                    "Exchange the places of the two areas",
                    Box::new(move |a| a.swap(Some(id), other)),
                ));
            }
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

/// Serialized layouts of a document (see [`Areas::to_blob`]).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LayoutBlob {
    pub version: u32,
    pub workspace: String,
    pub layouts: std::collections::BTreeMap<String, Layout>,
}

impl LayoutBlob {
    pub const VERSION: u32 = 1;

    /// Validated workspace and layouts; `None` for a future version, an
    /// unknown current workspace or ids shared between workspaces (ids are
    /// global, editors are cached by area id).
    pub fn into_layouts(self) -> Option<(Workspace, HashMap<Workspace, Layout>)> {
        if self.version > Self::VERSION {
            return None;
        }
        let mut ids: Vec<u32> = self.layouts.values().flat_map(|l| l.ids()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() != n {
            return None;
        }
        let workspace = Workspace::from_id(&self.workspace)?;
        let layouts = self
            .layouts
            .into_iter()
            .filter_map(|(id, l)| Some((Workspace::from_id(&id)?, l)))
            .collect();
        Some((workspace, layouts))
    }
}

#[derive(Clone)]
struct WeakAreas(Weak<Inner>);

impl WeakAreas {
    fn upgrade(&self) -> Option<Areas> {
        self.0.upgrade().map(|inner| Areas { inner })
    }
}

/// Palette commands that depend on the current layout: swapping the active
/// area with each other area.
pub struct AreaCommands(WeakAreas);

impl AreaCommands {
    pub fn new(areas: &Areas) -> Self {
        Self(areas.downgrade())
    }
}

impl CommandProvider for AreaCommands {
    fn commands(&self) -> Vec<Command> {
        let Some(areas) = self.0.upgrade() else {
            return Vec::new();
        };
        let layout = areas.layout();
        let Some(active) = areas.active_area() else {
            return Vec::new();
        };
        if layout.maximized().is_some() {
            return Vec::new();
        }
        layout
            .leaves()
            .into_iter()
            .enumerate()
            .filter(|(_, (id, _))| *id != active)
            .map(|(pos, (other, kind))| {
                let weak = areas.downgrade();
                Command {
                    id: format!("area.swap::{}", other.0),
                    title: format!("Swap Area With: {} (area {})", kind.title(), pos + 1),
                    category: "Area",
                    shortcut: None,
                    run: Rc::new(move || {
                        if let Some(areas) = weak.upgrade() {
                            areas.swap(Some(active), other);
                        }
                    }),
                }
            })
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_blob_round_trips_and_rejects_bad_input() {
        let mut ids = IdGen::default();
        let mut layouts = std::collections::BTreeMap::new();
        for ws in Workspace::ALL {
            layouts.insert(ws.id().to_owned(), ws.default_layout(&mut ids));
        }
        let blob = LayoutBlob {
            version: LayoutBlob::VERSION,
            workspace: "data".into(),
            layouts,
        };
        let json = serde_json::to_value(&blob).unwrap();
        let back: LayoutBlob = serde_json::from_value(json).unwrap();
        let (ws, map) = back.into_layouts().unwrap();
        assert_eq!(ws, Workspace::Data);
        assert_eq!(map.len(), 4);
        assert_eq!(
            map[&Workspace::Graph],
            Workspace::Graph.default_layout(&mut IdGen::default())
        );

        let future = LayoutBlob {
            version: 99,
            workspace: "graph".into(),
            layouts: Default::default(),
        };
        assert!(future.into_layouts().is_none());
        let unknown = LayoutBlob {
            version: 1,
            workspace: "nope".into(),
            layouts: Default::default(),
        };
        assert!(unknown.into_layouts().is_none());

        // The same layout (same ids) in two workspaces is rejected.
        let l = Workspace::Graph.default_layout(&mut IdGen::default());
        let dup = LayoutBlob {
            version: 1,
            workspace: "graph".into(),
            layouts: [("graph".to_owned(), l.clone()), ("data".to_owned(), l)]
                .into_iter()
                .collect(),
        };
        assert!(dup.into_layouts().is_none());
    }
}

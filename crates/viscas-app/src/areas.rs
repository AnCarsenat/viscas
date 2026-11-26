//! Area system widgets: builds the area trees of the current workspace
//! (main window plus floating windows) from its [`WorkspaceLayout`] and
//! keeps them in sync with the model.
//!
//! The model is the source of truth. Structural edits (split, close, join,
//! dock, tear off, swap, change editor, maximize, switch workspace) edit
//! the model and rebuild the widget trees on idle. Editor widgets are
//! cached by area id, which is globally unique (one [`IdGen`] for all
//! workspaces and windows), so they keep their state across rebuilds,
//! swaps and moves between windows. Divider drags write the ratio back
//! into the model, addressed by stable split id.
//!
//! Docking interactions (see [`crate::docking`]):
//! - corner grips: drag inward to split, outward into a neighbour to join;
//! - header drag (DnD): drop zones on areas and window edges, or tear off
//!   when dropped outside every window;
//! - every structural edit is recorded in the layout history.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib, graphene};

use crate::commands::{Command, CommandProvider};
use crate::ctx::AppCtx;
use crate::docking::model::{
    self, CenterMode, DockTarget, Dragged, DropZone, FLOAT_DEFAULT, Floating, Host, LayoutHistory,
    MIN_AREA, Refusal, WindowId, WorkspaceLayout,
};
use crate::docking::overlay::{self, Preview, Toast};
use crate::docking::widgets::{self, SashEvent};
use crate::editors::{self, Editor, pick_icon};
use crate::layout::{AreaId, Dir, EditorKind, IdGen, Layout, Node, Rect, SplitId, TabId, Tabs};
use crate::workspace::Workspace;

/// Callback run after workspace, layout, active or hovered area changes.
///
/// Listeners receive the [`Areas`] handle as argument and must not capture
/// an `Areas` strongly themselves: `Areas` owns its listeners, so that
/// would form a reference cycle. Capture widgets or weak handles instead.
type Listener = Rc<dyn Fn(&Areas)>;

/// Every workspace's layout and the current workspace: one layout undo
/// step.
type Snapshot = (Workspace, HashMap<Workspace, WorkspaceLayout>);

/// A resolved drop: target, preview and the area whose header to
/// highlight (none for window edges).
type ResolvedDrop = (DockTarget, Preview, Option<AreaId>);

/// Steps kept in the layout history.
const LAYOUT_UNDO_STEPS: usize = 64;

/// Drag payload of an area header or tab: the id and whether it is a
/// tab. A private type, so text fields never accept it.
#[derive(Clone, Copy, Debug, glib::Boxed)]
#[boxed_type(name = "ViscasAreaDrag")]
struct AreaDrag(u32, bool);

impl AreaDrag {
    fn new(item: Dragged) -> Self {
        match item {
            Dragged::Area(a) => AreaDrag(a.0, false),
            Dragged::Tab(t) => AreaDrag(t.0, true),
        }
    }

    fn item(self) -> Dragged {
        if self.1 {
            Dragged::Tab(TabId(self.0))
        } else {
            Dragged::Area(AreaId(self.0))
        }
    }
}

/// Frame (overlay with grips), area box and tab strip (with two or more
/// tabs) of a visible area.
type AreaParts = (gtk::Overlay, gtk::Box, Option<gtk::Box>);

/// Handle to the area system. Cloning is cheap.
#[derive(Clone)]
pub struct Areas {
    inner: Rc<Inner>,
}

struct Inner {
    ctx: AppCtx,
    main: Rc<HostView>,
    floating: RefCell<HashMap<WindowId, FloatWin>>,
    workspace: Cell<Workspace>,
    /// Id source shared by every workspace's layouts and windows.
    ids: RefCell<IdGen>,
    layouts: RefCell<HashMap<Workspace, WorkspaceLayout>>,
    /// Editors by tab: each tab keeps its editor state.
    editors: RefCell<HashMap<TabId, Editor>>,
    /// The active area: last clicked.
    active: Cell<Option<AreaId>>,
    /// The area under the pointer, if any.
    hovered: Cell<Option<AreaId>>,
    rebuild_pending: Cell<bool>,
    listeners: RefCell<Vec<Listener>>,
    history: RefCell<LayoutHistory<Snapshot>>,
    /// State before the current divider drag.
    sash_before: RefCell<Option<Snapshot>>,
    /// Area being dragged by its header.
    dragging: Cell<Option<Dragged>>,
    /// Whether the current header drag was dropped on a window.
    dropped: Cell<bool>,
    main_target_installed: Cell<bool>,
    hidden_tabs: RefCell<Vec<Workspace>>,
    toast: Rc<Toast>,
}

/// The widgets of one area tree (main window or a floating window).
struct HostView {
    overlay: gtk::Overlay,
    tree: gtk::Box,
    layer: gtk::DrawingArea,
    preview: Rc<Cell<Preview>>,
    /// Frame (overlay with grips) and area box of each visible area.
    parts: RefCell<HashMap<AreaId, AreaParts>>,
    highlighted: Cell<Option<AreaId>>,
}

impl HostView {
    fn new() -> Rc<Self> {
        let tree = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .css_classes(["area-root"])
            .build();
        let overlay = gtk::Overlay::builder()
            .child(&tree)
            .hexpand(true)
            .vexpand(true)
            .build();
        let (layer, preview) = overlay::layer();
        overlay.add_overlay(&layer);
        Rc::new(Self {
            overlay,
            tree,
            layer,
            preview,
            parts: RefCell::default(),
            highlighted: Cell::new(None),
        })
    }

    fn set_preview(&self, p: Preview) {
        if self.preview.get() != p {
            self.preview.set(p);
            self.layer.queue_draw();
        }
    }

    fn rect_of(&self, id: AreaId) -> Option<Rect> {
        let parts = self.parts.borrow();
        let (frame, _, _) = parts.get(&id)?;
        let b = frame.compute_bounds(&self.overlay)?;
        Some(Rect::new(
            f64::from(b.x()),
            f64::from(b.y()),
            f64::from(b.width()),
            f64::from(b.height()),
        ))
    }

    fn area_at(&self, x: f64, y: f64) -> Option<(AreaId, Rect)> {
        let ids: Vec<AreaId> = self.parts.borrow().keys().copied().collect();
        ids.into_iter()
            .filter_map(|id| Some((id, self.rect_of(id)?)))
            .find(|(_, r)| r.contains(x, y))
    }

    /// Marks `id` as the drop target (accent header border).
    fn highlight(&self, id: Option<AreaId>) {
        let old = self.highlighted.replace(id);
        if old == id {
            return;
        }
        let parts = self.parts.borrow();
        if let Some((_, area, _)) = old.and_then(|o| parts.get(&o)) {
            area.remove_css_class("drop-target");
        }
        if let Some((_, area, _)) = id.and_then(|n| parts.get(&n)) {
            area.add_css_class("drop-target");
        }
    }

    /// The tab strip under `(x, y)`: its area, the insertion index and
    /// the insertion line `(x, y0, y1)`.
    fn strip_at(&self, x: f64, y: f64) -> Option<(AreaId, usize, (f64, f64, f64))> {
        let parts = self.parts.borrow();
        for (id, (_, _, strip)) in parts.iter() {
            let Some(strip) = strip else { continue };
            let Some(b) = strip.compute_bounds(&self.overlay) else {
                continue;
            };
            let r = Rect::new(
                f64::from(b.x()),
                f64::from(b.y()),
                f64::from(b.width()),
                f64::from(b.height()),
            );
            if !r.contains(x, y) {
                continue;
            }
            let mut index = 0;
            let mut line = r.x + 2.0;
            let mut c = strip.first_child();
            while let Some(tab) = c {
                c = tab.next_sibling();
                let Some(tb) = tab.compute_bounds(&self.overlay) else {
                    continue;
                };
                let (tx, tw) = (f64::from(tb.x()), f64::from(tb.width()));
                if x > tx + tw / 2.0 {
                    index += 1;
                    line = tx + tw + 1.0;
                } else {
                    line = tx - 1.0;
                    break;
                }
            }
            return Some((*id, index, (line, r.y + 2.0, r.y + r.h - 2.0)));
        }
        None
    }

    fn clear_feedback(&self) {
        self.set_preview(Preview::None);
        self.highlight(None);
    }
}

struct FloatWin {
    window: gtk::Window,
    view: Rc<HostView>,
}

impl Areas {
    pub fn new(ctx: &AppCtx, workspace: Workspace) -> Self {
        let main = HostView::new();
        let toast = Toast::new();
        main.overlay.add_overlay(toast.widget());
        let areas = Self {
            inner: Rc::new(Inner {
                ctx: ctx.clone(),
                main,
                floating: RefCell::default(),
                workspace: Cell::new(workspace),
                ids: RefCell::default(),
                layouts: RefCell::default(),
                editors: RefCell::default(),
                active: Cell::new(None),
                hovered: Cell::new(None),
                rebuild_pending: Cell::new(false),
                listeners: RefCell::default(),
                history: RefCell::new(LayoutHistory::new(LAYOUT_UNDO_STEPS)),
                sash_before: RefCell::default(),
                dragging: Cell::new(None),
                dropped: Cell::new(false),
                main_target_installed: Cell::new(false),
                hidden_tabs: RefCell::default(),
                toast,
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
        let weak = areas.downgrade();
        areas.inner.toast.connect_undo(move || {
            if let Some(a) = weak.upgrade() {
                a.undo_layout();
            }
        });
        // Once in a window: accept header drops and create floating
        // windows (which need the main window as parent).
        let weak = areas.downgrade();
        areas.inner.main.overlay.connect_map(move |overlay| {
            let Some(areas) = weak.upgrade() else {
                return;
            };
            if !areas.inner.main_target_installed.replace(true)
                && let Some(window) = overlay.root().and_downcast::<gtk::Window>()
            {
                areas.install_drop_target(&window, Host::Main);
            }
            areas.schedule_rebuild();
        });
        areas.rebuild();
        areas
    }

    /// "Show unfinished editors" is off: unfinished workspaces and
    /// editors are only *displayed* as built ones (view-only; the stored
    /// layouts and workspace, and so saved documents, keep them).
    fn hiding_unfinished(&self) -> bool {
        !self
            .inner
            .ctx
            .preferences()
            .with(|p| p.interface.show_unfinished)
    }

    fn downgrade(&self) -> WeakAreas {
        WeakAreas(Rc::downgrade(&self.inner))
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.inner.main.overlay.upcast_ref()
    }

    /// The workspace on screen (Graph stands in for an unfinished one
    /// while those are hidden).
    pub fn workspace(&self) -> Workspace {
        let ws = self.inner.workspace.get();
        if editors::is_unfinished_workspace(ws) && self.hiding_unfinished() {
            Workspace::Graph
        } else {
            ws
        }
    }

    /// Calls `f` after every change of workspace, layout, active or
    /// hovered area. See [`Listener`] for the capture rules.
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
    fn with_ws<R>(&self, f: impl FnOnce(&mut WorkspaceLayout, &mut IdGen) -> R) -> R {
        let ws = self.workspace();
        let mut ids = self.inner.ids.borrow_mut();
        let mut layouts = self.inner.layouts.borrow_mut();
        let layout = layouts
            .entry(ws)
            .or_insert_with(|| WorkspaceLayout::new(ws.default_layout(&mut ids)));
        f(layout, &mut ids)
    }

    fn ws_layout(&self) -> WorkspaceLayout {
        self.with_ws(|w, _| w.clone())
    }

    fn view(&self, host: Host) -> Option<Rc<HostView>> {
        match host {
            Host::Main => Some(self.inner.main.clone()),
            Host::Floating(w) => self.inner.floating.borrow().get(&w).map(|f| f.view.clone()),
        }
    }

    /// Whether `id` is on screen (not hidden by a maximized area).
    fn is_visible(w: &WorkspaceLayout, id: AreaId) -> bool {
        match w.host_of(id) {
            Some(Host::Main) => w.main.maximized().is_none_or(|m| m == id),
            Some(host @ Host::Floating(_)) => w
                .layout(host)
                .is_some_and(|l| l.maximized().is_none_or(|m| m == id)),
            None => false,
        }
    }

    /// The active area: the last clicked area if it is still visible,
    /// else the maximized area, else the first viewport, else the first
    /// area.
    pub fn active_area(&self) -> Option<AreaId> {
        self.with_ws(|w, _| {
            let leaves = w.leaves();
            self.inner
                .active
                .get()
                .filter(|id| Self::is_visible(w, *id))
                .or(w.main.maximized())
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
        self.with_ws(|w, _| w.editor(id))
    }

    /// The area menu and palette actions apply to: the area under the
    /// pointer if there is one, else the active area.
    pub fn target_area(&self) -> Option<AreaId> {
        let hovered = self.inner.hovered.get();
        hovered
            .filter(|id| self.with_ws(|w, _| Self::is_visible(w, *id)))
            .or_else(|| self.active_area())
    }

    /// Editor of [`Areas::target_area`], for labels like "Close Area
    /// (Viewport)".
    pub fn target_editor(&self) -> Option<EditorKind> {
        let id = self.target_area()?;
        self.with_ws(|w, _| w.editor(id))
    }

    pub fn set_workspace(&self, workspace: Workspace) {
        if workspace == self.inner.workspace.get() {
            return;
        }
        self.inner.workspace.set(workspace);
        self.inner.active.set(None);
        self.inner.hovered.set(None);
        self.schedule_rebuild();
        // Tabs follow right away, not only after the rebuild.
        self.notify();
    }

    #[allow(dead_code, reason = "hook for the unfinished-editors preference")]
    /// Hides or shows a workspace's tab in the top bar (the workspace
    /// itself stays usable, e.g. from the palette). Listeners are notified.
    pub fn set_workspace_visible(&self, workspace: Workspace, visible: bool) {
        let changed = {
            let mut hidden = self.inner.hidden_tabs.borrow_mut();
            let was = !hidden.contains(&workspace);
            if visible {
                hidden.retain(|w| *w != workspace);
            } else if was {
                hidden.push(workspace);
            }
            was != visible
        };
        if changed {
            self.notify();
        }
    }

    /// Whether the workspace's tab is shown (see
    /// [`Areas::set_workspace_visible`]).
    pub fn workspace_visible(&self, workspace: Workspace) -> bool {
        !self.inner.hidden_tabs.borrow().contains(&workspace)
    }

    fn snapshot(&self) -> Snapshot {
        self.with_ws(|_, _| ());
        (self.workspace(), self.inner.layouts.borrow().clone())
    }

    fn record(&self, before: Snapshot) {
        self.inner.history.borrow_mut().push(before);
    }

    /// Runs a layout edit on the current workspace. On success it is
    /// recorded for layout undo, the trees are rebuilt and, with `toast`,
    /// a toast with an Undo button is shown; refusals show their reason.
    fn edit<R>(
        &self,
        toast: Option<&str>,
        f: impl FnOnce(&mut WorkspaceLayout, &mut IdGen) -> Result<R, Refusal>,
    ) -> Option<R> {
        let before = self.snapshot();
        match self.with_ws(f) {
            Ok(r) => {
                self.record(before);
                self.schedule_rebuild();
                if let Some(text) = toast {
                    self.inner.toast.show(&format!("{text}  ·"), true);
                }
                Some(r)
            }
            Err(refusal) => {
                if let Some(msg) = refusal.message() {
                    self.inner.toast.show(msg, false);
                }
                None
            }
        }
    }

    fn resolve(&self, id: Option<AreaId>) -> Option<AreaId> {
        id.or_else(|| self.target_area())
    }

    /// Splits `id` (or the target area) in half along `dir`.
    pub fn split(&self, id: Option<AreaId>, dir: Dir) {
        self.split_at(id, dir, 0.5);
    }

    fn split_at(&self, id: Option<AreaId>, dir: Dir, ratio: f64) {
        let Some(id) = self.resolve(id) else {
            return;
        };
        let new = self.edit(None, |w, ids| {
            let host = w.host_of(id).ok_or(Refusal::Invalid)?;
            let layout = w.layout_mut(host).ok_or(Refusal::Invalid)?;
            layout.split_at(id, dir, ratio, ids).ok_or(Refusal::Invalid)
        });
        if let Some(new) = new {
            self.inner.active.set(Some(new));
        }
    }

    /// Closes `id` (or the target area); its sibling takes the space.
    pub fn close(&self, id: Option<AreaId>) {
        if let Some(id) = self.resolve(id) {
            self.edit(Some("Area closed"), |w, _| w.close_area(id));
        }
    }

    /// Joins `absorb` into its neighbour `keep`.
    fn join(&self, keep: AreaId, absorb: AreaId) {
        self.edit(Some("Areas joined"), |w, _| w.join(keep, absorb));
    }

    /// Swaps `id` (or the target area) with `other`.
    pub fn swap(&self, id: Option<AreaId>, other: AreaId) {
        if let Some(id) = self.resolve(id) {
            self.edit(None, |w, _| w.swap(id, other));
        }
    }

    fn dock(&self, item: Dragged, target: DockTarget) {
        let toast = match (item, target) {
            (_, DockTarget::Strip { .. }) => None,
            (
                _,
                DockTarget::Area {
                    zone: DropZone::Center,
                    mode: CenterMode::Tab,
                    ..
                },
            ) => Some("Docked as a tab"),
            _ => Some("Area moved"),
        };
        self.edit(toast, |w, ids| match item {
            Dragged::Area(src) => w.dock(src, target, ids),
            Dragged::Tab(tab) => w.dock_tab(tab, target, ids),
        });
    }

    fn shown_tab(&self, id: AreaId) -> Option<TabId> {
        self.with_ws(|w, _| w.tabs(id).map(|t| t.active_tab().id))
    }

    /// Shows tab number `index` of area `id` (recorded for layout undo).
    pub fn select_tab(&self, id: AreaId, index: usize) {
        self.inner.active.set(Some(id));
        self.edit(None, |w, _| w.select_tab(id, index));
    }

    /// Closes a tab; an area's last tab closes the area.
    pub fn close_tab(&self, tab: TabId) {
        self.edit(Some("Tab closed"), |w, _| w.close_tab(tab));
    }

    /// Shows the next (`delta` 1) or previous (-1) tab of the target area.
    pub fn cycle_tab(&self, delta: isize) {
        if let Some(id) = self.target_area() {
            self.edit(None, |w, _| w.cycle_tab(id, delta));
        }
    }

    /// Moves a tab into its own floating window.
    pub fn tear_off_tab(&self, tab: TabId) {
        let size = self
            .with_ws(|w, _| w.area_of_tab(tab))
            .and_then(|a| {
                let host = self.with_ws(|w, _| w.host_of(a))?;
                self.view(host)?.rect_of(a)
            })
            .map_or(FLOAT_DEFAULT, |r| (r.w as i32, r.h as i32 + 28));
        self.edit(Some("Opened in a new window"), |w, ids| {
            w.tear_off_tab(tab, size, ids)
        });
    }

    /// Moves `id` (or the target area) into its own floating window.
    pub fn tear_off(&self, id: Option<AreaId>) {
        let Some(id) = self.resolve(id) else {
            return;
        };
        let size = self
            .inner
            .main
            .rect_of(id)
            .or_else(|| {
                self.with_ws(|w, _| w.host_of(id))
                    .and_then(|h| self.view(h)?.rect_of(id))
            })
            .map_or(FLOAT_DEFAULT, |r| (r.w as i32, r.h as i32 + 28));
        self.edit(Some("Opened in a new window"), |w, ids| {
            w.tear_off(id, size, ids)
        });
    }

    fn close_window(&self, id: WindowId) {
        self.sync_window_sizes();
        self.edit(Some("Window closed"), |w, _| w.close_window(id));
    }

    /// Maximizes `id` (or the target area), or restores the layout.
    pub fn toggle_maximize(&self, id: Option<AreaId>) {
        let Some(id) = self.resolve(id) else {
            return;
        };
        let changed = self.with_ws(|w, _| {
            let Some(host) = w.host_of(id) else {
                return false;
            };
            w.layout_mut(host).is_some_and(|l| l.toggle_maximize(id))
        });
        if changed {
            self.schedule_rebuild();
        }
    }

    /// Shows `kind` in `id` (or the target area).
    pub fn set_editor(&self, id: Option<AreaId>, kind: EditorKind) {
        let Some(id) = self.resolve(id) else {
            return;
        };
        if self.with_ws(|w, _| w.editor(id)) == Some(kind) {
            return;
        }
        self.edit(None, |w, _| {
            let host = w.host_of(id).ok_or(Refusal::Invalid)?;
            let layout = w.layout_mut(host).ok_or(Refusal::Invalid)?;
            if layout.set_editor(id, kind) {
                Ok(())
            } else {
                Err(Refusal::Invalid)
            }
        });
    }

    /// Puts `ws` (the current workspace if `None`) back to its default
    /// layout, closing its floating windows.
    pub fn reset_workspace(&self, ws: Option<Workspace>) {
        let ws = ws.unwrap_or_else(|| self.workspace());
        let before = self.snapshot();
        {
            let mut ids = self.inner.ids.borrow_mut();
            let fresh = WorkspaceLayout::new(ws.default_layout(&mut ids));
            self.inner.layouts.borrow_mut().insert(ws, fresh);
        }
        self.record(before);
        self.inner
            .toast
            .show(&format!("{} layout reset  ·", ws.title()), true);
        self.schedule_rebuild();
    }

    /// Puts every workspace back to its default layout.
    pub fn reset_all_workspaces(&self) {
        let before = self.snapshot();
        self.inner.layouts.borrow_mut().clear();
        self.record(before);
        self.inner
            .toast
            .show("All workspace layouts reset  ·", true);
        self.schedule_rebuild();
    }

    pub fn can_undo_layout(&self) -> bool {
        self.inner.history.borrow().can_undo()
    }

    pub fn can_redo_layout(&self) -> bool {
        self.inner.history.borrow().can_redo()
    }

    /// Undoes the last layout change (never a document change).
    pub fn undo_layout(&self) {
        let current = self.snapshot();
        let prev = self.inner.history.borrow_mut().undo(current);
        match prev {
            Some(s) => self.apply_snapshot(s),
            None => self.inner.toast.show("No layout change to undo", false),
        }
    }

    pub fn redo_layout(&self) {
        let current = self.snapshot();
        let next = self.inner.history.borrow_mut().redo(current);
        match next {
            Some(s) => self.apply_snapshot(s),
            None => self.inner.toast.show("No layout change to redo", false),
        }
    }

    /// Restores every workspace's layout; the visible workspace stays.
    fn apply_snapshot(&self, (_, layouts): Snapshot) {
        self.inner.toast.hide();
        *self.inner.layouts.borrow_mut() = layouts;
        self.schedule_rebuild();
        self.notify();
    }

    /// Stores the floating windows' current sizes in the model (GTK only
    /// reports them reliably on the live window).
    fn sync_window_sizes(&self) {
        let sizes: Vec<(WindowId, i32, i32)> = self
            .inner
            .floating
            .borrow()
            .iter()
            .map(|(id, f)| (*id, f.window.width(), f.window.height()))
            .filter(|(_, w, h)| *w > 0 && *h > 0)
            .collect();
        self.with_ws(|ws, _| {
            for (id, w, h) in sizes {
                ws.set_window_size(id, w, h);
            }
        });
    }

    /// Every workspace's layout plus the current workspace, as the
    /// document's layout blob.
    pub fn to_blob(&self) -> serde_json::Value {
        self.sync_window_sizes();
        let layouts = self.inner.layouts.borrow();
        let blob = LayoutBlob {
            version: LayoutBlob::VERSION,
            workspace: self.inner.workspace.get().id().to_owned(),
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
            l.reserve(&mut ids);
        }
        self.replace_all(ids, layouts, workspace);
        true
    }

    /// Back to the default layouts, in the Graph workspace (new document,
    /// or a document without a usable layout).
    pub fn reset_default(&self) {
        self.replace_all(IdGen::default(), HashMap::new(), Workspace::Graph);
    }

    fn replace_all(
        &self,
        ids: IdGen,
        layouts: HashMap<Workspace, WorkspaceLayout>,
        workspace: Workspace,
    ) {
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
        for e in &dropped {
            detach(&e.widget);
        }
        drop(dropped);
        self.inner.history.borrow_mut().clear();
        self.inner.workspace.set(workspace);
        self.inner.active.set(None);
        self.inner.hovered.set(None);
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

    fn main_window(&self) -> Option<gtk::Window> {
        self.inner.main.overlay.root().and_downcast::<gtk::Window>()
    }

    /// Replaces the widget trees with ones built from the current layout.
    fn rebuild(&self) {
        let ws = self.ws_layout();
        self.build_host(&self.inner.main, Host::Main, &ws.main);

        // Floating windows of this workspace: create, update, remove.
        let main_window = self.main_window();
        for f in &ws.floating {
            let existing = self
                .inner
                .floating
                .borrow()
                .get(&f.id)
                .map(|w| w.view.clone());
            let view = match (existing, &main_window) {
                (Some(v), _) => Some(v),
                (None, Some(main)) => {
                    let win = self.create_floating(f, main);
                    let view = win.view.clone();
                    self.inner.floating.borrow_mut().insert(f.id, win);
                    Some(view)
                }
                (None, None) => None, // not in a window yet; retried on map
            };
            if let Some(view) = view {
                self.build_host(&view, Host::Floating(f.id), &f.layout);
                if let Some(win) = self.inner.floating.borrow().get(&f.id) {
                    win.window.set_title(Some(&floating_title(&f.layout)));
                }
            }
        }
        let stale: Vec<FloatWin> = {
            let mut map = self.inner.floating.borrow_mut();
            let gone: Vec<WindowId> = map
                .keys()
                .filter(|id| !ws.floating.iter().any(|f| f.id == **id))
                .copied()
                .collect();
            gone.iter().filter_map(|id| map.remove(id)).collect()
        };
        for win in stale {
            // Editors still shown there stay cached for other workspaces.
            let window: &gtk::Widget = win.window.upcast_ref();
            let inside: Vec<gtk::Widget> = self
                .inner
                .editors
                .borrow()
                .values()
                .map(|e| e.widget.clone())
                .filter(|w| {
                    w.root()
                        .is_some_and(|r| r.upcast_ref::<gtk::Widget>() == window)
                })
                .collect();
            // Detached outside the borrow: unmap handlers may run.
            for w in inside {
                detach(&w);
            }
            win.window.destroy();
        }

        // Drop cached editors of tabs that exist in no workspace anymore
        // and in no layout undo/redo state: editors of closed or moved-away
        // tabs stay parked (detached) so undo brings them back with their
        // state.
        let mut live: Vec<TabId> = self
            .inner
            .layouts
            .borrow()
            .values()
            .flat_map(|l| l.all_tabs().into_iter().map(|(_, t)| t.id))
            .collect();
        live.extend(
            self.inner
                .history
                .borrow()
                .states()
                .flat_map(|(_, map)| map.values())
                .flat_map(|l| l.all_tabs().into_iter().map(|(_, t)| t.id)),
        );
        // Parked editors leave the widget tree.
        let parked: Vec<gtk::Widget> = self
            .inner
            .editors
            .borrow()
            .iter()
            .filter(|(id, _)| {
                !self
                    .inner
                    .layouts
                    .borrow()
                    .values()
                    .any(|l| l.area_of_tab(**id).is_some())
            })
            .map(|(_, e)| e.widget.clone())
            .collect();
        for w in parked {
            detach(&w);
        }
        let dropped: Vec<Editor> = {
            let mut editors = self.inner.editors.borrow_mut();
            let dead: Vec<TabId> = editors
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

    fn build_host(&self, view: &HostView, host: Host, layout: &Layout) {
        while let Some(child) = view.tree.first_child() {
            view.tree.remove(&child);
        }
        view.parts.borrow_mut().clear();
        view.highlighted.set(None);
        view.set_preview(Preview::None);
        let ws = self.ws_layout();
        // View-only: unfinished editors are shown as built ones while
        // "Show unfinished editors" is off (the model keeps them).
        let layout = editors::displayed_layout(layout, !self.hiding_unfinished());
        let tree = self.build_node(&ws, host, &layout, layout.visible_root());
        view.tree.append(&tree);
    }

    fn build_node(
        &self,
        ws: &WorkspaceLayout,
        host: Host,
        layout: &Layout,
        node: &Node,
    ) -> gtk::Widget {
        match node {
            Node::Leaf { id, tabs } => self.build_area(ws, host, layout, *id, tabs),
            Node::Split {
                id,
                dir,
                ratio,
                a,
                b,
            } => {
                let start = self.build_node(ws, host, layout, a);
                let end = self.build_node(ws, host, layout, b);
                let (ca, cb) = (a.depth_along(*dir), b.depth_along(*dir));
                let min = match dir {
                    Dir::Horizontal => MIN_AREA.0,
                    Dir::Vertical => MIN_AREA.1,
                };
                let split = *id;
                let weak = self.downgrade();
                widgets::split_view(
                    *dir,
                    Rc::new(Cell::new(*ratio)),
                    &start,
                    &end,
                    move |pos, span, snap| model::sash_ratio(pos, span, ca, cb, min, snap),
                    move |e| {
                        if let Some(areas) = weak.upgrade() {
                            areas.sash_event(host, split, e);
                        }
                    },
                )
                .upcast()
            }
        }
    }

    fn sash_event(&self, host: Host, split: SplitId, e: SashEvent) {
        let set = |r: f64| {
            self.with_ws(|w, _| w.layout_mut(host).map(|l| l.set_ratio(split, r)));
        };
        match e {
            SashEvent::Begin => {
                let s = self.snapshot();
                self.inner.sash_before.replace(Some(s));
            }
            SashEvent::Move(r) => set(r),
            SashEvent::End => {
                if let Some(before) = self.inner.sash_before.take()
                    && before.1 != self.snapshot().1
                {
                    self.record(before);
                }
            }
            SashEvent::Equalize => {
                // The drag gesture of the same clicks must not record too.
                self.inner.sash_before.take();
                let before = self.snapshot();
                self.with_ws(|w, _| w.layout_mut(host).map(|l| l.equalize(split)));
                if before.1 != self.snapshot().1 {
                    self.record(before);
                }
            }
        }
    }

    fn build_area(
        &self,
        ws: &WorkspaceLayout,
        host: Host,
        layout: &Layout,
        id: AreaId,
        tabs: &Tabs,
    ) -> gtk::Widget {
        let tab = tabs.active_tab();
        let kind = tab.editor;
        // Take the editor out of the cache so no borrow is held while
        // creating editors or building the header (which may re-enter).
        // Hidden tabs keep their editors cached, out of the widget tree.
        let cached = self.inner.editors.borrow_mut().remove(&tab.id);
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
        let frame = gtk::Overlay::builder()
            .child(&area)
            .css_classes(["area-frame"])
            .build();
        let strip = (tabs.len() > 1).then(|| self.build_tab_strip(id, tabs, &frame));
        if let Some((scroller, _)) = &strip {
            area.append(scroller);
        }
        area.append(&self.build_header(ws, host, &frame, &editor, id, maximized));
        editor.widget.set_hexpand(true);
        editor.widget.set_vexpand(true);
        let body = widgets::clip_bin(&editor.widget, &["area-body"]);
        body.set_hexpand(true);
        body.set_vexpand(true);
        area.append(&body);

        for left in [true, false] {
            frame.add_overlay(&self.corner_grip(host, id, left));
        }

        self.track_pointer(&frame, id);
        if let Some(view) = self.view(host) {
            view.parts
                .borrow_mut()
                .insert(id, (frame.clone(), area.clone(), strip.map(|(_, s)| s)));
        }
        self.inner.editors.borrow_mut().insert(tab.id, editor);
        frame.upcast()
    }

    fn build_header(
        &self,
        ws: &WorkspaceLayout,
        _host: Host,
        frame: &gtk::Overlay,
        editor: &Editor,
        id: AreaId,
        maximized: bool,
    ) -> gtk::Widget {
        let header = widgets::header_row(&["area-header"]);
        header.append(&self.editor_switch(ws, editor.kind, id, maximized));

        // Editor menus: whole menus disappear when they do not fit.
        let menus = widgets::overflow_row(2, &["header-menus"]);
        menus.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        for widget in editor.header_content() {
            menus.append(&widget);
        }
        header.append(&menus);

        let (menu, group) = self.area_menu(ws, id, maximized);
        header.insert_action_group("area", Some(&group));
        let more = gtk::MenuButton::builder()
            .icon_name(pick_icon(&[
                "view-more-horizontal-symbolic",
                "view-more-symbolic",
            ]))
            .menu_model(&menu)
            .tooltip_text("Area (right-click the header)")
            .valign(gtk::Align::Center)
            .css_classes(["area-more"])
            .build();
        if let Some(p) = more.popover() {
            p.add_css_class("area-menu");
        }
        header.append(&more);

        // Right-click anywhere on the header opens the same menu.
        let right = gtk::GestureClick::builder().button(3).build();
        right.connect_pressed(glib::clone!(
            #[weak]
            more,
            move |g, _, _, _| {
                g.set_state(gtk::EventSequenceState::Claimed);
                more.popup();
            }
        ));
        header.add_controller(right);

        self.install_drag(header.upcast_ref(), frame, Dragged::Area(id), editor.kind);
        header.upcast()
    }

    /// The tab strip of an area with several tabs: icon and name per tab,
    /// accent underline on the shown one, close button on hover. Click
    /// shows a tab, middle-click closes it, dragging moves it (reorder
    /// within the strip, onto another strip or area, or out of the
    /// window). Returns the scrolling container and the strip itself.
    fn build_tab_strip(
        &self,
        id: AreaId,
        tabs: &Tabs,
        frame: &gtk::Overlay,
    ) -> (gtk::Widget, gtk::Box) {
        let strip = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(1)
            .css_classes(["area-tabs"])
            .build();
        for (index, tab) in tabs.list().iter().copied().enumerate() {
            let row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(4)
                .tooltip_text(format!(
                    "{} · click to show, middle-click to close, drag to move",
                    tab.editor.title()
                ))
                .css_classes(["area-tab"])
                .build();
            if index == tabs.active() {
                row.add_css_class("active");
            }
            row.append(&gtk::Image::from_icon_name(editors::icon_name(tab.editor)));
            row.append(&gtk::Label::new(Some(tab.editor.title())));
            let close = gtk::Button::builder()
                .icon_name("window-close-symbolic")
                .tooltip_text("Close Tab")
                .valign(gtk::Align::Center)
                .css_classes(["tab-close"])
                .build();
            let weak = self.downgrade();
            close.connect_clicked(move |_| {
                if let Some(a) = weak.upgrade() {
                    a.close_tab(tab.id);
                }
            });
            row.append(&close);
            let click = gtk::GestureClick::builder().button(0).build();
            let weak = self.downgrade();
            click.connect_released(move |g, _, _, _| {
                let Some(a) = weak.upgrade() else { return };
                match g.current_button() {
                    1 => a.select_tab(id, index),
                    2 => a.close_tab(tab.id),
                    _ => {}
                }
            });
            row.add_controller(click);
            self.install_drag(row.upcast_ref(), frame, Dragged::Tab(tab.id), tab.editor);
            strip.append(&row);
        }
        let scroller = gtk::ScrolledWindow::builder()
            .child(&strip)
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .css_classes(["area-tabs-scroller"])
            .build();
        (scroller.upcast(), strip)
    }

    /// The "⋯" area menu and the `area.*` actions it uses.
    fn area_menu(
        &self,
        ws: &WorkspaceLayout,
        id: AreaId,
        maximized: bool,
    ) -> (gio::Menu, gio::SimpleActionGroup) {
        let menu = gio::Menu::new();
        let split = gio::Menu::new();
        for (dir, label) in [
            (Dir::Horizontal, "Split Horizontal"),
            (Dir::Vertical, "Split Vertical"),
        ] {
            split.append(Some(label), Some(&format!("area.split::{}", dir.id())));
        }
        menu.append_section(None, &split);

        let arrange = gio::Menu::new();
        let max = gio::MenuItem::new(
            Some(if maximized {
                "Restore Layout"
            } else {
                "Maximize"
            }),
            Some("area.maximize"),
        );
        max.set_attribute_value("accel", Some(&"<Control>space".to_variant()));
        arrange.append_item(&max);
        let others: Vec<(usize, AreaId, EditorKind)> = ws
            .leaves()
            .into_iter()
            .enumerate()
            .filter(|(_, (other, _))| *other != id)
            .map(|(pos, (other, kind))| (pos + 1, other, kind))
            .collect();
        let swap = gio::Menu::new();
        for (pos, other, kind) in &others {
            swap.append(
                Some(&format!("{}  ·  area {pos}", kind.title())),
                Some(&format!("area.swap::{}", other.0)),
            );
        }
        if !others.is_empty() && !maximized {
            arrange.append_submenu(Some("Swap with…"), &swap);
        }
        arrange.append(Some("Open in New Window"), Some("area.float"));
        menu.append_section(None, &arrange);

        let tab_section = gio::Menu::new();
        tab_section.append(Some("Close Tab"), Some("area.close-tab"));
        tab_section.append(Some("Move Tab to New Window"), Some("area.float-tab"));
        menu.append_section(None, &tab_section);

        let close = gio::Menu::new();
        close.append(Some("Close"), Some("area.close"));
        menu.append_section(None, &close);

        let group = gio::SimpleActionGroup::new();
        let weak = self.downgrade();
        let simple = |name: &str, enabled: bool, f: fn(&Areas, AreaId)| {
            let action = gio::SimpleAction::new(name, None);
            action.set_enabled(enabled);
            let weak = weak.clone();
            action.connect_activate(move |_, _| {
                if let Some(a) = weak.upgrade() {
                    f(&a, id);
                }
            });
            action
        };
        let can_close = ws.clone().close_area(id).is_ok();
        let can_float = ws
            .clone()
            .tear_off(id, FLOAT_DEFAULT, &mut IdGen::default())
            .is_ok();
        group.add_action(&simple("maximize", true, |a, id| {
            a.toggle_maximize(Some(id))
        }));
        group.add_action(&simple("float", can_float, |a, id| a.tear_off(Some(id))));
        group.add_action(&simple("close", can_close, |a, id| a.close(Some(id))));
        let shown_tab = ws.tabs(id).map(|t| t.active_tab().id);
        let can_close_tab = shown_tab.is_some_and(|t| ws.clone().close_tab(t).is_ok());
        let can_float_tab = shown_tab.is_some_and(|t| {
            ws.clone()
                .tear_off_tab(t, FLOAT_DEFAULT, &mut IdGen::default())
                .is_ok()
        });
        group.add_action(&simple("close-tab", can_close_tab, |a, id| {
            if let Some(t) = a.shown_tab(id) {
                a.close_tab(t);
            }
        }));
        group.add_action(&simple("float-tab", can_float_tab, |a, id| {
            if let Some(t) = a.shown_tab(id) {
                a.tear_off_tab(t);
            }
        }));

        let split_action = gio::SimpleAction::new("split", Some(glib::VariantTy::STRING));
        let w = weak.clone();
        split_action.connect_activate(move |_, p| {
            if let (Some(a), Some(dir)) =
                (w.upgrade(), p.and_then(|p| p.str()).and_then(Dir::from_id))
            {
                a.split(Some(id), dir);
            }
        });
        group.add_action(&split_action);
        let swap_action = gio::SimpleAction::new("swap", Some(glib::VariantTy::STRING));
        swap_action.connect_activate(move |_, p| {
            let other = p
                .and_then(|p| p.str())
                .and_then(|s| s.parse().ok())
                .map(AreaId);
            if let (Some(a), Some(other)) = (weak.upgrade(), other) {
                a.swap(Some(id), other);
            }
        });
        group.add_action(&swap_action);
        (menu, group)
    }

    /// Editor-type switch: current editor's icon and name (icon only when
    /// narrow), opening a list of all editors and the areas to swap with.
    /// Dragging it moves the area like dragging the header.
    fn editor_switch(
        &self,
        ws: &WorkspaceLayout,
        current: EditorKind,
        id: AreaId,
        maximized: bool,
    ) -> gtk::Widget {
        let content = widgets::switch_row(&["editor-switch-content"]);
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
        content.append(
            &gtk::Image::builder()
                .icon_name("pan-down-symbolic")
                .css_classes(["arrow"])
                .build(),
        );
        let button = gtk::Box::builder()
            .css_classes(["editor-switch"])
            .valign(gtk::Align::Center)
            .tooltip_text(format!(
                "Editor Type: {} (drag to move the area)",
                current.title()
            ))
            .build();
        button.append(&content);

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
        popover.set_parent(&button);
        button.connect_destroy(glib::clone!(
            #[strong]
            popover,
            move |_| popover.unparent()
        ));
        let click = gtk::GestureClick::new();
        click.connect_released(glib::clone!(
            #[weak]
            popover,
            move |_, _, _, _| popover.popup()
        ));
        button.add_controller(click);

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

        let others: Vec<(usize, AreaId, EditorKind)> = ws
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
        button.upcast()
    }

    /// A triangular grip in a top corner of an area: drag inward to split,
    /// outward into a neighbour sharing the full edge to join it, Esc to
    /// cancel.
    fn corner_grip(&self, host: Host, id: AreaId, left: bool) -> gtk::DrawingArea {
        let grip = gtk::DrawingArea::builder()
            .content_width(12)
            .content_height(12)
            .halign(if left {
                gtk::Align::Start
            } else {
                gtk::Align::End
            })
            .valign(gtk::Align::Start)
            .tooltip_text("Drag inward to split the area, outward to join a neighbour")
            .css_classes(["corner-grip", if left { "left" } else { "right" }])
            .build();
        grip.set_cursor_from_name(Some("crosshair"));
        grip.set_draw_func(move |g, cr, w, h| {
            let c = g.color();
            cr.set_source_rgba(
                f64::from(c.red()),
                f64::from(c.green()),
                f64::from(c.blue()),
                f64::from(c.alpha()),
            );
            let (w, h) = (f64::from(w), f64::from(h));
            cr.move_to(0.0, 0.0);
            cr.line_to(w, 0.0);
            if left {
                cr.line_to(0.0, h);
            } else {
                cr.line_to(w, h);
            }
            cr.close_path();
            let _ = cr.fill();
        });

        #[derive(Clone, Copy, PartialEq)]
        enum Action {
            None,
            Split(Dir, f64),
            Join(AreaId),
        }
        struct State {
            start: Cell<(f64, f64)>,
            action: Cell<Action>,
            cancelled: Cell<bool>,
            esc: RefCell<Option<(gtk::Window, gtk::EventControllerKey)>>,
        }
        let state = Rc::new(State {
            start: Cell::new((0.0, 0.0)),
            action: Cell::new(Action::None),
            cancelled: Cell::new(false),
            esc: RefCell::default(),
        });
        let remove_esc = |state: &State| {
            if let Some((window, keys)) = state.esc.take() {
                window.remove_controller(&keys);
            }
        };

        let drag = gtk::GestureDrag::new();
        let weak = self.downgrade();
        drag.connect_drag_begin(glib::clone!(
            #[strong]
            state,
            #[weak]
            grip,
            move |g, x, y| {
                let Some(areas) = weak.upgrade() else {
                    return;
                };
                let Some(view) = areas.view(host) else {
                    return;
                };
                g.set_state(gtk::EventSequenceState::Claimed);
                let p = grip
                    .compute_point(&view.overlay, &graphene::Point::new(x as f32, y as f32))
                    .map_or((0.0, 0.0), |p| (f64::from(p.x()), f64::from(p.y())));
                state.start.set(p);
                state.cancelled.set(false);
                state.action.set(Action::None);
                // Esc cancels: listen on the window, focus may be anywhere.
                if let Some(window) = view.overlay.root().and_downcast::<gtk::Window>() {
                    let keys = gtk::EventControllerKey::new();
                    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
                    let weak_g = g.downgrade();
                    let st = Rc::downgrade(&state);
                    let v = Rc::downgrade(&view);
                    keys.connect_key_pressed(move |_, key, _, _| {
                        if key != gdk::Key::Escape {
                            return glib::Propagation::Proceed;
                        }
                        if let Some(s) = st.upgrade() {
                            s.cancelled.set(true);
                            s.action.set(Action::None);
                        }
                        if let Some(v) = v.upgrade() {
                            v.clear_feedback();
                        }
                        if let Some(g) = weak_g.upgrade() {
                            g.set_state(gtk::EventSequenceState::Denied);
                        }
                        glib::Propagation::Stop
                    });
                    window.add_controller(keys.clone());
                    state.esc.replace(Some((window, keys)));
                }
            }
        ));
        let weak = self.downgrade();
        drag.connect_drag_update(glib::clone!(
            #[strong]
            state,
            move |_, dx, dy| {
                if state.cancelled.get() {
                    return;
                }
                let (Some(areas), Some(view)) =
                    (weak.upgrade(), weak.upgrade().and_then(|a| a.view(host)))
                else {
                    return;
                };
                let (sx, sy) = state.start.get();
                let (px, py) = (sx + dx, sy + dy);
                let Some(r) = view.rect_of(id) else {
                    return;
                };
                let (action, preview) = if r.contains(px, py) {
                    match model::corner_split(px - r.x, py - r.y, dx, dy, r.w, r.h) {
                        Some((dir, ratio)) => {
                            let line = match dir {
                                Dir::Horizontal => {
                                    let x = r.x + ratio * r.w;
                                    ((x, r.y), (x, r.y + r.h))
                                }
                                Dir::Vertical => {
                                    let y = r.y + ratio * r.h;
                                    ((r.x, y), (r.x + r.w, y))
                                }
                            };
                            (
                                Action::Split(dir, ratio),
                                Preview::SplitLine {
                                    from: line.0,
                                    to: line.1,
                                },
                            )
                        }
                        None => (Action::None, Preview::None),
                    }
                } else {
                    let side = view.area_at(px, py).and_then(|(other, orect)| {
                        let ws = areas.ws_layout();
                        let side = ws.layout(host)?.join_side(id, other)?;
                        Some((other, orect, side))
                    });
                    match side {
                        Some((other, rect, toward)) => {
                            (Action::Join(other), Preview::Join { rect, toward })
                        }
                        None => (Action::None, Preview::None),
                    }
                };
                state.action.set(action);
                view.set_preview(preview);
            }
        ));
        let weak = self.downgrade();
        drag.connect_drag_end(glib::clone!(
            #[strong]
            state,
            move |_, _, _| {
                remove_esc(&state);
                let Some(areas) = weak.upgrade() else {
                    return;
                };
                if let Some(view) = areas.view(host) {
                    view.clear_feedback();
                }
                if state.cancelled.replace(false) {
                    return;
                }
                match state.action.replace(Action::None) {
                    Action::None => {}
                    Action::Split(dir, ratio) => areas.split_at(Some(id), dir, ratio),
                    Action::Join(other) => areas.join(id, other),
                }
            }
        ));
        drag.connect_cancel(glib::clone!(
            #[strong]
            state,
            move |_, _| remove_esc(&state)
        ));
        grip.add_controller(drag);
        grip
    }

    /// Drag and drop of an area (by its header, or the editor switch past
    /// the drag threshold) or of a tab: lifts a ghost of the area;
    /// dropping on an area, a tab strip or a window edge docks it,
    /// dropping outside every window tears it off.
    fn install_drag(
        &self,
        widget: &gtk::Widget,
        frame: &gtk::Overlay,
        item: Dragged,
        kind: EditorKind,
    ) {
        let source = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE | gdk::DragAction::COPY)
            .build();
        let weak = self.downgrade();
        source.connect_prepare(move |_, _, _| {
            let areas = weak.upgrade()?;
            areas.inner.dragging.set(Some(item));
            areas.inner.dropped.set(false);
            Some(gdk::ContentProvider::for_value(
                &AreaDrag::new(item).to_value(),
            ))
        });
        source.connect_drag_begin(glib::clone!(
            #[weak]
            frame,
            move |_, drag| {
                let icon = gtk::DragIcon::for_drag(drag);
                icon.set_child(Some(&overlay::ghost(frame.upcast_ref(), kind)));
                frame.add_css_class("drag-source");
            }
        ));
        let weak = self.downgrade();
        source.connect_drag_cancel(move |_, _, reason| {
            let Some(areas) = weak.upgrade() else {
                return false;
            };
            if reason == gdk::DragCancelReason::NoTarget && !areas.inner.dropped.get() {
                // Dropped outside every window: tear off. Deferred, the
                // drag is still being torn down.
                let weak = areas.downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(a) = weak.upgrade() {
                        match item {
                            Dragged::Area(id) => a.tear_off(Some(id)),
                            Dragged::Tab(t) => a.tear_off_tab(t),
                        }
                    }
                });
                return true;
            }
            false
        });
        let weak = self.downgrade();
        source.connect_drag_end(glib::clone!(
            #[weak]
            frame,
            move |_, _, _| {
                frame.remove_css_class("drag-source");
                if let Some(areas) = weak.upgrade() {
                    areas.inner.dragging.set(None);
                    areas.clear_all_feedback();
                }
            }
        ));
        widget.add_controller(source);
    }

    fn clear_all_feedback(&self) {
        self.inner.main.clear_feedback();
        let views: Vec<Rc<HostView>> = self
            .inner
            .floating
            .borrow()
            .values()
            .map(|w| w.view.clone())
            .collect();
        for v in views {
            v.clear_feedback();
        }
    }

    /// Accepts area drops anywhere in `window` (over areas and their
    /// edges; elsewhere the drop is swallowed so it does not tear off).
    fn install_drop_target(&self, window: &gtk::Window, host: Host) {
        let target = gtk::DropTarget::new(
            AreaDrag::static_type(),
            gdk::DragAction::MOVE | gdk::DragAction::COPY,
        );
        let hover = {
            let weak = self.downgrade();
            move |t: &gtk::DropTarget, x: f64, y: f64| -> gdk::DragAction {
                let Some(areas) = weak.upgrade() else {
                    return gdk::DragAction::empty();
                };
                let Some(view) = areas.view(host) else {
                    return gdk::DragAction::empty();
                };
                match areas.resolve_drop(t, host, x, y) {
                    Some((_, preview, dst)) => {
                        view.set_preview(preview);
                        view.highlight(dst);
                    }
                    None => view.clear_feedback(),
                }
                gdk::DragAction::MOVE
            }
        };
        let hover = Rc::new(hover);
        let h = hover.clone();
        target.connect_enter(move |t, x, y| h(t, x, y));
        target.connect_motion(move |t, x, y| hover(t, x, y));
        let weak = self.downgrade();
        target.connect_leave(move |_| {
            if let Some(view) = weak.upgrade().and_then(|a| a.view(host)) {
                view.clear_feedback();
            }
        });
        let weak = self.downgrade();
        target.connect_drop(move |t, value, x, y| {
            let Some(areas) = weak.upgrade() else {
                return false;
            };
            areas.inner.dropped.set(true);
            let resolved = areas.resolve_drop(t, host, x, y);
            if let Some(view) = areas.view(host) {
                view.clear_feedback();
            }
            let src = value
                .get::<AreaDrag>()
                .ok()
                .map(AreaDrag::item)
                .or(areas.inner.dragging.get());
            if let (Some(src), Some((target, _, _))) = (src, resolved) {
                areas.dock(src, target);
            }
            true
        });
        window.add_controller(target);
    }

    /// Dock target and highlight rectangle for a drop at window
    /// coordinates `(x, y)`.
    fn resolve_drop(
        &self,
        t: &gtk::DropTarget,
        host: Host,
        x: f64,
        y: f64,
    ) -> Option<ResolvedDrop> {
        let item = self.inner.dragging.get()?;
        let ws = self.ws_layout();
        let (src, tab_count) = match item {
            Dragged::Area(a) => (a, 0),
            Dragged::Tab(t) => {
                let a = ws.area_of_tab(t)?;
                (a, ws.tabs(a).map_or(1, Tabs::len))
            }
        };
        // A tab alone in its area moves like the area.
        let moves_area = matches!(item, Dragged::Area(_)) || tab_count == 1;
        let view = self.view(host)?;
        let window = t.widget()?;
        let p = window.compute_point(&view.overlay, &graphene::Point::new(x as f32, y as f32))?;
        let (px, py) = (f64::from(p.x()), f64::from(p.y()));
        let (w, h) = (
            f64::from(view.overlay.width()),
            f64::from(view.overlay.height()),
        );
        if let Some(side) = model::window_edge(px, py, w, h) {
            let rect = Rect::new(0.0, 0.0, w, h).part(side, model::EDGE_SHARE);
            let preview = Preview::Drop {
                rect,
                area: None,
                label: None,
            };
            return Some((DockTarget::Edge { host, side }, preview, None));
        }
        if let Some((dst, index, (lx, y0, y1))) = view.strip_at(px, py) {
            if dst == src && moves_area {
                return None;
            }
            let preview = Preview::Insert { x: lx, y0, y1 };
            return Some((DockTarget::Strip { dst, index }, preview, Some(dst)));
        }
        let (dst, r) = view.area_at(px, py)?;
        let zone = model::drop_zone(px - r.x, py - r.y, r.w, r.h);
        // Onto its own area: only a tab can split off, to an edge.
        if dst == src && (moves_area || zone == DropZone::Center) {
            return None;
        }
        let state = t.current_event_state();
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK)
            || t.current_drop()
                .is_some_and(|d| d.actions() == gdk::DragAction::COPY);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let mut mode = CenterMode::from_modifiers(shift, ctrl);
        if mode == CenterMode::Swap && !matches!(item, Dragged::Area(_)) {
            mode = CenterMode::Tab; // a tab docks as a tab
        }
        let target = DockTarget::Area { dst, zone, mode };
        let preview = Preview::Drop {
            rect: model::zone_rect(zone, r),
            area: Some(r),
            label: (zone == DropZone::Center).then(|| mode.label()),
        };
        Some((target, preview, Some(dst)))
    }

    fn create_floating(&self, f: &Floating, main: &gtk::Window) -> FloatWin {
        let view = HostView::new();
        let window = gtk::Window::builder()
            .default_width(f.width)
            .default_height(f.height)
            .width_request(200)
            .height_request(140)
            .transient_for(main)
            .destroy_with_parent(true)
            .child(&view.overlay)
            .css_classes(["floating-area-window"])
            .build();
        if let Some(app) = main.application() {
            window.set_application(Some(&app));
        }
        // Window actions and their accelerators work here too.
        if let Some(main) = main.downcast_ref::<gtk::ApplicationWindow>() {
            window.insert_action_group("win", Some(main));
        }
        let id = f.id;
        let weak = self.downgrade();
        window.connect_close_request(move |_| {
            if let Some(a) = weak.upgrade() {
                a.close_window(id);
            }
            glib::Propagation::Stop
        });
        self.install_drop_target(&window, Host::Floating(id));
        crate::keymap::live::install(&window, &self.inner.ctx);
        window.present();
        FloatWin { window, view }
    }

    /// Hover tracking (target of menu and palette actions) and click to
    /// activate.
    fn track_pointer(&self, frame: &gtk::Overlay, id: AreaId) {
        let motion = gtk::EventControllerMotion::new();
        let weak = self.downgrade();
        motion.connect_enter(move |_, _, _| {
            if let Some(areas) = weak.upgrade() {
                areas.set_hovered(Some(id));
            }
        });
        let weak = self.downgrade();
        motion.connect_leave(move |_| {
            if let Some(areas) = weak.upgrade()
                && areas.inner.hovered.get() == Some(id)
            {
                areas.set_hovered(None);
            }
        });
        frame.add_controller(motion);

        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        click.connect_pressed(move |_, _, _, _| {
            if let Some(areas) = weak.upgrade() {
                areas.set_active(id);
            }
        });
        frame.add_controller(click);
    }

    fn set_hovered(&self, id: Option<AreaId>) {
        if self.inner.hovered.replace(id) != id {
            self.notify();
        }
    }

    fn set_active(&self, id: AreaId) {
        if self.inner.active.replace(Some(id)) != Some(id) {
            self.refresh_active_class();
            self.notify();
        }
    }

    fn refresh_active_class(&self) {
        let active = self.active_area();
        let mut views = vec![self.inner.main.clone()];
        views.extend(
            self.inner
                .floating
                .borrow()
                .values()
                .map(|w| w.view.clone()),
        );
        for view in views {
            for (id, (_, area, _)) in view.parts.borrow().iter() {
                if Some(*id) == active {
                    area.add_css_class("active");
                } else {
                    area.remove_css_class("active");
                }
            }
        }
    }
}

/// Title of a floating window: its editors.
fn floating_title(layout: &Layout) -> String {
    let names: Vec<&str> = layout.leaves().iter().map(|(_, e)| e.title()).collect();
    format!("{} — Viscas", names.join(" · "))
}

/// Serialized layouts of a document (see [`Areas::to_blob`]).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LayoutBlob {
    pub version: u32,
    pub workspace: String,
    /// Version 1 stored bare layouts; [`WorkspaceLayout`] reads both.
    pub layouts: std::collections::BTreeMap<String, WorkspaceLayout>,
}

impl LayoutBlob {
    /// 2: floating windows. 3: tabs (single-editor leaves still load).
    pub const VERSION: u32 = 3;

    /// Validated workspace and layouts; `None` for a future version, an
    /// unknown current workspace or ids shared between workspaces (ids are
    /// global, editors are cached by area id).
    pub fn into_layouts(self) -> Option<(Workspace, HashMap<Workspace, WorkspaceLayout>)> {
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

/// Palette commands that depend on the current layout, labelled with the
/// area they apply to (the area under the pointer when the palette
/// opened, else the active area).
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
        let ws = areas.ws_layout();
        let Some(target) = areas.target_area() else {
            return Vec::new();
        };
        let Some(kind) = ws.editor(target) else {
            return Vec::new();
        };
        let name = kind.title();
        let cmd = |id: String, title: String, run: Box<dyn Fn(&Areas)>| {
            let weak = areas.downgrade();
            Command {
                id,
                title,
                category: "Area",
                shortcut: None,
                run: Rc::new(move || {
                    if let Some(areas) = weak.upgrade() {
                        run(&areas);
                    }
                }),
            }
        };
        let mut out = vec![
            cmd(
                "area.split::horizontal".into(),
                format!("Split Area Horizontally ({name})"),
                Box::new(move |a| a.split(Some(target), Dir::Horizontal)),
            ),
            cmd(
                "area.split::vertical".into(),
                format!("Split Area Vertically ({name})"),
                Box::new(move |a| a.split(Some(target), Dir::Vertical)),
            ),
            cmd(
                "area.float".into(),
                format!("Open Area in New Window ({name})"),
                Box::new(move |a| a.tear_off(Some(target))),
            ),
            cmd(
                "area.close".into(),
                format!("Close Area ({name})"),
                Box::new(move |a| a.close(Some(target))),
            ),
        ];
        if ws.main.maximized().is_none() {
            for (pos, (other, other_kind)) in ws.leaves().into_iter().enumerate() {
                if other == target {
                    continue;
                }
                out.push(cmd(
                    format!("area.swap::{}", other.0),
                    format!(
                        "Swap Area ({name}) With: {} (area {})",
                        other_kind.title(),
                        pos + 1
                    ),
                    Box::new(move |a| a.swap(Some(target), other)),
                ));
            }
        }
        out
    }
}

/// Removes a cached editor widget from its previous area body, if any.
fn detach(widget: &gtk::Widget) {
    popdown_all(widget);
    if let Some(parent) = widget.parent() {
        match parent.downcast::<gtk::Box>() {
            Ok(body) => body.remove(widget),
            Err(_) => widget.unparent(),
        }
    }
}

/// Closes every popover inside `widget`, so none stays attached to a
/// surface the widget is leaving.
fn popdown_all(widget: &gtk::Widget) {
    if let Some(p) = widget.downcast_ref::<gtk::Popover>() {
        p.popdown();
    }
    let mut c = widget.first_child();
    while let Some(child) = c {
        popdown_all(&child);
        c = child.next_sibling();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_blob_round_trips_and_rejects_bad_input() {
        let mut ids = IdGen::default();
        let mut layouts = std::collections::BTreeMap::new();
        for ws in Workspace::ALL {
            layouts.insert(
                ws.id().to_owned(),
                WorkspaceLayout::new(ws.default_layout(&mut ids)),
            );
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
            map[&Workspace::Graph].main,
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
        let l = WorkspaceLayout::new(Workspace::Graph.default_layout(&mut IdGen::default()));
        let dup = LayoutBlob {
            version: 1,
            workspace: "graph".into(),
            layouts: [("graph".to_owned(), l.clone()), ("data".to_owned(), l)]
                .into_iter()
                .collect(),
        };
        assert!(dup.into_layouts().is_none());
    }

    #[test]
    fn version_1_blobs_still_load() {
        let main = Workspace::Graph.default_layout(&mut IdGen::default());
        let json = serde_json::json!({
            "version": 1,
            "workspace": "graph",
            "layouts": { "graph": serde_json::to_value(&main).unwrap() },
        });
        let blob: LayoutBlob = serde_json::from_value(json).unwrap();
        let (_, map) = blob.into_layouts().unwrap();
        assert_eq!(map[&Workspace::Graph].main, main);
        assert!(map[&Workspace::Graph].floating.is_empty());
    }
}

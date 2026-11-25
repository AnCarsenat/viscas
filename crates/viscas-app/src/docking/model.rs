//! Pure docking model: a workspace's main layout plus its floating
//! windows, the operations moving areas between them, drop-zone geometry
//! and the layout undo history. No GTK.

use serde::{Deserialize, Serialize};

use crate::layout::{AreaId, Dir, EditorKind, IdGen, Layout, MAX_ID, Rect, Side, Tab, TabId, Tabs};

/// Identifies a floating window. Shares the id space of areas and splits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WindowId(pub u32);

/// Default and limits of a floating window's size, in px.
pub const FLOAT_DEFAULT: (i32, i32) = (520, 380);
const FLOAT_MIN: (i32, i32) = (200, 140);
const FLOAT_MAX: i32 = 16_000;

/// Smallest area size reachable by dragging (REQUIREMENTS §2), in px.
pub const MIN_AREA: (f64, f64) = (120.0, 80.0);

/// An area layout in its own top-level window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Floating {
    pub id: WindowId,
    pub layout: Layout,
    pub width: i32,
    pub height: i32,
    /// Last known position. GTK 4 can neither read nor set window
    /// positions on most platforms; kept for platforms and future
    /// versions that can.
    #[serde(default)]
    pub x: Option<i32>,
    #[serde(default)]
    pub y: Option<i32>,
}

/// Which layout of a workspace an area lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Host {
    Main,
    Floating(WindowId),
}

/// Where a dragged area or tab is dropped on an area (see [`drop_zone`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    /// Split the target and put the area on that side.
    Edge(Side),
    /// Center: see [`CenterMode`].
    Center,
}

/// What a center drop does (chosen with modifiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CenterMode {
    /// Plain: dock as new tab(s) of the target.
    Tab,
    /// Shift: exchange the two areas (a dragged tab joins as a tab).
    Swap,
    /// Ctrl: replace the target area.
    Replace,
}

impl CenterMode {
    pub fn from_modifiers(shift: bool, ctrl: bool) -> Self {
        if ctrl {
            CenterMode::Replace
        } else if shift {
            CenterMode::Swap
        } else {
            CenterMode::Tab
        }
    }

    /// Label drawn inside the center zone.
    pub fn label(self) -> &'static str {
        match self {
            CenterMode::Tab => "Tab",
            CenterMode::Swap => "Swap",
            CenterMode::Replace => "Replace",
        }
    }
}

/// A complete drop target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockTarget {
    Area {
        dst: AreaId,
        zone: DropZone,
        mode: CenterMode,
    },
    /// A whole window edge: the area spans the full height or width.
    Edge { host: Host, side: Side },
    /// Into an area's tab strip, before tab number `index`.
    Strip { dst: AreaId, index: usize },
}

/// What is being dragged: a whole area (header) or one tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dragged {
    Area(AreaId),
    Tab(TabId),
}

/// Why a layout operation was refused (shown as a toast).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing to do (unknown area, dropped onto itself, ...).
    Invalid,
    /// The main window always keeps one area.
    LastArea,
    /// A workspace always keeps one Viewport.
    LastViewport,
}

impl Refusal {
    pub fn message(self) -> Option<&'static str> {
        match self {
            Refusal::Invalid => None,
            Refusal::LastArea => Some("The main window needs at least one area"),
            Refusal::LastViewport => Some("Each workspace keeps at least one Viewport"),
        }
    }
}

/// Share of the space an area docked along a window edge gets.
pub const EDGE_SHARE: f64 = 0.25;

/// A workspace's layout: the main window tree and floating windows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkspaceLayout {
    pub main: Layout,
    pub floating: Vec<Floating>,
}

/// Accepts both the current form and a bare [`Layout`] (layout blob
/// version 1, before floating windows).
#[derive(Deserialize)]
#[serde(untagged)]
enum RawWorkspaceLayout {
    Full {
        main: Layout,
        #[serde(default)]
        floating: Vec<Floating>,
    },
    Bare(Layout),
}

impl<'de> Deserialize<'de> for WorkspaceLayout {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (main, floating) = match RawWorkspaceLayout::deserialize(d)? {
            RawWorkspaceLayout::Full { main, floating } => (main, floating),
            RawWorkspaceLayout::Bare(main) => (main, Vec::new()),
        };
        WorkspaceLayout::validated(main, floating).map_err(serde::de::Error::custom)
    }
}

/// Whether `ids` are unique, non-zero and at most [`MAX_ID`].
fn ids_ok(mut ids: Vec<u32>) -> Result<(), String> {
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != n {
        return Err("layout ids are used more than once".to_owned());
    }
    if ids.first() == Some(&0) {
        return Err("layout id 0 is reserved".to_owned());
    }
    if ids.last().is_some_and(|id| *id > MAX_ID) {
        return Err("layout id is too large".to_owned());
    }
    Ok(())
}

impl WorkspaceLayout {
    pub fn new(main: Layout) -> Self {
        Self {
            main,
            floating: Vec::new(),
        }
    }

    /// Checks ids are unique across the main layout, floating layouts and
    /// window ids (and tab ids among tabs), and clamps window sizes.
    pub fn validated(main: Layout, mut floating: Vec<Floating>) -> Result<Self, String> {
        for f in &mut floating {
            f.width = f.width.clamp(FLOAT_MIN.0, FLOAT_MAX);
            f.height = f.height.clamp(FLOAT_MIN.1, FLOAT_MAX);
        }
        let this = Self { main, floating };
        ids_ok(this.ids())?;
        ids_ok(this.tab_ids())?;
        if this.viewports() == 0 {
            return Err("a workspace needs a Viewport".to_owned());
        }
        Ok(this)
    }

    /// Every area, split and window id.
    pub fn ids(&self) -> Vec<u32> {
        let mut ids = self.main.ids();
        for f in &self.floating {
            ids.push(f.id.0);
            ids.extend(f.layout.ids());
        }
        ids
    }

    /// Every tab id.
    pub fn tab_ids(&self) -> Vec<u32> {
        self.all_tabs().into_iter().map(|(_, t)| t.id.0).collect()
    }

    /// Makes `id_gen` hand out ids above every id used here.
    pub fn reserve(&self, id_gen: &mut IdGen) {
        for id in self.ids().into_iter().chain(self.tab_ids()) {
            id_gen.reserve_id(id);
        }
    }

    pub fn layout(&self, host: Host) -> Option<&Layout> {
        match host {
            Host::Main => Some(&self.main),
            Host::Floating(w) => self.floating.iter().find(|f| f.id == w).map(|f| &f.layout),
        }
    }

    pub fn layout_mut(&mut self, host: Host) -> Option<&mut Layout> {
        match host {
            Host::Main => Some(&mut self.main),
            Host::Floating(w) => self
                .floating
                .iter_mut()
                .find(|f| f.id == w)
                .map(|f| &mut f.layout),
        }
    }

    fn layouts(&self) -> impl Iterator<Item = &Layout> {
        std::iter::once(&self.main).chain(self.floating.iter().map(|f| &f.layout))
    }

    /// The layout holding area `id`.
    pub fn host_of(&self, id: AreaId) -> Option<Host> {
        if self.main.contains(id) {
            return Some(Host::Main);
        }
        self.floating
            .iter()
            .find(|f| f.layout.contains(id))
            .map(|f| Host::Floating(f.id))
    }

    /// The area holding tab `tab`.
    pub fn area_of_tab(&self, tab: TabId) -> Option<AreaId> {
        self.layouts().find_map(|l| l.area_of_tab(tab))
    }

    /// The editor shown in area `id`.
    pub fn editor(&self, id: AreaId) -> Option<EditorKind> {
        let host = self.host_of(id)?;
        self.layout(host)?.editor(id)
    }

    /// The tabs of area `id`.
    pub fn tabs(&self, id: AreaId) -> Option<&Tabs> {
        self.layouts().find_map(|l| l.tabs(id))
    }

    fn layout_of_mut(&mut self, id: AreaId) -> Result<&mut Layout, Refusal> {
        let host = self.host_of(id).ok_or(Refusal::Invalid)?;
        self.layout_mut(host).ok_or(Refusal::Invalid)
    }

    /// Every area of the workspace with its shown editor, main window first.
    pub fn leaves(&self) -> Vec<(AreaId, EditorKind)> {
        self.layouts().flat_map(Layout::leaves).collect()
    }

    /// Every tab of the workspace with its area.
    pub fn all_tabs(&self) -> Vec<(AreaId, Tab)> {
        self.layouts().flat_map(Layout::all_tabs).collect()
    }

    fn viewports(&self) -> usize {
        self.all_tabs()
            .iter()
            .filter(|(_, t)| t.editor == EditorKind::Viewport)
            .count()
    }

    /// Runs `f` on a copy and keeps the result only when it succeeds and
    /// does not remove the workspace's last Viewport.
    fn attempt<R>(
        &mut self,
        f: impl FnOnce(&mut WorkspaceLayout) -> Result<R, Refusal>,
    ) -> Result<R, Refusal> {
        let had_viewport = self.viewports() > 0;
        let mut next = self.clone();
        let r = f(&mut next)?;
        if had_viewport && next.viewports() == 0 {
            return Err(Refusal::LastViewport);
        }
        *self = next;
        Ok(r)
    }

    /// Removes area `id` from wherever it is, returning its tabs. A
    /// floating window losing its last area disappears; the main window
    /// keeps at least one.
    fn take(&mut self, id: AreaId) -> Result<Tabs, Refusal> {
        match self.host_of(id).ok_or(Refusal::Invalid)? {
            Host::Main => self.main.take(id).ok_or(Refusal::LastArea),
            Host::Floating(w) => {
                let pos = self
                    .floating
                    .iter()
                    .position(|f| f.id == w)
                    .ok_or(Refusal::Invalid)?;
                let layout = &mut self.floating[pos].layout;
                if layout.leaves().len() == 1 {
                    let tabs = layout.tabs(id).cloned().ok_or(Refusal::Invalid)?;
                    self.floating.remove(pos);
                    Ok(tabs)
                } else {
                    layout.take(id).ok_or(Refusal::Invalid)
                }
            }
        }
    }

    /// Closes area `id`; its sibling takes the space.
    pub fn close_area(&mut self, id: AreaId) -> Result<(), Refusal> {
        self.attempt(|w| w.take(id).map(drop))
    }

    /// Joins `absorb` into its neighbour `keep` (same window, full edge).
    pub fn join(&mut self, keep: AreaId, absorb: AreaId) -> Result<(), Refusal> {
        let host = self.host_of(keep).ok_or(Refusal::Invalid)?;
        if self.host_of(absorb) != Some(host) {
            return Err(Refusal::Invalid);
        }
        self.attempt(|w| {
            let layout = w.layout_mut(host).ok_or(Refusal::Invalid)?;
            ok(layout.join(keep, absorb))
        })
    }

    /// Moves area `src` (with all its tabs) to `target`.
    pub fn dock(
        &mut self,
        src: AreaId,
        target: DockTarget,
        ids: &mut IdGen,
    ) -> Result<(), Refusal> {
        let src_host = self.host_of(src).ok_or(Refusal::Invalid)?;
        match target {
            DockTarget::Area { dst, .. } | DockTarget::Strip { dst, .. } if dst == src => {
                Err(Refusal::Invalid)
            }
            DockTarget::Area {
                dst,
                zone: DropZone::Center,
                mode: CenterMode::Swap,
            } => self.swap(src, dst),
            DockTarget::Area {
                dst,
                zone: DropZone::Edge(side),
                ..
            } if self.host_of(dst) == Some(src_host) => self.attempt(|w| {
                let layout = w.layout_mut(src_host).ok_or(Refusal::Invalid)?;
                if layout.move_leaf(src, dst, side, ids) {
                    Ok(())
                } else if layout.leaves().len() == 1 {
                    Err(Refusal::Invalid)
                } else {
                    Err(Refusal::LastArea)
                }
            }),
            DockTarget::Edge { host, side } if host == src_host => self.attempt(|w| {
                let layout = w.layout_mut(host).ok_or(Refusal::Invalid)?;
                ok(layout.dock_root(src, side, EDGE_SHARE, ids))
            }),
            _ => self.attempt(|w| {
                if let DockTarget::Edge { host, .. } = target
                    && w.layout(host).is_none()
                {
                    return Err(Refusal::Invalid);
                }
                let tabs = w.take(src)?;
                w.place(src, tabs, target, ids)
            }),
        }
    }

    /// Moves tab `tab` to `target`. A tab alone in its area moves the
    /// whole area; otherwise it leaves its area and lands as a new area
    /// (edges) or a tab (center, tab strips).
    pub fn dock_tab(
        &mut self,
        tab: TabId,
        target: DockTarget,
        ids: &mut IdGen,
    ) -> Result<(), Refusal> {
        let area = self.area_of_tab(tab).ok_or(Refusal::Invalid)?;
        let alone = self.tabs(area).is_some_and(|t| t.len() == 1);
        match target {
            DockTarget::Strip { dst, index } if dst == area => {
                let layout = self.layout_of_mut(area)?;
                ok(layout.reorder_tab(tab, index))
            }
            DockTarget::Area {
                dst,
                zone: DropZone::Center,
                ..
            } if dst == area => Err(Refusal::Invalid),
            _ if alone => {
                // Center swap of a lone tab still docks it as a tab.
                let target = match target {
                    DockTarget::Area {
                        dst,
                        zone: DropZone::Center,
                        mode: CenterMode::Swap,
                    } => DockTarget::Area {
                        dst,
                        zone: DropZone::Center,
                        mode: CenterMode::Tab,
                    },
                    t => t,
                };
                self.dock(area, target, ids)
            }
            _ => self.attempt(|w| {
                let t = w
                    .layout_of_mut(area)?
                    .remove_tab(tab)
                    .ok_or(Refusal::Invalid)?;
                let new = ids.area();
                w.place(new, Tabs::one(t), target, ids)
            }),
        }
    }

    /// Puts area `id` with `tabs` (in no layout) at `target`.
    fn place(
        &mut self,
        id: AreaId,
        tabs: Tabs,
        target: DockTarget,
        ids: &mut IdGen,
    ) -> Result<(), Refusal> {
        match target {
            DockTarget::Area {
                dst,
                zone: DropZone::Edge(side),
                ..
            } => ok(self
                .layout_of_mut(dst)?
                .insert(dst, side, id, tabs, 0.5, ids)),
            DockTarget::Area {
                dst,
                zone: DropZone::Center,
                mode: CenterMode::Replace,
            } => ok(self.layout_of_mut(dst)?.replace_leaf(dst, id, tabs)),
            DockTarget::Area { dst, .. } => ok(self.layout_of_mut(dst)?.add_tabs(dst, tabs, None)),
            DockTarget::Strip { dst, index } => {
                ok(self.layout_of_mut(dst)?.add_tabs(dst, tabs, Some(index)))
            }
            DockTarget::Edge { host, side } => {
                let layout = self.layout_mut(host).ok_or(Refusal::Invalid)?;
                ok(layout.insert_root(side, id, tabs, EDGE_SHARE, ids))
            }
        }
    }

    /// Exchanges two areas, possibly in different windows.
    pub fn swap(&mut self, a: AreaId, b: AreaId) -> Result<(), Refusal> {
        let (Some(ha), Some(hb)) = (self.host_of(a), self.host_of(b)) else {
            return Err(Refusal::Invalid);
        };
        if a == b {
            return Err(Refusal::Invalid);
        }
        if ha == hb {
            let layout = self.layout_mut(ha).ok_or(Refusal::Invalid)?;
            return ok(layout.swap(a, b));
        }
        let (ta, tb) = (
            self.tabs(a).cloned().ok_or(Refusal::Invalid)?,
            self.tabs(b).cloned().ok_or(Refusal::Invalid)?,
        );
        // Ids are unique across windows, so each can take the other's id.
        self.attempt(|w| {
            let la = w.layout_mut(ha).ok_or(Refusal::Invalid)?;
            la.replace_leaf(a, b, tb);
            let lb = w.layout_mut(hb).ok_or(Refusal::Invalid)?;
            ok(lb.replace_leaf(b, a, ta))
        })
    }

    /// Moves area `src` into a new floating window of the given size.
    pub fn tear_off(
        &mut self,
        src: AreaId,
        size: (i32, i32),
        ids: &mut IdGen,
    ) -> Result<WindowId, Refusal> {
        let host = self.host_of(src).ok_or(Refusal::Invalid)?;
        if matches!(host, Host::Floating(_))
            && self.layout(host).is_some_and(|l| l.leaves().len() == 1)
        {
            return Err(Refusal::Invalid); // already alone in its window
        }
        let id = WindowId(ids.window());
        self.attempt(|w| {
            let tabs = w.take(src)?;
            w.push_window(id, Layout::single(src, tabs), size);
            Ok(())
        })?;
        Ok(id)
    }

    /// Moves tab `tab` into a new floating window (the whole area when the
    /// tab is alone in it).
    pub fn tear_off_tab(
        &mut self,
        tab: TabId,
        size: (i32, i32),
        ids: &mut IdGen,
    ) -> Result<WindowId, Refusal> {
        let area = self.area_of_tab(tab).ok_or(Refusal::Invalid)?;
        if self.tabs(area).is_some_and(|t| t.len() == 1) {
            return self.tear_off(area, size, ids);
        }
        let id = WindowId(ids.window());
        let new = ids.area();
        self.attempt(|w| {
            let t = w
                .layout_of_mut(area)?
                .remove_tab(tab)
                .ok_or(Refusal::Invalid)?;
            w.push_window(id, Layout::single(new, Tabs::one(t)), size);
            Ok(())
        })?;
        Ok(id)
    }

    fn push_window(&mut self, id: WindowId, layout: Layout, size: (i32, i32)) {
        self.floating.push(Floating {
            id,
            layout,
            width: size.0.clamp(FLOAT_MIN.0, FLOAT_MAX),
            height: size.1.clamp(FLOAT_MIN.1, FLOAT_MAX),
            x: None,
            y: None,
        });
    }

    /// Closes tab `tab`; closing an area's last tab closes the area.
    pub fn close_tab(&mut self, tab: TabId) -> Result<(), Refusal> {
        let area = self.area_of_tab(tab).ok_or(Refusal::Invalid)?;
        if self.tabs(area).is_some_and(|t| t.len() == 1) {
            return self.close_area(area);
        }
        self.attempt(|w| {
            w.layout_of_mut(area)?
                .remove_tab(tab)
                .map(drop)
                .ok_or(Refusal::Invalid)
        })
    }

    /// Shows tab number `index` of area `id`.
    pub fn select_tab(&mut self, id: AreaId, index: usize) -> Result<(), Refusal> {
        ok(self.layout_of_mut(id)?.select_tab(id, index))
    }

    /// Shows the next/previous tab of area `id`.
    pub fn cycle_tab(&mut self, id: AreaId, delta: isize) -> Result<(), Refusal> {
        ok(self.layout_of_mut(id)?.cycle_tab(id, delta))
    }

    /// Closes a floating window with every area in it (refused when it
    /// holds the workspace's last Viewport).
    pub fn close_window(&mut self, id: WindowId) -> Result<(), Refusal> {
        if !self.floating.iter().any(|f| f.id == id) {
            return Err(Refusal::Invalid);
        }
        self.attempt(|w| {
            w.floating.retain(|f| f.id != id);
            Ok(())
        })
    }

    /// Records a floating window's current size.
    pub fn set_window_size(&mut self, id: WindowId, width: i32, height: i32) {
        if let Some(f) = self.floating.iter_mut().find(|f| f.id == id) {
            f.width = width.clamp(FLOAT_MIN.0, FLOAT_MAX);
            f.height = height.clamp(FLOAT_MIN.1, FLOAT_MAX);
        }
    }
}

fn ok(done: bool) -> Result<(), Refusal> {
    if done { Ok(()) } else { Err(Refusal::Invalid) }
}

/// Drop zone for a pointer at `(x, y)` inside a `w`×`h` area: the four
/// outer 25% bands are edges (the nearest edge wins in corners), the rest
/// is the center.
pub fn drop_zone(x: f64, y: f64, w: f64, h: f64) -> DropZone {
    if w <= 0.0 || h <= 0.0 {
        return DropZone::Center;
    }
    let (fx, fy) = (x / w, y / h);
    let candidates = [
        (fx, Side::Left),
        (1.0 - fx, Side::Right),
        (fy, Side::Top),
        (1.0 - fy, Side::Bottom),
    ];
    let (dist, side) = candidates
        .into_iter()
        .fold((f64::INFINITY, Side::Left), |best, c| {
            if c.0 < best.0 { c } else { best }
        });
    if dist < 0.25 {
        DropZone::Edge(side)
    } else {
        DropZone::Center
    }
}

/// Rectangle highlighted for `zone` over an area occupying `r`: the half
/// the dropped area will take, or the whole area for the center.
pub fn zone_rect(zone: DropZone, r: Rect) -> Rect {
    match zone {
        DropZone::Edge(side) => r.part(side, 0.5),
        DropZone::Center => r,
    }
}

/// Width of the strip along a window's border that docks at root level.
pub const EDGE_STRIP: f64 = 12.0;

/// The window edge whose [`EDGE_STRIP`] contains `(x, y)` in a `w`×`h`
/// window, if any.
pub fn window_edge(x: f64, y: f64, w: f64, h: f64) -> Option<Side> {
    if x < 0.0 || y < 0.0 || x > w || y > h {
        return None;
    }
    let candidates = [
        (x, Side::Left),
        (w - x, Side::Right),
        (y, Side::Top),
        (h - y, Side::Bottom),
    ];
    candidates
        .into_iter()
        .filter(|(d, _)| *d < EDGE_STRIP)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| s)
}

/// Split proposed by a corner drag that moved `(dx, dy)` and is now at
/// `(x, y)` inside a `w`×`h` area: along the dominant drag axis, at the
/// pointer. `None` until the drag moved far enough, or when either part
/// would be smaller than [`MIN_AREA`].
pub fn corner_split(x: f64, y: f64, dx: f64, dy: f64, w: f64, h: f64) -> Option<(Dir, f64)> {
    const THRESHOLD: f64 = 8.0;
    if dx.abs().max(dy.abs()) < THRESHOLD {
        return None;
    }
    let (dir, pos, len, min) = if dx.abs() >= dy.abs() {
        (Dir::Horizontal, x, w, MIN_AREA.0)
    } else {
        (Dir::Vertical, y, h, MIN_AREA.1)
    };
    if len < 2.0 * min {
        return None;
    }
    let pos = pos.clamp(min, len - min);
    Some((dir, pos / len))
}

/// Clamps a divider drag: `pos` px into a split of `span` px whose sides
/// stack `count_a`/`count_b` areas along it, keeping every area above
/// `min` px when there is room. `snap` rounds to 10% steps.
pub fn sash_ratio(pos: f64, span: f64, count_a: u32, count_b: u32, min: f64, snap: bool) -> f64 {
    if span <= 0.0 {
        return 0.5;
    }
    let mut ratio = pos / span;
    if snap {
        ratio = (ratio * 10.0).round() / 10.0;
    }
    let (lo, hi) = (f64::from(count_a) * min, span - f64::from(count_b) * min);
    if lo <= hi {
        ratio = ratio.clamp(lo / span, hi / span);
    }
    ratio.clamp(crate::layout::MIN_RATIO, 1.0 - crate::layout::MIN_RATIO)
}

/// A bounded undo/redo history of layout snapshots, separate from the
/// document's undo (layout edits never touch the document).
#[derive(Debug, Clone)]
pub struct LayoutHistory<S> {
    undo: Vec<S>,
    redo: Vec<S>,
    limit: usize,
}

impl<S> LayoutHistory<S> {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Records the state before an edit; clears redo.
    pub fn push(&mut self, before: S) {
        self.undo.push(before);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The state to go back to, given the current one.
    pub fn undo(&mut self, current: S) -> Option<S> {
        let s = self.undo.pop()?;
        self.redo.push(current);
        Some(s)
    }

    pub fn redo(&mut self, current: S) -> Option<S> {
        let s = self.redo.pop()?;
        self.undo.push(current);
        Some(s)
    }

    /// Every recorded state (undo and redo).
    pub fn states(&self) -> impl Iterator<Item = &S> {
        self.undo.iter().chain(&self.redo)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Node;
    use EditorKind::*;

    /// `[SceneTree | [Viewport / Console]]`: ids 2, 4, 5.
    fn ws() -> (WorkspaceLayout, IdGen) {
        let mut ids = IdGen::default();
        let main = Layout::new(
            Node::split(
                Dir::Horizontal,
                0.25,
                Node::leaf(SceneTree),
                Node::split(
                    Dir::Vertical,
                    0.7,
                    Node::leaf(Viewport),
                    Node::leaf(Console),
                ),
            ),
            &mut ids,
        );
        (WorkspaceLayout::new(main), ids)
    }
    const SCENE: AreaId = AreaId(2);
    const VIEW: AreaId = AreaId(4);
    const CONSOLE: AreaId = AreaId(5);

    #[test]
    fn tear_off_and_dock_back() {
        let (mut w, mut ids) = ws();
        let win = w.tear_off(CONSOLE, (400, 300), &mut ids).unwrap();
        assert_eq!(w.host_of(CONSOLE), Some(Host::Floating(win)));
        assert_eq!(w.main.leaves().len(), 2);
        // Alone in its window: tearing off again does nothing.
        assert_eq!(w.tear_off(CONSOLE, (1, 1), &mut ids), Err(Refusal::Invalid));
        // Splitting inside the floating window works like anywhere else.
        let extra = w
            .layout_mut(Host::Floating(win))
            .unwrap()
            .split(CONSOLE, Dir::Vertical, &mut ids)
            .unwrap();
        assert_eq!(w.host_of(extra), Some(Host::Floating(win)));
        // Dock both back; the window disappears with its last area.
        let t = |dst| DockTarget::Area {
            dst,
            zone: DropZone::Edge(Side::Bottom),
            mode: CenterMode::Tab,
        };
        w.dock(extra, t(VIEW), &mut ids).unwrap();
        assert_eq!(w.floating.len(), 1);
        w.dock(CONSOLE, t(VIEW), &mut ids).unwrap();
        assert!(w.floating.is_empty());
        assert_eq!(w.main.leaves().len(), 4);
        let mut sorted = w.ids();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), w.ids().len());
    }

    #[test]
    fn main_keeps_one_area_and_workspace_one_viewport() {
        let mut ids = IdGen::default();
        let mut w = WorkspaceLayout::new(Layout::new(Node::leaf(Console), &mut ids));
        assert_eq!(
            w.tear_off(AreaId(1), (300, 200), &mut ids),
            Err(Refusal::LastArea)
        );
        let (mut w2, mut ids2) = ws();
        assert_eq!(w2.close_area(VIEW), Err(Refusal::LastViewport));
        assert_eq!(w2.join(CONSOLE, VIEW), Err(Refusal::LastViewport));
        assert!(w2.close_area(CONSOLE).is_ok());
        // Replacing the viewport with the scene tree would remove it too.
        let replace = DockTarget::Area {
            dst: VIEW,
            zone: DropZone::Center,
            mode: CenterMode::Replace,
        };
        assert_eq!(
            w2.dock(SCENE, replace, &mut ids2),
            Err(Refusal::LastViewport)
        );
        // A second viewport makes it closable.
        w2.main.split(VIEW, Dir::Horizontal, &mut ids2);
        assert!(w2.close_area(VIEW).is_ok());
    }

    #[test]
    fn center_swaps_and_ctrl_center_replaces() {
        let (mut w, mut ids) = ws();
        let swap = DockTarget::Area {
            dst: CONSOLE,
            zone: DropZone::Center,
            mode: CenterMode::Swap,
        };
        w.dock(SCENE, swap, &mut ids).unwrap();
        assert_eq!(w.main.leaves()[0], (CONSOLE, Console));
        let replace = DockTarget::Area {
            dst: SCENE,
            zone: DropZone::Center,
            mode: CenterMode::Replace,
        };
        w.dock(CONSOLE, replace, &mut ids).unwrap();
        assert_eq!(w.main.leaves(), vec![(VIEW, Viewport), (CONSOLE, Console)]);
        // Dropping onto itself is a no-op.
        let selfdrop = DockTarget::Area {
            dst: VIEW,
            zone: DropZone::Edge(Side::Left),
            mode: CenterMode::Tab,
        };
        assert_eq!(w.dock(VIEW, selfdrop, &mut ids), Err(Refusal::Invalid));
    }

    #[test]
    fn swap_across_windows() {
        let (mut w, mut ids) = ws();
        let win = w.tear_off(CONSOLE, (400, 300), &mut ids).unwrap();
        w.swap(SCENE, CONSOLE).unwrap();
        assert_eq!(w.host_of(SCENE), Some(Host::Floating(win)));
        assert_eq!(w.host_of(CONSOLE), Some(Host::Main));
        assert_eq!(w.editor(SCENE), Some(SceneTree));
    }

    #[test]
    fn window_edge_docks_at_root() {
        let (mut w, mut ids) = ws();
        let target = DockTarget::Edge {
            host: Host::Main,
            side: Side::Top,
        };
        w.dock(CONSOLE, target, &mut ids).unwrap();
        assert_eq!(w.main.leaves()[0], (CONSOLE, Console));
        let r = w.main.leaf_rects()[0].1;
        assert_eq!((r.w, r.h), (1.0, EDGE_SHARE));
        // From a floating window into the main window's edge.
        let win = w.tear_off(SCENE, (300, 300), &mut ids).unwrap();
        let own = DockTarget::Edge {
            host: Host::Floating(win),
            side: Side::Left,
        };
        assert_eq!(w.dock(SCENE, own, &mut ids), Err(Refusal::Invalid));
        let main = DockTarget::Edge {
            host: Host::Main,
            side: Side::Right,
        };
        w.dock(SCENE, main, &mut ids).unwrap();
        assert!(w.floating.is_empty());
    }

    #[test]
    fn close_window_and_sizes() {
        let (mut w, mut ids) = ws();
        let win = w.tear_off(SCENE, (10, 99_999), &mut ids).unwrap();
        assert_eq!((w.floating[0].width, w.floating[0].height), (200, 16_000));
        w.set_window_size(win, 640, 480);
        assert_eq!((w.floating[0].width, w.floating[0].height), (640, 480));
        assert!(w.close_window(win).is_ok());
        assert_eq!(w.close_window(win), Err(Refusal::Invalid));
        assert!(w.host_of(SCENE).is_none());
    }

    #[test]
    fn serde_round_trip_validates_and_reads_bare_layouts() {
        let (mut w, mut ids) = ws();
        w.tear_off(SCENE, (300, 200), &mut ids).unwrap();
        let json = serde_json::to_value(&w).unwrap();
        let back: WorkspaceLayout = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, w);
        // Version 1 blobs stored a bare layout.
        let bare = serde_json::to_value(&w.main).unwrap();
        let old: WorkspaceLayout = serde_json::from_value(bare).unwrap();
        assert_eq!(old.main, w.main);
        assert!(old.floating.is_empty());
        // A window id colliding with an area id is rejected.
        let mut bad = json;
        bad["floating"][0]["id"] = 4.into();
        assert!(serde_json::from_value::<WorkspaceLayout>(bad).is_err());
        // A window id near u32::MAX would make the generator saturate.
        let mut huge = serde_json::to_value(&w).unwrap();
        huge["floating"][0]["id"] = u32::MAX.into();
        assert!(serde_json::from_value::<WorkspaceLayout>(huge).is_err());
        // Reserving moves the generator past window ids too.
        let mut id_gen = IdGen::default();
        back.reserve(&mut id_gen);
        assert!(id_gen.area().0 > *back.ids().iter().max().unwrap());
    }

    #[test]
    fn closing_a_window_keeps_the_last_viewport() {
        let (mut w, mut ids) = ws();
        let win = w.tear_off(VIEW, (300, 200), &mut ids).unwrap();
        assert_eq!(w.close_window(win), Err(Refusal::LastViewport));
        assert_eq!(w.floating.len(), 1);
        // With another viewport in the main window it may close.
        w.main.split(CONSOLE, Dir::Vertical, &mut ids);
        w.main
            .set_editor(*w.main.leaves().last().map(|(a, _)| a).unwrap(), Viewport);
        assert!(w.close_window(win).is_ok());
    }

    #[test]
    fn loading_rejects_workspaces_without_viewport() {
        let mut ids = IdGen::default();
        let w = WorkspaceLayout::new(Layout::new(Node::leaf(Console), &mut ids));
        let json = serde_json::to_value(&w).unwrap();
        assert!(serde_json::from_value::<WorkspaceLayout>(json).is_err());
    }

    /// `ws()` with two more tabs (Sheet, Script) in the console area.
    fn ws_tabs() -> (WorkspaceLayout, IdGen, TabId, TabId) {
        let (mut w, mut ids) = ws();
        let sheet = Tab {
            id: ids.tab(),
            editor: Sheet,
        };
        let script = Tab {
            id: ids.tab(),
            editor: Script,
        };
        w.main.take(CONSOLE).unwrap();
        // Put the console back with three tabs.
        let mut set = Tabs::one(Tab {
            id: TabId(99),
            editor: Console,
        });
        for t in [sheet, script] {
            set.insert(t, None);
        }
        w.main
            .insert(VIEW, Side::Bottom, CONSOLE, set, 0.3, &mut ids);
        ids.reserve_id(99);
        (w, ids, sheet.id, script.id)
    }

    #[test]
    fn center_drop_docks_as_tab() {
        let (mut w, mut ids) = ws();
        let tab = DockTarget::Area {
            dst: VIEW,
            zone: DropZone::Center,
            mode: CenterMode::Tab,
        };
        w.dock(SCENE, tab, &mut ids).unwrap();
        assert!(!w.main.contains(SCENE));
        let tabs = w.tabs(VIEW).unwrap();
        assert_eq!(tabs.len(), 2);
        // The docked tab is shown.
        assert_eq!(tabs.editor(), SceneTree);
    }

    #[test]
    fn tab_drags_split_off_move_and_reorder() {
        let (mut w, mut ids, sheet, script) = ws_tabs();
        // To an edge of its own area: a new area with just that tab.
        let edge = DockTarget::Area {
            dst: CONSOLE,
            zone: DropZone::Edge(Side::Right),
            mode: CenterMode::Tab,
        };
        w.dock_tab(script, edge, &mut ids).unwrap();
        let new = w.area_of_tab(script).unwrap();
        assert_ne!(new, CONSOLE);
        assert_eq!(w.tabs(new).unwrap().len(), 1);
        assert_eq!(w.tabs(CONSOLE).unwrap().len(), 2);
        // Its own center: nothing to do.
        let center = DockTarget::Area {
            dst: CONSOLE,
            zone: DropZone::Center,
            mode: CenterMode::Tab,
        };
        assert_eq!(w.dock_tab(sheet, center, &mut ids), Err(Refusal::Invalid));
        // Onto another strip: moves there at the index.
        let strip = DockTarget::Strip {
            dst: VIEW,
            index: 0,
        };
        w.dock_tab(sheet, strip, &mut ids).unwrap();
        assert_eq!(w.tabs(VIEW).unwrap().list()[0].id, sheet);
        // Within its own strip: reorder.
        let back = DockTarget::Strip {
            dst: VIEW,
            index: 1,
        };
        w.dock_tab(sheet, back, &mut ids).unwrap();
        assert_eq!(w.tabs(VIEW).unwrap().list()[1].id, sheet);
        // A lone tab moves its whole area (here: into the view's strip).
        w.dock_tab(
            script,
            DockTarget::Strip {
                dst: VIEW,
                index: 0,
            },
            &mut ids,
        )
        .unwrap();
        assert!(!w.main.contains(new));
    }

    #[test]
    fn close_and_tear_off_tabs() {
        let (mut w, mut ids, sheet, script) = ws_tabs();
        w.close_tab(sheet).unwrap();
        assert_eq!(w.tabs(CONSOLE).unwrap().len(), 2);
        let win = w.tear_off_tab(script, (300, 200), &mut ids).unwrap();
        assert_eq!(w.tabs(CONSOLE).unwrap().len(), 1);
        let f = &w.floating[0];
        assert_eq!(f.id, win);
        assert_eq!(f.layout.leaves()[0].1, Script);
        // Closing an area's last tab closes the area (with the usual rules).
        let console_tab = w.tabs(CONSOLE).unwrap().active_tab().id;
        w.close_tab(console_tab).unwrap();
        assert!(!w.main.contains(CONSOLE));
        let view_tab = w.tabs(VIEW).unwrap().active_tab().id;
        assert_eq!(w.close_tab(view_tab), Err(Refusal::LastViewport));
        // Select and cycle.
        let (mut w, _, _, _) = ws_tabs();
        w.select_tab(CONSOLE, 0).unwrap();
        assert_eq!(w.editor(CONSOLE), Some(Console));
        w.cycle_tab(CONSOLE, -1).unwrap();
        assert_eq!(w.editor(CONSOLE), Some(Script));
    }

    #[test]
    fn last_viewport_counts_background_tabs() {
        let (mut w, mut ids) = ws();
        // Dock the only viewport as a tab behind the console.
        let tab = DockTarget::Area {
            dst: CONSOLE,
            zone: DropZone::Center,
            mode: CenterMode::Tab,
        };
        w.dock(VIEW, tab, &mut ids).unwrap();
        w.select_tab(CONSOLE, 0).unwrap();
        assert_eq!(w.editor(CONSOLE), Some(Console));
        // Closing the area would drop the hidden viewport too.
        assert_eq!(w.close_area(CONSOLE), Err(Refusal::LastViewport));
    }

    #[test]
    fn drop_zones() {
        assert_eq!(
            drop_zone(5.0, 50.0, 100.0, 100.0),
            DropZone::Edge(Side::Left)
        );
        assert_eq!(
            drop_zone(95.0, 50.0, 100.0, 100.0),
            DropZone::Edge(Side::Right)
        );
        assert_eq!(
            drop_zone(50.0, 10.0, 100.0, 100.0),
            DropZone::Edge(Side::Top)
        );
        assert_eq!(
            drop_zone(50.0, 90.0, 100.0, 100.0),
            DropZone::Edge(Side::Bottom)
        );
        assert_eq!(drop_zone(50.0, 50.0, 100.0, 100.0), DropZone::Center);
        // Corner: the nearer edge wins.
        assert_eq!(
            drop_zone(2.0, 10.0, 100.0, 100.0),
            DropZone::Edge(Side::Left)
        );
        assert_eq!(drop_zone(0.0, 0.0, 0.0, 10.0), DropZone::Center);
        let r = Rect::new(10.0, 10.0, 100.0, 50.0);
        assert_eq!(
            zone_rect(DropZone::Edge(Side::Right), r),
            Rect::new(60.0, 10.0, 50.0, 50.0)
        );
        assert_eq!(zone_rect(DropZone::Center, r), r);
    }

    #[test]
    fn window_edges() {
        assert_eq!(window_edge(3.0, 300.0, 800.0, 600.0), Some(Side::Left));
        assert_eq!(window_edge(400.0, 595.0, 800.0, 600.0), Some(Side::Bottom));
        assert_eq!(window_edge(400.0, 300.0, 800.0, 600.0), None);
        assert_eq!(window_edge(-1.0, 300.0, 800.0, 600.0), None);
    }

    #[test]
    fn corner_split_follows_dominant_axis() {
        assert_eq!(corner_split(10.0, 10.0, 3.0, 2.0, 600.0, 400.0), None);
        let (dir, r) = corner_split(300.0, 20.0, 40.0, 5.0, 600.0, 400.0).unwrap();
        assert_eq!(dir, Dir::Horizontal);
        assert!((r - 0.5).abs() < 1e-9);
        let (dir, r) = corner_split(5.0, 10.0, 2.0, 30.0, 600.0, 400.0).unwrap();
        assert_eq!(dir, Dir::Vertical);
        assert!((r - 0.2).abs() < 1e-9); // clamped to 80 px
        assert_eq!(corner_split(50.0, 10.0, 40.0, 0.0, 200.0, 400.0), None);
    }

    #[test]
    fn sash_snaps_and_keeps_minimum() {
        assert!((sash_ratio(330.0, 1000.0, 1, 1, 120.0, false) - 0.33).abs() < 1e-9);
        assert!((sash_ratio(330.0, 1000.0, 1, 1, 120.0, true) - 0.3).abs() < 1e-9);
        assert!((sash_ratio(50.0, 1000.0, 1, 1, 120.0, false) - 0.12).abs() < 1e-9);
        // Two areas stacked on the right need 240 px.
        assert!((sash_ratio(900.0, 1000.0, 1, 2, 120.0, false) - 0.76).abs() < 1e-9);
        // Too small for the minimum: only the ratio bounds apply.
        assert!((sash_ratio(10.0, 100.0, 1, 1, 120.0, false) - 0.1).abs() < 1e-9);
        assert_eq!(sash_ratio(10.0, 0.0, 1, 1, 120.0, false), 0.5);
    }

    #[test]
    fn history_undo_redo_and_limit() {
        let mut h = LayoutHistory::new(2);
        assert!(!h.can_undo());
        h.push(1);
        h.push(2);
        h.push(3); // drops 1
        assert_eq!(h.undo(4), Some(3));
        assert!(h.can_redo());
        assert_eq!(h.redo(3), Some(4));
        assert_eq!(h.undo(4), Some(3));
        assert_eq!(h.undo(3), Some(2));
        assert_eq!(h.undo(2), None);
        h.push(9);
        assert!(!h.can_redo());
        h.clear();
        assert!(!h.can_undo());
    }
}

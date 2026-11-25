//! Pure area layout model (REQUIREMENTS §2), independent of GTK.
//!
//! A layout is a binary tree: inner nodes split their space in two along a
//! direction, leaves are *areas* each hosting one editor. Widgets are rebuilt
//! from this model whenever it changes structurally, so all operations here
//! (split, close/join, swap, change editor, maximize) are plain data edits that can
//! be unit tested and serialized.

use serde::{Deserialize, Serialize};

/// The editors an area can host (REQUIREMENTS §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EditorKind {
    Viewport,
    SceneTree,
    Inspector,
    Console,
    Expressions,
    Sheet,
    Script,
    DependencyGraph,
}

impl EditorKind {
    /// Every editor, in menu order.
    pub const ALL: [EditorKind; 8] = [
        EditorKind::Viewport,
        EditorKind::SceneTree,
        EditorKind::Inspector,
        EditorKind::Console,
        EditorKind::Expressions,
        EditorKind::Sheet,
        EditorKind::Script,
        EditorKind::DependencyGraph,
    ];

    /// Stable identifier, used as action target.
    pub fn id(self) -> &'static str {
        match self {
            EditorKind::Viewport => "viewport",
            EditorKind::SceneTree => "scene-tree",
            EditorKind::Inspector => "inspector",
            EditorKind::Console => "console",
            EditorKind::Expressions => "expressions",
            EditorKind::Sheet => "sheet",
            EditorKind::Script => "script",
            EditorKind::DependencyGraph => "dependency-graph",
        }
    }

    /// Looks an editor up by [`EditorKind::id`].
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }

    /// Human readable name.
    pub fn title(self) -> &'static str {
        match self {
            EditorKind::Viewport => "Viewport",
            EditorKind::SceneTree => "Scene Tree",
            EditorKind::Inspector => "Inspector",
            EditorKind::Console => "Console",
            EditorKind::Expressions => "Expressions",
            EditorKind::Sheet => "Sheet",
            EditorKind::Script => "Script",
            EditorKind::DependencyGraph => "Dependency Graph",
        }
    }

    /// One-line description of what the editor is for.
    pub fn description(self) -> &'static str {
        match self {
            EditorKind::Viewport => "Renders a viewport node: plots, points and geometry.",
            EditorKind::SceneTree => {
                "Every node of the document as a tree: rename, reparent, hide, lock."
            }
            EditorKind::Inspector => "Properties of the selected nodes, with drag-to-scrub fields.",
            EditorKind::Console => {
                "Evaluate expressions exactly, convert units and pin results as nodes."
            }
            EditorKind::Expressions => "Flat, ordered list of statement nodes for quick editing.",
            EditorKind::Sheet => "Spreadsheet editor for table nodes; columns are lists.",
            EditorKind::Script => "Code editor for script nodes, in the Viscas language or Rhai.",
            EditorKind::DependencyGraph => {
                "Nodes as boxes, references as edges; rewire to rewrite."
            }
        }
    }

    /// Short, slightly playful line shown under the description in the
    /// editor's empty state.
    pub fn quip(self) -> &'static str {
        match self {
            EditorKind::Viewport => "The plane is yours.",
            EditorKind::SceneTree => "No nodes yet. Every tree starts as a seed.",
            EditorKind::Inspector => "Select something and it will tell you everything.",
            EditorKind::Console => "Ask it anything. It prefers exact answers.",
            EditorKind::Expressions => "One line at a time.",
            EditorKind::Sheet => "Rows and columns, waiting for numbers.",
            EditorKind::Script => "Teach the scene a few new tricks.",
            EditorKind::DependencyGraph => "Who depends on whom, drawn as boxes and wires.",
        }
    }

    /// Milestone in which the editor becomes functional (REQUIREMENTS §13).
    pub fn milestone(self) -> &'static str {
        match self {
            EditorKind::Viewport => "M0",
            EditorKind::Console => "M1",
            EditorKind::SceneTree | EditorKind::Inspector | EditorKind::Expressions => "M2",
            EditorKind::DependencyGraph => "M4",
            EditorKind::Sheet => "M6",
            EditorKind::Script => "M7",
        }
    }
}

/// Direction in which a split divides its space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir {
    /// Children side by side: `a` left, `b` right.
    Horizontal,
    /// Children stacked: `a` top, `b` bottom.
    Vertical,
}

impl Dir {
    pub fn id(self) -> &'static str {
        match self {
            Dir::Horizontal => "horizontal",
            Dir::Vertical => "vertical",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "horizontal" => Some(Dir::Horizontal),
            "vertical" => Some(Dir::Vertical),
            _ => None,
        }
    }
}

/// A side of an area, or of a whole window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    #[cfg(test)]
    pub const ALL: [Side; 4] = [Side::Left, Side::Right, Side::Top, Side::Bottom];

    /// Direction of the split that puts something on this side.
    pub fn dir(self) -> Dir {
        match self {
            Side::Left | Side::Right => Dir::Horizontal,
            Side::Top | Side::Bottom => Dir::Vertical,
        }
    }

    /// Whether this side is the `a` (left/top) child of such a split.
    pub fn is_start(self) -> bool {
        matches!(self, Side::Left | Side::Top)
    }
}

/// An axis-aligned rectangle, in layout-relative units (the root is the
/// unit square) or in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const UNIT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };

    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    /// Smallest rectangle covering both.
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        let r = (self.x + self.w).max(o.x + o.w);
        let b = (self.y + self.h).max(o.y + o.h);
        Rect::new(x, y, r - x, b - y)
    }

    /// Start and length along `dir`.
    fn span(&self, dir: Dir) -> (f64, f64) {
        match dir {
            Dir::Horizontal => (self.x, self.w),
            Dir::Vertical => (self.y, self.h),
        }
    }

    /// The two parts of a split along `dir` giving `ratio` to the first.
    pub fn split(&self, dir: Dir, ratio: f64) -> (Rect, Rect) {
        match dir {
            Dir::Horizontal => {
                let wa = self.w * ratio;
                (
                    Rect::new(self.x, self.y, wa, self.h),
                    Rect::new(self.x + wa, self.y, self.w - wa, self.h),
                )
            }
            Dir::Vertical => {
                let ha = self.h * ratio;
                (
                    Rect::new(self.x, self.y, self.w, ha),
                    Rect::new(self.x, self.y + ha, self.w, self.h - ha),
                )
            }
        }
    }

    /// The part of the rectangle on `side`, `share` of its extent.
    pub fn part(&self, side: Side, share: f64) -> Rect {
        if side.is_start() {
            self.split(side.dir(), share).0
        } else {
            self.split(side.dir(), 1.0 - share).1
        }
    }
}

/// Identifies an area. Unique across every layout sharing one [`IdGen`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AreaId(pub u32);

/// Identifies a split, so divider positions survive structural edits
/// elsewhere in the tree. Shares the id space of [`AreaId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SplitId(pub u32);

/// Hands out area and split ids. One generator is shared by all layouts of
/// a window, so ids are globally unique and never reused.
#[derive(Debug, Clone)]
pub struct IdGen {
    next: u32,
}

impl Default for IdGen {
    fn default() -> Self {
        Self { next: 1 }
    }
}

impl IdGen {
    fn next(&mut self) -> u32 {
        let id = self.next;
        self.next = self.next.saturating_add(1);
        id
    }

    pub fn area(&mut self) -> AreaId {
        AreaId(self.next())
    }

    pub fn split(&mut self) -> SplitId {
        SplitId(self.next())
    }

    pub fn tab(&mut self) -> TabId {
        TabId(self.next())
    }

    /// A fresh id for a floating window (same id space).
    pub fn window(&mut self) -> u32 {
        self.next()
    }

    /// Makes sure future ids are greater than `id`.
    pub fn reserve_id(&mut self, id: u32) {
        self.next = self.next.max(id.saturating_add(1));
    }

    /// Makes sure future ids are greater than every id of `layout`.
    #[cfg(test)]
    pub fn reserve(&mut self, layout: &Layout) {
        self.next = self.next.max(layout.max_id().saturating_add(1));
        for t in layout.tab_ids() {
            self.reserve_id(t);
        }
    }
}

/// Largest id a loaded layout may use. Ids are handed out by counting
/// up, so a crafted file with an id near `u32::MAX` would make the
/// generator saturate and repeat ids; such files are rejected instead.
pub const MAX_ID: u32 = 1 << 30;

/// Smallest fraction of a split either child may get.
pub const MIN_RATIO: f64 = 0.05;

/// Identifies an editor tab. Tab ids are unique among tabs (a separate
/// namespace from area and split ids, drawn from the same [`IdGen`]);
/// editor widgets are cached by tab id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TabId(pub u32);

/// One editor tab of an area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub editor: EditorKind,
}

/// The ordered tabs of an area and the shown one. Never empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tabs {
    list: Vec<Tab>,
    active: usize,
}

#[derive(Deserialize)]
struct RawTabs {
    list: Vec<Tab>,
    #[serde(default)]
    active: usize,
}

impl<'de> Deserialize<'de> for Tabs {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = RawTabs::deserialize(d)?;
        if raw.list.is_empty() {
            return Err(serde::de::Error::custom("an area needs at least one tab"));
        }
        let active = raw.active.min(raw.list.len() - 1);
        Ok(Tabs {
            list: raw.list,
            active,
        })
    }
}

impl Tabs {
    pub fn one(tab: Tab) -> Self {
        Self {
            list: vec![tab],
            active: 0,
        }
    }

    pub fn list(&self) -> &[Tab] {
        &self.list
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn active_tab(&self) -> Tab {
        self.list[self.active]
    }

    /// Editor of the shown tab.
    pub fn editor(&self) -> EditorKind {
        self.active_tab().editor
    }

    pub fn position(&self, id: TabId) -> Option<usize> {
        self.list.iter().position(|t| t.id == id)
    }

    /// Inserts `tab` at `index` (end if `None` or past the end) and shows it.
    #[cfg(test)]
    pub fn insert(&mut self, tab: Tab, index: Option<usize>) {
        let i = index.unwrap_or(self.list.len()).min(self.list.len());
        self.list.insert(i, tab);
        self.active = i;
    }

    /// Removes tab `id`, keeping at least one tab. The neighbour to the
    /// right (else left) of a removed shown tab becomes shown.
    pub fn remove(&mut self, id: TabId) -> Option<Tab> {
        if self.list.len() < 2 {
            return None;
        }
        let i = self.position(id)?;
        let tab = self.list.remove(i);
        if self.active > i || self.active >= self.list.len() {
            self.active = self.active.saturating_sub(1);
        }
        Some(tab)
    }

    pub fn select(&mut self, index: usize) -> bool {
        if index < self.list.len() && index != self.active {
            self.active = index;
            true
        } else {
            false
        }
    }

    /// Moves tab `id` to `index` (counted after removal), keeping it shown
    /// if it was.
    pub fn reorder(&mut self, id: TabId, index: usize) -> bool {
        let Some(from) = self.position(id) else {
            return false;
        };
        let shown = self.active_tab().id;
        let tab = self.list.remove(from);
        let to = index.min(self.list.len());
        self.list.insert(to, tab);
        self.active = self.position(shown).unwrap_or(0);
        from != to
    }

    fn set_editor(&mut self, editor: EditorKind) {
        self.list[self.active].editor = editor;
    }

    /// Appends every tab of `other`; the shown tab of `other` is shown.
    fn absorb(&mut self, other: Tabs, index: Option<usize>) {
        let base = index.unwrap_or(self.list.len()).min(self.list.len());
        let shown = base + other.active;
        for (k, t) in other.list.into_iter().enumerate() {
            self.list.insert(base + k, t);
        }
        self.active = shown;
    }
}

/// A node of the layout tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawNode")]
pub enum Node {
    Split {
        id: SplitId,
        dir: Dir,
        /// Fraction of the space given to `a`, in `MIN_RATIO..=1-MIN_RATIO`.
        ratio: f64,
        a: Box<Node>,
        b: Box<Node>,
    },
    Leaf {
        id: AreaId,
        tabs: Tabs,
    },
}

/// Serialized node; leaves written before tabs (layout blob v1/v2) have
/// a single `editor`, whose tab takes the area's id.
#[derive(Deserialize)]
enum RawNode {
    Split {
        id: SplitId,
        dir: Dir,
        ratio: f64,
        a: Box<RawNode>,
        b: Box<RawNode>,
    },
    Leaf {
        id: AreaId,
        #[serde(default)]
        editor: Option<EditorKind>,
        #[serde(default)]
        tabs: Option<Tabs>,
    },
}

impl TryFrom<RawNode> for Node {
    type Error = String;

    fn try_from(raw: RawNode) -> Result<Self, String> {
        Ok(match raw {
            RawNode::Split {
                id,
                dir,
                ratio,
                a,
                b,
            } => Node::Split {
                id,
                dir,
                ratio,
                a: Box::new(Node::try_from(*a)?),
                b: Box::new(Node::try_from(*b)?),
            },
            RawNode::Leaf {
                id,
                tabs: Some(tabs),
                ..
            } => Node::Leaf { id, tabs },
            RawNode::Leaf {
                id,
                editor: Some(editor),
                tabs: None,
            } => Node::Leaf {
                id,
                tabs: Tabs::one(Tab {
                    id: TabId(id.0),
                    editor,
                }),
            },
            RawNode::Leaf { .. } => return Err("an area has no editor".to_owned()),
        })
    }
}

impl Node {
    /// A one-tab leaf; its ids are assigned by [`Layout::new`].
    pub fn leaf(editor: EditorKind) -> Self {
        Node::Leaf {
            id: AreaId(0),
            tabs: Tabs::one(Tab {
                id: TabId(0),
                editor,
            }),
        }
    }

    /// A leaf with several tabs (the first shown); ids are assigned by
    /// [`Layout::new`].
    #[cfg(test)]
    pub fn tabbed(editors: &[EditorKind]) -> Self {
        let mut n = Node::leaf(editors[0]);
        if let Node::Leaf { tabs, .. } = &mut n {
            for e in &editors[1..] {
                tabs.list.push(Tab {
                    id: TabId(0),
                    editor: *e,
                });
            }
        }
        n
    }

    /// A split giving `ratio` of the space to `a`; its id is assigned by
    /// [`Layout::new`].
    pub fn split(dir: Dir, ratio: f64, a: Node, b: Node) -> Self {
        Node::Split {
            id: SplitId(0),
            dir,
            ratio: clamp_ratio(ratio),
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    fn find_leaf(&self, target: AreaId) -> Option<&Node> {
        match self {
            Node::Leaf { id, .. } if *id == target => Some(self),
            Node::Leaf { .. } => None,
            Node::Split { a, b, .. } => a.find_leaf(target).or_else(|| b.find_leaf(target)),
        }
    }

    fn find_leaf_mut(&mut self, target: AreaId) -> Option<&mut Node> {
        match self {
            Node::Leaf { id, .. } if *id == target => Some(self),
            Node::Leaf { .. } => None,
            Node::Split { a, b, .. } => match a.find_leaf_mut(target) {
                Some(n) => Some(n),
                None => b.find_leaf_mut(target),
            },
        }
    }

    #[cfg(test)]
    fn find_split(&self, target: SplitId) -> Option<&Node> {
        match self {
            Node::Leaf { .. } => None,
            Node::Split { id, .. } if *id == target => Some(self),
            Node::Split { a, b, .. } => a.find_split(target).or_else(|| b.find_split(target)),
        }
    }

    fn find_split_mut(&mut self, target: SplitId) -> Option<&mut Node> {
        match self {
            Node::Leaf { .. } => None,
            Node::Split { id, .. } if *id == target => Some(self),
            Node::Split { a, b, .. } => match a.find_split_mut(target) {
                Some(n) => Some(n),
                None => b.find_split_mut(target),
            },
        }
    }

    fn collect_leaves(&self, out: &mut Vec<(AreaId, EditorKind)>) {
        match self {
            Node::Leaf { id, tabs } => out.push((*id, tabs.editor())),
            Node::Split { a, b, .. } => {
                a.collect_leaves(out);
                b.collect_leaves(out);
            }
        }
    }

    /// Every id in the subtree (areas and splits share one id space).
    fn collect_ids(&self, out: &mut Vec<u32>) {
        match self {
            Node::Leaf { id, .. } => out.push(id.0),
            Node::Split { id, a, b, .. } => {
                out.push(id.0);
                a.collect_ids(out);
                b.collect_ids(out);
            }
        }
    }

    /// Rectangle of every node (by id) when the node fills `r`.
    fn collect_rects(&self, r: Rect, out: &mut Vec<(u32, Rect)>) {
        match self {
            Node::Leaf { id, .. } => out.push((id.0, r)),
            Node::Split {
                id,
                dir,
                ratio,
                a,
                b,
            } => {
                out.push((id.0, r));
                let (ra, rb) = r.split(*dir, *ratio);
                a.collect_rects(ra, out);
                b.collect_rects(rb, out);
            }
        }
    }

    /// Path from this node to leaf `target`: `false` for `a`, `true` for `b`.
    fn path_to(&self, target: AreaId, path: &mut Vec<bool>) -> bool {
        match self {
            Node::Leaf { id, .. } => *id == target,
            Node::Split { a, b, .. } => {
                for (side, child) in [(false, a), (true, b)] {
                    path.push(side);
                    if child.path_to(target, path) {
                        return true;
                    }
                    path.pop();
                }
                false
            }
        }
    }

    fn at(&self, path: &[bool]) -> &Node {
        match (self, path.split_first()) {
            (Node::Split { a, b, .. }, Some((side, rest))) => {
                if *side {
                    b.at(rest)
                } else {
                    a.at(rest)
                }
            }
            _ => self,
        }
    }

    /// Sets the ratios of the splits listed in `only` so that the leaves
    /// get the rectangles of `want`. Returns the subtree's bounding box.
    fn fit(&mut self, want: &[(u32, Rect)], only: &[u32]) -> Option<Rect> {
        match self {
            Node::Leaf { id, .. } => want.iter().find(|(i, _)| *i == id.0).map(|(_, r)| *r),
            Node::Split {
                id,
                dir,
                ratio,
                a,
                b,
            } => {
                let ra = a.fit(want, only)?;
                let rb = b.fit(want, only)?;
                let all = ra.union(&rb);
                if only.contains(&id.0) {
                    let (start, len) = all.span(*dir);
                    let (a_start, a_len) = ra.span(*dir);
                    if len > 0.0 {
                        *ratio = clamp_ratio((a_start + a_len - start) / len);
                    }
                }
                Some(all)
            }
        }
    }

    /// How many areas this subtree stacks along `dir`, at most (used to
    /// keep areas above their minimum size when dragging dividers).
    pub fn depth_along(&self, dir: Dir) -> u32 {
        match self {
            Node::Leaf { .. } => 1,
            Node::Split { dir: d, a, b, .. } if *d == dir => {
                a.depth_along(dir) + b.depth_along(dir)
            }
            Node::Split { a, b, .. } => a.depth_along(dir).max(b.depth_along(dir)),
        }
    }

    fn collect_tabs(&self, out: &mut Vec<(AreaId, Tab)>) {
        match self {
            Node::Leaf { id, tabs } => out.extend(tabs.list.iter().map(|t| (*id, *t))),
            Node::Split { a, b, .. } => {
                a.collect_tabs(out);
                b.collect_tabs(out);
            }
        }
    }

    /// Visits every node in pre-order.
    fn for_each_mut(&mut self, f: &mut impl FnMut(&mut Node)) {
        f(self);
        if let Node::Split { a, b, .. } = self {
            a.for_each_mut(f);
            b.for_each_mut(f);
        }
    }

    /// Replaces the split whose child is the leaf `target` by the other child.
    /// Returns whether the leaf was found below a split.
    fn remove_leaf(&mut self, target: AreaId) -> bool {
        let Node::Split { a, b, .. } = self else {
            return false;
        };
        let is_target = |n: &Node| matches!(n, Node::Leaf { id, .. } if *id == target);
        let sibling = if is_target(a) {
            Some(std::mem::replace(
                b.as_mut(),
                Node::leaf(EditorKind::Viewport),
            ))
        } else if is_target(b) {
            Some(std::mem::replace(
                a.as_mut(),
                Node::leaf(EditorKind::Viewport),
            ))
        } else {
            None
        };
        match sibling {
            Some(s) => {
                *self = s;
                true
            }
            None => a.remove_leaf(target) || b.remove_leaf(target),
        }
    }
}

fn clamp_ratio(ratio: f64) -> f64 {
    if ratio.is_finite() {
        ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO)
    } else {
        0.5
    }
}

/// Why a deserialized layout was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// An id is used twice (ids are unique across areas and splits).
    DuplicateId(u32),
    /// Id 0 is reserved for nodes not yet numbered by [`Layout::new`].
    ZeroId,
    /// An id above [`MAX_ID`].
    IdTooLarge(u32),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::DuplicateId(id) => write!(f, "layout id {id} is used more than once"),
            LayoutError::ZeroId => f.write_str("layout id 0 is reserved"),
            LayoutError::IdTooLarge(id) => write!(f, "layout id {id} is too large"),
        }
    }
}

/// Serialized form of a [`Layout`], validated on the way in.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawLayout {
    root: Node,
    maximized: Option<AreaId>,
}

/// An area layout: the split tree plus maximize state.
///
/// Ids come from an external [`IdGen`] rather than a per-layout counter, so
/// they are unique across all workspaces. Deserialization validates the
/// tree: ids must be non-zero and unique, ratios are clamped, and a
/// maximized area that does not exist is dropped. After loading, call
/// [`IdGen::reserve`] so new ids do not collide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawLayout", into = "RawLayout")]
pub struct Layout {
    root: Node,
    maximized: Option<AreaId>,
}

impl TryFrom<RawLayout> for Layout {
    type Error = LayoutError;

    fn try_from(raw: RawLayout) -> Result<Self, Self::Error> {
        let mut root = raw.root;
        let mut ids = Vec::new();
        root.collect_ids(&mut ids);
        ids.sort_unstable();
        if ids.first() == Some(&0) {
            return Err(LayoutError::ZeroId);
        }
        if let Some(&id) = ids.last().filter(|id| **id > MAX_ID) {
            return Err(LayoutError::IdTooLarge(id));
        }
        if let Some(pair) = ids.windows(2).find(|w| w[0] == w[1]) {
            return Err(LayoutError::DuplicateId(pair[0]));
        }
        // Tab ids: their own namespace, same rules.
        let mut tabs = Vec::new();
        root.collect_tabs(&mut tabs);
        let mut tids: Vec<u32> = tabs.iter().map(|(_, t)| t.id.0).collect();
        tids.sort_unstable();
        if tids.first() == Some(&0) {
            return Err(LayoutError::ZeroId);
        }
        if let Some(&id) = tids.last().filter(|id| **id > MAX_ID) {
            return Err(LayoutError::IdTooLarge(id));
        }
        if let Some(pair) = tids.windows(2).find(|w| w[0] == w[1]) {
            return Err(LayoutError::DuplicateId(pair[0]));
        }
        root.for_each_mut(&mut |n| {
            if let Node::Split { ratio, .. } = n {
                *ratio = clamp_ratio(*ratio);
            }
        });
        let maximized = raw.maximized.filter(|id| root.find_leaf(*id).is_some());
        Ok(Self { root, maximized })
    }
}

impl From<Layout> for RawLayout {
    fn from(layout: Layout) -> Self {
        Self {
            root: layout.root,
            maximized: layout.maximized,
        }
    }
}

impl Layout {
    /// Creates a layout from `root`, numbering its nodes (pre-order) with
    /// fresh ids from `ids`.
    pub fn new(mut root: Node, ids: &mut IdGen) -> Self {
        // Nodes first (pre-order), then tabs.
        root.for_each_mut(&mut |n| match n {
            Node::Leaf { id, .. } => *id = ids.area(),
            Node::Split { id, .. } => *id = ids.split(),
        });
        root.for_each_mut(&mut |n| {
            if let Node::Leaf { tabs, .. } = n {
                for t in &mut tabs.list {
                    t.id = ids.tab();
                }
            }
        });
        Self {
            root,
            maximized: None,
        }
    }

    /// The full tree, ignoring maximize state.
    #[cfg(test)]
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// The tree to display: the maximized area alone, or the full tree.
    pub fn visible_root(&self) -> &Node {
        self.maximized
            .and_then(|id| self.root.find_leaf(id))
            .unwrap_or(&self.root)
    }

    /// The maximized area, if any.
    pub fn maximized(&self) -> Option<AreaId> {
        self.maximized
    }

    /// All areas with their editors, left to right / top to bottom.
    pub fn leaves(&self) -> Vec<(AreaId, EditorKind)> {
        let mut out = Vec::new();
        self.root.collect_leaves(&mut out);
        out
    }

    /// Every tab with its area, in area order.
    pub fn all_tabs(&self) -> Vec<(AreaId, Tab)> {
        let mut out = Vec::new();
        self.root.collect_tabs(&mut out);
        out
    }

    /// Every tab id.
    #[cfg(test)]
    pub fn tab_ids(&self) -> Vec<u32> {
        self.all_tabs().into_iter().map(|(_, t)| t.id.0).collect()
    }

    /// The tabs of area `id`.
    pub fn tabs(&self, id: AreaId) -> Option<&Tabs> {
        match self.root.find_leaf(id) {
            Some(Node::Leaf { tabs, .. }) => Some(tabs),
            _ => None,
        }
    }

    fn tabs_mut(&mut self, id: AreaId) -> Option<&mut Tabs> {
        match self.root.find_leaf_mut(id) {
            Some(Node::Leaf { tabs, .. }) => Some(tabs),
            _ => None,
        }
    }

    /// The area holding tab `tab`.
    pub fn area_of_tab(&self, tab: TabId) -> Option<AreaId> {
        self.all_tabs()
            .into_iter()
            .find(|(_, t)| t.id == tab)
            .map(|(a, _)| a)
    }

    /// Adds `tab` to area `id` at `index` (end if `None`) and shows it.
    #[cfg(test)]
    pub fn add_tab(&mut self, id: AreaId, tab: Tab, index: Option<usize>) -> bool {
        if self.area_of_tab(tab.id).is_some() {
            return false;
        }
        match self.tabs_mut(id) {
            Some(tabs) => {
                tabs.insert(tab, index);
                true
            }
            None => false,
        }
    }

    /// Adds every tab of `other` to area `id` at `index`.
    pub fn add_tabs(&mut self, id: AreaId, other: Tabs, index: Option<usize>) -> bool {
        if other.list.iter().any(|t| self.area_of_tab(t.id).is_some()) {
            return false;
        }
        match self.tabs_mut(id) {
            Some(tabs) => {
                tabs.absorb(other, index);
                true
            }
            None => false,
        }
    }

    /// Removes tab `tab` from an area that keeps at least one tab.
    pub fn remove_tab(&mut self, tab: TabId) -> Option<Tab> {
        let area = self.area_of_tab(tab)?;
        self.tabs_mut(area)?.remove(tab)
    }

    /// Changes the editor of tab `tab` (shown or not).
    pub fn set_tab_editor(&mut self, tab: TabId, editor: EditorKind) -> bool {
        let Some(area) = self.area_of_tab(tab) else {
            return false;
        };
        let Some(tabs) = self.tabs_mut(area) else {
            return false;
        };
        match tabs.list.iter_mut().find(|t| t.id == tab) {
            Some(t) => {
                t.editor = editor;
                true
            }
            None => false,
        }
    }

    /// Shows tab number `index` of area `id`.
    pub fn select_tab(&mut self, id: AreaId, index: usize) -> bool {
        self.tabs_mut(id).is_some_and(|t| t.select(index))
    }

    /// Shows the next (`delta` 1) or previous (-1) tab of area `id`,
    /// wrapping around.
    pub fn cycle_tab(&mut self, id: AreaId, delta: isize) -> bool {
        let Some(tabs) = self.tabs_mut(id) else {
            return false;
        };
        let n = tabs.len() as isize;
        let next = (tabs.active as isize + delta).rem_euclid(n) as usize;
        tabs.select(next)
    }

    /// Moves tab `tab` to position `index` within its area.
    pub fn reorder_tab(&mut self, tab: TabId, index: usize) -> bool {
        let Some(area) = self.area_of_tab(tab) else {
            return false;
        };
        self.tabs_mut(area).is_some_and(|t| t.reorder(tab, index))
    }

    /// Every area and split id of the tree.
    pub fn ids(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        self.root.collect_ids(&mut ids);
        ids
    }

    /// Largest id used by the layout.
    #[cfg(test)]
    pub fn max_id(&self) -> u32 {
        let mut ids = Vec::new();
        self.root.collect_ids(&mut ids);
        ids.extend(self.tab_ids());
        ids.into_iter().max().unwrap_or(0)
    }

    /// Whether `id` is an area of this layout.
    pub fn contains(&self, id: AreaId) -> bool {
        self.root.find_leaf(id).is_some()
    }

    /// The editor shown in area `id` (its shown tab's).
    pub fn editor(&self, id: AreaId) -> Option<EditorKind> {
        self.tabs(id).map(Tabs::editor)
    }

    /// Splits area `id` in two along `dir`. The new area (`b`, right or
    /// bottom) gets one tab with the shown editor. Returns the new area's
    /// id. Leaves maximize mode, since the result must be visible.
    pub fn split(&mut self, id: AreaId, dir: Dir, ids: &mut IdGen) -> Option<AreaId> {
        let node = self.root.find_leaf_mut(id)?;
        let Node::Leaf { tabs, .. } = node else {
            return None;
        };
        let editor = tabs.editor();
        let old = std::mem::replace(node, Node::leaf(editor));
        let new_id = ids.area();
        let new_tab = Tab {
            id: ids.tab(),
            editor,
        };
        *node = Node::Split {
            id: ids.split(),
            dir,
            ratio: 0.5,
            a: Box::new(old),
            b: Box::new(Node::Leaf {
                id: new_id,
                tabs: Tabs::one(new_tab),
            }),
        };
        self.maximized = None;
        Some(new_id)
    }

    /// Closes area `id`; its sibling grows to take the freed space (a
    /// Blender "join"). The last remaining area cannot be closed.
    pub fn close(&mut self, id: AreaId) -> bool {
        if !self.root.remove_leaf(id) {
            return false;
        }
        if self.maximized == Some(id) {
            self.maximized = None;
        }
        true
    }

    /// Shows `editor` in area `id`'s shown tab.
    pub fn set_editor(&mut self, id: AreaId, editor: EditorKind) -> bool {
        match self.tabs_mut(id) {
            Some(tabs) => {
                tabs.set_editor(editor);
                true
            }
            None => false,
        }
    }

    /// Exchanges the places of areas `a` and `b` with all their tabs.
    /// Returns whether both exist and differ.
    pub fn swap(&mut self, a: AreaId, b: AreaId) -> bool {
        let (Some(ta), Some(tb)) = (self.tabs(a).cloned(), self.tabs(b).cloned()) else {
            return false;
        };
        if a == b {
            return false;
        }
        self.root.for_each_mut(&mut |n| {
            if let Node::Leaf { id, tabs } = n {
                if *id == a {
                    (*id, *tabs) = (b, tb.clone());
                } else if *id == b {
                    (*id, *tabs) = (a, ta.clone());
                }
            }
        });
        true
    }

    /// Maximizes area `id`, or restores the full layout when an area is
    /// already maximized. Returns whether anything changed.
    pub fn toggle_maximize(&mut self, id: AreaId) -> bool {
        if self.maximized.is_some() {
            self.maximized = None;
            return true;
        }
        if !self.contains(id) {
            return false;
        }
        self.maximized = Some(id);
        true
    }

    /// Ratio of split `id`, if it exists.
    #[cfg(test)]
    pub fn ratio(&self, id: SplitId) -> Option<f64> {
        match self.root.find_split(id) {
            Some(Node::Split { ratio, .. }) => Some(*ratio),
            _ => None,
        }
    }

    /// Sets the ratio of split `id` (clamped). Returns whether it exists.
    pub fn set_ratio(&mut self, id: SplitId, ratio: f64) -> bool {
        match self.root.find_split_mut(id) {
            Some(Node::Split { ratio: r, .. }) => {
                *r = clamp_ratio(ratio);
                true
            }
            _ => false,
        }
    }

    /// A one-area layout keeping an existing area id (e.g. for an area
    /// torn off into its own window).
    pub fn single(id: AreaId, tabs: Tabs) -> Self {
        Self {
            root: Node::Leaf { id, tabs },
            maximized: None,
        }
    }

    /// Rectangle of every area in the unit square (ignoring maximize).
    #[cfg(test)]
    pub fn leaf_rects(&self) -> Vec<(AreaId, Rect)> {
        let mut all = Vec::new();
        self.root.collect_rects(Rect::UNIT, &mut all);
        let leaves = self.leaves();
        all.into_iter()
            .filter(|(id, _)| leaves.iter().any(|(l, _)| l.0 == *id))
            .map(|(id, r)| (AreaId(id), r))
            .collect()
    }

    /// Splits area `id` along `dir`, giving `ratio` of the space to the
    /// original area (left/top); the new area shows the same editor.
    pub fn split_at(
        &mut self,
        id: AreaId,
        dir: Dir,
        ratio: f64,
        ids: &mut IdGen,
    ) -> Option<AreaId> {
        let new = self.split(id, dir, ids)?;
        if let Some(Node::Split { ratio: r, .. }) = self.parent_split_mut(new) {
            *r = clamp_ratio(ratio);
        }
        Some(new)
    }

    fn parent_split_mut(&mut self, id: AreaId) -> Option<&mut Node> {
        let mut path = Vec::new();
        if !self.root.path_to(id, &mut path) || path.is_empty() {
            return None;
        }
        path.pop();
        let mut node = &mut self.root;
        for side in path {
            node = match node {
                Node::Split { a, b, .. } => {
                    if side {
                        b.as_mut()
                    } else {
                        a.as_mut()
                    }
                }
                Node::Leaf { .. } => return None,
            };
        }
        Some(node)
    }

    /// Removes area `id` (its sibling takes the space) and returns its
    /// tabs. The last area cannot be removed.
    pub fn take(&mut self, id: AreaId) -> Option<Tabs> {
        let tabs = self.tabs(id)?.clone();
        self.close(id).then_some(tabs)
    }

    /// Replaces area `old` by area `new` with `tabs`, in place.
    pub fn replace_leaf(&mut self, old: AreaId, new: AreaId, tabs: Tabs) -> bool {
        if old != new && self.contains(new) {
            return false;
        }
        let Some(node) = self.root.find_leaf_mut(old) else {
            return false;
        };
        *node = Node::Leaf { id: new, tabs };
        if self.maximized == Some(old) {
            self.maximized = Some(new);
        }
        true
    }

    /// Puts area `id` (not yet in this layout) next to area `dst` on
    /// `side`, giving it `share` of `dst`'s space.
    pub fn insert(
        &mut self,
        dst: AreaId,
        side: Side,
        id: AreaId,
        tabs: Tabs,
        share: f64,
        ids: &mut IdGen,
    ) -> bool {
        if self.contains(id) {
            return false;
        }
        let Some(node) = self.root.find_leaf_mut(dst) else {
            return false;
        };
        let old = std::mem::replace(node, Node::leaf(EditorKind::Viewport));
        *node = wrap(old, side, Node::Leaf { id, tabs }, share, ids);
        self.maximized = None;
        true
    }

    /// Puts area `id` (not yet in this layout) along a whole edge of the
    /// layout, giving it `share` of the space.
    pub fn insert_root(
        &mut self,
        side: Side,
        id: AreaId,
        tabs: Tabs,
        share: f64,
        ids: &mut IdGen,
    ) -> bool {
        if self.contains(id) {
            return false;
        }
        let old = std::mem::replace(&mut self.root, Node::leaf(EditorKind::Viewport));
        self.root = wrap(old, side, Node::Leaf { id, tabs }, share, ids);
        self.maximized = None;
        true
    }

    /// Moves area `src` next to area `dst`, on `dst`'s `side`, splitting
    /// `dst` in half. The area keeps its id (and so its editor state).
    pub fn move_leaf(&mut self, src: AreaId, dst: AreaId, side: Side, ids: &mut IdGen) -> bool {
        if src == dst || !self.contains(dst) {
            return false;
        }
        let mut next = self.clone();
        let Some(tabs) = next.take(src) else {
            return false;
        };
        if !next.insert(dst, side, src, tabs, 0.5, ids) {
            return false;
        }
        *self = next;
        true
    }

    /// Moves area `src` along a whole edge of the layout.
    pub fn dock_root(&mut self, src: AreaId, side: Side, share: f64, ids: &mut IdGen) -> bool {
        let mut next = self.clone();
        let Some(tabs) = next.take(src) else {
            return false;
        };
        if !next.insert_root(side, src, tabs, share, ids) {
            return false;
        }
        *self = next;
        true
    }

    /// Side of `keep` on which `absorb` lies, when the two can be joined:
    /// they share a full edge, i.e. their lowest common split runs along
    /// that edge and every split between it and either area does too,
    /// with the area always on the side facing the other one.
    pub fn join_side(&self, keep: AreaId, absorb: AreaId) -> Option<Side> {
        if keep == absorb {
            return None;
        }
        let (mut pk, mut pa) = (Vec::new(), Vec::new());
        if !self.root.path_to(keep, &mut pk) || !self.root.path_to(absorb, &mut pa) {
            return None;
        }
        let common = pk.iter().zip(&pa).take_while(|(x, y)| x == y).count();
        let Node::Split { dir, .. } = self.root.at(&pk[..common]) else {
            return None;
        };
        let keep_is_b = pk[common];
        let chain_ok = |path: &[bool], toward_b: bool| {
            (common + 1..path.len()).all(|i| match self.root.at(&path[..i]) {
                Node::Split { dir: d, .. } => d == dir && path[i] == toward_b,
                Node::Leaf { .. } => false,
            })
        };
        // An area on the b side must be the first (a) child all the way
        // down to face the other one, and vice versa.
        if !chain_ok(&pk, !keep_is_b) || !chain_ok(&pa, keep_is_b) {
            return None;
        }
        Some(match (dir, keep_is_b) {
            (Dir::Horizontal, false) => Side::Right,
            (Dir::Horizontal, true) => Side::Left,
            (Dir::Vertical, false) => Side::Bottom,
            (Dir::Vertical, true) => Side::Top,
        })
    }

    /// Blender's join: area `keep` grows over its neighbour `absorb`,
    /// which is removed. Every other area keeps its place and size.
    pub fn join(&mut self, keep: AreaId, absorb: AreaId) -> bool {
        if self.join_side(keep, absorb).is_none() {
            return false;
        }
        let (mut pk, mut pa) = (Vec::new(), Vec::new());
        self.root.path_to(keep, &mut pk);
        self.root.path_to(absorb, &mut pa);
        let common = pk.iter().zip(&pa).take_while(|(x, y)| x == y).count();
        let mut only = Vec::new();
        self.root.at(&pk[..common]).collect_ids(&mut only);

        let mut want = Vec::new();
        self.root.collect_rects(Rect::UNIT, &mut want);
        let find = |id: AreaId| want.iter().find(|(i, _)| *i == id.0).map(|(_, r)| *r);
        let (Some(rk), Some(ra)) = (find(keep), find(absorb)) else {
            return false;
        };
        let grown = rk.union(&ra);
        for (id, r) in &mut want {
            if *id == keep.0 {
                *r = grown;
            }
        }
        if !self.close(absorb) {
            return false;
        }
        self.root.fit(&want, &only);
        true
    }

    /// Gives both sides of split `id` the same space (double-click on a
    /// divider).
    pub fn equalize(&mut self, id: SplitId) -> bool {
        self.set_ratio(id, 0.5)
    }
}

/// A split putting `new` on `side` of `old`, with `share` of the space.
fn wrap(old: Node, side: Side, new: Node, share: f64, ids: &mut IdGen) -> Node {
    let (a, b, ratio) = if side.is_start() {
        (new, old, share)
    } else {
        (old, new, 1.0 - share)
    };
    Node::Split {
        id: ids.split(),
        dir: side.dir(),
        ratio: clamp_ratio(ratio),
        a: Box::new(a),
        b: Box::new(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use EditorKind::*;

    /// `[SceneTree | [Viewport / Console]]`, numbered in pre-order.
    fn three() -> Layout {
        Layout::new(
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
            &mut IdGen::default(),
        )
    }

    const ROOT: SplitId = SplitId(1);
    const SCENE: AreaId = AreaId(2);
    const RIGHT: SplitId = SplitId(3);
    const VIEW: AreaId = AreaId(4);
    const CONSOLE: AreaId = AreaId(5);

    fn ids_after(l: &Layout) -> IdGen {
        let mut ids = IdGen::default();
        ids.reserve(l);
        ids
    }

    #[test]
    fn new_numbers_nodes_in_pre_order() {
        let l = three();
        assert_eq!(
            l.leaves(),
            vec![(SCENE, SceneTree), (VIEW, Viewport), (CONSOLE, Console)]
        );
        assert_eq!(l.ratio(ROOT), Some(0.25));
        assert_eq!(l.ratio(RIGHT), Some(0.7));
        assert_eq!(l.max_id(), 8); // 5 nodes, then 3 tabs
    }

    #[test]
    fn ids_are_unique_across_layouts_sharing_a_generator() {
        let mut ids = IdGen::default();
        let a = Layout::new(Node::leaf(Viewport), &mut ids);
        let b = Layout::new(Node::leaf(Viewport), &mut ids);
        assert_ne!(a.leaves()[0].0, b.leaves()[0].0);
    }

    #[test]
    fn split_adds_area_with_same_editor() {
        let mut l = three();
        let mut ids = ids_after(&l);
        let new = l.split(VIEW, Dir::Horizontal, &mut ids).unwrap();
        assert!(new.0 > 5);
        assert_eq!(l.leaves().len(), 4);
        assert_eq!(l.editor(new), Some(Viewport));
        // The original keeps its place, the new one follows it.
        let order: Vec<_> = l.leaves().into_iter().map(|(id, _)| id).collect();
        assert_eq!(order, vec![SCENE, VIEW, new, CONSOLE]);
        // Existing splits keep their ids and ratios.
        assert_eq!(l.ratio(RIGHT), Some(0.7));
    }

    #[test]
    fn split_unknown_area_fails() {
        let mut l = three();
        let before = l.clone();
        assert_eq!(
            l.split(AreaId(99), Dir::Vertical, &mut IdGen::default()),
            None
        );
        assert_eq!(l, before);
    }

    #[test]
    fn ids_are_never_reused() {
        let mut l = three();
        let mut ids = ids_after(&l);
        let a = l.split(SCENE, Dir::Vertical, &mut ids).unwrap();
        assert!(l.close(a));
        let b = l.split(SCENE, Dir::Vertical, &mut ids).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn close_lets_sibling_take_space() {
        let mut l = three();
        assert!(l.close(CONSOLE));
        assert!(matches!(l.root(), Node::Split { id: ROOT, .. }));
        assert_eq!(l.leaves(), vec![(SCENE, SceneTree), (VIEW, Viewport)]);
        assert_eq!(l.ratio(ROOT), Some(0.25));
        // Closing the left area keeps the subtree on the right intact.
        let mut l = three();
        assert!(l.close(SCENE));
        assert_eq!(l.leaves(), vec![(VIEW, Viewport), (CONSOLE, Console)]);
        assert_eq!(l.ratio(RIGHT), Some(0.7));
        assert_eq!(l.ratio(ROOT), None);
    }

    #[test]
    fn split_then_close_restores_layout() {
        let mut l = three();
        let before = l.clone();
        let new = l
            .split(CONSOLE, Dir::Horizontal, &mut ids_after(&l))
            .unwrap();
        assert!(l.close(new));
        assert_eq!(l, before);
    }

    #[test]
    fn last_area_cannot_be_closed() {
        let mut l = Layout::new(Node::leaf(Viewport), &mut IdGen::default());
        assert!(!l.close(AreaId(1)));
        assert_eq!(l.leaves(), vec![(AreaId(1), Viewport)]);
        assert!(!three().close(AreaId(42)));
    }

    #[test]
    fn set_editor_changes_only_target() {
        let mut l = three();
        assert!(l.set_editor(VIEW, Sheet));
        assert_eq!(l.editor(VIEW), Some(Sheet));
        assert_eq!(l.editor(SCENE), Some(SceneTree));
        assert!(!l.set_editor(AreaId(9), Sheet));
    }

    #[test]
    fn swap_exchanges_places() {
        let mut l = three();
        assert!(l.swap(SCENE, CONSOLE));
        assert_eq!(
            l.leaves(),
            vec![(CONSOLE, Console), (VIEW, Viewport), (SCENE, SceneTree)]
        );
        // Split ratios stay where they were.
        assert_eq!(l.ratio(ROOT), Some(0.25));
        // Swapping back restores the original.
        assert!(l.swap(CONSOLE, SCENE));
        assert_eq!(l, three());
    }

    #[test]
    fn swap_rejects_unknown_or_identical_areas() {
        let mut l = three();
        assert!(!l.swap(SCENE, SCENE));
        assert!(!l.swap(SCENE, AreaId(77)));
        assert_eq!(l, three());
    }

    #[test]
    fn maximize_shows_single_area_and_toggles_back() {
        let mut l = three();
        assert!(l.toggle_maximize(VIEW));
        assert_eq!(l.maximized(), Some(VIEW));
        assert!(matches!(l.visible_root(), Node::Leaf { id: VIEW, .. }));
        // Toggling from any area restores the full layout.
        assert!(l.toggle_maximize(SCENE));
        assert_eq!(l.maximized(), None);
        assert_eq!(l.visible_root(), l.root());
        assert!(!l.toggle_maximize(AreaId(77)));
    }

    #[test]
    fn closing_maximized_area_restores() {
        let mut l = three();
        l.toggle_maximize(CONSOLE);
        assert!(l.close(CONSOLE));
        assert_eq!(l.maximized(), None);
    }

    #[test]
    fn split_leaves_maximize_mode() {
        let mut l = three();
        l.toggle_maximize(VIEW);
        l.split(VIEW, Dir::Vertical, &mut ids_after(&l));
        assert_eq!(l.maximized(), None);
    }

    #[test]
    fn ratios_are_addressed_by_split_id_and_clamped() {
        let mut l = three();
        assert!(l.set_ratio(RIGHT, 0.4));
        assert!(l.set_ratio(ROOT, 2.0));
        assert!(!l.set_ratio(SplitId(2), 0.5)); // an area's id
        assert!(!l.set_ratio(SplitId(42), 0.5));
        assert_eq!(l.ratio(RIGHT), Some(0.4));
        assert_eq!(l.ratio(ROOT), Some(1.0 - MIN_RATIO));
        assert!(l.set_ratio(ROOT, f64::NAN));
        assert_eq!(l.ratio(ROOT), Some(0.5));
    }

    #[test]
    fn split_ids_survive_structural_edits() {
        let mut l = three();
        // Closing an area elsewhere moves the right split up the tree; a
        // path-based address would now be stale, its id is not.
        assert!(l.close(SCENE));
        assert!(l.set_ratio(RIGHT, 0.6));
        assert_eq!(l.ratio(RIGHT), Some(0.6));
        // A removed split is simply gone.
        assert!(!l.set_ratio(ROOT, 0.3));
    }

    #[test]
    fn layout_round_trips_through_json() {
        let mut l = three();
        l.toggle_maximize(VIEW);
        let json = serde_json::to_string(&l).unwrap();
        let back: Layout = serde_json::from_str(&json).unwrap();
        assert_eq!(back, l);
    }

    fn json_with(edit: impl FnOnce(&mut serde_json::Value)) -> serde_json::Result<Layout> {
        let mut v = serde_json::to_value(three()).unwrap();
        edit(&mut v);
        serde_json::from_value(v)
    }

    #[test]
    fn deserialize_rejects_duplicate_ids() {
        // Give the console the scene tree's id.
        let err = json_with(|v| v["root"]["Split"]["b"]["Split"]["b"]["Leaf"]["id"] = 2.into());
        assert!(err.unwrap_err().to_string().contains("used more than once"));
        // Areas and splits share one id space.
        assert!(json_with(|v| v["root"]["Split"]["id"] = 4.into()).is_err());
    }

    #[test]
    fn deserialize_rejects_huge_ids() {
        // Near u32::MAX the id generator would saturate and repeat ids.
        let err = json_with(|v| v["root"]["Split"]["a"]["Leaf"]["id"] = u32::MAX.into());
        assert!(err.unwrap_err().to_string().contains("too large"));
        assert!(json_with(|v| v["root"]["Split"]["a"]["Leaf"]["id"] = MAX_ID.into()).is_ok());
    }

    #[test]
    fn deserialize_rejects_zero_ids() {
        let err = json_with(|v| v["root"]["Split"]["a"]["Leaf"]["id"] = 0.into());
        assert!(err.unwrap_err().to_string().contains("reserved"));
    }

    #[test]
    fn deserialize_clamps_ratios_and_drops_unknown_maximized() {
        let l = json_with(|v| {
            v["root"]["Split"]["ratio"] = 7.5.into();
            v["maximized"] = 99.into();
        })
        .unwrap();
        assert_eq!(l.ratio(ROOT), Some(1.0 - MIN_RATIO));
        assert_eq!(l.maximized(), None);
    }

    #[test]
    fn reserve_moves_generator_past_loaded_ids() {
        let l = three();
        let mut ids = IdGen::default();
        ids.reserve(&l);
        assert!(ids.area().0 > l.max_id());
    }

    #[test]
    fn move_leaf_keeps_id_and_splits_target() {
        let mut l = three();
        let mut ids = ids_after(&l);
        assert!(l.move_leaf(SCENE, CONSOLE, Side::Right, &mut ids));
        // The scene tree left; the right split moved up to the root.
        assert_eq!(
            l.leaves(),
            vec![(VIEW, Viewport), (CONSOLE, Console), (SCENE, SceneTree)]
        );
        assert_eq!(l.ratio(RIGHT), Some(0.7));
        assert_eq!(l.join_side(CONSOLE, SCENE), Some(Side::Right));
        // Onto itself, or to an unknown target, nothing happens.
        let before = l.clone();
        assert!(!l.move_leaf(VIEW, VIEW, Side::Left, &mut ids));
        assert!(!l.move_leaf(VIEW, AreaId(99), Side::Left, &mut ids));
        assert_eq!(l, before);
        // The last area cannot move.
        let mut one = Layout::new(Node::leaf(Viewport), &mut IdGen::default());
        assert!(!one.move_leaf(AreaId(1), AreaId(1), Side::Top, &mut ids));
    }

    #[test]
    fn move_leaf_on_each_side() {
        for side in Side::ALL {
            let mut l = three();
            let mut ids = ids_after(&l);
            assert!(l.move_leaf(CONSOLE, VIEW, side, &mut ids));
            let rects = l.leaf_rects();
            let r = |id| rects.iter().find(|(a, _)| *a == id).unwrap().1;
            let (c, v) = (r(CONSOLE), r(VIEW));
            match side {
                Side::Left => assert!(c.x < v.x && c.y == v.y),
                Side::Right => assert!(c.x > v.x && c.y == v.y),
                Side::Top => assert!(c.y < v.y && c.x == v.x),
                Side::Bottom => assert!(c.y > v.y && c.x == v.x),
            }
        }
    }

    #[test]
    fn dock_root_spans_whole_edge() {
        let mut l = three();
        let mut ids = ids_after(&l);
        assert!(l.dock_root(SCENE, Side::Bottom, 0.25, &mut ids));
        let rects = l.leaf_rects();
        let scene = rects.iter().find(|(a, _)| *a == SCENE).unwrap().1;
        assert_eq!((scene.x, scene.w), (0.0, 1.0));
        assert!((scene.h - 0.25).abs() < 1e-9);
        let mut one = Layout::new(Node::leaf(Viewport), &mut IdGen::default());
        assert!(!one.dock_root(AreaId(1), Side::Left, 0.25, &mut ids));
    }

    #[test]
    fn insert_and_replace_leaf() {
        let mut l = three();
        let mut ids = ids_after(&l);
        let one = |ids: &mut IdGen, editor| {
            Tabs::one(Tab {
                id: ids.tab(),
                editor,
            })
        };
        let new = ids.area();
        let sheet = one(&mut ids, Sheet);
        assert!(l.insert(VIEW, Side::Top, new, sheet.clone(), 0.3, &mut ids));
        assert!(!l.insert(VIEW, Side::Top, new, sheet, 0.3, &mut ids));
        assert_eq!(l.editor(new), Some(Sheet));
        let other = ids.area();
        let script = one(&mut ids, Script);
        assert!(l.insert_root(Side::Left, other, script, 0.2, &mut ids));
        assert_eq!(l.leaves()[0], (other, Script));
        let console = one(&mut ids, Console);
        assert!(l.replace_leaf(new, AreaId(500), console.clone()));
        assert!(!l.contains(new));
        assert!(!l.replace_leaf(SCENE, VIEW, console)); // already present
    }

    #[test]
    fn join_siblings_restores_parent_space() {
        let mut l = three();
        assert_eq!(l.join_side(VIEW, CONSOLE), Some(Side::Bottom));
        assert_eq!(l.join_side(CONSOLE, VIEW), Some(Side::Top));
        assert!(l.join(VIEW, CONSOLE));
        assert_eq!(l.leaves(), vec![(SCENE, SceneTree), (VIEW, Viewport)]);
        assert_eq!(l.ratio(ROOT), Some(0.25));
    }

    #[test]
    fn join_across_nested_splits_keeps_other_sizes() {
        // [A | [B | C]] with A 0.2, B 0.4 of the rest: A and B share an edge.
        let mut ids = IdGen::default();
        let mut l = Layout::new(
            Node::split(
                Dir::Horizontal,
                0.2,
                Node::leaf(SceneTree),
                Node::split(
                    Dir::Horizontal,
                    0.5,
                    Node::leaf(Viewport),
                    Node::leaf(Console),
                ),
            ),
            &mut ids,
        );
        let (a, b, c) = (AreaId(2), AreaId(4), AreaId(5));
        assert_eq!(l.join_side(a, b), Some(Side::Right));
        assert_eq!(l.join_side(a, c), None); // B is in between
        let before = l.leaf_rects();
        assert!(l.join(a, b));
        let after = l.leaf_rects();
        let get = |v: &[(AreaId, Rect)], id| v.iter().find(|(x, _)| *x == id).unwrap().1;
        assert!((get(&after, a).w - 0.6).abs() < 1e-9);
        let (ca, cb) = (get(&after, c), get(&before, c));
        assert!((ca.x - cb.x).abs() < 1e-9 && (ca.w - cb.w).abs() < 1e-9);
    }

    #[test]
    fn join_rejects_partial_edges() {
        // [[A / B] | [C / D]]: A and C line up only by accident of ratios.
        let mut l = Layout::new(
            Node::split(
                Dir::Horizontal,
                0.5,
                Node::split(
                    Dir::Vertical,
                    0.5,
                    Node::leaf(Viewport),
                    Node::leaf(Console),
                ),
                Node::split(Dir::Vertical, 0.5, Node::leaf(Sheet), Node::leaf(Script)),
            ),
            &mut IdGen::default(),
        );
        let (a, c) = (AreaId(3), AreaId(6));
        assert_eq!(l.join_side(a, c), None);
        assert!(!l.join(a, c));
        // In three(): the scene tree borders both view and console.
        assert_eq!(three().join_side(SCENE, VIEW), None);
        assert_eq!(three().join_side(VIEW, SCENE), None);
        assert!(!l.join(a, a));
    }

    #[test]
    fn split_at_uses_ratio_and_depth_counts() {
        let mut l = three();
        let mut ids = ids_after(&l);
        let new = l.split_at(VIEW, Dir::Horizontal, 0.3, &mut ids).unwrap();
        let rects = l.leaf_rects();
        let v = rects.iter().find(|(a, _)| *a == VIEW).unwrap().1;
        let n = rects.iter().find(|(a, _)| *a == new).unwrap().1;
        assert!((v.w / (v.w + n.w) - 0.3).abs() < 1e-9);
        assert_eq!(l.root().depth_along(Dir::Horizontal), 3);
        assert_eq!(l.root().depth_along(Dir::Vertical), 2);
        assert!(l.equalize(RIGHT));
        assert_eq!(l.ratio(RIGHT), Some(0.5));
    }

    #[test]
    fn rect_helpers() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert_eq!(r.part(Side::Left, 0.25), Rect::new(0.0, 0.0, 25.0, 50.0));
        assert_eq!(r.part(Side::Bottom, 0.5), Rect::new(0.0, 25.0, 100.0, 25.0));
        assert!(r.contains(99.0, 49.0) && !r.contains(100.0, 0.0));
    }

    /// `[SceneTree | Viewport, Console, Inspector]`: area 3 has three tabs.
    fn tabbed() -> (Layout, IdGen) {
        let mut ids = IdGen::default();
        let l = Layout::new(
            Node::split(
                Dir::Horizontal,
                0.3,
                Node::leaf(SceneTree),
                Node::tabbed(&[Viewport, Console, Inspector]),
            ),
            &mut ids,
        );
        (l, ids)
    }

    #[test]
    fn tabs_are_numbered_after_nodes() {
        let (l, _) = tabbed();
        assert_eq!(l.ids(), vec![1, 2, 3]);
        let tabs = l.tabs(AreaId(3)).unwrap();
        let ids: Vec<u32> = tabs.list().iter().map(|t| t.id.0).collect();
        assert_eq!(ids, vec![5, 6, 7]);
        assert_eq!(tabs.active(), 0);
        assert_eq!(l.editor(AreaId(3)), Some(Viewport));
        assert_eq!(l.area_of_tab(TabId(6)), Some(AreaId(3)));
        assert_eq!(l.all_tabs().len(), 4);
    }

    #[test]
    fn select_cycle_and_reorder_tabs() {
        let (mut l, _) = tabbed();
        let area = AreaId(3);
        assert!(l.select_tab(area, 2));
        assert_eq!(l.editor(area), Some(Inspector));
        assert!(!l.select_tab(area, 2)); // already shown
        assert!(!l.select_tab(area, 9));
        assert!(l.cycle_tab(area, 1)); // wraps to the first
        assert_eq!(l.editor(area), Some(Viewport));
        assert!(l.cycle_tab(area, -1));
        assert_eq!(l.editor(area), Some(Inspector));
        // Reordering keeps the shown tab shown.
        assert!(l.reorder_tab(TabId(7), 0));
        let order: Vec<_> = l
            .tabs(area)
            .unwrap()
            .list()
            .iter()
            .map(|t| t.editor)
            .collect();
        assert_eq!(order, vec![Inspector, Viewport, Console]);
        assert_eq!(l.editor(area), Some(Inspector));
        // set_editor changes the shown tab only.
        assert!(l.set_editor(area, Sheet));
        assert_eq!(l.tabs(area).unwrap().list()[0].editor, Sheet);
        assert_eq!(l.tabs(area).unwrap().list()[1].editor, Viewport);
    }

    #[test]
    fn add_and_remove_tabs() {
        let (mut l, mut ids) = tabbed();
        let area = AreaId(3);
        let t = Tab {
            id: ids.tab(),
            editor: Script,
        };
        assert!(l.add_tab(area, t, Some(1)));
        assert!(!l.add_tab(area, t, None)); // already present
        assert_eq!(l.editor(area), Some(Script)); // added tab is shown
        assert_eq!(l.remove_tab(t.id), Some(t));
        // The shown tab was removed: its right neighbour is shown.
        assert_eq!(l.editor(area), Some(Console));
        assert!(l.remove_tab(TabId(5)).is_some());
        assert!(l.remove_tab(TabId(6)).is_some());
        // The last tab of an area stays (closing the area is separate).
        assert_eq!(l.remove_tab(TabId(7)), None);
        assert_eq!(l.editor(area), Some(Inspector));
    }

    #[test]
    fn split_and_swap_keep_tabs() {
        let (mut l, mut ids) = tabbed();
        let area = AreaId(3);
        let new = l.split(area, Dir::Vertical, &mut ids).unwrap();
        // The new area gets one fresh tab with the shown editor.
        assert_eq!(l.tabs(new).unwrap().len(), 1);
        assert_eq!(l.editor(new), Some(Viewport));
        assert_eq!(l.tabs(area).unwrap().len(), 3);
        assert!(l.swap(area, AreaId(2)));
        assert_eq!(l.tabs(area).unwrap().len(), 3);
        assert_eq!(l.leaves()[0], (area, Viewport));
        let taken = l.take(area).unwrap();
        assert_eq!(taken.len(), 3);
    }

    #[test]
    fn tabbed_layout_round_trips_and_validates() {
        let (l, _) = tabbed();
        let json = serde_json::to_value(&l).unwrap();
        let back: Layout = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, l);
        // Out-of-range active index is clamped.
        let mut v = json.clone();
        v["root"]["Split"]["b"]["Leaf"]["tabs"]["active"] = 9.into();
        let back: Layout = serde_json::from_value(v).unwrap();
        assert_eq!(back.tabs(AreaId(3)).unwrap().active(), 2);
        // Empty tab lists, duplicate or huge tab ids are rejected.
        let mut v = json.clone();
        v["root"]["Split"]["b"]["Leaf"]["tabs"]["list"] = serde_json::json!([]);
        assert!(serde_json::from_value::<Layout>(v).is_err());
        let mut v = json.clone();
        v["root"]["Split"]["b"]["Leaf"]["tabs"]["list"][1]["id"] = 5.into();
        assert!(serde_json::from_value::<Layout>(v).is_err());
        let mut v = json;
        v["root"]["Split"]["b"]["Leaf"]["tabs"]["list"][1]["id"] = u32::MAX.into();
        assert!(serde_json::from_value::<Layout>(v).is_err());
    }

    #[test]
    fn single_editor_leaves_migrate_to_one_tab() {
        // Layout blob v1/v2 leaves: `{"id": .., "editor": ..}`.
        let json = serde_json::json!({
            "root": {"Split": {"id": 1, "dir": "Horizontal", "ratio": 0.4,
                "a": {"Leaf": {"id": 2, "editor": "SceneTree"}},
                "b": {"Leaf": {"id": 3, "editor": "Viewport"}}}},
            "maximized": null,
        });
        let l: Layout = serde_json::from_value(json).unwrap();
        let tabs = l.tabs(AreaId(3)).unwrap();
        assert_eq!(tabs.len(), 1);
        // The tab takes the area's id (tab ids are their own namespace).
        assert_eq!(tabs.active_tab().id, TabId(3));
        assert_eq!(l.editor(AreaId(2)), Some(SceneTree));
        let mut ids = IdGen::default();
        ids.reserve(&l);
        assert!(ids.tab().0 > 3);
        // A leaf with neither tabs nor editor is rejected.
        let bad = serde_json::json!({"root": {"Leaf": {"id": 1}}, "maximized": null});
        assert!(serde_json::from_value::<Layout>(bad).is_err());
    }

    #[test]
    fn editor_ids_round_trip() {
        for kind in EditorKind::ALL {
            assert_eq!(EditorKind::from_id(kind.id()), Some(kind));
        }
        for dir in [Dir::Horizontal, Dir::Vertical] {
            assert_eq!(Dir::from_id(dir.id()), Some(dir));
        }
    }
}

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

    /// Makes sure future ids are greater than every id of `layout`, e.g.
    /// after loading it from disk.
    pub fn reserve(&mut self, layout: &Layout) {
        self.next = self.next.max(layout.max_id().saturating_add(1));
    }
}

/// Smallest fraction of a split either child may get.
pub const MIN_RATIO: f64 = 0.05;

/// A node of the layout tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        editor: EditorKind,
    },
}

impl Node {
    /// A leaf; its id is assigned by [`Layout::new`].
    pub fn leaf(editor: EditorKind) -> Self {
        Node::Leaf {
            id: AreaId(0),
            editor,
        }
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
            Node::Leaf { id, editor } => out.push((*id, *editor)),
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
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::DuplicateId(id) => write!(f, "layout id {id} is used more than once"),
            LayoutError::ZeroId => f.write_str("layout id 0 is reserved"),
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
        if let Some(pair) = ids.windows(2).find(|w| w[0] == w[1]) {
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
        root.for_each_mut(&mut |n| match n {
            Node::Leaf { id, .. } => *id = ids.area(),
            Node::Split { id, .. } => *id = ids.split(),
        });
        Self {
            root,
            maximized: None,
        }
    }

    /// The full tree, ignoring maximize state.
    #[allow(dead_code, reason = "used by tests and upcoming layout persistence")]
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

    /// Largest id used by the layout.
    /// Every area and split id of the tree.
    pub fn ids(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        self.root.collect_ids(&mut ids);
        ids
    }

    pub fn max_id(&self) -> u32 {
        let mut ids = Vec::new();
        self.root.collect_ids(&mut ids);
        ids.into_iter().max().unwrap_or(0)
    }

    /// Whether `id` is an area of this layout.
    pub fn contains(&self, id: AreaId) -> bool {
        self.root.find_leaf(id).is_some()
    }

    /// The editor shown in area `id`.
    pub fn editor(&self, id: AreaId) -> Option<EditorKind> {
        match self.root.find_leaf(id) {
            Some(Node::Leaf { editor, .. }) => Some(*editor),
            _ => None,
        }
    }

    /// Splits area `id` in two along `dir`. The new area (`b`, right or
    /// bottom) shows the same editor. Returns the new area's id. Leaves
    /// maximize mode, since the result must be visible.
    pub fn split(&mut self, id: AreaId, dir: Dir, ids: &mut IdGen) -> Option<AreaId> {
        let node = self.root.find_leaf_mut(id)?;
        let Node::Leaf { editor, .. } = *node else {
            return None;
        };
        let new_id = ids.area();
        *node = Node::Split {
            id: ids.split(),
            dir,
            ratio: 0.5,
            a: Box::new(Node::Leaf { id, editor }),
            b: Box::new(Node::Leaf { id: new_id, editor }),
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

    /// Shows `editor` in area `id`.
    pub fn set_editor(&mut self, id: AreaId, editor: EditorKind) -> bool {
        match self.root.find_leaf_mut(id) {
            Some(Node::Leaf { editor: e, .. }) => {
                *e = editor;
                true
            }
            _ => false,
        }
    }

    /// Exchanges the places of areas `a` and `b`. Ids move with their
    /// editors, so editor state (cached by area id) follows the area.
    /// Returns whether both exist and differ.
    pub fn swap(&mut self, a: AreaId, b: AreaId) -> bool {
        let (Some(ea), Some(eb)) = (self.editor(a), self.editor(b)) else {
            return false;
        };
        if a == b {
            return false;
        }
        self.root.for_each_mut(&mut |n| {
            if let Node::Leaf { id, editor } = n {
                if *id == a {
                    (*id, *editor) = (b, eb);
                } else if *id == b {
                    (*id, *editor) = (a, ea);
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
        assert_eq!(l.max_id(), 5);
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
        assert_eq!(
            l.root(),
            &Node::Split {
                id: ROOT,
                dir: Dir::Horizontal,
                ratio: 0.25,
                a: Box::new(Node::Leaf {
                    id: SCENE,
                    editor: SceneTree
                }),
                b: Box::new(Node::Leaf {
                    id: VIEW,
                    editor: Viewport
                }),
            }
        );
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
        assert_eq!(
            l.visible_root(),
            &Node::Leaf {
                id: VIEW,
                editor: Viewport
            }
        );
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
    fn editor_ids_round_trip() {
        for kind in EditorKind::ALL {
            assert_eq!(EditorKind::from_id(kind.id()), Some(kind));
        }
        for dir in [Dir::Horizontal, Dir::Vertical] {
            assert_eq!(Dir::from_id(dir.id()), Some(dir));
        }
    }
}

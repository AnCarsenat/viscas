//! Pure area layout model (REQUIREMENTS §2), independent of GTK.
//!
//! A layout is a binary tree: inner nodes split their space in two along a
//! direction, leaves are *areas* each hosting one editor. Widgets are rebuilt
//! from this model whenever it changes structurally, so all operations here
//! (split, close/join, change editor, maximize) are plain data edits that can
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

/// Identifies an area within one [`Layout`]; never reused by that layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AreaId(pub u32);

/// Which child of a split to descend into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Branch {
    A,
    B,
}

/// Path from the root to a node, used to address splits.
pub type Path = Vec<Branch>;

/// Smallest fraction of a split either child may get.
pub const MIN_RATIO: f64 = 0.05;

/// A node of the layout tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Split {
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

    /// A split giving `ratio` of the space to `a`.
    pub fn split(dir: Dir, ratio: f64, a: Node, b: Node) -> Self {
        Node::Split {
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

    fn at_path_mut(&mut self, path: &[Branch]) -> Option<&mut Node> {
        match (path.split_first(), self) {
            (None, node) => Some(node),
            (Some((Branch::A, rest)), Node::Split { a, .. }) => a.at_path_mut(rest),
            (Some((Branch::B, rest)), Node::Split { b, .. }) => b.at_path_mut(rest),
            (Some(_), Node::Leaf { .. }) => None,
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

/// An area layout: the split tree plus maximize state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    root: Node,
    next_id: u32,
    maximized: Option<AreaId>,
}

impl Layout {
    /// Creates a layout from `root`, numbering its leaves left to right.
    pub fn new(mut root: Node) -> Self {
        fn number(node: &mut Node, next: &mut u32) {
            match node {
                Node::Leaf { id, .. } => {
                    *id = AreaId(*next);
                    *next += 1;
                }
                Node::Split { a, b, .. } => {
                    number(a, next);
                    number(b, next);
                }
            }
        }
        let mut next_id = 1;
        number(&mut root, &mut next_id);
        Self {
            root,
            next_id,
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
    pub fn split(&mut self, id: AreaId, dir: Dir) -> Option<AreaId> {
        let new_id = AreaId(self.next_id);
        let node = self.root.find_leaf_mut(id)?;
        let Node::Leaf { editor, .. } = *node else {
            return None;
        };
        *node = Node::split(
            dir,
            0.5,
            Node::Leaf { id, editor },
            Node::Leaf { id: new_id, editor },
        );
        self.next_id += 1;
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

    /// Maximizes area `id`, or restores the full layout when `id` is
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

    /// Ratio of the split at `path`, if there is one.
    pub fn ratio(&self, path: &[Branch]) -> Option<f64> {
        let mut node = &self.root;
        for branch in path {
            node = match (branch, node) {
                (Branch::A, Node::Split { a, .. }) => a,
                (Branch::B, Node::Split { b, .. }) => b,
                (_, Node::Leaf { .. }) => return None,
            };
        }
        match node {
            Node::Split { ratio, .. } => Some(*ratio),
            Node::Leaf { .. } => None,
        }
    }

    /// Sets the ratio of the split at `path` (clamped). Returns whether a
    /// split exists there.
    pub fn set_ratio(&mut self, path: &[Branch], ratio: f64) -> bool {
        match self.root.at_path_mut(path) {
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

    fn three() -> Layout {
        // [SceneTree | [Viewport / Console]]
        Layout::new(Node::split(
            Dir::Horizontal,
            0.25,
            Node::leaf(SceneTree),
            Node::split(
                Dir::Vertical,
                0.7,
                Node::leaf(Viewport),
                Node::leaf(Console),
            ),
        ))
    }

    #[test]
    fn new_numbers_leaves_in_order() {
        let l = three();
        assert_eq!(
            l.leaves(),
            vec![
                (AreaId(1), SceneTree),
                (AreaId(2), Viewport),
                (AreaId(3), Console)
            ]
        );
    }

    #[test]
    fn split_adds_area_with_same_editor() {
        let mut l = three();
        let new = l.split(AreaId(2), Dir::Horizontal).unwrap();
        assert_eq!(new, AreaId(4));
        assert_eq!(l.leaves().len(), 4);
        assert_eq!(l.editor(new), Some(Viewport));
        // The original keeps its place, the new one follows it.
        let ids: Vec<_> = l.leaves().into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![AreaId(1), AreaId(2), AreaId(4), AreaId(3)]);
    }

    #[test]
    fn split_unknown_area_fails() {
        let mut l = three();
        let before = l.clone();
        assert_eq!(l.split(AreaId(99), Dir::Vertical), None);
        assert_eq!(l, before);
    }

    #[test]
    fn ids_are_never_reused() {
        let mut l = three();
        let a = l.split(AreaId(1), Dir::Vertical).unwrap();
        assert!(l.close(a));
        let b = l.split(AreaId(1), Dir::Vertical).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn close_lets_sibling_take_space() {
        let mut l = three();
        assert!(l.close(AreaId(3)));
        assert_eq!(
            l.root(),
            &Node::Split {
                dir: Dir::Horizontal,
                ratio: 0.25,
                a: Box::new(Node::Leaf {
                    id: AreaId(1),
                    editor: SceneTree
                }),
                b: Box::new(Node::Leaf {
                    id: AreaId(2),
                    editor: Viewport
                }),
            }
        );
        // Closing the left area keeps the subtree on the right intact.
        let mut l = three();
        assert!(l.close(AreaId(1)));
        assert_eq!(
            l.leaves(),
            vec![(AreaId(2), Viewport), (AreaId(3), Console)]
        );
    }

    #[test]
    fn split_then_close_restores_layout() {
        let mut l = three();
        let before = l.leaves();
        let new = l.split(AreaId(3), Dir::Horizontal).unwrap();
        assert!(l.close(new));
        assert_eq!(l.leaves(), before);
    }

    #[test]
    fn last_area_cannot_be_closed() {
        let mut l = Layout::new(Node::leaf(Viewport));
        assert!(!l.close(AreaId(1)));
        assert_eq!(l.leaves(), vec![(AreaId(1), Viewport)]);
        assert!(!three().close(AreaId(42)));
    }

    #[test]
    fn set_editor_changes_only_target() {
        let mut l = three();
        assert!(l.set_editor(AreaId(2), Sheet));
        assert_eq!(l.editor(AreaId(2)), Some(Sheet));
        assert_eq!(l.editor(AreaId(1)), Some(SceneTree));
        assert!(!l.set_editor(AreaId(9), Sheet));
    }

    #[test]
    fn maximize_shows_single_area_and_toggles_back() {
        let mut l = three();
        assert!(l.toggle_maximize(AreaId(2)));
        assert_eq!(l.maximized(), Some(AreaId(2)));
        assert_eq!(
            l.visible_root(),
            &Node::Leaf {
                id: AreaId(2),
                editor: Viewport
            }
        );
        // Toggling from any area restores the full layout.
        assert!(l.toggle_maximize(AreaId(1)));
        assert_eq!(l.maximized(), None);
        assert_eq!(l.visible_root(), l.root());
        assert!(!l.toggle_maximize(AreaId(77)));
    }

    #[test]
    fn closing_maximized_area_restores() {
        let mut l = three();
        l.toggle_maximize(AreaId(3));
        assert!(l.close(AreaId(3)));
        assert_eq!(l.maximized(), None);
    }

    #[test]
    fn split_leaves_maximize_mode() {
        let mut l = three();
        l.toggle_maximize(AreaId(2));
        l.split(AreaId(2), Dir::Vertical);
        assert_eq!(l.maximized(), None);
    }

    #[test]
    fn ratios_are_addressed_by_path_and_clamped() {
        let mut l = three();
        assert!(l.set_ratio(&[Branch::B], 0.4));
        assert!(l.set_ratio(&[], 2.0));
        assert!(!l.set_ratio(&[Branch::A], 0.5)); // a leaf
        assert!(!l.set_ratio(&[Branch::B, Branch::B, Branch::A], 0.5));
        match l.root() {
            Node::Split { ratio, b, .. } => {
                assert_eq!(*ratio, 1.0 - MIN_RATIO);
                assert!(matches!(**b, Node::Split { ratio, .. } if ratio == 0.4));
            }
            Node::Leaf { .. } => panic!("root should be a split"),
        }
        assert_eq!(l.ratio(&[Branch::B]), Some(0.4));
        assert_eq!(l.ratio(&[Branch::A]), None);
        assert_eq!(l.ratio(&[Branch::A, Branch::B]), None);
        assert!(l.set_ratio(&[], f64::NAN));
        assert!(matches!(l.root(), Node::Split { ratio, .. } if *ratio == 0.5));
    }

    #[test]
    fn layout_round_trips_through_json() {
        let mut l = three();
        l.toggle_maximize(AreaId(2));
        let json = serde_json::to_string(&l).unwrap();
        let back: Layout = serde_json::from_str(&json).unwrap();
        assert_eq!(back, l);
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

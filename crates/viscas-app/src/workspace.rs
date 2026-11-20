//! Workspaces: named, predefined area layouts shown as top bar tabs.

use crate::layout::{Dir, EditorKind, Layout, Node};

/// The built-in workspaces, in tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Workspace {
    Graph,
    Algebra,
    Data,
    Script,
}

impl Workspace {
    pub const ALL: [Workspace; 4] = [
        Workspace::Graph,
        Workspace::Algebra,
        Workspace::Data,
        Workspace::Script,
    ];

    /// Stable identifier, used as action target.
    pub fn id(self) -> &'static str {
        match self {
            Workspace::Graph => "graph",
            Workspace::Algebra => "algebra",
            Workspace::Data => "data",
            Workspace::Script => "script",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.id() == id)
    }

    /// Tab label.
    pub fn title(self) -> &'static str {
        match self {
            Workspace::Graph => "Graph",
            Workspace::Algebra => "Algebra",
            Workspace::Data => "Data",
            Workspace::Script => "Script",
        }
    }

    /// The layout a fresh document starts with in this workspace.
    pub fn default_layout(self) -> Layout {
        use Dir::{Horizontal as H, Vertical as V};
        use EditorKind::*;
        let leaf = Node::leaf;
        let root = match self {
            // [SceneTree / Console] | Viewport | Inspector
            Workspace::Graph => Node::split(
                H,
                0.2,
                Node::split(V, 0.55, leaf(SceneTree), leaf(Console)),
                Node::split(H, 0.76, leaf(Viewport), leaf(Inspector)),
            ),
            // Console | [Viewport / Expressions]
            Workspace::Algebra => Node::split(
                H,
                0.55,
                leaf(Console),
                Node::split(V, 0.6, leaf(Viewport), leaf(Expressions)),
            ),
            // Sheet | [Viewport / Inspector]
            Workspace::Data => Node::split(
                H,
                0.55,
                leaf(Sheet),
                Node::split(V, 0.65, leaf(Viewport), leaf(Inspector)),
            ),
            // [Script / Console] | Viewport
            Workspace::Script => Node::split(
                H,
                0.5,
                Node::split(V, 0.68, leaf(Script), leaf(Console)),
                leaf(Viewport),
            ),
        };
        Layout::new(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for w in Workspace::ALL {
            assert_eq!(Workspace::from_id(w.id()), Some(w));
        }
    }

    #[test]
    fn graph_layout_matches_spec() {
        use EditorKind::*;
        let editors: Vec<_> = Workspace::Graph
            .default_layout()
            .leaves()
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        assert_eq!(editors, vec![SceneTree, Console, Viewport, Inspector]);
    }

    #[test]
    fn every_workspace_has_a_viewport() {
        for w in Workspace::ALL {
            let leaves = w.default_layout().leaves();
            assert!(leaves.iter().any(|(_, e)| *e == EditorKind::Viewport));
        }
    }
}

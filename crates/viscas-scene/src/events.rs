//! Change notifications the UI drains after each mutation.

use crate::node::NodeId;

/// Which aspect of a node changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Change {
    Name,
    /// Kind or body (expression text, table cells, settings).
    Kind,
    Visible,
    Locked,
    Style,
    Props,
    /// Evaluation status / result.
    Status,
}

/// A scene notification, queued in mutation order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneEvent {
    NodeAdded(NodeId),
    NodeRemoved(NodeId),
    Changed {
        id: NodeId,
        what: Change,
    },
    /// Reparented and/or reordered.
    Moved {
        id: NodeId,
        old_parent: Option<NodeId>,
        new_parent: Option<NodeId>,
    },
    SelectionChanged,
    /// Edges or diagnostics may have changed because these nodes were
    /// added, removed, renamed or edited (empty: everything, e.g. after a
    /// builtins change).
    GraphChanged {
        touched: Vec<NodeId>,
    },
    /// Undo/redo availability or labels changed.
    HistoryChanged,
}

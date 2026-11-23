//! Undo/redo as reversible commands.
//!
//! Every scene mutation is recorded as a [`Command`] holding enough state
//! to apply it in both directions. A user-visible undo step is a
//! [`Transaction`] (one command, or a group). Consecutive content updates
//! carrying the same coalescing key on the same node (slider drag, typing)
//! merge into one step until the history is sealed.

use crate::node::{Node, NodeContent, NodeId};

/// One reversible mutation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    /// Insert a subtree (nodes in preorder, links intact) under `parent`
    /// at `index`.
    Insert {
        nodes: Vec<Node>,
        parent: Option<NodeId>,
        index: usize,
    },
    /// Remove the subtree rooted at `nodes[0]`.
    Remove {
        nodes: Vec<Node>,
        parent: Option<NodeId>,
        index: usize,
    },
    Move {
        id: NodeId,
        from: (Option<NodeId>, usize),
        to: (Option<NodeId>, usize),
    },
    Update {
        id: NodeId,
        before: Box<NodeContent>,
        after: Box<NodeContent>,
        key: Option<String>,
    },
}

impl Command {
    pub(crate) fn inverse(&self) -> Command {
        match self {
            Command::Insert {
                nodes,
                parent,
                index,
            } => Command::Remove {
                nodes: nodes.clone(),
                parent: *parent,
                index: *index,
            },
            Command::Remove {
                nodes,
                parent,
                index,
            } => Command::Insert {
                nodes: nodes.clone(),
                parent: *parent,
                index: *index,
            },
            Command::Move { id, from, to } => Command::Move {
                id: *id,
                from: *to,
                to: *from,
            },
            Command::Update {
                id,
                before,
                after,
                key,
            } => Command::Update {
                id: *id,
                before: after.clone(),
                after: before.clone(),
                key: key.clone(),
            },
        }
    }

    /// Merge `next` into `self` if both are updates of the same node with
    /// the same coalescing key.
    fn try_merge(&mut self, next: &Command) -> bool {
        match (self, next) {
            (
                Command::Update {
                    id,
                    after,
                    key: Some(key),
                    ..
                },
                Command::Update {
                    id: id2,
                    after: after2,
                    key: Some(key2),
                    ..
                },
            ) if id == id2 && key == key2 => {
                *after = after2.clone();
                true
            }
            _ => false,
        }
    }

    fn is_noop(&self) -> bool {
        matches!(self, Command::Update { before, after, .. } if before == after)
            || matches!(self, Command::Move { from, to, .. } if from == to)
    }
}

/// A user-visible undo step.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Transaction {
    pub label: String,
    pub commands: Vec<Command>,
}

#[derive(Debug)]
pub(crate) struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    group: Option<Transaction>,
    depth: usize,
    /// May the next coalescible update merge into the last step?
    mergeable: bool,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        History {
            undo: Vec::new(),
            redo: Vec::new(),
            group: None,
            depth: 0,
            mergeable: false,
            limit: 1000,
        }
    }
}

impl History {
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(1);
        self.trim();
    }

    fn trim(&mut self) {
        if self.undo.len() > self.limit {
            let excess = self.undo.len() - self.limit;
            self.undo.drain(..excess);
        }
    }

    pub fn record(&mut self, label: &str, cmd: Command) {
        if cmd.is_noop() {
            return;
        }
        self.redo.clear();
        let coalescible = matches!(&cmd, Command::Update { key: Some(_), .. });
        if let Some(group) = &mut self.group {
            let merged = self.mergeable
                && group
                    .commands
                    .last_mut()
                    .is_some_and(|last| last.try_merge(&cmd));
            if !merged {
                group.commands.push(cmd);
            }
            self.mergeable = coalescible;
            return;
        }
        let merged = self.mergeable
            && self
                .undo
                .last_mut()
                .is_some_and(|t| t.commands.len() == 1 && t.commands[0].try_merge(&cmd));
        if merged {
            // Dragging back to the start value cancels the step entirely.
            if self.undo.last().is_some_and(|t| t.commands[0].is_noop()) {
                self.undo.pop();
                self.mergeable = false;
                return;
            }
        } else {
            self.undo.push(Transaction {
                label: label.to_string(),
                commands: vec![cmd],
            });
            self.trim();
        }
        self.mergeable = coalescible;
    }

    /// Stop coalescing into the last step (end of a drag / edit).
    pub fn seal(&mut self) {
        self.mergeable = false;
    }

    pub fn begin_group(&mut self, label: &str) {
        if self.depth == 0 {
            self.group = Some(Transaction {
                label: label.to_string(),
                commands: Vec::new(),
            });
            self.mergeable = false;
        }
        self.depth += 1;
    }

    /// Close one nesting level; returns true if a step was committed.
    pub fn end_group(&mut self) -> bool {
        if self.depth == 0 {
            return false;
        }
        self.depth -= 1;
        if self.depth > 0 {
            return false;
        }
        self.mergeable = false;
        match self.group.take() {
            Some(t) if !t.commands.is_empty() => {
                self.undo.push(t);
                self.trim();
                true
            }
            _ => false,
        }
    }

    pub fn in_group(&self) -> bool {
        self.depth > 0
    }

    /// Close any open group (before undo/redo).
    pub fn close_groups(&mut self) {
        while self.depth > 0 {
            self.end_group();
        }
    }

    pub fn pop_undo(&mut self) -> Option<Transaction> {
        self.close_groups();
        self.mergeable = false;
        self.undo.pop()
    }

    pub fn push_redo(&mut self, t: Transaction) {
        self.redo.push(t);
    }

    pub fn pop_redo(&mut self) -> Option<Transaction> {
        self.close_groups();
        self.mergeable = false;
        self.redo.pop()
    }

    pub fn push_undo(&mut self, t: Transaction) {
        self.undo.push(t);
        self.trim();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.group.as_ref().is_some_and(|g| !g.commands.is_empty())
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|t| t.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|t| t.label.as_str())
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    pub fn clear(&mut self) {
        *self = History {
            limit: self.limit,
            ..History::default()
        };
    }
}

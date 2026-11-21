//! The scene: node tree + cached analysis + dependency graph + history.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use crate::analysis::{Analysis, analyze};
use crate::eval::{NodeEvaluator, NodeResult, Status};
use crate::events::{Change, SceneEvent};
use crate::graph::{DepGraph, Diagnostic, GraphNode, Resolution};
use crate::history::{Command, History};
use crate::names::{Builtins, DefaultBuiltins, is_valid_identifier, unique_name};
use crate::node::{KindTag, Node, NodeContent, NodeId, NodeKind, PropValue, SourceField, Style};

/// Errors of scene operations. A failed operation changes nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneError {
    NoSuchNode(NodeId),
    /// Not a valid identifier for a defining node / parameter.
    InvalidName(String),
    /// Another node already defines this name.
    DuplicateName(String),
    /// Reparenting `node` under `parent` would create a cycle.
    WouldCycle {
        node: NodeId,
        parent: NodeId,
    },
    IndexOutOfRange {
        index: usize,
        len: usize,
    },
    /// The node's kind has no such source field.
    NoSuchField(SourceField),
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SceneError::NoSuchNode(id) => write!(f, "no node {id}"),
            SceneError::InvalidName(n) => write!(f, "`{n}` is not a valid name"),
            SceneError::DuplicateName(n) => write!(f, "`{n}` is already defined"),
            SceneError::WouldCycle { node, parent } => {
                write!(f, "cannot move {node} into its own descendant {parent}")
            }
            SceneError::IndexOutOfRange { index, len } => {
                write!(f, "index {index} out of range (0..={len})")
            }
            SceneError::NoSuchField(field) => write!(f, "node has no `{field}` field"),
        }
    }
}

impl std::error::Error for SceneError {}

pub type Result<T, E = SceneError> = std::result::Result<T, E>;

/// A document's node tree with its reactive dependency graph.
pub struct Scene {
    pub(crate) nodes: HashMap<NodeId, Node>,
    pub(crate) roots: Vec<NodeId>,
    pub(crate) next_id: u64,
    analysis: HashMap<NodeId, Analysis>,
    graph: DepGraph,
    results: HashMap<NodeId, NodeResult>,
    dirty: BTreeSet<NodeId>,
    selection: Vec<NodeId>,
    events: Vec<SceneEvent>,
    history: History,
    builtins: Arc<dyn Builtins>,
    pub(crate) layout: Option<serde_json::Value>,
}

impl Default for Scene {
    fn default() -> Self {
        Scene::new()
    }
}

impl fmt::Debug for Scene {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scene")
            .field("nodes", &self.nodes.len())
            .field("roots", &self.roots)
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

/// Depth-first (preorder) iterator over the tree.
pub struct DepthFirst<'a> {
    scene: &'a Scene,
    stack: Vec<NodeId>,
}

impl<'a> Iterator for DepthFirst<'a> {
    type Item = &'a Node;

    fn next(&mut self) -> Option<&'a Node> {
        let id = self.stack.pop()?;
        let node = self.scene.nodes.get(&id)?;
        self.stack.extend(node.children.iter().rev());
        Some(node)
    }
}

impl Scene {
    pub fn new() -> Scene {
        Scene {
            nodes: HashMap::new(),
            roots: Vec::new(),
            next_id: 1,
            analysis: HashMap::new(),
            graph: DepGraph::default(),
            results: HashMap::new(),
            dirty: BTreeSet::new(),
            selection: Vec::new(),
            events: Vec::new(),
            history: History::default(),
            builtins: Arc::new(DefaultBuiltins),
            layout: None,
        }
    }

    // ------------------------------------------------------------------
    // Tree queries
    // ------------------------------------------------------------------

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    fn node(&self, id: NodeId) -> Result<&Node> {
        self.nodes.get(&id).ok_or(SceneError::NoSuchNode(id))
    }

    /// Top-level nodes in order.
    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    /// Children of `parent` (`None` = top level).
    pub fn children(&self, parent: Option<NodeId>) -> &[NodeId] {
        match parent {
            Some(p) => self.nodes.get(&p).map_or(&[], |n| n.children.as_slice()),
            None => &self.roots,
        }
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.get(&id).and_then(|n| n.parent)
    }

    /// Position of `id` among its siblings.
    pub fn index_in_parent(&self, id: NodeId) -> Option<usize> {
        let parent = self.nodes.get(&id)?.parent;
        self.children(parent).iter().position(|&c| c == id)
    }

    /// Depth-first preorder over the whole tree.
    pub fn iter(&self) -> DepthFirst<'_> {
        DepthFirst {
            scene: self,
            stack: self.roots.iter().rev().copied().collect(),
        }
    }

    /// Depth-first preorder over the subtree rooted at `id` (inclusive).
    pub fn iter_subtree(&self, id: NodeId) -> DepthFirst<'_> {
        DepthFirst {
            scene: self,
            stack: if self.contains(id) { vec![id] } else { vec![] },
        }
    }

    /// Ids in depth-first preorder.
    pub fn dfs_ids(&self) -> Vec<NodeId> {
        self.iter().map(|n| n.id).collect()
    }

    /// Descendants of `id` (exclusive), preorder.
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        self.iter_subtree(id).skip(1).map(|n| n.id).collect()
    }

    /// Is `ancestor` a strict ancestor of `id`?
    pub fn is_ancestor(&self, ancestor: NodeId, id: NodeId) -> bool {
        let mut cur = self.parent(id);
        while let Some(p) = cur {
            if p == ancestor {
                return true;
            }
            cur = self.parent(p);
        }
        false
    }

    /// Depth (0 for top-level nodes).
    pub fn depth(&self, id: NodeId) -> usize {
        let mut d = 0;
        let mut cur = self.parent(id);
        while let Some(p) = cur {
            d += 1;
            cur = self.parent(p);
        }
        d
    }

    /// Nodes matching `pred`, in tree order.
    pub fn find(&self, mut pred: impl FnMut(&Node) -> bool) -> Vec<NodeId> {
        self.iter().filter(|n| pred(n)).map(|n| n.id).collect()
    }

    /// Nodes of one kind, in tree order.
    pub fn find_kind(&self, tag: KindTag) -> Vec<NodeId> {
        self.find(|n| n.kind.tag() == tag)
    }

    /// The node defining `name`, or else the first node labelled `name`.
    pub fn lookup(&self, name: &str) -> Option<NodeId> {
        self.graph.definers(name).first().copied().or_else(|| {
            self.iter()
                .find(|n| n.name.as_deref() == Some(name))
                .map(|n| n.id)
        })
    }

    /// Is `name` defined by a node other than `except`?
    pub fn is_name_taken(&self, name: &str, except: Option<NodeId>) -> bool {
        self.graph.definers(name).iter().any(|&d| Some(d) != except)
    }

    /// A free name derived from `base` (`a`, `a_2`, …).
    pub fn unique_name(&self, base: &str) -> String {
        unique_name(base, |n| self.is_name_taken(n, None))
    }

    // ------------------------------------------------------------------
    // Analysis and graph queries
    // ------------------------------------------------------------------

    /// Cached parse + reference analysis of a node.
    pub fn analysis(&self, id: NodeId) -> Option<&Analysis> {
        self.analysis.get(&id)
    }

    pub fn graph(&self) -> &DepGraph {
        &self.graph
    }

    /// Names a node defines.
    pub fn defines(&self, id: NodeId) -> &[String] {
        self.analysis.get(&id).map_or(&[], |a| a.defines.as_slice())
    }

    /// Diagnostics of a node (parse errors, cycles, duplicates, unknown
    /// names).
    pub fn diagnostics(&self, id: NodeId) -> &[Diagnostic] {
        self.graph.diagnostics(id)
    }

    /// Transitive inputs of `id`, topological order.
    pub fn upstream(&self, id: NodeId) -> Vec<NodeId> {
        self.graph.upstream(id)
    }

    /// Transitive dependents of `id`, topological order.
    pub fn downstream(&self, id: NodeId) -> Vec<NodeId> {
        self.graph.downstream(id)
    }

    /// Everything to highlight when hovering `id`: upstream and downstream.
    pub fn related(&self, id: NodeId) -> Vec<NodeId> {
        let mut v = self.upstream(id);
        v.extend(self.downstream(id));
        self.graph.sorted(v.into_iter().collect::<BTreeSet<_>>())
    }

    /// All nodes, dependencies first.
    pub fn topo_order(&self) -> &[NodeId] {
        self.graph.topo_order()
    }

    /// Replace the built-in name oracle (rebuilds the graph, marks all
    /// nodes dirty).
    pub fn set_builtins(&mut self, builtins: Arc<dyn Builtins>) {
        self.builtins = builtins;
        self.rebuild_graph();
        self.mark_all_dirty();
    }

    // ------------------------------------------------------------------
    // Dirty tracking and evaluation
    // ------------------------------------------------------------------

    pub fn is_dirty(&self, id: NodeId) -> bool {
        self.dirty.contains(&id)
    }

    /// Dirty nodes in topological order (without clearing).
    pub fn dirty(&self) -> Vec<NodeId> {
        self.graph.sorted(self.dirty.iter().copied())
    }

    /// Take the dirty set in topological order (inputs first).
    pub fn take_dirty(&mut self) -> Vec<NodeId> {
        let set = std::mem::take(&mut self.dirty);
        self.graph.sorted(set)
    }

    /// Mark `id` and everything downstream dirty (e.g. re-roll `random`).
    pub fn mark_dirty(&mut self, id: NodeId) {
        if !self.contains(id) {
            return;
        }
        self.set_dirty(id);
        for d in self.graph.downstream(id) {
            self.set_dirty(d);
        }
    }

    pub fn mark_all_dirty(&mut self) {
        for id in self.dfs_ids() {
            self.set_dirty(id);
        }
    }

    fn set_dirty(&mut self, id: NodeId) {
        self.dirty.insert(id);
        let r = self.results.entry(id).or_default();
        if r.status != Status::Pending {
            *r = NodeResult::default();
            self.events.push(SceneEvent::Changed {
                id,
                what: Change::Status,
            });
        }
    }

    pub fn status(&self, id: NodeId) -> &Status {
        static PENDING: Status = Status::Pending;
        self.results.get(&id).map_or(&PENDING, |r| &r.status)
    }

    pub fn result(&self, id: NodeId) -> Option<&NodeResult> {
        self.results.get(&id)
    }

    /// Store an evaluation result.
    pub fn set_result(&mut self, id: NodeId, result: NodeResult) {
        if !self.contains(id) {
            return;
        }
        if self.results.get(&id) != Some(&result) {
            self.results.insert(id, result);
            self.events.push(SceneEvent::Changed {
                id,
                what: Change::Status,
            });
        }
    }

    /// Evaluate every dirty node, inputs first. Nodes with parse errors,
    /// blocking diagnostics (cycle, duplicate, ambiguous) or an errored
    /// input get an error status without calling the evaluator. Returns
    /// the processed nodes in order.
    pub fn evaluate_dirty(&mut self, evaluator: &mut dyn NodeEvaluator) -> Vec<NodeId> {
        let order = self.take_dirty();
        for &id in &order {
            let result = self
                .blocked_result(id)
                .unwrap_or_else(|| evaluator.evaluate(self, id));
            self.set_result(id, result);
        }
        order
    }

    fn blocked_result(&self, id: NodeId) -> Option<NodeResult> {
        if let Some((field, e)) = self.analysis.get(&id).and_then(Analysis::first_error) {
            return Some(NodeResult {
                status: Status::Error {
                    message: e.message.clone(),
                    field: Some(field),
                    span: Some(e.span),
                },
                display: None,
            });
        }
        if let Some(d) = self.graph.diagnostics(id).iter().find(|d| d.is_error()) {
            return Some(NodeResult {
                status: Status::Error {
                    message: d.message.clone(),
                    field: d.field,
                    span: d.span,
                },
                display: None,
            });
        }
        let failed = self
            .graph
            .direct_upstream(id)
            .into_iter()
            .find(|&u| u != id && self.status(u).is_error())?;
        let name = self
            .nodes
            .get(&failed)
            .map_or_else(String::new, Node::title);
        Some(NodeResult::error(format!(
            "depends on `{name}`, which has an error"
        )))
    }

    // ------------------------------------------------------------------
    // Events
    // ------------------------------------------------------------------

    /// Take queued notifications.
    pub fn drain_events(&mut self) -> Vec<SceneEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn has_events(&self) -> bool {
        !self.events.is_empty()
    }

    // ------------------------------------------------------------------
    // Selection (not part of undo)
    // ------------------------------------------------------------------

    /// Selected nodes in selection order; the last one is primary.
    pub fn selection(&self) -> &[NodeId] {
        &self.selection
    }

    pub fn primary_selection(&self) -> Option<NodeId> {
        self.selection.last().copied()
    }

    pub fn is_selected(&self, id: NodeId) -> bool {
        self.selection.contains(&id)
    }

    /// Replace the selection.
    pub fn set_selection(&mut self, ids: impl IntoIterator<Item = NodeId>) {
        let mut new = Vec::new();
        for id in ids {
            if self.contains(id) && !new.contains(&id) {
                new.push(id);
            }
        }
        if new != self.selection {
            self.selection = new;
            self.events.push(SceneEvent::SelectionChanged);
        }
    }

    /// Select only `id`.
    pub fn select(&mut self, id: NodeId) {
        self.set_selection([id]);
    }

    /// Add `id` (becomes primary).
    pub fn add_to_selection(&mut self, id: NodeId) {
        if !self.contains(id) {
            return;
        }
        let mut s: Vec<NodeId> = self
            .selection
            .iter()
            .copied()
            .filter(|&x| x != id)
            .collect();
        s.push(id);
        self.set_selection(s);
    }

    pub fn deselect(&mut self, id: NodeId) {
        let s: Vec<NodeId> = self
            .selection
            .iter()
            .copied()
            .filter(|&x| x != id)
            .collect();
        self.set_selection(s);
    }

    pub fn toggle_selection(&mut self, id: NodeId) {
        if self.is_selected(id) {
            self.deselect(id);
        } else {
            self.add_to_selection(id);
        }
    }

    pub fn clear_selection(&mut self) {
        self.set_selection([]);
    }

    pub fn select_all(&mut self) {
        let all = self.dfs_ids();
        self.set_selection(all);
    }

    // ------------------------------------------------------------------
    // Layout blob
    // ------------------------------------------------------------------

    /// App-owned area/workspace layout stored with the document.
    pub fn layout(&self) -> Option<&serde_json::Value> {
        self.layout.as_ref()
    }

    pub fn set_layout(&mut self, layout: Option<serde_json::Value>) {
        self.layout = layout;
    }

    // ------------------------------------------------------------------
    // History
    // ------------------------------------------------------------------

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.history.undo_label()
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.history.redo_label()
    }

    /// Number of undo steps.
    pub fn undo_depth(&self) -> usize {
        self.history.undo_len()
    }

    pub fn redo_depth(&self) -> usize {
        self.history.redo_len()
    }

    pub fn set_history_limit(&mut self, limit: usize) {
        self.history.set_limit(limit);
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.events.push(SceneEvent::HistoryChanged);
    }

    /// End coalescing: the next edit starts a new undo step (call at the
    /// end of a drag or when a text field loses focus).
    pub fn seal_history(&mut self) {
        self.history.seal();
    }

    /// Start grouping mutations into one undo step. Groups nest; only the
    /// outermost label is kept.
    pub fn begin_group(&mut self, label: &str) {
        self.history.begin_group(label);
    }

    /// Is an undo group open?
    pub fn in_group(&self) -> bool {
        self.history.in_group()
    }

    pub fn end_group(&mut self) {
        if self.history.end_group() {
            self.events.push(SceneEvent::HistoryChanged);
        }
    }

    /// Run `f` inside one undo group.
    pub fn transaction<R>(&mut self, label: &str, f: impl FnOnce(&mut Scene) -> R) -> R {
        self.begin_group(label);
        let r = f(self);
        self.end_group();
        r
    }

    pub fn undo(&mut self) -> bool {
        let Some(t) = self.history.pop_undo() else {
            return false;
        };
        for c in t.commands.iter().rev() {
            self.apply(&c.inverse());
        }
        self.history.push_redo(t);
        self.events.push(SceneEvent::HistoryChanged);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(t) = self.history.pop_redo() else {
            return false;
        };
        for c in &t.commands {
            self.apply(c);
        }
        self.history.push_undo(t);
        self.events.push(SceneEvent::HistoryChanged);
        true
    }

    fn execute(&mut self, label: &str, cmd: Command) {
        self.apply(&cmd);
        self.history.record(label, cmd);
        self.events.push(SceneEvent::HistoryChanged);
    }

    // ------------------------------------------------------------------
    // Mutations
    // ------------------------------------------------------------------

    fn alloc_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    fn check_index(
        &self,
        parent: Option<NodeId>,
        index: usize,
        moving: Option<NodeId>,
    ) -> Result<()> {
        if let Some(p) = parent {
            self.node(p)?;
        }
        let mut len = self.children(parent).len();
        if moving.is_some_and(|m| self.parent(m) == parent) {
            len -= 1;
        }
        if index > len {
            return Err(SceneError::IndexOutOfRange { index, len });
        }
        Ok(())
    }

    /// Validate content about to be stored on `id` (or a new node).
    fn validate(&self, id: Option<NodeId>, c: &NodeContent) -> Result<()> {
        if c.kind.defines_by_name()
            && let Some(name) = &c.name
        {
            if !is_valid_identifier(name) {
                return Err(SceneError::InvalidName(name.clone()));
            }
            if self.is_name_taken(name, id) {
                return Err(SceneError::DuplicateName(name.clone()));
            }
        }
        match &c.kind {
            NodeKind::Function { params, .. } => {
                let mut seen = HashSet::new();
                for p in params {
                    if !is_valid_identifier(p) || !seen.insert(p) {
                        return Err(SceneError::InvalidName(p.clone()));
                    }
                }
            }
            NodeKind::Regression { params, .. } => {
                let mut seen = HashSet::new();
                for p in params {
                    if !is_valid_identifier(p) || !seen.insert(p) {
                        return Err(SceneError::InvalidName(p.clone()));
                    }
                    if self.is_name_taken(p, id) {
                        return Err(SceneError::DuplicateName(p.clone()));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn normalize(mut c: NodeContent) -> NodeContent {
        if c.name.as_deref().is_some_and(|n| n.trim().is_empty()) {
            c.name = None;
        }
        c
    }

    /// Append a new node under `parent` (`None` = top level).
    pub fn add(&mut self, parent: Option<NodeId>, content: NodeContent) -> Result<NodeId> {
        let index = self.children(parent).len();
        self.insert(parent, index, content)
    }

    /// Insert a new node under `parent` at `index`.
    pub fn insert(
        &mut self,
        parent: Option<NodeId>,
        index: usize,
        content: NodeContent,
    ) -> Result<NodeId> {
        let content = Self::normalize(content);
        self.check_index(parent, index, None)?;
        self.validate(None, &content)?;
        let id = self.alloc_id();
        let node = Node::new(id, content);
        self.execute(
            "Add node",
            Command::Insert {
                nodes: vec![node],
                parent,
                index,
            },
        );
        Ok(id)
    }

    /// Snapshot of the subtree rooted at `id`, preorder.
    fn snapshot(&self, id: NodeId) -> Vec<Node> {
        self.iter_subtree(id).cloned().collect()
    }

    /// Delete `id` and its whole subtree.
    pub fn remove(&mut self, id: NodeId) -> Result<()> {
        let node = self.node(id)?;
        let parent = node.parent;
        let index = self.index_in_parent(id).unwrap_or(0);
        let nodes = self.snapshot(id);
        self.execute(
            "Delete node",
            Command::Remove {
                nodes,
                parent,
                index,
            },
        );
        Ok(())
    }

    /// Delete several nodes (and subtrees) as one undo step. Ids inside an
    /// already-deleted subtree are skipped.
    pub fn remove_many(&mut self, ids: &[NodeId]) -> Result<()> {
        for &id in ids {
            self.node(id)?;
        }
        self.transaction("Delete nodes", |s| {
            for &id in ids {
                if s.contains(id) {
                    s.remove(id)?;
                }
            }
            Ok(())
        })
    }

    /// Reparent and/or reorder: `id` ends up at `index` among the children
    /// of `parent` (index counted without `id`). Moving a node into its own
    /// subtree is refused.
    pub fn move_node(&mut self, id: NodeId, parent: Option<NodeId>, index: usize) -> Result<()> {
        let from_parent = self.node(id)?.parent;
        if let Some(p) = parent
            && (p == id || self.is_ancestor(id, p))
        {
            self.node(p)?;
            return Err(SceneError::WouldCycle {
                node: id,
                parent: p,
            });
        }
        self.check_index(parent, index, Some(id))?;
        let from_index = self.index_in_parent(id).unwrap_or(0);
        self.execute(
            "Move node",
            Command::Move {
                id,
                from: (from_parent, from_index),
                to: (parent, index),
            },
        );
        Ok(())
    }

    /// Move `id` to `index` among its current siblings.
    pub fn reorder(&mut self, id: NodeId, index: usize) -> Result<()> {
        let parent = self.node(id)?.parent;
        self.move_node(id, parent, index)
    }

    /// Replace a node's whole content (validated). `key` enables
    /// coalescing with the previous edit carrying the same key.
    pub fn set_content(
        &mut self,
        id: NodeId,
        content: NodeContent,
        key: Option<&str>,
        label: &str,
    ) -> Result<()> {
        let content = Self::normalize(content);
        let before = self.node(id)?.content();
        if before == content {
            return Ok(());
        }
        let name_changed = before.name != content.name || before.kind.tag() != content.kind.tag();
        let params_changed = match (&before.kind, &content.kind) {
            (NodeKind::Regression { params: a, .. }, NodeKind::Regression { params: b, .. })
            | (NodeKind::Function { params: a, .. }, NodeKind::Function { params: b, .. }) => {
                a != b
            }
            _ => false,
        };
        if name_changed || params_changed {
            self.validate(Some(id), &content)?;
        }
        self.execute(
            label,
            Command::Update {
                id,
                before: Box::new(before),
                after: Box::new(content),
                key: key.map(String::from),
            },
        );
        Ok(())
    }

    /// Edit a node's content in place (one undo step).
    pub fn update(&mut self, id: NodeId, f: impl FnOnce(&mut NodeContent)) -> Result<()> {
        let mut c = self.node(id)?.content();
        f(&mut c);
        self.set_content(id, c, None, "Edit node")
    }

    /// Like [`Scene::update`] but coalesces with previous edits using the
    /// same `key` on the same node until [`Scene::seal_history`].
    pub fn update_coalesced(
        &mut self,
        id: NodeId,
        key: &str,
        f: impl FnOnce(&mut NodeContent),
    ) -> Result<()> {
        let mut c = self.node(id)?.content();
        f(&mut c);
        self.set_content(id, c, Some(key), "Edit node")
    }

    /// Rename (`None` / empty clears the name). Names of defining nodes
    /// must be unique valid identifiers; other kinds take free labels.
    pub fn rename(&mut self, id: NodeId, name: Option<&str>) -> Result<()> {
        let mut c = self.node(id)?.content();
        c.name = name.map(String::from);
        self.set_content(id, c, None, "Rename")
    }

    /// Rename a defining node and rewrite every reference to its old name
    /// in its dependents (and in its own body), as one undo step.
    pub fn rename_refactor(&mut self, id: NodeId, new_name: &str) -> Result<()> {
        let node = self.node(id)?;
        let Some(old) = node.defined_name().map(String::from) else {
            return self.rename(id, Some(new_name));
        };
        let mut c = node.content();
        c.name = Some(new_name.to_string());
        self.validate(Some(id), &Self::normalize(c))?;
        // Dependents plus the node itself (recursive functions).
        let mut targets = self.graph.direct_downstream(id);
        if !targets.contains(&id) {
            targets.push(id);
        }
        self.transaction("Rename", |s| {
            for d in targets {
                s.repoint(d, &old, new_name)?;
            }
            s.rename(id, Some(new_name))
        })
    }

    pub fn set_visible(&mut self, id: NodeId, visible: bool) -> Result<()> {
        let mut c = self.node(id)?.content();
        c.visible = visible;
        self.set_content(id, c, None, if visible { "Show" } else { "Hide" })
    }

    pub fn set_locked(&mut self, id: NodeId, locked: bool) -> Result<()> {
        let mut c = self.node(id)?.content();
        c.locked = locked;
        self.set_content(id, c, None, if locked { "Lock" } else { "Unlock" })
    }

    /// Set the style (coalescing key `style`).
    pub fn set_style(&mut self, id: NodeId, style: Style) -> Result<()> {
        let mut c = self.node(id)?.content();
        c.style = style;
        self.set_content(id, c, Some("style"), "Change style")
    }

    /// Set a free-form property (coalescing key `prop:<key>`).
    pub fn set_prop(&mut self, id: NodeId, key: &str, value: impl Into<PropValue>) -> Result<()> {
        let mut c = self.node(id)?.content();
        c.props.insert(key.to_string(), value.into());
        self.set_content(id, c, Some(&format!("prop:{key}")), "Set property")
    }

    pub fn remove_prop(&mut self, id: NodeId, key: &str) -> Result<()> {
        let mut c = self.node(id)?.content();
        c.props.remove(key);
        self.set_content(id, c, None, "Remove property")
    }

    /// Replace the kind/body.
    pub fn set_kind(&mut self, id: NodeId, kind: NodeKind) -> Result<()> {
        let mut c = self.node(id)?.content();
        c.kind = kind;
        self.set_content(id, c, None, "Change node")
    }

    /// Set one source field's text (coalescing key `source:<field>`, so a
    /// slider drag or a typing burst is one undo step).
    pub fn set_source(&mut self, id: NodeId, field: SourceField, text: &str) -> Result<()> {
        let mut c = self.node(id)?.content();
        let slot = c
            .kind
            .source_mut(field)
            .ok_or(SceneError::NoSuchField(field))?;
        *slot = text.to_string();
        self.set_content(id, c, Some(&format!("source:{field}")), "Edit expression")
    }

    /// Re-point references: rewrite every free occurrence of `from` in the
    /// node's sources to `to`, leaving the rest of the text untouched.
    /// Letter-split occurrences (`ab` read as `a·b`) are spelled out
    /// (`c b`). Returns the number of rewritten occurrences.
    pub fn repoint(&mut self, id: NodeId, from: &str, to: &str) -> Result<usize> {
        if !is_valid_identifier(to) {
            return Err(SceneError::InvalidName(to.to_string()));
        }
        let node = self.node(id)?;
        let Some(analysis) = self.analysis.get(&id) else {
            return Ok(0);
        };
        let resolutions = self.graph.resolutions(id);
        let mut edits: Vec<(SourceField, usize, usize, String)> = Vec::new();
        for (i, r) in analysis.references.iter().enumerate() {
            if r.name == from {
                edits.push((r.field, r.span.start, r.span.end, to.to_string()));
            } else if let (Some(Resolution::Split(resolved)), Some(letters)) =
                (resolutions.get(i), &r.split)
                && resolved.iter().any(|(l, _)| l == from)
            {
                let spelled: Vec<&str> = letters
                    .iter()
                    .map(|l| {
                        if !l.bound && l.name == from {
                            to
                        } else {
                            l.name.as_str()
                        }
                    })
                    .collect();
                edits.push((r.field, r.span.start, r.span.end, spelled.join(" ")));
            }
        }
        if edits.is_empty() {
            return Ok(0);
        }
        let mut c = node.content();
        edits.sort_by_key(|e| std::cmp::Reverse((e.0, e.1)));
        for (field, start, end, text) in &edits {
            let slot = c
                .kind
                .source_mut(*field)
                .ok_or(SceneError::NoSuchField(*field))?;
            if slot.get(*start..*end).is_none() {
                continue;
            }
            slot.replace_range(*start..*end, text);
        }
        self.set_content(id, c, None, "Re-point reference")?;
        Ok(edits.len())
    }

    /// Patch-cable helper: make `dependent` reference `to` instead of
    /// `from` (both must define a name).
    pub fn repoint_edge(&mut self, dependent: NodeId, from: NodeId, to: NodeId) -> Result<usize> {
        let old = self
            .defines(from)
            .first()
            .cloned()
            .ok_or(SceneError::NoSuchNode(from))?;
        let new = self
            .defines(to)
            .first()
            .cloned()
            .ok_or(SceneError::NoSuchNode(to))?;
        self.repoint(dependent, &old, &new)
    }

    /// Deep-copy a subtree right after the original. Defined names get
    /// fresh unique names. Returns the new root.
    pub fn duplicate(&mut self, id: NodeId) -> Result<NodeId> {
        let parent = self.node(id)?.parent;
        let index = self.index_in_parent(id).unwrap_or(0) + 1;
        let originals = self.snapshot(id);
        let mut map = HashMap::new();
        for n in &originals {
            let new = self.alloc_id();
            map.insert(n.id, new);
        }
        let mut taken: HashSet<String> = HashSet::new();
        let mut nodes = Vec::with_capacity(originals.len());
        for n in originals {
            let mut copy = n.clone();
            copy.id = map[&n.id];
            copy.parent = n.parent.map(|p| map.get(&p).copied().unwrap_or(p));
            copy.children = n.children.iter().map(|c| map[c]).collect();
            if copy.kind.defines_by_name()
                && let Some(name) = &copy.name
            {
                let fresh = unique_name(name, |c| self.is_name_taken(c, None) || taken.contains(c));
                taken.insert(fresh.clone());
                copy.name = Some(fresh);
            }
            if let NodeKind::Regression { params, .. } = &mut copy.kind {
                for p in params.iter_mut() {
                    let fresh =
                        unique_name(p, |c| self.is_name_taken(c, None) || taken.contains(c));
                    taken.insert(fresh.clone());
                    *p = fresh;
                }
            }
            nodes.push(copy);
        }
        let root = nodes[0].id;
        self.execute(
            "Duplicate",
            Command::Insert {
                nodes,
                parent,
                index,
            },
        );
        Ok(root)
    }

    // ------------------------------------------------------------------
    // Raw (unrecorded) operations
    // ------------------------------------------------------------------

    pub(crate) fn apply(&mut self, cmd: &Command) {
        match cmd {
            Command::Insert {
                nodes,
                parent,
                index,
            } => self.raw_insert(nodes.clone(), *parent, *index),
            Command::Remove { nodes, .. } => {
                if let Some(root) = nodes.first() {
                    self.raw_remove(root.id);
                }
            }
            Command::Move { id, to, .. } => self.raw_move(*id, to.0, to.1),
            Command::Update { id, after, .. } => self.raw_update(*id, (**after).clone()),
        }
    }

    fn child_list_mut(&mut self, parent: Option<NodeId>) -> &mut Vec<NodeId> {
        match parent {
            Some(p) if self.nodes.contains_key(&p) => {
                &mut self.nodes.get_mut(&p).expect("checked").children
            }
            _ => &mut self.roots,
        }
    }

    /// Nodes affected by a change of `ids` in the current graph.
    fn affected(&self, ids: &[NodeId]) -> BTreeSet<NodeId> {
        let mut set = BTreeSet::new();
        for &id in ids {
            if self.contains(id) {
                set.insert(id);
                set.extend(self.graph.downstream(id));
            }
        }
        set
    }

    /// Rebuild the graph; if `dirty`, mark `ids`, `before` and the new
    /// downstream of `ids` dirty.
    fn refresh(&mut self, ids: &[NodeId], before: BTreeSet<NodeId>, dirty: bool) {
        self.rebuild_graph();
        if dirty {
            let mut set = before;
            set.extend(self.affected(ids));
            for id in set {
                if self.contains(id) {
                    self.set_dirty(id);
                }
            }
        }
    }

    pub(crate) fn rebuild_graph(&mut self) {
        let order = self.dfs_ids();
        let empty = Analysis::default();
        let nodes: Vec<GraphNode<'_>> = order
            .iter()
            .map(|id| {
                let n = &self.nodes[id];
                GraphNode {
                    id: *id,
                    analysis: self.analysis.get(id).unwrap_or(&empty),
                    recursive: n.kind.allows_recursion(),
                    title: n.defined_name().map_or_else(|| n.title(), String::from),
                }
            })
            .collect();
        let g = DepGraph::build(&nodes, &*self.builtins);
        if g != self.graph {
            self.graph = g;
            self.events.push(SceneEvent::GraphChanged);
        }
    }

    pub(crate) fn raw_insert(&mut self, nodes: Vec<Node>, parent: Option<NodeId>, index: usize) {
        let Some(root) = nodes.first().map(|n| n.id) else {
            return;
        };
        let ids: Vec<NodeId> = nodes.iter().map(|n| n.id).collect();
        for mut n in nodes {
            if n.id == root {
                n.parent = parent.filter(|p| self.nodes.contains_key(p));
            }
            self.next_id = self.next_id.max(n.id.0 + 1);
            self.analysis.insert(n.id, analyze(&n.content()));
            self.nodes.insert(n.id, n);
        }
        let list = self.child_list_mut(parent);
        let i = index.min(list.len());
        list.insert(i, root);
        for &id in &ids {
            self.events.push(SceneEvent::NodeAdded(id));
        }
        self.refresh(&ids, BTreeSet::new(), true);
    }

    pub(crate) fn raw_remove(&mut self, root: NodeId) {
        let Some(node) = self.nodes.get(&root) else {
            return;
        };
        let parent = node.parent;
        let ids: Vec<NodeId> = self.iter_subtree(root).map(|n| n.id).collect();
        let before = self.affected(&ids);
        self.child_list_mut(parent).retain(|&c| c != root);
        for &id in &ids {
            self.nodes.remove(&id);
            self.analysis.remove(&id);
            self.results.remove(&id);
            self.dirty.remove(&id);
            self.events.push(SceneEvent::NodeRemoved(id));
        }
        let sel: Vec<NodeId> = self
            .selection
            .iter()
            .copied()
            .filter(|s| !ids.contains(s))
            .collect();
        self.set_selection(sel);
        self.refresh(&[], before, true);
    }

    pub(crate) fn raw_move(&mut self, id: NodeId, parent: Option<NodeId>, index: usize) {
        let Some(node) = self.nodes.get(&id) else {
            return;
        };
        let old_parent = node.parent;
        self.child_list_mut(old_parent).retain(|&c| c != id);
        let parent = parent.filter(|p| self.nodes.contains_key(p));
        let list = self.child_list_mut(parent);
        let i = index.min(list.len());
        list.insert(i, id);
        if let Some(n) = self.nodes.get_mut(&id) {
            n.parent = parent;
        }
        self.events.push(SceneEvent::Moved {
            id,
            old_parent,
            new_parent: parent,
        });
        self.refresh(&[], BTreeSet::new(), false);
    }

    pub(crate) fn raw_update(&mut self, id: NodeId, content: NodeContent) {
        let Some(node) = self.nodes.get(&id) else {
            return;
        };
        let old = node.content();
        let mut changes = Vec::new();
        if old.name != content.name {
            changes.push(Change::Name);
        }
        if old.kind != content.kind {
            changes.push(Change::Kind);
        }
        if old.visible != content.visible {
            changes.push(Change::Visible);
        }
        if old.locked != content.locked {
            changes.push(Change::Locked);
        }
        if old.style != content.style {
            changes.push(Change::Style);
        }
        if old.props != content.props {
            changes.push(Change::Props);
        }
        let semantic = changes.contains(&Change::Name) || changes.contains(&Change::Kind);
        let before = if semantic {
            self.affected(&[id])
        } else {
            BTreeSet::new()
        };
        if semantic {
            self.analysis.insert(id, analyze(&content));
        }
        if let Some(n) = self.nodes.get_mut(&id) {
            n.set_content(content);
        }
        for what in changes {
            self.events.push(SceneEvent::Changed { id, what });
        }
        if semantic {
            self.refresh(&[id], before, true);
        }
    }

    /// Recompute analysis for every node and rebuild the graph (after
    /// loading).
    pub(crate) fn reanalyze_all(&mut self) {
        self.analysis = self
            .nodes
            .iter()
            .map(|(id, n)| (*id, analyze(&n.content())))
            .collect();
        self.rebuild_graph();
        self.mark_all_dirty();
    }
}

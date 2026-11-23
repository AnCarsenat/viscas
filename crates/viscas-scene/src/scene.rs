//! The scene: node tree + cached analysis + dependency graph + history.
//!
//! Performance model: per-node analysis is cached; two indexes (`definers`:
//! name → defining nodes, `mentions`: name → nodes referencing or writing
//! it) are updated incrementally on every insert/remove/update and drive
//! name validation and dirty propagation. The full [`DepGraph`] is built
//! lazily on the first read after a semantic change; it does not depend on
//! tree order, so moves and reorders never invalidate it.

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;

use viscas_core::syntax::{TokenKind, lexer};

use crate::analysis::{Analysis, Reference, analyze};
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
    /// A rewrite would make `name` in `node` refer to something else (a
    /// local binder or a built-in).
    NameCapture {
        node: NodeId,
        name: String,
    },
    /// Cached spans no longer match the node's source text.
    SourceMismatch {
        node: NodeId,
        field: SourceField,
    },
    /// The node defines no name (needed as a re-point target).
    NotDefining(NodeId),
    /// A node with this id already exists.
    IdInUse(NodeId),
    /// No more node ids can be allocated.
    IdsExhausted,
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
            SceneError::NameCapture { node, name } => {
                write!(f, "`{name}` would be captured in {node}")
            }
            SceneError::SourceMismatch { node, field } => {
                write!(f, "source of {node} ({field}) changed under the edit")
            }
            SceneError::NotDefining(id) => write!(f, "{id} defines no name"),
            SceneError::IdInUse(id) => write!(f, "node id {id} is already in use"),
            SceneError::IdsExhausted => f.write_str("node ids exhausted"),
        }
    }
}

impl std::error::Error for SceneError {}

pub type Result<T, E = SceneError> = std::result::Result<T, E>;

/// Outcome of [`Scene::rename_refactor`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenameReport {
    /// Rewritten identifier occurrences.
    pub rewritten: usize,
    /// Nodes whose sources were rewritten.
    pub nodes: Vec<NodeId>,
}

/// An identifier occurrence offered to a rewrite.
enum Occ<'a> {
    Read(&'a Reference, Option<&'a Resolution>),
    Write(&'a Reference, &'a [NodeId]),
    /// Occurrence of a name the node defines internally.
    Own(&'a Reference),
}

/// Replacement text plus the names it introduces (checked for capture).
type Pick = Option<(String, Vec<String>)>;

/// Spell a split name (`ab`) with chosen letters replaced (`q b`).
fn spell(r: &Reference, replace: impl Fn(&str) -> Option<String>) -> Pick {
    let letters = r.split.as_ref()?;
    let mut introduced = Vec::new();
    let parts: Vec<String> = letters
        .iter()
        .map(|l| match (!l.bound).then(|| replace(&l.name)).flatten() {
            Some(new) => {
                introduced.push(new.clone());
                new
            }
            None => l.name.clone(),
        })
        .collect();
    (!introduced.is_empty()).then(|| (parts.join(" "), introduced))
}

/// Is `text` exactly the identifier `name`?
fn is_ident_text(text: &str, name: &str) -> bool {
    let (tokens, errors) = lexer::lex(text);
    errors.is_empty()
        && matches!(tokens.as_slice(), [t, e]
            if matches!(&t.kind, TokenKind::Ident(n) if n == name)
            && matches!(e.kind, TokenKind::Eof))
}

/// A document's node tree with its reactive dependency graph.
pub struct Scene {
    pub(crate) nodes: HashMap<NodeId, Node>,
    pub(crate) roots: Vec<NodeId>,
    pub(crate) next_id: u64,
    analysis: HashMap<NodeId, Analysis>,
    /// name → nodes defining it.
    definers: HashMap<String, BTreeSet<NodeId>>,
    /// name → nodes reading (incl. split letters) or writing it.
    mentions: HashMap<String, BTreeSet<NodeId>>,
    graph: OnceCell<DepGraph>,
    results: HashMap<NodeId, NodeResult>,
    dirty: BTreeSet<NodeId>,
    selection: Vec<NodeId>,
    events: Vec<SceneEvent>,
    history: History,
    builtins: Arc<dyn Builtins>,
    pub(crate) layout: Option<serde_json::Value>,
    pub(crate) doc_extra: BTreeMap<String, String>,
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
            definers: HashMap::new(),
            mentions: HashMap::new(),
            graph: OnceCell::new(),
            results: HashMap::new(),
            dirty: BTreeSet::new(),
            selection: Vec::new(),
            events: Vec::new(),
            history: History::default(),
            builtins: Arc::new(DefaultBuiltins),
            layout: None,
            doc_extra: BTreeMap::new(),
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

    /// Nodes defining `name`, in id order (several = duplicate).
    pub fn definers(&self, name: &str) -> Vec<NodeId> {
        self.definers
            .get(name)
            .map_or_else(Vec::new, |s| s.iter().copied().collect())
    }

    /// The node defining `name` when exactly one does; `None` when the
    /// name is defined several times (see [`Scene::definers`]). Names no
    /// node defines fall back to the first node labelled `name`.
    pub fn lookup(&self, name: &str) -> Option<NodeId> {
        match self.definers.get(name) {
            Some(set) if set.len() == 1 => set.iter().next().copied(),
            Some(set) if !set.is_empty() => None,
            _ => self
                .iter()
                .find(|n| n.name.as_deref() == Some(name))
                .map(|n| n.id),
        }
    }

    /// Is `name` defined by a node other than `except`?
    pub fn is_name_taken(&self, name: &str, except: Option<NodeId>) -> bool {
        self.definers
            .get(name)
            .is_some_and(|s| s.iter().any(|&d| Some(d) != except))
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

    /// The dependency graph (built on first use after a change).
    pub fn graph(&self) -> &DepGraph {
        self.graph.get_or_init(|| self.build_graph())
    }

    /// Names a node defines.
    pub fn defines(&self, id: NodeId) -> &[String] {
        self.analysis.get(&id).map_or(&[], |a| a.defines.as_slice())
    }

    /// Diagnostics of a node (parse errors, cycles, duplicates, unknown
    /// names, unknown write targets).
    pub fn diagnostics(&self, id: NodeId) -> &[Diagnostic] {
        self.graph().diagnostics(id)
    }

    /// Transitive inputs of `id`, topological order.
    pub fn upstream(&self, id: NodeId) -> Vec<NodeId> {
        self.graph().upstream(id)
    }

    /// Transitive dependents of `id`, topological order.
    pub fn downstream(&self, id: NodeId) -> Vec<NodeId> {
        self.graph().downstream(id)
    }

    /// Everything to highlight when hovering `id`: upstream and downstream.
    pub fn related(&self, id: NodeId) -> Vec<NodeId> {
        let mut v = self.upstream(id);
        v.extend(self.downstream(id));
        self.graph().sorted(v.into_iter().collect::<BTreeSet<_>>())
    }

    /// All nodes, dependencies first.
    pub fn topo_order(&self) -> &[NodeId] {
        self.graph().topo_order()
    }

    /// Replace the built-in name oracle (invalidates the graph, marks all
    /// nodes dirty).
    pub fn set_builtins(&mut self, builtins: Arc<dyn Builtins>) {
        self.builtins = builtins;
        self.invalidate(Vec::new());
        self.mark_all_dirty();
    }

    fn build_graph(&self) -> DepGraph {
        let mut ids: Vec<NodeId> = self.nodes.keys().copied().collect();
        ids.sort_unstable();
        let empty = Analysis::default();
        let nodes: Vec<GraphNode<'_>> = ids
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
        DepGraph::build(&nodes, &*self.builtins)
    }

    fn invalidate(&mut self, touched: Vec<NodeId>) {
        self.graph = OnceCell::new();
        self.events.push(SceneEvent::GraphChanged { touched });
    }

    // ------------------------------------------------------------------
    // Indexes
    // ------------------------------------------------------------------

    fn index_add(&mut self, id: NodeId) {
        let Some(a) = self.analysis.get(&id) else {
            return;
        };
        for name in &a.defines {
            self.definers.entry(name.clone()).or_default().insert(id);
        }
        for name in a.dependency_names() {
            self.mentions
                .entry(name.to_string())
                .or_default()
                .insert(id);
        }
    }

    fn index_remove(&mut self, id: NodeId) {
        let Some(a) = self.analysis.get(&id) else {
            return;
        };
        for name in &a.defines {
            if let Some(set) = self.definers.get_mut(name) {
                set.remove(&id);
                if set.is_empty() {
                    self.definers.remove(name);
                }
            }
        }
        for name in a.dependency_names() {
            if let Some(set) = self.mentions.get_mut(name) {
                set.remove(&id);
                if set.is_empty() {
                    self.mentions.remove(name);
                }
            }
        }
    }

    /// Nodes whose evaluation may change when `ids` change: the nodes
    /// themselves, every other definer of a name they define (duplicates),
    /// and everything mentioning those names, transitively.
    fn impact(&self, ids: &[NodeId]) -> BTreeSet<NodeId> {
        let mut seen: BTreeSet<NodeId> = BTreeSet::new();
        let mut queue = VecDeque::new();
        let push = |id: NodeId, seen: &mut BTreeSet<NodeId>, q: &mut VecDeque<NodeId>| {
            if seen.insert(id) {
                q.push_back(id);
            }
        };
        for &id in ids {
            if !self.contains(id) {
                continue;
            }
            push(id, &mut seen, &mut queue);
            for name in self.defines(id) {
                for &d in self.definers.get(name).into_iter().flatten() {
                    push(d, &mut seen, &mut queue);
                }
            }
        }
        while let Some(v) = queue.pop_front() {
            for name in self.defines(v) {
                for &m in self.mentions.get(name).into_iter().flatten() {
                    push(m, &mut seen, &mut queue);
                }
            }
        }
        seen
    }

    // ------------------------------------------------------------------
    // Dirty tracking and evaluation
    // ------------------------------------------------------------------

    pub fn is_dirty(&self, id: NodeId) -> bool {
        self.dirty.contains(&id)
    }

    /// Dirty nodes in topological order (without clearing).
    pub fn dirty(&self) -> Vec<NodeId> {
        self.graph().sorted(self.dirty.iter().copied())
    }

    /// Take the dirty set in topological order (inputs first).
    pub fn take_dirty(&mut self) -> Vec<NodeId> {
        let set = std::mem::take(&mut self.dirty);
        self.graph().sorted(set)
    }

    /// Mark `id` and everything downstream dirty (e.g. re-roll `random`).
    pub fn mark_dirty(&mut self, id: NodeId) {
        for d in self.impact(&[id]) {
            self.set_dirty(d);
        }
    }

    pub fn mark_all_dirty(&mut self) {
        let ids: Vec<NodeId> = self.nodes.keys().copied().collect();
        for id in ids {
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
    /// blocking diagnostics (cycle, duplicate, ambiguous, unknown write
    /// target) or an errored input get an error status without calling the
    /// evaluator. Members of a function-only cycle come in id order; the
    /// evaluator resolves functions lazily at call time. Returns the
    /// processed nodes in order.
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
        if let Some(d) = self.graph().diagnostics(id).iter().find(|d| d.is_error()) {
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
            .graph()
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
            let r = self.apply(&c.inverse());
            debug_assert!(r.is_ok(), "undo failed: {r:?}");
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
            let r = self.apply(c);
            debug_assert!(r.is_ok(), "redo failed: {r:?}");
        }
        self.history.push_undo(t);
        self.events.push(SceneEvent::HistoryChanged);
        true
    }

    fn execute(&mut self, label: &str, cmd: Command) -> Result<()> {
        self.apply(&cmd)?;
        self.history.record(label, cmd);
        self.events.push(SceneEvent::HistoryChanged);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Mutations
    // ------------------------------------------------------------------

    fn alloc_id(&mut self) -> Result<NodeId> {
        let id = NodeId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(SceneError::IdsExhausted)?;
        Ok(id)
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
    /// `new_defines` are the names the content would define; names it does
    /// not define already must be free.
    fn validate(&self, id: Option<NodeId>, c: &NodeContent, new_defines: &[String]) -> Result<()> {
        if c.kind.defines_by_name()
            && let Some(name) = &c.name
            && !is_valid_identifier(name)
        {
            return Err(SceneError::InvalidName(name.clone()));
        }
        if let NodeKind::Function { params, .. } | NodeKind::Regression { params, .. } = &c.kind {
            let mut seen = HashSet::new();
            for p in params {
                if !is_valid_identifier(p) || !seen.insert(p) {
                    return Err(SceneError::InvalidName(p.clone()));
                }
            }
        }
        let old: &[String] = id.map_or(&[], |id| self.defines(id));
        for name in new_defines {
            if !old.contains(name) && self.is_name_taken(name, id) {
                return Err(SceneError::DuplicateName(name.clone()));
            }
        }
        Ok(())
    }

    pub(crate) fn normalize(mut c: NodeContent) -> NodeContent {
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
        self.validate(None, &content, &analyze(&content).defines)?;
        let id = self.alloc_id()?;
        let node = Node::new(id, content);
        self.execute(
            "Add node",
            Command::Insert {
                nodes: vec![node],
                parent,
                index,
            },
        )?;
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
        )
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
        )
    }

    /// Move `id` to `index` among its current siblings.
    pub fn reorder(&mut self, id: NodeId, index: usize) -> Result<()> {
        let parent = self.node(id)?.parent;
        self.move_node(id, parent, index)
    }

    /// Move `id` right before `sibling` (possibly under another parent).
    pub fn move_before(&mut self, id: NodeId, sibling: NodeId) -> Result<()> {
        self.node(id)?;
        let parent = self.node(sibling)?.parent;
        if id == sibling {
            return Ok(());
        }
        let index = self
            .children(parent)
            .iter()
            .filter(|&&c| c != id)
            .position(|&c| c == sibling)
            .unwrap_or(0);
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
        let params_changed = match (&before.kind, &content.kind) {
            (NodeKind::Regression { params: a, .. }, NodeKind::Regression { params: b, .. })
            | (NodeKind::Function { params: a, .. }, NodeKind::Function { params: b, .. }) => {
                a != b
            }
            _ => false,
        };
        let defines_may_change = before.name != content.name
            || before.kind.tag() != content.kind.tag()
            || params_changed
            || matches!(content.kind, NodeKind::Script { .. });
        if defines_may_change {
            self.validate(Some(id), &content, &analyze(&content).defines)?;
        }
        self.execute(
            label,
            Command::Update {
                id,
                before: Box::new(before),
                after: Box::new(content),
                key: key.map(String::from),
            },
        )
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

    /// Rename a defining node and rewrite every occurrence that refers to
    /// it — in dependents, in writers (`a -> …`) and in its own body
    /// (recursion) — as one undo step. Refused with
    /// [`SceneError::NameCapture`] when the new name would be captured by a
    /// local binder or would rebind an existing built-in use.
    pub fn rename_refactor(&mut self, id: NodeId, new_name: &str) -> Result<RenameReport> {
        let node = self.node(id)?;
        if node.defined_name().is_none() {
            self.rename(id, Some(new_name))?;
            return Ok(RenameReport::default());
        }
        let mut c = node.content();
        c.name = Some(new_name.to_string());
        let c = Self::normalize(c);
        self.validate(Some(id), &c, &analyze(&c).defines)?;
        // Existing uses of `new_name` that resolve to a built-in would be
        // rebound to this node.
        for &m in self.mentions.get(new_name).into_iter().flatten() {
            let res = self.graph().resolutions(m);
            let refs = &self.analysis[&m].references;
            for (r, res) in refs.iter().zip(res) {
                let builtin_use = match res {
                    Resolution::Builtin => r.name == new_name,
                    Resolution::Split(letters) => letters
                        .iter()
                        .any(|(l, defs)| l == new_name && defs.is_empty()),
                    _ => false,
                };
                if builtin_use {
                    return Err(SceneError::NameCapture {
                        node: m,
                        name: new_name.to_string(),
                    });
                }
            }
        }
        let g = self.graph();
        let mut targets: BTreeSet<NodeId> = g.direct_downstream(id).into_iter().collect();
        targets.extend(g.written_by(id));
        targets.insert(id);
        let mut plans = Vec::new();
        for t in targets {
            if let Some(plan) = self.plan_rewrite(t, &|occ| pick_node(occ, id, new_name))? {
                plans.push((t, plan));
            }
        }
        let report = RenameReport {
            rewritten: plans.iter().map(|(_, (_, n))| n).sum(),
            nodes: plans.iter().map(|(t, _)| *t).collect(),
        };
        self.transaction("Rename", |s| {
            for (t, (content, _)) in plans {
                s.set_content(t, content, None, "Re-point reference")?;
            }
            s.rename(id, Some(new_name))
        })?;
        Ok(report)
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

    /// Plan a rewrite of the occurrences `pick` selects in node `id`:
    /// returns the new content and the number of rewritten occurrences, or
    /// `None` if nothing matched. Fails without side effects on name
    /// capture or when cached spans do not match the source.
    fn plan_rewrite(
        &self,
        id: NodeId,
        pick: &dyn Fn(Occ<'_>) -> Pick,
    ) -> Result<Option<(NodeContent, usize)>> {
        let node = self.node(id)?;
        let Some(analysis) = self.analysis.get(&id) else {
            return Ok(None);
        };
        let g = self.graph();
        let resolutions = g.resolutions(id);
        let write_res = g.write_resolutions(id);
        let mut picked: Vec<(&Reference, Pick)> = Vec::new();
        for (i, r) in analysis.references.iter().enumerate() {
            picked.push((r, pick(Occ::Read(r, resolutions.get(i)))));
        }
        for (i, r) in analysis.writes.iter().enumerate() {
            let defs = write_res.get(i).map_or(&[][..], Vec::as_slice);
            picked.push((r, pick(Occ::Write(r, defs))));
        }
        for r in &analysis.defined_uses {
            picked.push((r, pick(Occ::Own(r))));
        }
        let mut edits: Vec<(&Reference, String)> = Vec::new();
        for (r, p) in picked {
            let Some((text, introduced)) = p else {
                continue;
            };
            if let Some(name) = introduced.iter().find(|n| r.scope.contains(n)) {
                return Err(SceneError::NameCapture {
                    node: id,
                    name: name.clone(),
                });
            }
            edits.push((r, text));
        }
        if edits.is_empty() {
            return Ok(None);
        }
        let mut c = node.content();
        edits.sort_by_key(|(r, _)| std::cmp::Reverse((r.field, r.span.start)));
        for (r, text) in &edits {
            let slot = c
                .kind
                .source_mut(r.field)
                .ok_or(SceneError::NoSuchField(r.field))?;
            let range = r.span.start..r.span.end;
            if !slot
                .get(range.clone())
                .is_some_and(|t| is_ident_text(t, &r.name))
            {
                return Err(SceneError::SourceMismatch {
                    node: id,
                    field: r.field,
                });
            }
            slot.replace_range(range, text);
        }
        Ok(Some((c, edits.len())))
    }

    /// Re-point references by name: rewrite every free occurrence of `from`
    /// (reads, letter-split reads such as `ab`, write targets) in the node's
    /// sources to `to`, leaving the rest of the text untouched. Returns the
    /// number of rewritten occurrences.
    pub fn repoint(&mut self, id: NodeId, from: &str, to: &str) -> Result<usize> {
        if !is_valid_identifier(to) {
            return Err(SceneError::InvalidName(to.to_string()));
        }
        let pick = |occ: Occ<'_>| -> Pick {
            match occ {
                Occ::Read(r, _) if r.name == from => Some((to.to_string(), vec![to.into()])),
                Occ::Read(r, Some(Resolution::Split(_))) => {
                    spell(r, |l| (l == from).then(|| to.to_string()))
                }
                Occ::Write(r, _) if r.name == from => Some((to.to_string(), vec![to.into()])),
                _ => None,
            }
        };
        self.apply_plan(id, &pick, "Re-point reference")
    }

    /// Patch-cable helper: rewrite exactly the occurrences in `dependent`
    /// that resolve to `from` (whatever name they use) so they refer to the
    /// name `to` defines. Returns the number of rewritten occurrences.
    pub fn repoint_edge(&mut self, dependent: NodeId, from: NodeId, to: NodeId) -> Result<usize> {
        self.node(from)?;
        let new = self
            .defines(to)
            .first()
            .cloned()
            .ok_or(SceneError::NotDefining(to))?;
        self.apply_plan(
            dependent,
            &|occ| pick_node(occ, from, &new),
            "Re-point reference",
        )
    }

    fn apply_plan(
        &mut self,
        id: NodeId,
        pick: &dyn Fn(Occ<'_>) -> Pick,
        label: &str,
    ) -> Result<usize> {
        match self.plan_rewrite(id, pick)? {
            Some((content, n)) => {
                self.set_content(id, content, None, label)?;
                Ok(n)
            }
            None => Ok(0),
        }
    }

    /// Deep-copy a subtree right after the original. Every name defined in
    /// the subtree (node names, regression parameters, script functions)
    /// gets a fresh unique name, and occurrences inside the copy that refer
    /// to the subtree are rewritten, so the copy is self-contained (a copied
    /// recursive `f` calls its copy `f_2`). Returns the new root.
    pub fn duplicate(&mut self, id: NodeId) -> Result<NodeId> {
        let parent = self.node(id)?.parent;
        let index = self.index_in_parent(id).unwrap_or(0) + 1;
        let originals = self.snapshot(id);
        let subtree: HashSet<NodeId> = originals.iter().map(|n| n.id).collect();

        let mut renames: HashMap<String, String> = HashMap::new();
        let mut taken: HashSet<String> = HashSet::new();
        for n in &originals {
            for name in self.defines(n.id) {
                if renames.contains_key(name) {
                    continue;
                }
                let fresh = unique_name(name, |c| self.is_name_taken(c, None) || taken.contains(c));
                taken.insert(fresh.clone());
                renames.insert(name.clone(), fresh);
            }
        }
        let mapped = |name: &str| renames.get(name).cloned();
        let in_subtree = |ds: &[NodeId]| ds.iter().any(|d| subtree.contains(d));
        let split_defs = |res: Option<&Resolution>| -> HashMap<String, Vec<NodeId>> {
            match res {
                Some(Resolution::Split(letters)) => letters.iter().cloned().collect(),
                _ => HashMap::new(),
            }
        };
        let pick = |occ: Occ<'_>| -> Pick {
            let whole = |r: &Reference| mapped(&r.name).map(|n| (n.clone(), vec![n]));
            match occ {
                Occ::Read(r, Some(Resolution::Defined(ds))) if in_subtree(ds) => whole(r),
                Occ::Read(r, res @ Some(Resolution::Split(_))) => {
                    let defs = split_defs(res);
                    spell(r, |l| {
                        defs.get(l)
                            .filter(|ds| in_subtree(ds))
                            .and_then(|_| mapped(l))
                    })
                }
                Occ::Write(r, ds) if in_subtree(ds) => whole(r),
                Occ::Own(r) => whole(r),
                _ => None,
            }
        };

        let mut copies = Vec::with_capacity(originals.len());
        let mut ids = HashMap::new();
        for n in &originals {
            let content = match self.plan_rewrite(n.id, &pick)? {
                Some((c, _)) => c,
                None => n.content(),
            };
            copies.push((n.clone(), content));
        }
        for (n, _) in &copies {
            let new = self.alloc_id()?;
            ids.insert(n.id, new);
        }
        let mut nodes = Vec::with_capacity(copies.len());
        for (orig, mut content) in copies {
            if content.kind.defines_by_name()
                && let Some(name) = &content.name
                && let Some(fresh) = renames.get(name)
            {
                content.name = Some(fresh.clone());
            }
            if let NodeKind::Regression { params, .. } = &mut content.kind {
                for p in params.iter_mut() {
                    if let Some(fresh) = renames.get(p) {
                        *p = fresh.clone();
                    }
                }
            }
            let mut copy = Node::new(ids[&orig.id], content);
            copy.parent = orig.parent.map(|p| ids.get(&p).copied().unwrap_or(p));
            copy.children = orig.children.iter().map(|c| ids[c]).collect();
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
        )?;
        Ok(root)
    }

    // ------------------------------------------------------------------
    // Raw (unrecorded) operations
    // ------------------------------------------------------------------

    pub(crate) fn apply(&mut self, cmd: &Command) -> Result<()> {
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
                Ok(())
            }
            Command::Move { id, to, .. } => {
                self.raw_move(*id, to.0, to.1);
                Ok(())
            }
            Command::Update { id, after, .. } => {
                self.raw_update(*id, (**after).clone());
                Ok(())
            }
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

    fn mark_set_dirty(&mut self, set: BTreeSet<NodeId>) {
        for id in set {
            if self.contains(id) {
                self.set_dirty(id);
            }
        }
    }

    pub(crate) fn raw_insert(
        &mut self,
        nodes: Vec<Node>,
        parent: Option<NodeId>,
        index: usize,
    ) -> Result<()> {
        let Some(root) = nodes.first().map(|n| n.id) else {
            return Ok(());
        };
        let mut seen = HashSet::new();
        for n in &nodes {
            if self.nodes.contains_key(&n.id) || !seen.insert(n.id) {
                return Err(SceneError::IdInUse(n.id));
            }
        }
        let ids: Vec<NodeId> = nodes.iter().map(|n| n.id).collect();
        for mut n in nodes {
            if n.id == root {
                n.parent = parent.filter(|p| self.nodes.contains_key(p));
            }
            self.next_id = self.next_id.max(n.id.0.saturating_add(1));
            self.analysis.insert(n.id, analyze(&n.content()));
            self.nodes.insert(n.id, n);
        }
        for &id in &ids {
            self.index_add(id);
        }
        let list = self.child_list_mut(parent);
        let i = index.min(list.len());
        list.insert(i, root);
        for &id in &ids {
            self.events.push(SceneEvent::NodeAdded(id));
        }
        let dirty = self.impact(&ids);
        self.mark_set_dirty(dirty);
        self.invalidate(ids);
        Ok(())
    }

    pub(crate) fn raw_remove(&mut self, root: NodeId) {
        let Some(node) = self.nodes.get(&root) else {
            return;
        };
        let parent = node.parent;
        let ids: Vec<NodeId> = self.iter_subtree(root).map(|n| n.id).collect();
        let before = self.impact(&ids);
        self.child_list_mut(parent).retain(|&c| c != root);
        for &id in &ids {
            self.index_remove(id);
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
        self.mark_set_dirty(before);
        self.invalidate(ids);
    }

    /// Moves never touch the graph: it does not depend on tree order.
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
        let mut dirty = BTreeSet::new();
        if semantic {
            dirty = self.impact(&[id]);
            self.index_remove(id);
            self.analysis.insert(id, analyze(&content));
            self.index_add(id);
        }
        if let Some(n) = self.nodes.get_mut(&id) {
            n.set_content(content);
        }
        for what in changes {
            self.events.push(SceneEvent::Changed { id, what });
        }
        if semantic {
            dirty.extend(self.impact(&[id]));
            self.mark_set_dirty(dirty);
            self.invalidate(vec![id]);
        }
    }

    /// Install loaded nodes (document load): analysis, indexes, all dirty,
    /// no events or history.
    pub(crate) fn load_nodes(&mut self, nodes: Vec<Node>, roots: Vec<NodeId>, next_id: u64) {
        for n in nodes {
            self.analysis.insert(n.id, analyze(&n.content()));
            self.nodes.insert(n.id, n);
        }
        let ids: Vec<NodeId> = self.nodes.keys().copied().collect();
        for id in ids {
            self.index_add(id);
        }
        self.roots = roots;
        self.next_id = next_id;
        self.graph = OnceCell::new();
        self.mark_all_dirty();
        self.events.clear();
    }
}

/// Select occurrences resolving to node `from` and rename them `to`.
fn pick_node(occ: Occ<'_>, from: NodeId, to: &str) -> Pick {
    let whole = || Some((to.to_string(), vec![to.to_string()]));
    match occ {
        Occ::Read(_, Some(Resolution::Defined(ds))) if ds.contains(&from) => whole(),
        Occ::Read(r, Some(Resolution::Split(letters))) => {
            let hits: Vec<&str> = letters
                .iter()
                .filter(|(_, ds)| ds.contains(&from))
                .map(|(l, _)| l.as_str())
                .collect();
            spell(r, |l| hits.contains(&l).then(|| to.to_string()))
        }
        Occ::Write(_, ds) if ds.contains(&from) => whole(),
        _ => None,
    }
}

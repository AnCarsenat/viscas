//! Dependency graph: name resolution, edges, cycles and topological order.
//!
//! Edges point from a node to the nodes defining the names it references
//! (its *upstream*). The graph is rebuilt from the cached per-node
//! [`Analysis`] after every structural or content change; that is linear in
//! the number of references, which is cheap at document scale.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;

use viscas_core::syntax::Span;

use crate::analysis::Analysis;
use crate::names::Builtins;
use crate::node::{NodeId, SourceField};

/// What a diagnostic is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// A source field failed to parse.
    Parse,
    /// The node takes part in a dependency cycle (listed in order).
    Cycle { members: Vec<NodeId> },
    /// Another node defines the same name.
    DuplicateDefinition { name: String, others: Vec<NodeId> },
    /// A referenced name is defined by several nodes.
    AmbiguousReference { name: String, definers: Vec<NodeId> },
    /// A referenced name is neither defined nor built in.
    UnknownName { name: String },
}

/// A problem found while analysing a node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub message: String,
    pub field: Option<SourceField>,
    pub span: Option<Span>,
}

impl Diagnostic {
    /// Errors block evaluation; unknown names are left to the evaluator
    /// (the scene's built-in list is only advisory).
    pub fn is_error(&self) -> bool {
        !matches!(self.kind, DiagnosticKind::UnknownName { .. })
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// How one [`crate::Reference`] resolved (parallel to
/// [`Analysis::references`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Defined by these nodes (several = ambiguous).
    Defined(Vec<NodeId>),
    Builtin,
    /// Read letter by letter: each unbound letter with its definers
    /// (empty for built-in letters).
    Split(Vec<(String, Vec<NodeId>)>),
    Unknown,
}

/// Input describing one node for [`DepGraph::build`].
pub(crate) struct GraphNode<'a> {
    pub id: NodeId,
    pub analysis: &'a Analysis,
    pub recursive: bool,
    pub title: String,
}

/// The resolved dependency graph of a scene.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DepGraph {
    definers: HashMap<String, Vec<NodeId>>,
    upstream: HashMap<NodeId, BTreeSet<NodeId>>,
    downstream: HashMap<NodeId, BTreeSet<NodeId>>,
    diagnostics: HashMap<NodeId, Vec<Diagnostic>>,
    resolutions: HashMap<NodeId, Vec<Resolution>>,
    topo: Vec<NodeId>,
    topo_index: HashMap<NodeId, usize>,
    cycles: Vec<Vec<NodeId>>,
}

impl DepGraph {
    /// Build from nodes listed in depth-first tree order.
    pub(crate) fn build(nodes: &[GraphNode<'_>], builtins: &dyn Builtins) -> DepGraph {
        let mut g = DepGraph::default();
        let order: HashMap<NodeId, usize> =
            nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
        let titles: HashMap<NodeId, &str> =
            nodes.iter().map(|n| (n.id, n.title.as_str())).collect();

        for n in nodes {
            g.upstream.entry(n.id).or_default();
            g.downstream.entry(n.id).or_default();
            for name in &n.analysis.defines {
                let list = g.definers.entry(name.clone()).or_default();
                if !list.contains(&n.id) {
                    list.push(n.id);
                }
            }
            // Parse errors are reported as diagnostics too.
            for s in &n.analysis.sources {
                for e in &s.errors {
                    g.diag(
                        n.id,
                        DiagnosticKind::Parse,
                        e.message.clone(),
                        Some(s.field),
                        Some(e.span),
                    );
                }
            }
        }

        for n in nodes {
            let mut resolutions = Vec::with_capacity(n.analysis.references.len());
            let mut reported: HashSet<&str> = HashSet::new();
            for r in &n.analysis.references {
                let res = if let Some(defs) = g.definers.get(&r.name).cloned() {
                    if defs.len() > 1 && reported.insert(&r.name) {
                        g.diag(
                            n.id,
                            DiagnosticKind::AmbiguousReference {
                                name: r.name.clone(),
                                definers: defs.clone(),
                            },
                            format!("`{}` is defined {} times", r.name, defs.len()),
                            Some(r.field),
                            Some(r.span),
                        );
                    }
                    Resolution::Defined(defs)
                } else if builtins.is_builtin(&r.name) {
                    Resolution::Builtin
                } else if let Some(letters) = r.split.as_ref().and_then(|letters| {
                    letters
                        .iter()
                        .filter(|l| !l.bound)
                        .map(|l| match g.definers.get(&l.name) {
                            Some(d) => Some((l.name.clone(), d.clone())),
                            None if builtins.is_builtin(&l.name) => {
                                Some((l.name.clone(), Vec::new()))
                            }
                            None => None,
                        })
                        .collect::<Option<Vec<_>>>()
                }) {
                    Resolution::Split(letters)
                } else {
                    if reported.insert(&r.name) {
                        g.diag(
                            n.id,
                            DiagnosticKind::UnknownName {
                                name: r.name.clone(),
                            },
                            format!("unknown name `{}`", r.name),
                            Some(r.field),
                            Some(r.span),
                        );
                    }
                    Resolution::Unknown
                };
                let targets: Vec<NodeId> = match &res {
                    Resolution::Defined(d) => d.clone(),
                    Resolution::Split(letters) => letters
                        .iter()
                        .flat_map(|(_, d)| d.iter().copied())
                        .collect(),
                    Resolution::Builtin | Resolution::Unknown => Vec::new(),
                };
                for t in targets {
                    if t == n.id && n.recursive {
                        continue;
                    }
                    g.upstream.entry(n.id).or_default().insert(t);
                    g.downstream.entry(t).or_default().insert(n.id);
                }
                resolutions.push(res);
            }
            g.resolutions.insert(n.id, resolutions);
        }

        let dupes: Vec<(String, Vec<NodeId>)> = g
            .definers
            .iter()
            .filter(|(_, d)| d.len() > 1)
            .map(|(n, d)| (n.clone(), d.clone()))
            .collect();
        for (name, defs) in dupes {
            for &id in &defs {
                let others: Vec<NodeId> = defs.iter().copied().filter(|&o| o != id).collect();
                g.diag(
                    id,
                    DiagnosticKind::DuplicateDefinition {
                        name: name.clone(),
                        others,
                    },
                    format!("`{name}` is already defined elsewhere"),
                    None,
                    None,
                );
            }
        }

        let recursive: HashMap<NodeId, bool> = nodes.iter().map(|n| (n.id, n.recursive)).collect();
        for mut scc in g.tarjan(nodes) {
            scc.sort_by_key(|id| order[id]);
            let is_cycle = scc.len() > 1 || g.upstream[&scc[0]].contains(&scc[0]);
            if is_cycle && !scc.iter().all(|id| recursive[id]) {
                let mut names: Vec<&str> = scc.iter().map(|id| titles[id]).collect();
                names.push(names[0]);
                let message = format!("circular dependency: {}", names.join(" → "));
                for &id in &scc {
                    g.diag(
                        id,
                        DiagnosticKind::Cycle {
                            members: scc.clone(),
                        },
                        message.clone(),
                        None,
                        None,
                    );
                }
                g.cycles.push(scc.clone());
            }
            g.topo.extend(scc);
        }
        g.topo_index = g.topo.iter().enumerate().map(|(i, &id)| (id, i)).collect();
        for list in g.diagnostics.values_mut() {
            list.sort_by_key(|d| (d.field, d.span.map(|s| (s.start, s.end))));
        }
        g
    }

    fn diag(
        &mut self,
        id: NodeId,
        kind: DiagnosticKind,
        message: String,
        field: Option<SourceField>,
        span: Option<Span>,
    ) {
        self.diagnostics.entry(id).or_default().push(Diagnostic {
            kind,
            message,
            field,
            span,
        });
    }

    /// Iterative Tarjan SCC over upstream edges. SCCs come out
    /// dependencies-first, which is the evaluation order.
    fn tarjan(&self, nodes: &[GraphNode<'_>]) -> Vec<Vec<NodeId>> {
        let mut index: HashMap<NodeId, usize> = HashMap::new();
        let mut low: HashMap<NodeId, usize> = HashMap::new();
        let mut on_stack: HashSet<NodeId> = HashSet::new();
        let mut stack: Vec<NodeId> = Vec::new();
        let mut out = Vec::new();
        let mut counter = 0usize;
        let empty = BTreeSet::new();

        for root in nodes.iter().map(|n| n.id) {
            if index.contains_key(&root) {
                continue;
            }
            let mut call: Vec<(NodeId, Vec<NodeId>, usize)> = Vec::new();
            let mut visit = |v: NodeId,
                             call: &mut Vec<(NodeId, Vec<NodeId>, usize)>,
                             stack: &mut Vec<NodeId>,
                             on_stack: &mut HashSet<NodeId>,
                             index: &mut HashMap<NodeId, usize>,
                             low: &mut HashMap<NodeId, usize>| {
                index.insert(v, counter);
                low.insert(v, counter);
                counter += 1;
                stack.push(v);
                on_stack.insert(v);
                let next: Vec<NodeId> = self
                    .upstream
                    .get(&v)
                    .unwrap_or(&empty)
                    .iter()
                    .copied()
                    .collect();
                call.push((v, next, 0));
            };
            visit(
                root,
                &mut call,
                &mut stack,
                &mut on_stack,
                &mut index,
                &mut low,
            );
            while let Some((v, next, pos)) = call.last_mut() {
                let v = *v;
                if *pos < next.len() {
                    let w = next[*pos];
                    *pos += 1;
                    if !index.contains_key(&w) {
                        visit(
                            w,
                            &mut call,
                            &mut stack,
                            &mut on_stack,
                            &mut index,
                            &mut low,
                        );
                    } else if on_stack.contains(&w) {
                        let lw = index[&w];
                        let lv = low.get_mut(&v).expect("visited");
                        *lv = (*lv).min(lw);
                    }
                    continue;
                }
                call.pop();
                if low[&v] == index[&v] {
                    let mut scc = Vec::new();
                    while let Some(w) = stack.pop() {
                        on_stack.remove(&w);
                        scc.push(w);
                        if w == v {
                            break;
                        }
                    }
                    out.push(scc);
                }
                if let Some((p, _, _)) = call.last() {
                    let lv = low[&v];
                    let lp = low.get_mut(p).expect("visited");
                    *lp = (*lp).min(lv);
                }
            }
        }
        out
    }

    /// Nodes defining `name`, in tree order.
    pub fn definers(&self, name: &str) -> &[NodeId] {
        self.definers.get(name).map_or(&[], Vec::as_slice)
    }

    /// Every defined name.
    pub fn defined_names(&self) -> impl Iterator<Item = &str> {
        self.definers.keys().map(String::as_str)
    }

    /// Direct inputs of `id`.
    pub fn direct_upstream(&self, id: NodeId) -> Vec<NodeId> {
        self.upstream
            .get(&id)
            .map_or_else(Vec::new, |s| self.sorted(s.iter().copied()))
    }

    /// Direct dependents of `id`.
    pub fn direct_downstream(&self, id: NodeId) -> Vec<NodeId> {
        self.downstream
            .get(&id)
            .map_or_else(Vec::new, |s| self.sorted(s.iter().copied()))
    }

    /// All transitive inputs of `id` (excluding `id`), in topological order.
    pub fn upstream(&self, id: NodeId) -> Vec<NodeId> {
        self.reach(id, &self.upstream)
    }

    /// All transitive dependents of `id` (excluding `id`), in topological
    /// order — what the UI highlights on hover.
    pub fn downstream(&self, id: NodeId) -> Vec<NodeId> {
        self.reach(id, &self.downstream)
    }

    fn reach(&self, id: NodeId, edges: &HashMap<NodeId, BTreeSet<NodeId>>) -> Vec<NodeId> {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([id]);
        while let Some(v) = queue.pop_front() {
            if let Some(next) = edges.get(&v) {
                for &w in next {
                    if seen.insert(w) {
                        queue.push_back(w);
                    }
                }
            }
        }
        seen.remove(&id);
        self.sorted(seen)
    }

    /// Sort ids by topological position.
    pub fn sorted(&self, ids: impl IntoIterator<Item = NodeId>) -> Vec<NodeId> {
        let mut v: Vec<NodeId> = ids.into_iter().collect();
        v.sort_by_key(|id| self.topo_index.get(id).copied().unwrap_or(usize::MAX));
        v
    }

    /// Every node, dependencies first (cycle members adjacent).
    pub fn topo_order(&self) -> &[NodeId] {
        &self.topo
    }

    /// Diagnostics of a node (parse, cycle, duplicate, unknown names).
    pub fn diagnostics(&self, id: NodeId) -> &[Diagnostic] {
        self.diagnostics.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Nodes with at least one diagnostic.
    pub fn nodes_with_diagnostics(&self) -> Vec<NodeId> {
        self.sorted(self.diagnostics.keys().copied())
    }

    /// Every illegal cycle, members in tree order.
    pub fn cycles(&self) -> &[Vec<NodeId>] {
        &self.cycles
    }

    /// Is `id` part of an illegal cycle?
    pub fn in_cycle(&self, id: NodeId) -> bool {
        self.cycles.iter().any(|c| c.contains(&id))
    }

    /// Resolution of each reference of `id`.
    pub fn resolutions(&self, id: NodeId) -> &[Resolution] {
        self.resolutions.get(&id).map_or(&[], Vec::as_slice)
    }

    /// All edges `(dependent, input)`.
    pub fn edges(&self) -> Vec<(NodeId, NodeId)> {
        let mut out = Vec::new();
        for &id in &self.topo {
            for &u in &self.upstream[&id] {
                out.push((id, u));
            }
        }
        out
    }
}

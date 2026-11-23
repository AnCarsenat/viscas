//! Unit tests for the scene crate.

mod analysis;
mod document;
mod eval;
mod graph;
mod history;
mod names;
mod repoint;
mod review;
mod tree;

use crate::*;

/// Add a top-level variable.
pub(crate) fn var(s: &mut Scene, name: &str, expr: &str) -> NodeId {
    s.add(None, NodeContent::variable(name, expr)).unwrap()
}

/// Add a top-level function.
pub(crate) fn func(s: &mut Scene, name: &str, params: &[&str], body: &str) -> NodeId {
    s.add(None, NodeContent::function(name, params, body))
        .unwrap()
}

/// Add a top-level plot.
pub(crate) fn plot(s: &mut Scene, relation: &str) -> NodeId {
    s.add(None, NodeContent::plot(relation)).unwrap()
}

/// Distinct free names referenced by a content, sorted.
pub(crate) fn refs(c: &NodeContent) -> Vec<String> {
    analyze(c)
        .referenced_names()
        .into_iter()
        .map(String::from)
        .collect()
}

/// Titles of nodes.
pub(crate) fn titles(s: &Scene, ids: &[NodeId]) -> Vec<String> {
    ids.iter().map(|id| s.get(*id).unwrap().title()).collect()
}

/// Expression text of a variable-like node's main field.
pub(crate) fn main_src(s: &Scene, id: NodeId) -> String {
    s.get(id)
        .unwrap()
        .kind
        .source(SourceField::Main)
        .unwrap()
        .to_string()
}

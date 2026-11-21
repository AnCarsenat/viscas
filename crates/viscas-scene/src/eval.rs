//! Boundary to the evaluator.
//!
//! The scene does not know how to compute values. An evaluator (the core
//! crate's, later) implements [`NodeEvaluator`]; [`crate::Scene::evaluate_dirty`]
//! feeds it the dirty nodes in dependency order and stores the returned
//! [`NodeResult`]. Values themselves stay in the evaluator's own cache,
//! keyed by [`NodeId`]; the scene keeps only status and a display string,
//! so it stays independent of the value type.

use viscas_core::syntax::Span;

use crate::node::{NodeId, SourceField};
use crate::scene::Scene;

/// Evaluation status of a node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Not evaluated since the last change.
    #[default]
    Pending,
    Ok,
    Error {
        message: String,
        field: Option<SourceField>,
        span: Option<Span>,
    },
}

impl Status {
    pub fn error(message: impl Into<String>) -> Status {
        Status::Error {
            message: message.into(),
            field: None,
            span: None,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Status::Error { .. })
    }
}

/// What an evaluation produced, as far as the scene is concerned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeResult {
    pub status: Status,
    /// Short rendering for the UI (`= 1/3`), if any.
    pub display: Option<String>,
}

impl NodeResult {
    pub fn ok(display: impl Into<String>) -> NodeResult {
        NodeResult {
            status: Status::Ok,
            display: Some(display.into()),
        }
    }

    pub fn error(message: impl Into<String>) -> NodeResult {
        NodeResult {
            status: Status::error(message),
            display: None,
        }
    }
}

/// Implemented by the evaluator. Called once per dirty node, inputs first;
/// nodes with parse errors, illegal cycles or failed inputs are not passed
/// to it. The parsed ASTs are available through [`Scene::analysis`].
pub trait NodeEvaluator {
    fn evaluate(&mut self, scene: &Scene, node: NodeId) -> NodeResult;
}

impl<F: FnMut(&Scene, NodeId) -> NodeResult> NodeEvaluator for F {
    fn evaluate(&mut self, scene: &Scene, node: NodeId) -> NodeResult {
        self(scene, node)
    }
}

//! Unit tests for the engine crate.

mod analysis;
mod complete;
mod console;
mod document;
mod eval;
mod plot;

use viscas_plot::{Primitive, Rect, View};
use viscas_scene::{NodeContent, NodeId, SourceField, Status};

use crate::*;

pub(crate) fn engine() -> Engine {
    Engine::with_workers(2)
}

pub(crate) fn view() -> View {
    View::new(Rect::new(-10.0, -10.0, 10.0, 10.0), 400.0, 400.0)
}

pub(crate) fn var(e: &mut Engine, name: &str, expr: &str) -> NodeId {
    e.add_node(None, NodeContent::variable(name, expr)).unwrap()
}

pub(crate) fn func(e: &mut Engine, name: &str, params: &[&str], body: &str) -> NodeId {
    e.add_node(None, NodeContent::function(name, params, body))
        .unwrap()
}

pub(crate) fn plot(e: &mut Engine, relation: &str) -> NodeId {
    e.add_node(None, NodeContent::plot(relation)).unwrap()
}

pub(crate) fn set(e: &mut Engine, id: NodeId, text: &str) {
    e.scene_mut()
        .set_source(id, SourceField::Main, text)
        .unwrap();
    e.update();
}

/// Exact text of a node's value.
pub(crate) fn exact(e: &Engine, id: NodeId) -> String {
    e.value(id)
        .map(|v| v.exact.clone())
        .unwrap_or_else(|| panic!("no value for {id}: {:?}", e.scene().status(id)))
}

/// Error message, field and span of a node.
pub(crate) fn error(
    e: &Engine,
    id: NodeId,
) -> (
    String,
    Option<SourceField>,
    Option<viscas_core::syntax::Span>,
) {
    match e.scene().status(id) {
        Status::Error {
            message,
            field,
            span,
        } => (message.clone(), *field, *span),
        other => panic!("expected an error for {id}, got {other:?}"),
    }
}

/// Render the node synchronously in viewport 1 and return its layer.
pub(crate) fn layer(e: &mut Engine, id: NodeId) -> PlotLayer {
    let vp = ViewportId(1);
    let job = e.prepare_job(vp, view());
    assert!(e.run_job_now(&job));
    e.layers(vp)
        .into_iter()
        .find(|l| l.node_id == id)
        .unwrap_or_else(|| panic!("no layer for {id}: {:?}", e.scene().status(id)))
}

pub(crate) fn polylines(l: &PlotLayer) -> usize {
    l.primitives
        .iter()
        .filter(|p| matches!(p, Primitive::Polyline { .. }))
        .count()
}

pub(crate) fn polygons(l: &PlotLayer) -> usize {
    l.primitives
        .iter()
        .filter(|p| matches!(p, Primitive::Polygon(_)))
        .count()
}

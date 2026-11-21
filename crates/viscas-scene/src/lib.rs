//! Node tree and reactive dependency graph for Viscas (GTK-free).
//!
//! - [`Scene`]: a Godot-style tree of [`Node`]s with stable [`NodeId`]s,
//!   ordered children, selection, change events ([`SceneEvent`]) and
//!   undo/redo of every mutation (grouping and coalescing of repeated
//!   edits such as slider drags).
//! - [`analysis`]: each node's sources are parsed with
//!   [`viscas_core::syntax`]; the AST, parse errors, defined names and free
//!   identifier references (respecting bound variables) are cached.
//! - [`graph`]: references resolve to defining nodes, giving a dependency
//!   DAG with cycle / duplicate / unknown-name diagnostics, transitive
//!   upstream/downstream queries, a topological order and dirty
//!   propagation consumed by an evaluator through [`NodeEvaluator`].
//! - [`document`]: the versioned, human-readable `.viscas` format (RON).

pub mod analysis;
pub mod document;
pub mod eval;
pub mod events;
pub mod graph;
mod history;
pub mod names;
pub mod node;
pub mod scene;
pub use analysis::{Analysis, Letter, Parsed, ParsedSource, Reference, analyze};
pub use document::{
    DocNode, Document, DocumentError, FORMAT_VERSION, load_document, migrate, save_document,
};
pub use eval::{NodeEvaluator, NodeResult, Status};
pub use events::{Change, SceneEvent};
pub use graph::{DepGraph, Diagnostic, DiagnosticKind, Resolution};
pub use names::{
    Builtins, DefaultBuiltins, definition_line, is_valid_identifier, parse_definition, unique_name,
};
pub use node::{
    Animation, AnimationMode, Bounds, Color, Column, GridKind, KindTag, LineStyle, Node,
    NodeContent, NodeId, NodeKind, ParseMode, Projection, PropValue, ScriptLanguage, Source,
    SourceField, Style,
};
pub use scene::{DepthFirst, Scene, SceneError};

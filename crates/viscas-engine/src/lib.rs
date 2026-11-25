//! Scene–evaluator–plot bridge for Viscas (GTK-free).
//!
//! [`Engine`] owns the [`viscas_scene::Scene`] (with its undo history) and
//! wires it to the `viscas_core` evaluator and the `viscas_plot` samplers:
//!
//! - **Evaluation** ([`SceneEvaluator`], a [`viscas_scene::NodeEvaluator`]):
//!   dirty nodes are evaluated inputs-first; node values are offered to the
//!   core evaluator through a `Resolver`, so expressions reference other
//!   nodes by name. Each node keeps a [`NodeValue`] (value + exact and
//!   decimal text) and errors carry the source field and span.
//!   [`EngineSettings`] (exact/approx, precision, angle unit, real/complex,
//!   definition operator) apply everywhere; changing them re-evaluates.
//! - **Console** ([`Engine::console_eval`]): sees the scene's definitions,
//!   keeps its own session definitions and `ans`/`ans2`, supports
//!   `:help`, `:mode`, `:pin` (undoable), `:clear`, `:vars`, input history
//!   and [`Engine::complete`].
//! - **Plotting**: plot-like nodes compile to `Send + Sync` closures
//!   ([`PlotShape`]); [`Engine::request_plots`] snapshots them with a
//!   [`viscas_plot::View`] into a [`PlotJob`] rendered on a std-thread
//!   worker pool. A newer job for the same viewport cancels the older one;
//!   [`Engine::poll_plots`] drains results (non-blocking `try_recv`) into a
//!   per-(node, viewport) cache and [`Engine::layers`] returns the
//!   [`PlotLayer`]s in draw order.
//! - **Analysis**: [`Engine::features_near`] finds roots, extrema and
//!   intersections of explicit curves near a clicked point.
//! - **Documents**: new/open/save `.viscas` with the app's layout blob.
//!
//! ```
//! use viscas_engine::Engine;
//! use viscas_scene::NodeContent;
//!
//! let mut engine = Engine::with_workers(1);
//! let a = engine.add_node(None, NodeContent::variable("a", "2")).unwrap();
//! engine.add_node(None, NodeContent::variable("b", "a / 3")).unwrap();
//! assert_eq!(engine.console_eval("b + a").results[0].exact, "8/3");
//! engine.scene_mut().set_source(a, viscas_scene::SourceField::Main, "5").unwrap();
//! engine.update();
//! assert_eq!(engine.console_eval("b").results[0].exact, "5/3");
//! ```

mod complete;
mod console;
mod engine;
mod eval;
pub mod help;
pub mod lines;
mod plot;
mod settings;
mod worker;

#[cfg(test)]
mod tests;

pub use complete::{Completion, CompletionKind, completion_start, suggest};
pub use console::{AutoAdd, ConsoleError, ConsoleItem, ConsoleOutput, HISTORY_LIMIT, ItemKind};
pub use engine::{
    DEFAULT_PALETTE, Engine, EngineError, FeatureHit, FeaturesResult, MAX_FEATURE_CURVES,
    builtins_oracle,
};
pub use eval::{NodeValue, SceneEvaluator};
pub use plot::{
    CompiledPlot, DEFAULT_RANGE, Fn1, Fn2, PlotItem, PlotJob, PlotKind, PlotLayer, PlotShape,
    PlotStyle, Rebuild, ViewportId, render,
};
pub use settings::EngineSettings;
pub use worker::{PlotPool, PlotResult};

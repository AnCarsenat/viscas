//! Scene evaluation: the [`NodeEvaluator`] over `viscas_core::eval`.
//!
//! Node values live in a name → value map read by the core evaluator
//! through a [`Resolver`]. During an evaluation pass the map is mutated in
//! place (behind a lock; values never capture the environment). Plot nodes
//! are compiled *after* the pass against one immutable snapshot of the map
//! ([`SceneEvaluator::finish_pass`]), so compiled closures — which may run
//! on worker threads — always see a consistent scene, and the snapshot is
//! cloned at most once per pass.
//!
//! Nodes are evaluated in topological order by
//! [`viscas_scene::Scene::evaluate_dirty`]; the engine forgets the values of
//! every dirty node first, so a node with an error never leaks a stale
//! value to its dependents.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, PoisonError, RwLock};

use viscas_core::eval::{
    AngleUnit, Env, EvalError, EvalOptions, FnBody, Function, Resolver, StatementKind, UserFn,
    Value, canonical_name, classify, compile_plot_fn, eval_expr, format_value, free_vars,
    run_program,
};
use viscas_core::syntax::{
    AddOp, CmpOp, DefinitionOp, Expr, ExprKind, Span, Stmt, StmtKind, Term, parse_statement_with,
};
use viscas_plot::{Comparison, Vec2};
use viscas_scene::{
    NodeEvaluator, NodeId, NodeKind, NodeResult, Parsed, PropValue, Scene, ScriptLanguage,
    SourceField, Status,
};

use crate::lines::line_parse_options;
use crate::plot::{CompiledPlot, DEFAULT_RANGE, Fn1, Fn2, PlotKind, PlotShape, Rebuild};
use crate::settings::EngineSettings;

/// Per-sample step budget of plot closures that fall back to the general
/// evaluator (keeps one sample short; the job flag interrupts it anyway).
const PLOT_MAX_STEPS: u64 = 1_000_000;

/// The value of a node with its exact and decimal renderings.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeValue {
    pub value: Value,
    /// Exact-style text (`1/3`, `≈1.41421356237`).
    pub exact: String,
    /// Decimal text (`0.333333333333`).
    pub approx: String,
}

impl NodeValue {
    pub(crate) fn new(value: Value, settings: &EngineSettings) -> NodeValue {
        NodeValue {
            exact: format_value(&value, &settings.exact_format()),
            approx: format_value(&value, &settings.approx_format()),
            value,
        }
    }

    /// The primary rendering for the given mode.
    pub fn display(&self, approx: bool) -> &str {
        if approx { &self.approx } else { &self.exact }
    }
}

/// Immutable name → value snapshot of the scene definitions.
pub(crate) type Defs = Arc<HashMap<String, Value>>;
type Live = Arc<RwLock<HashMap<String, Value>>>;

struct SnapshotResolver(Defs);

impl Resolver for SnapshotResolver {
    fn resolve(&self, name: &str) -> Option<Value> {
        self.0.get(canonical_name(name)).cloned()
    }
}

struct LiveResolver(Live);

impl Resolver for LiveResolver {
    fn resolve(&self, name: &str) -> Option<Value> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(canonical_name(name))
            .cloned()
    }
}

/// An environment whose resolver reads `defs`.
pub(crate) fn env_with(defs: &Defs) -> Env {
    let mut env = Env::new();
    env.set_resolver(Some(Arc::new(SnapshotResolver(defs.clone()))));
    env
}

/// A failed node evaluation.
struct Fail {
    message: String,
    field: SourceField,
    span: Option<Span>,
}

impl Fail {
    fn new(message: impl Into<String>) -> Fail {
        Fail {
            message: message.into(),
            field: SourceField::Main,
            span: None,
        }
    }

    fn eval(e: EvalError, field: SourceField) -> Fail {
        Fail {
            message: e.message,
            field,
            span: Some(e.span),
        }
    }
}

#[derive(Default)]
struct Done {
    /// Value defined under the node's name / shown in the UI.
    value: Option<Value>,
    plot: Option<CompiledPlot>,
    /// Extra names defined by the node (script `fn`s).
    extra: Vec<(String, Value)>,
}

/// Everything needed to (re)compile a plot relation's closures.
#[derive(Clone)]
struct PlotSpec {
    stmt: Stmt,
    t_range: (f64, f64),
    theta_range: (f64, f64),
    degrees: bool,
}

/// Evaluates scene nodes and keeps their values, definitions and compiled
/// plots. Owned by [`crate::Engine`].
pub struct SceneEvaluator {
    settings: EngineSettings,
    interrupt: Arc<AtomicBool>,
    live: Live,
    frozen: Option<Defs>,
    names: HashMap<NodeId, Vec<String>>,
    values: HashMap<NodeId, NodeValue>,
    plots: HashMap<NodeId, Arc<CompiledPlot>>,
    revisions: HashMap<NodeId, u64>,
    next_revision: u64,
    /// Plot nodes evaluated in the current pass, compiled by
    /// [`SceneEvaluator::finish_pass`].
    pending_plots: Vec<NodeId>,
}

impl SceneEvaluator {
    pub fn new(settings: EngineSettings, interrupt: Arc<AtomicBool>) -> SceneEvaluator {
        SceneEvaluator {
            settings,
            interrupt,
            live: Arc::new(RwLock::new(HashMap::new())),
            frozen: None,
            names: HashMap::new(),
            values: HashMap::new(),
            plots: HashMap::new(),
            revisions: HashMap::new(),
            next_revision: 1,
            pending_plots: Vec::new(),
        }
    }

    pub fn settings(&self) -> &EngineSettings {
        &self.settings
    }

    pub(crate) fn set_settings(&mut self, settings: EngineSettings) {
        self.settings = settings;
    }

    /// Options for scene and console evaluation (engine interrupt flag).
    pub(crate) fn eval_options(&self) -> EvalOptions {
        self.settings.eval_options(Some(self.interrupt.clone()))
    }

    /// Options baked into plot closures: no engine-wide flag (jobs bind
    /// their own, see [`CompiledPlot::for_job`]) and a per-sample budget.
    fn plot_options(&self) -> EvalOptions {
        EvalOptions {
            max_steps: PLOT_MAX_STEPS,
            ..self.settings.eval_options(None)
        }
    }

    /// Immutable snapshot of the scene definitions (cloned only when they
    /// changed since the last snapshot).
    pub(crate) fn snapshot(&mut self) -> Defs {
        if let Some(d) = &self.frozen {
            return d.clone();
        }
        let map = self
            .live
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let d = Arc::new(map);
        self.frozen = Some(d.clone());
        d
    }

    fn live_env(&self) -> Env {
        let mut env = Env::new();
        env.set_resolver(Some(Arc::new(LiveResolver(self.live.clone()))));
        env
    }

    pub fn value(&self, id: NodeId) -> Option<&NodeValue> {
        self.values.get(&id)
    }

    pub fn plot(&self, id: NodeId) -> Option<&Arc<CompiledPlot>> {
        self.plots.get(&id)
    }

    /// Bumped every time the node is (re-)evaluated or forgotten.
    pub fn revision(&self, id: NodeId) -> u64 {
        self.revisions.get(&id).copied().unwrap_or(0)
    }

    /// Value of a scene definition by name.
    pub fn lookup(&self, name: &str) -> Option<Value> {
        self.live
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(canonical_name(name))
            .cloned()
    }

    /// Is `name` defined by a scene node?
    pub fn defines(&self, name: &str) -> bool {
        self.live
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(canonical_name(name))
    }

    fn write(&mut self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Value>> {
        self.frozen = None;
        self.live.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Drops everything known about `id` (its defined names included).
    pub(crate) fn forget(&mut self, id: NodeId) {
        if let Some(names) = self.names.remove(&id) {
            let mut defs = self.write();
            for n in names {
                defs.remove(&n);
            }
        }
        self.values.remove(&id);
        self.plots.remove(&id);
        self.bump(id);
    }

    /// [`SceneEvaluator::forget`] for several nodes.
    pub(crate) fn forget_many(&mut self, ids: &[NodeId]) {
        for &id in ids {
            self.forget(id);
        }
    }

    /// Forget every node no longer in the scene.
    pub(crate) fn prune(&mut self, scene: &Scene) {
        let mut gone: Vec<NodeId> = self
            .names
            .keys()
            .chain(self.values.keys())
            .chain(self.plots.keys())
            .copied()
            .filter(|id| !scene.contains(*id))
            .collect();
        gone.sort_unstable();
        gone.dedup();
        for id in gone {
            self.forget(id);
            self.revisions.remove(&id);
        }
    }

    /// Forget everything (new document, settings change).
    pub(crate) fn reset(&mut self) {
        self.write().clear();
        self.names.clear();
        self.values.clear();
        self.plots.clear();
        self.pending_plots.clear();
        let ids: Vec<NodeId> = self.revisions.keys().copied().collect();
        for id in ids {
            self.bump(id);
        }
    }

    fn bump(&mut self, id: NodeId) {
        self.revisions.insert(id, self.next_revision);
        self.next_revision += 1;
    }

    fn define(&mut self, id: NodeId, name: &str, value: Value) {
        let name = canonical_name(name).to_string();
        self.write().insert(name.clone(), value);
        self.names.entry(id).or_default().push(name);
    }

    fn run(&self, scene: &Scene, id: NodeId) -> Result<Done, Fail> {
        let Some(node) = scene.get(id) else {
            return Err(Fail::new("no such node"));
        };
        let env = self.live_env();
        let opts = self.eval_options();
        let ast = |field: SourceField| scene.analysis(id).and_then(|a| a.ast(field));
        let expr = |field: SourceField| match ast(field) {
            Some(Parsed::Expr(e)) => Some(e),
            _ => None,
        };
        let eval_field = |field: SourceField| -> Result<Option<Value>, Fail> {
            match expr(field) {
                Some(e) => eval_expr(e, &env, &opts)
                    .map(Some)
                    .map_err(|err| Fail::eval(err, field)),
                None => Ok(None),
            }
        };
        let points_plot = |points| {
            CompiledPlot::new(
                PlotKind::Points,
                PlotShape::Points {
                    points,
                    connect: false,
                },
            )
        };
        match &node.kind {
            NodeKind::Folder | NodeKind::Note { .. } | NodeKind::Viewport { .. } => {
                Ok(Done::default())
            }
            NodeKind::Variable { .. } => Ok(Done {
                value: eval_field(SourceField::Main)?,
                ..Done::default()
            }),
            NodeKind::List { .. } => {
                let value = eval_field(SourceField::Main)?;
                let plot = value.as_ref().and_then(points_of).map(points_plot);
                Ok(Done {
                    value,
                    plot,
                    ..Done::default()
                })
            }
            NodeKind::Point { .. } => {
                let value = eval_field(SourceField::Main)?;
                let plot = match &value {
                    None => None,
                    Some(v) => match points_of(v) {
                        Some(points) => Some(points_plot(points)),
                        None => {
                            return Err(Fail::new(format!(
                                "a point needs two real coordinates, got a {}",
                                v.type_name()
                            )));
                        }
                    },
                };
                Ok(Done {
                    value,
                    plot,
                    ..Done::default()
                })
            }
            NodeKind::Slider { .. } => {
                let value = eval_field(SourceField::Main)?;
                for field in [SourceField::Min, SourceField::Max, SourceField::Step] {
                    if let Some(v) = eval_field(field)?
                        && v.as_number().is_none_or(|n| !n.is_real())
                    {
                        return Err(Fail {
                            message: format!("expected a real number, got a {}", v.type_name()),
                            field,
                            span: expr(field).map(|e| e.span),
                        });
                    }
                }
                Ok(Done {
                    value,
                    ..Done::default()
                })
            }
            NodeKind::Function { params, .. } => {
                let Some(body) = expr(SourceField::Main) else {
                    return Ok(Done::default());
                };
                let f = UserFn {
                    name: node.name.clone(),
                    params: params
                        .iter()
                        .map(|p| canonical_name(p).to_string())
                        .collect(),
                    body: FnBody::Expr(body.clone()),
                    captured: Arc::new(Vec::new()),
                };
                Ok(Done {
                    value: Some(Value::Function(Arc::new(Function::User(f)))),
                    ..Done::default()
                })
            }
            // Compiled after the pass against one snapshot.
            NodeKind::Plot { .. } => Ok(Done::default()),
            NodeKind::Script { language, .. } => match (language, ast(SourceField::Main)) {
                (ScriptLanguage::Rhai, _) => Err(Fail::new("Rhai scripts are not supported yet")),
                (_, Some(Parsed::Program(p))) => {
                    let mut env = env.clone();
                    let v = run_program(p, &mut env, &opts)
                        .map_err(|e| Fail::eval(e, SourceField::Main))?;
                    let extra = scene
                        .defines(id)
                        .iter()
                        .filter_map(|n| env.get(n).map(|v| (n.clone(), v.clone())))
                        .collect();
                    Ok(Done {
                        value: (!v.is_undefined()).then_some(v),
                        extra,
                        ..Done::default()
                    })
                }
                _ => Ok(Done::default()),
            },
            NodeKind::Table { .. } | NodeKind::Action { .. } => Ok(Done::default()),
            NodeKind::Regression { .. } => Err(Fail::new("regression is not implemented yet")),
        }
    }

    /// Compile the plot nodes evaluated in this pass against one snapshot
    /// of the definitions and store their results in the scene.
    pub(crate) fn finish_pass(&mut self, scene: &mut Scene) {
        let pending = std::mem::take(&mut self.pending_plots);
        if pending.is_empty() {
            return;
        }
        let defs = self.snapshot();
        for id in pending {
            let result = match self.plot_node(scene, id, &defs) {
                Ok(done) => {
                    if let Some(p) = done.plot {
                        self.plots.insert(id, Arc::new(p));
                    }
                    self.store_value(id, done.value)
                }
                Err(f) => error_result(scene, id, f),
            };
            scene.set_result(id, result);
        }
    }

    /// Classify and compile a plot node's relation.
    fn plot_node(&self, scene: &Scene, id: NodeId, defs: &Defs) -> Result<Done, Fail> {
        let Some(node) = scene.get(id) else {
            return Ok(Done::default());
        };
        let NodeKind::Plot { relation } = &node.kind else {
            return Ok(Done::default());
        };
        // Same reading as console lines and `add_line` (matters for `=`
        // definitions under `DefinitionOp::Eq`).
        let reparsed;
        let stmt = if self.settings.definition_op == DefinitionOp::ColonEq {
            match scene.analysis(id).and_then(|a| a.ast(SourceField::Main)) {
                Some(Parsed::Stmt(s)) => s,
                _ => return Ok(Done::default()),
            }
        } else {
            let opts = line_parse_options(relation, &self.settings);
            match parse_statement_with(relation, &opts) {
                Ok(s) => {
                    reparsed = s;
                    &reparsed
                }
                Err(errs) => {
                    let e = &errs[0];
                    return Err(Fail {
                        message: e.message.clone(),
                        field: SourceField::Main,
                        span: Some(e.span),
                    });
                }
            }
        };
        let env = env_with(defs);
        let opts = self.plot_options();
        let info = classify(stmt, &env);
        let Some(expr) = &info.expr else {
            return Err(Fail::new("not a plot relation"));
        };
        let fail = |e: EvalError| Fail::eval(e, SourceField::Main);
        let degrees = self.settings.angle == AngleUnit::Degrees;
        match &info.kind {
            StatementKind::Point | StatementKind::Expression => {
                let v =
                    eval_expr(&restrict(expr, &info.restrictions), &env, &opts).map_err(fail)?;
                let plot = points_of(&v).map(|points| {
                    CompiledPlot::new(
                        PlotKind::Points,
                        PlotShape::Points {
                            points,
                            connect: false,
                        },
                    )
                });
                return Ok(Done {
                    value: Some(v),
                    plot,
                    ..Done::default()
                });
            }
            StatementKind::Equation => return Ok(Done::default()),
            StatementKind::Definition { .. } => {
                return Err(Fail::new(
                    "a plot relation cannot define a name; use a variable or function node",
                ));
            }
            StatementKind::Regression => {
                return Err(Fail::new("regression is not implemented yet"));
            }
            _ => {}
        }
        let spec = PlotSpec {
            stmt: stmt.clone(),
            t_range: match info.kind {
                StatementKind::Parametric { .. } => range(node, "t", &env, &opts, false)?,
                _ => DEFAULT_RANGE,
            },
            theta_range: match info.kind {
                StatementKind::Polar => range(node, "theta", &env, &opts, degrees)?,
                _ => DEFAULT_RANGE,
            },
            degrees,
        };
        let fallback = Cell::new(false);
        let (kind, shape) = build_shape(&spec, &env, &opts, &fallback)?;
        let rebuild: Option<Rebuild> = fallback.get().then(|| {
            let defs = defs.clone();
            let opts = opts.clone();
            let spec = spec.clone();
            let r: Rebuild = Arc::new(move |flag: Arc<AtomicBool>| {
                let opts = EvalOptions {
                    interrupt: Some(flag),
                    ..opts.clone()
                };
                build_shape(&spec, &env_with(&defs), &opts, &Cell::new(false))
                    .ok()
                    .map(|(_, s)| s)
            });
            r
        });
        Ok(Done {
            plot: Some(CompiledPlot {
                kind,
                shape,
                rebuild,
            }),
            ..Done::default()
        })
    }

    fn store_value(&mut self, id: NodeId, value: Option<Value>) -> NodeResult {
        match value {
            Some(v) => {
                let nv = NodeValue::new(v, &self.settings);
                let display = nv.display(self.settings.approx).to_string();
                self.values.insert(id, nv);
                NodeResult::ok(display)
            }
            None => NodeResult {
                status: Status::Ok,
                display: None,
            },
        }
    }
}

impl NodeEvaluator for SceneEvaluator {
    fn evaluate(&mut self, scene: &Scene, id: NodeId) -> NodeResult {
        self.forget(id);
        if matches!(scene.get(id).map(|n| &n.kind), Some(NodeKind::Plot { .. })) {
            self.pending_plots.push(id);
            return NodeResult {
                status: Status::Pending,
                display: None,
            };
        }
        match self.run(scene, id) {
            Ok(done) => {
                let name = scene
                    .get(id)
                    .and_then(|n| n.defined_name())
                    .map(String::from);
                if let (Some(name), Some(v)) = (&name, &done.value) {
                    self.define(id, name, v.clone());
                }
                for (n, v) in done.extra {
                    self.define(id, &n, v);
                }
                if let Some(p) = done.plot {
                    self.plots.insert(id, Arc::new(p));
                }
                self.store_value(id, done.value)
            }
            Err(f) => error_result(scene, id, f),
        }
    }
}

/// Error status with the span kept only when it is a span of the field's
/// own syntax tree: errors raised inside another node's function body
/// carry spans into *that* text, which would mislead here.
fn error_result(scene: &Scene, id: NodeId, f: Fail) -> NodeResult {
    let text = scene
        .get(id)
        .and_then(|n| n.kind.source(f.field))
        .unwrap_or("");
    let own = scene.analysis(id).and_then(|a| a.ast(f.field)).map(|ast| {
        let mut spans = Vec::new();
        match ast {
            Parsed::Expr(e) => expr_spans(e, &mut spans),
            Parsed::Stmt(s) => stmt_spans(s, &mut spans),
            Parsed::Program(p) => p.stmts.iter().for_each(|s| stmt_spans(s, &mut spans)),
        }
        spans
    });
    let span = f.span.filter(|s| {
        s.start <= s.end
            && s.end <= text.len()
            && text.is_char_boundary(s.start)
            && text.is_char_boundary(s.end)
            && own.as_ref().is_none_or(|spans| spans.contains(s))
    });
    NodeResult {
        status: Status::Error {
            message: f.message,
            field: Some(f.field),
            span,
        },
        display: None,
    }
}

pub(crate) fn stmt_spans(s: &Stmt, out: &mut Vec<Span>) {
    out.push(s.span);
    match &s.kind {
        StmtKind::Expr { expr, restrictions } => {
            expr_spans(expr, out);
            restrictions.iter().for_each(|r| expr_spans(r, out));
        }
        StmtKind::Equation {
            lhs,
            rhs,
            restrictions,
        }
        | StmtKind::Definition {
            lhs,
            rhs,
            restrictions,
            ..
        }
        | StmtKind::Regression {
            lhs,
            rhs,
            restrictions,
        } => {
            expr_spans(lhs, out);
            expr_spans(rhs, out);
            // `lhs - rhs` built by statement analysis.
            out.push(lhs.span.to(rhs.span));
            restrictions.iter().for_each(|r| expr_spans(r, out));
        }
        _ => {}
    }
}

/// Every span of an expression tree.
fn expr_spans(e: &Expr, out: &mut Vec<Span>) {
    out.push(e.span);
    let mut each = |items: &[Expr]| items.iter().for_each(|i| expr_spans(i, out));
    match &*e.kind {
        ExprKind::Number { .. } | ExprKind::Str(_) | ExprKind::Ident(_) => {}
        ExprKind::Prime { expr, .. }
        | ExprKind::Unary { expr, .. }
        | ExprKind::Postfix { expr, .. }
        | ExprKind::Member { expr, .. }
        | ExprKind::Derivative { expr, .. } => expr_spans(expr, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            expr_spans(lhs, out);
            expr_spans(rhs, out);
        }
        ExprKind::Add(terms) => terms.iter().for_each(|t| expr_spans(&t.expr, out)),
        ExprKind::Mul(factors) => factors.iter().for_each(|f| expr_spans(&f.expr, out)),
        ExprKind::And(items) | ExprKind::Or(items) | ExprKind::List(items) => each(items),
        ExprKind::Tuple(items) => each(items),
        ExprKind::Compare { first, rest } => {
            expr_spans(first, out);
            rest.iter().for_each(|(_, r)| expr_spans(r, out));
        }
        ExprKind::Call { callee, args } => {
            expr_spans(callee, out);
            args.iter().for_each(|a| expr_spans(a, out));
        }
        ExprKind::Index { expr, indices } => {
            expr_spans(expr, out);
            indices.iter().for_each(|i| expr_spans(i, out));
        }
        ExprKind::Range { start, second, end } => {
            expr_spans(start, out);
            if let Some(s) = second {
                expr_spans(s, out);
            }
            expr_spans(end, out);
        }
        ExprKind::Comprehension { body, bindings } => {
            expr_spans(body, out);
            bindings.iter().for_each(|b| expr_spans(&b.iter, out));
        }
        ExprKind::Piecewise {
            branches,
            otherwise,
        } => {
            for b in branches {
                expr_spans(&b.cond, out);
                if let Some(v) = &b.value {
                    expr_spans(v, out);
                }
            }
            if let Some(o) = otherwise {
                expr_spans(o, out);
            }
        }
        ExprKind::Restrict { expr, conds } => {
            expr_spans(expr, out);
            conds.iter().for_each(|c| expr_spans(c, out));
        }
        ExprKind::Integral { bounds, body, .. } => {
            if let Some((lo, hi)) = bounds {
                expr_spans(lo, out);
                expr_spans(hi, out);
            }
            expr_spans(body, out);
        }
        ExprKind::BigOp { from, to, body, .. } => {
            expr_spans(from, out);
            expr_spans(to, out);
            expr_spans(body, out);
        }
    }
}

/// Parameter range from the node props `<prefix>_min` / `<prefix>_max`
/// (numbers or expressions, in the current angle unit for θ), default
/// `[0, 2π]`.
fn range(
    node: &viscas_scene::Node,
    prefix: &str,
    env: &Env,
    opts: &EvalOptions,
    degrees: bool,
) -> Result<(f64, f64), Fail> {
    let get = |key: String, default: f64| -> Result<f64, Fail> {
        let v = match node.props.get(&key) {
            None => return Ok(default),
            Some(PropValue::Float(f)) => *f,
            Some(PropValue::Int(i)) => *i as f64,
            Some(PropValue::Text(t)) => {
                let e = viscas_core::syntax::parse_expr(t)
                    .map_err(|_| Fail::new(format!("`{key}` is not an expression: {t}")))?;
                eval_expr(&e, env, opts)
                    .map_err(|e| Fail::new(format!("`{key}`: {}", e.message)))?
                    .to_f64()
            }
            Some(PropValue::Bool(_)) => {
                return Err(Fail::new(format!("`{key}` must be a number")));
            }
        };
        if !v.is_finite() {
            return Err(Fail::new(format!("`{key}` must be a finite real number")));
        }
        Ok(if degrees { v.to_radians() } else { v })
    };
    let lo = get(format!("{prefix}_min"), DEFAULT_RANGE.0)?;
    let hi = get(format!("{prefix}_max"), DEFAULT_RANGE.1)?;
    Ok((lo, hi))
}

/// Compile the closures of a relation. Sets `fallback` when any closure
/// uses the general evaluator.
fn build_shape(
    spec: &PlotSpec,
    env: &Env,
    opts: &EvalOptions,
    fallback: &Cell<bool>,
) -> Result<(PlotKind, PlotShape), Fail> {
    let info = classify(&spec.stmt, env);
    let restr = &info.restrictions;
    let Some(expr) = &info.expr else {
        return Err(Fail::new("not a plot relation"));
    };
    let deg = spec.degrees;
    let c1 = |e: &Expr, var: &str| compile1(&restrict(e, restr), var, env, opts, fallback);
    Ok(match &info.kind {
        StatementKind::ExplicitY => (PlotKind::ExplicitY, PlotShape::ExplicitY(c1(expr, "x")?)),
        StatementKind::ExplicitX => (PlotKind::ExplicitX, PlotShape::ExplicitX(c1(expr, "y")?)),
        StatementKind::Polar => {
            let f = c1(expr, "θ")?;
            let r: Fn1 = if deg {
                Arc::new(move |th: f64| f(th.to_degrees()))
            } else {
                f
            };
            let range = spec.theta_range;
            (PlotKind::Polar, PlotShape::Polar { r, range })
        }
        StatementKind::PolarAngle => {
            let g = c1(expr, "r")?;
            let theta: Fn1 = if deg {
                Arc::new(move |r: f64| g(r).to_radians())
            } else {
                g
            };
            (PlotKind::PolarAngle, PlotShape::PolarAngle { theta })
        }
        StatementKind::Implicit => {
            let f = compile2(&restrict(expr, restr), ["x", "y"], env, opts, fallback)?;
            (PlotKind::Implicit, PlotShape::Implicit(f))
        }
        StatementKind::Inequality { .. } => (
            PlotKind::Inequality,
            inequality(expr, restr, false, deg, env, opts, fallback)?,
        ),
        StatementKind::PolarInequality { .. } => (
            PlotKind::PolarInequality,
            inequality(expr, restr, true, deg, env, opts, fallback)?,
        ),
        StatementKind::Parametric { param } => {
            let ExprKind::Tuple(items) = &*expr.kind else {
                return Err(Fail::new("a parametric curve is a point expression"));
            };
            if items.len() != 2 {
                return Err(Fail {
                    message: format!(
                        "a parametric curve needs 2 coordinates, got {}",
                        items.len()
                    ),
                    field: SourceField::Main,
                    span: Some(expr.span),
                });
            }
            let x = c1(&items[0], param)?;
            let y = c1(&items[1], param)?;
            let range = spec.t_range;
            (PlotKind::Parametric, PlotShape::Parametric { x, y, range })
        }
        _ => return Err(Fail::new("not a plot relation")),
    })
}

/// Compile an inequality (Cartesian or polar).
fn inequality(
    expr: &Expr,
    restr: &[Expr],
    polar: bool,
    deg: bool,
    env: &Env,
    opts: &EvalOptions,
    fallback: &Cell<bool>,
) -> Result<PlotShape, Fail> {
    let vars = if polar { ["r", "θ"] } else { ["x", "y"] };
    let to_xy = |f: Fn2| -> Fn2 {
        if polar {
            Arc::new(move |x: f64, y: f64| {
                let th = y.atan2(x);
                f(x.hypot(y), if deg { th.to_degrees() } else { th })
            })
        } else {
            f
        }
    };
    let ExprKind::Compare { first, rest } = &*expr.kind else {
        return Err(Fail::new("not an inequality"));
    };
    if let [(op, rhs)] = rest.as_slice()
        && let Some(cmp) = comparison(*op)
    {
        if !polar {
            let mentions = |e: &Expr, v: &str| free_vars(e, env).iter().any(|f| f == v);
            let c1 = |e: &Expr, var: &str| compile1(&restrict(e, restr), var, env, opts, fallback);
            if is_ident(first, "y") && !mentions(rhs, "y") {
                let f = c1(rhs, "x")?;
                return Ok(PlotShape::InequalityY { f, cmp });
            }
            if is_ident(rhs, "y") && !mentions(first, "y") {
                let f = c1(first, "x")?;
                return Ok(PlotShape::InequalityY { f, cmp: flip(cmp) });
            }
            if is_ident(first, "x") && !mentions(rhs, "x") {
                let f = c1(rhs, "y")?;
                return Ok(PlotShape::InequalityX { f, cmp });
            }
            if is_ident(rhs, "x") && !mentions(first, "x") {
                let f = c1(first, "y")?;
                return Ok(PlotShape::InequalityX { f, cmp: flip(cmp) });
            }
        }
        let diff = restrict(&sub(first, rhs), restr);
        let f = compile2(&diff, vars, env, opts, fallback)?;
        return Ok(PlotShape::InequalityXY { f: to_xy(f), cmp });
    }
    // Chains (`0 < θ < π/2`, `x < y < 2`) and `==`/`!=`: a ±1 indicator
    // (inside = -1) whose sign change is the boundary.
    let g = compile2(&restrict(expr, restr), vars, env, opts, fallback)?;
    let indicator: Fn2 = Arc::new(move |a: f64, b: f64| {
        let v = g(a, b);
        if v.is_nan() {
            f64::NAN
        } else if v != 0.0 {
            -1.0
        } else {
            1.0
        }
    });
    Ok(PlotShape::InequalityXY {
        f: to_xy(indicator),
        cmp: Comparison::Less,
    })
}

fn compile1(
    e: &Expr,
    var: &str,
    env: &Env,
    opts: &EvalOptions,
    fallback: &Cell<bool>,
) -> Result<Fn1, Fail> {
    let f = compile_plot_fn(e, &[var], env, opts).map_err(|e| Fail::eval(e, SourceField::Main))?;
    if !f.is_compiled() {
        fallback.set(true);
    }
    Ok(Arc::new(f.into_fn1()))
}

fn compile2(
    e: &Expr,
    vars: [&str; 2],
    env: &Env,
    opts: &EvalOptions,
    fallback: &Cell<bool>,
) -> Result<Fn2, Fail> {
    let f = compile_plot_fn(e, &vars, env, opts).map_err(|e| Fail::eval(e, SourceField::Main))?;
    if !f.is_compiled() {
        fallback.set(true);
    }
    Ok(Arc::new(f.into_fn2()))
}

/// `e {restrictions}`.
fn restrict(e: &Expr, restr: &[Expr]) -> Expr {
    if restr.is_empty() {
        e.clone()
    } else {
        Expr::new(
            ExprKind::Restrict {
                expr: e.clone(),
                conds: restr.to_vec(),
            },
            e.span,
        )
    }
}

/// `lhs - rhs`.
fn sub(lhs: &Expr, rhs: &Expr) -> Expr {
    Expr::new(
        ExprKind::Add(vec![
            Term {
                op: AddOp::Add,
                expr: lhs.clone(),
            },
            Term {
                op: AddOp::Sub,
                expr: rhs.clone(),
            },
        ]),
        lhs.span.to(rhs.span),
    )
}

fn is_ident(e: &Expr, name: &str) -> bool {
    matches!(&*e.kind, ExprKind::Ident(n) if canonical_name(n) == name)
}

fn comparison(op: CmpOp) -> Option<Comparison> {
    Some(match op {
        CmpOp::Lt => Comparison::Less,
        CmpOp::Le => Comparison::LessEq,
        CmpOp::Gt => Comparison::Greater,
        CmpOp::Ge => Comparison::GreaterEq,
        CmpOp::Eq | CmpOp::Ne => return None,
    })
}

/// `a < b` ⇔ `b > a`.
fn flip(c: Comparison) -> Comparison {
    match c {
        Comparison::Less => Comparison::Greater,
        Comparison::LessEq => Comparison::GreaterEq,
        Comparison::Greater => Comparison::Less,
        Comparison::GreaterEq => Comparison::LessEq,
    }
}

fn real(v: &Value) -> Option<f64> {
    v.as_number()
        .filter(|n| n.is_real())
        .map(|n| n.to_f64())
        .filter(|x| x.is_finite())
}

/// A 2D point value.
pub(crate) fn point_of(v: &Value) -> Option<Vec2> {
    match v {
        Value::Tuple(items) if items.len() == 2 => {
            Some(Vec2::new(real(&items[0])?, real(&items[1])?))
        }
        _ => None,
    }
}

/// A point or a non-empty list of points.
pub(crate) fn points_of(v: &Value) -> Option<Vec<Vec2>> {
    match v {
        Value::Tuple(_) => point_of(v).map(|p| vec![p]),
        Value::List(items) if !items.is_empty() => items.iter().map(point_of).collect(),
        _ => None,
    }
}

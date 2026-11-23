//! Scene evaluation: the [`NodeEvaluator`] over `viscas_core::eval`.
//!
//! Every defining node's value lives in a name → value map that is handed
//! to the core evaluator as a [`Resolver`]. The map is an `Arc` snapshot:
//! evaluating a node clones the `Arc` (cheap), so compiled plot closures
//! that fall back to the general evaluator keep a consistent, immutable
//! view of the scene while they run on worker threads.
//!
//! Nodes are evaluated in topological order by
//! [`viscas_scene::Scene::evaluate_dirty`]; the engine forgets the values of
//! every dirty node first, so a node with an error never leaks a stale
//! value to its dependents.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use viscas_core::eval::{
    AngleUnit, Env, EvalError, EvalOptions, FnBody, Function, Resolver, StatementKind, UserFn,
    Value, canonical_name, classify, compile_plot_fn, eval_expr, format_value, free_vars,
    run_program,
};
use viscas_core::syntax::{AddOp, CmpOp, Expr, ExprKind, Span, Stmt, Term};
use viscas_plot::{Comparison, Vec2};
use viscas_scene::{
    NodeEvaluator, NodeId, NodeKind, NodeResult, Parsed, PropValue, Scene, ScriptLanguage,
    SourceField, Status,
};

use crate::plot::{CompiledPlot, DEFAULT_RANGE, Fn1, Fn2, PlotKind, PlotShape};
use crate::settings::EngineSettings;

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

struct SnapshotResolver(Defs);

impl Resolver for SnapshotResolver {
    fn resolve(&self, name: &str) -> Option<Value> {
        self.0.get(canonical_name(name)).cloned()
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

/// Evaluates scene nodes and keeps their values, definitions and compiled
/// plots. Owned by [`crate::Engine`].
pub struct SceneEvaluator {
    settings: EngineSettings,
    interrupt: Arc<AtomicBool>,
    defs: Defs,
    names: HashMap<NodeId, Vec<String>>,
    values: HashMap<NodeId, NodeValue>,
    plots: HashMap<NodeId, Arc<CompiledPlot>>,
    revisions: HashMap<NodeId, u64>,
    next_revision: u64,
}

impl SceneEvaluator {
    pub fn new(settings: EngineSettings, interrupt: Arc<AtomicBool>) -> SceneEvaluator {
        SceneEvaluator {
            settings,
            interrupt,
            defs: Arc::new(HashMap::new()),
            names: HashMap::new(),
            values: HashMap::new(),
            plots: HashMap::new(),
            revisions: HashMap::new(),
            next_revision: 1,
        }
    }

    pub fn settings(&self) -> &EngineSettings {
        &self.settings
    }

    pub(crate) fn set_settings(&mut self, settings: EngineSettings) {
        self.settings = settings;
    }

    pub(crate) fn eval_options(&self) -> EvalOptions {
        self.settings.eval_options(Some(self.interrupt.clone()))
    }

    /// Current snapshot of the scene definitions.
    pub(crate) fn defs(&self) -> &Defs {
        &self.defs
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
    pub fn lookup(&self, name: &str) -> Option<&Value> {
        self.defs.get(canonical_name(name))
    }

    /// Drops everything known about `id` (its defined names included).
    pub(crate) fn forget(&mut self, id: NodeId) {
        if let Some(names) = self.names.remove(&id) {
            let defs = Arc::make_mut(&mut self.defs);
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
        self.defs = Arc::new(HashMap::new());
        self.names.clear();
        self.values.clear();
        self.plots.clear();
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
        Arc::make_mut(&mut self.defs).insert(name.clone(), value);
        self.names.entry(id).or_default().push(name);
    }

    fn run(&self, scene: &Scene, id: NodeId) -> Result<Done, Fail> {
        let Some(node) = scene.get(id) else {
            return Err(Fail::new("no such node"));
        };
        let env = env_with(&self.defs);
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
                let plot = value
                    .as_ref()
                    .and_then(points_of)
                    .map(|points| CompiledPlot {
                        kind: PlotKind::Points,
                        shape: PlotShape::Points {
                            points,
                            connect: false,
                        },
                    });
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
                        Some(points) => Some(CompiledPlot {
                            kind: PlotKind::Points,
                            shape: PlotShape::Points {
                                points,
                                connect: false,
                            },
                        }),
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
            NodeKind::Plot { .. } => match ast(SourceField::Main) {
                Some(Parsed::Stmt(stmt)) => self.plot_statement(node, stmt, &env, &opts),
                _ => Ok(Done::default()),
            },
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

    /// Classify and compile a plot relation.
    fn plot_statement(
        &self,
        node: &viscas_scene::Node,
        stmt: &Stmt,
        env: &Env,
        opts: &EvalOptions,
    ) -> Result<Done, Fail> {
        let info = classify(stmt, env);
        let restr = &info.restrictions;
        let fail = |e: EvalError| Fail::eval(e, SourceField::Main);
        let Some(expr) = &info.expr else {
            return Err(Fail::new("not a plot relation"));
        };
        let deg = self.settings.angle == AngleUnit::Degrees;
        let c1 = |e: &Expr, var: &str| compile1(&restrict(e, restr), var, env, opts).map_err(fail);
        let plot = |kind, shape| {
            Ok(Done {
                plot: Some(CompiledPlot { kind, shape }),
                ..Done::default()
            })
        };
        match &info.kind {
            StatementKind::ExplicitY => {
                plot(PlotKind::ExplicitY, PlotShape::ExplicitY(c1(expr, "x")?))
            }
            StatementKind::ExplicitX => {
                plot(PlotKind::ExplicitX, PlotShape::ExplicitX(c1(expr, "y")?))
            }
            StatementKind::Polar => {
                let f = c1(expr, "θ")?;
                let r: Fn1 = if deg {
                    Arc::new(move |th: f64| f(th.to_degrees()))
                } else {
                    f
                };
                let range = self.range(node, "theta", env, opts, deg)?;
                plot(PlotKind::Polar, PlotShape::Polar { r, range })
            }
            StatementKind::PolarAngle => {
                let g = c1(expr, "r")?;
                let theta: Fn1 = if deg {
                    Arc::new(move |r: f64| g(r).to_radians())
                } else {
                    g
                };
                plot(PlotKind::PolarAngle, PlotShape::PolarAngle { theta })
            }
            StatementKind::Implicit => {
                let f = compile2(&restrict(expr, restr), ["x", "y"], env, opts).map_err(fail)?;
                plot(PlotKind::Implicit, PlotShape::Implicit(f))
            }
            StatementKind::Inequality { .. } => {
                let shape = self.inequality(expr, restr, false, env, opts)?;
                plot(PlotKind::Inequality, shape)
            }
            StatementKind::PolarInequality { .. } => {
                let shape = self.inequality(expr, restr, true, env, opts)?;
                plot(PlotKind::PolarInequality, shape)
            }
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
                let range = self.range(node, "t", env, opts, false)?;
                plot(PlotKind::Parametric, PlotShape::Parametric { x, y, range })
            }
            StatementKind::Point | StatementKind::Expression => {
                let v = eval_expr(&restrict(expr, restr), env, opts).map_err(fail)?;
                let plot = points_of(&v).map(|points| CompiledPlot {
                    kind: PlotKind::Points,
                    shape: PlotShape::Points {
                        points,
                        connect: false,
                    },
                });
                Ok(Done {
                    value: Some(v),
                    plot,
                    ..Done::default()
                })
            }
            StatementKind::Equation => Ok(Done::default()),
            StatementKind::Definition { .. } => Err(Fail::new(
                "a plot relation cannot define a name; use a variable or function node",
            )),
            StatementKind::Regression => Err(Fail::new("regression is not implemented yet")),
            _ => Err(Fail::new("not a plot relation")),
        }
    }

    /// Parameter range from the node props `<prefix>_min` / `<prefix>_max`
    /// (numbers or expressions, in the current angle unit for θ), default
    /// `[0, 2π]`.
    fn range(
        &self,
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

    /// Compile an inequality (Cartesian or polar).
    fn inequality(
        &self,
        expr: &Expr,
        restr: &[Expr],
        polar: bool,
        env: &Env,
        opts: &EvalOptions,
    ) -> Result<PlotShape, Fail> {
        let fail = |e: EvalError| Fail::eval(e, SourceField::Main);
        let vars = if polar { ["r", "θ"] } else { ["x", "y"] };
        let deg = self.settings.angle == AngleUnit::Degrees;
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
                let c1 = |e: &Expr, var: &str| {
                    compile1(&restrict(e, restr), var, env, opts).map_err(fail)
                };
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
            let f = compile2(&diff, vars, env, opts).map_err(fail)?;
            return Ok(PlotShape::InequalityXY { f: to_xy(f), cmp });
        }
        // Chains (`0 < θ < π/2`, `x < y < 2`) and `==`/`!=`: a ±1 indicator
        // (inside = -1) whose sign change is the boundary.
        let g = compile2(&restrict(expr, restr), vars, env, opts).map_err(fail)?;
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
}

impl NodeEvaluator for SceneEvaluator {
    fn evaluate(&mut self, scene: &Scene, id: NodeId) -> NodeResult {
        self.forget(id);
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
                match done.value {
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
            Err(f) => {
                let len = scene
                    .get(id)
                    .and_then(|n| n.kind.source(f.field))
                    .map_or(0, str::len);
                // Spans from other nodes' bodies (a failing function call)
                // do not index this field; drop them rather than mislead.
                let span = f.span.filter(|s| s.start <= s.end && s.end <= len);
                NodeResult {
                    status: Status::Error {
                        message: f.message,
                        field: Some(f.field),
                        span,
                    },
                    display: None,
                }
            }
        }
    }
}

fn compile1(e: &Expr, var: &str, env: &Env, opts: &EvalOptions) -> Result<Fn1, EvalError> {
    let f = compile_plot_fn(e, &[var], env, opts)?;
    Ok(Arc::new(f.into_fn1()))
}

fn compile2(e: &Expr, vars: [&str; 2], env: &Env, opts: &EvalOptions) -> Result<Fn2, EvalError> {
    let f = compile_plot_fn(e, &vars, env, opts)?;
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

//! Per-node analysis: parse every source field, then extract the names a
//! node defines and the free identifiers it references.
//!
//! Scoping rules for free identifiers:
//!
//! - Function parameters are bound in the body; regression parameters are
//!   bound in the regression.
//! - `let`, `for`, script `fn` parameters, comprehension bindings
//!   (`[k^2 for k = L]`, later bindings see earlier ones), `∑`/`∏` indices,
//!   `d/dx` and `∫ … dx` variables bind in their bodies only (bounds and
//!   ranges are outside the binder).
//! - Plots bind `x`, `y` and `θ`/`theta`; `t` when the relation is a
//!   parametric tuple; `r` when the relation is polar (`r = …` or it
//!   mentions `θ`).
//! - In actions, plain targets (`a` in `a -> a + 1`) are *writes*, not
//!   references.
//! - A multi-letter name made only of letters (`ab`) is also recorded
//!   letter by letter ([`Reference::split`]), because the evaluator reads
//!   an undefined `ab` as `a·b`.

use viscas_core::syntax::{
    self, Block, ElseBranch, Expr, ExprKind, ParseError, Program, Span, Stmt, StmtKind,
};

use crate::node::{NodeContent, NodeKind, ParseMode, SourceField};

/// A parsed source field.
#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
    Expr(Expr),
    Stmt(Stmt),
    Program(Program),
}

/// Parse result of one source field. Scripts keep the statements that
/// parsed even when there are errors.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedSource {
    pub field: SourceField,
    /// `None` for blank fields and fields that failed to parse.
    pub ast: Option<Parsed>,
    pub errors: Vec<ParseError>,
}

/// One letter of a split multi-letter identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Letter {
    pub name: String,
    /// Bound locally at the occurrence (e.g. the `x` of `ax` in a plot).
    pub bound: bool,
}

/// A free identifier occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub name: String,
    pub field: SourceField,
    /// Byte span of the identifier inside the field's text.
    pub span: Span,
    /// Letter-wise reading for multi-letter names (`ab` → `a·b`).
    pub split: Option<Vec<Letter>>,
}

/// Everything derived from a node's content without looking at other
/// nodes. Cached by the scene; not serialized.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    pub sources: Vec<ParsedSource>,
    /// Names this node defines.
    pub defines: Vec<String>,
    /// Free identifier occurrences, in source order per field.
    pub references: Vec<Reference>,
    /// Names assigned by actions / scripts (`a` in `a -> a + 1`).
    pub writes: Vec<String>,
}

impl Analysis {
    /// Does any source field have a parse error?
    pub fn has_errors(&self) -> bool {
        self.sources.iter().any(|s| !s.errors.is_empty())
    }

    /// First parse error with its field.
    pub fn first_error(&self) -> Option<(SourceField, &ParseError)> {
        self.sources
            .iter()
            .find_map(|s| s.errors.first().map(|e| (s.field, e)))
    }

    /// Parsed AST of a field.
    pub fn ast(&self, field: SourceField) -> Option<&Parsed> {
        self.sources
            .iter()
            .find(|s| s.field == field)
            .and_then(|s| s.ast.as_ref())
    }

    /// Distinct referenced names (unsplit), sorted.
    pub fn referenced_names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.references.iter().map(|r| r.name.as_str()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// Analyze a node's content.
pub fn analyze(content: &NodeContent) -> Analysis {
    let mut out = Analysis::default();
    let kind = &content.kind;

    if kind.defines_by_name()
        && let Some(n) = &content.name
    {
        out.defines.push(n.clone());
    }

    for src in kind.sources() {
        let mut parsed = ParsedSource {
            field: src.field,
            ast: None,
            errors: Vec::new(),
        };
        if src.text.trim().is_empty() {
            out.sources.push(parsed);
            continue;
        }
        match src.mode {
            ParseMode::Expr => match syntax::parse_expr(src.text) {
                Ok(e) => parsed.ast = Some(Parsed::Expr(e)),
                Err(errs) => parsed.errors = errs,
            },
            ParseMode::Statement => match syntax::parse_statement(src.text) {
                Ok(s) => parsed.ast = Some(Parsed::Stmt(s)),
                Err(errs) => parsed.errors = errs,
            },
            ParseMode::Program => {
                let (p, errs) = syntax::parse_program_partial(src.text);
                parsed.ast = Some(Parsed::Program(p));
                parsed.errors = errs;
            }
        }
        out.sources.push(parsed);
    }

    for parsed in &out.sources {
        let Some(ast) = &parsed.ast else { continue };
        let mut w = Walker {
            field: parsed.field,
            scope: Vec::new(),
            refs: Vec::new(),
            writes: Vec::new(),
            script: false,
            theta_seen: false,
        };
        match (kind, ast) {
            (NodeKind::Function { params, .. }, Parsed::Expr(e)) => {
                w.scope.extend(params.iter().cloned());
                w.expr(e);
            }
            (NodeKind::Regression { params, .. }, Parsed::Stmt(s)) => {
                w.scope.extend(params.iter().cloned());
                w.stmt(s);
            }
            (NodeKind::Plot { .. }, Parsed::Stmt(s)) => {
                let parametric = is_parametric(s);
                w.scope.extend(["x", "y", "θ", "theta"].map(String::from));
                if parametric {
                    w.scope.push("t".into());
                }
                w.stmt(s);
                let polar = lhs_is(s, "r") || w.saw_theta();
                if polar {
                    w.bind_after("r");
                }
            }
            (NodeKind::Script { .. }, Parsed::Program(p)) => {
                w.script = true;
                let top = w.program(p);
                out.defines.extend(top);
            }
            (_, Parsed::Expr(e)) => w.expr(e),
            (_, Parsed::Stmt(s)) => w.stmt(s),
            (_, Parsed::Program(p)) => {
                w.program(p);
            }
        }
        out.references.append(&mut w.refs);
        for name in w.writes {
            if !out.writes.contains(&name) {
                out.writes.push(name);
            }
        }
    }
    if let NodeKind::Regression { params, .. } = kind {
        out.defines.extend(params.iter().cloned());
    }
    out
}

/// `(f(t), g(t))` relations (optionally with restrictions).
fn is_parametric(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Expr { expr, .. } => matches!(&*expr.kind, ExprKind::Tuple(_)),
        _ => false,
    }
}

fn lhs_is(s: &Stmt, name: &str) -> bool {
    match &s.kind {
        StmtKind::Equation { lhs, .. } | StmtKind::Expr { expr: lhs, .. } => {
            matches!(&*lhs.kind, ExprKind::Ident(n) if n == name)
                || matches!(&*lhs.kind, ExprKind::Compare { first, .. }
                    if matches!(&*first.kind, ExprKind::Ident(n) if n == name))
        }
        _ => false,
    }
}

/// Is `name` splittable into letters (`ab`, not `a1`, `x_1`, `km` is)?
fn split_letters(name: &str) -> Option<Vec<String>> {
    let mut chars = name.chars();
    chars.next()?;
    chars.next()?;
    name.chars()
        .all(char::is_alphabetic)
        .then(|| name.chars().map(String::from).collect())
}

struct Walker {
    field: SourceField,
    scope: Vec<String>,
    refs: Vec<Reference>,
    writes: Vec<String>,
    script: bool,
    theta_seen: bool,
}

impl Walker {
    fn bound(&self, name: &str) -> bool {
        self.scope.iter().any(|s| s == name)
    }

    fn ident(&mut self, name: &str, span: Span) {
        if name == "θ" || name == "theta" {
            self.theta_seen = true;
        }
        if self.bound(name) {
            return;
        }
        let split = split_letters(name).map(|letters| {
            letters
                .into_iter()
                .map(|l| Letter {
                    bound: self.bound(&l),
                    name: l,
                })
                .collect()
        });
        self.refs.push(Reference {
            name: name.to_string(),
            field: self.field,
            span,
            split,
        });
    }

    fn saw_theta(&self) -> bool {
        self.theta_seen
    }

    /// Retroactively bind `name` (polar `r`): drop its references and mark
    /// it bound in split letters.
    fn bind_after(&mut self, name: &str) {
        self.refs.retain(|r| r.name != name);
        for r in &mut self.refs {
            if let Some(letters) = &mut r.split {
                for l in letters.iter_mut().filter(|l| l.name == name) {
                    l.bound = true;
                }
            }
        }
    }

    fn with<R>(
        &mut self,
        names: impl IntoIterator<Item = String>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let mark = self.scope.len();
        self.scope.extend(names);
        let r = f(self);
        self.scope.truncate(mark);
        r
    }

    fn exprs(&mut self, items: &[Expr]) {
        for e in items {
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &*e.kind {
            ExprKind::Number { .. } | ExprKind::Str(_) => {}
            ExprKind::Ident(name) => self.ident(name, e.span),
            ExprKind::Prime { expr, .. }
            | ExprKind::Unary { expr, .. }
            | ExprKind::Postfix { expr, .. }
            | ExprKind::Member { expr, .. } => self.expr(expr),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Add(terms) => terms.iter().for_each(|t| self.expr(&t.expr)),
            ExprKind::Mul(factors) => factors.iter().for_each(|f| self.expr(&f.expr)),
            ExprKind::And(items)
            | ExprKind::Or(items)
            | ExprKind::List(items)
            | ExprKind::Tuple(items) => self.exprs(items),
            ExprKind::Compare { first, rest } => {
                self.expr(first);
                rest.iter().for_each(|(_, e)| self.expr(e));
            }
            ExprKind::Call { callee, args } => {
                self.expr(callee);
                self.exprs(args);
            }
            ExprKind::Index { expr, indices } => {
                self.expr(expr);
                self.exprs(indices);
            }
            ExprKind::Range { start, second, end } => {
                self.expr(start);
                if let Some(s) = second {
                    self.expr(s);
                }
                self.expr(end);
            }
            ExprKind::Comprehension { body, bindings } => {
                let mark = self.scope.len();
                for b in bindings {
                    self.expr(&b.iter);
                    self.scope.push(b.name.clone());
                }
                self.expr(body);
                self.scope.truncate(mark);
            }
            ExprKind::Piecewise {
                branches,
                otherwise,
            } => {
                for b in branches {
                    self.expr(&b.cond);
                    if let Some(v) = &b.value {
                        self.expr(v);
                    }
                }
                if let Some(o) = otherwise {
                    self.expr(o);
                }
            }
            ExprKind::Restrict { expr, conds } => {
                self.expr(expr);
                self.exprs(conds);
            }
            ExprKind::Derivative { var, expr, .. } => {
                self.with([var.clone()], |w| w.expr(expr));
            }
            ExprKind::Integral { bounds, body, var } => {
                if let Some((lo, hi)) = bounds {
                    self.expr(lo);
                    self.expr(hi);
                }
                self.with([var.clone()], |w| w.expr(body));
            }
            ExprKind::BigOp {
                var,
                from,
                to,
                body,
                ..
            } => {
                self.expr(from);
                self.expr(to);
                self.with([var.clone()], |w| w.expr(body));
            }
        }
    }

    fn write(&mut self, name: &str) {
        if !self.bound(name) && !self.writes.iter().any(|w| w == name) {
            self.writes.push(name.to_string());
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Expr { expr, restrictions } => {
                self.expr(expr);
                self.exprs(restrictions);
            }
            StmtKind::Equation {
                lhs,
                rhs,
                restrictions,
            } if self.script => match &*lhs.kind {
                // `a = 5` in a script assigns the scene variable `a`.
                ExprKind::Ident(name) => {
                    self.expr(rhs);
                    self.exprs(restrictions);
                    self.write(name);
                }
                // `g(x) = …` defines a local function.
                ExprKind::Call { callee, args }
                    if matches!(&*callee.kind, ExprKind::Ident(_))
                        && args.iter().all(|a| matches!(&*a.kind, ExprKind::Ident(_))) =>
                {
                    let params = args.iter().filter_map(|a| match &*a.kind {
                        ExprKind::Ident(p) => Some(p.clone()),
                        _ => None,
                    });
                    let params: Vec<String> = params.collect();
                    self.with(params, |w| {
                        w.expr(rhs);
                        w.exprs(restrictions);
                    });
                    if let ExprKind::Ident(f) = &*callee.kind {
                        self.scope.push(f.clone());
                    }
                }
                _ => {
                    self.expr(lhs);
                    self.expr(rhs);
                    self.exprs(restrictions);
                }
            },
            StmtKind::Equation {
                lhs,
                rhs,
                restrictions,
            }
            | StmtKind::Regression {
                lhs,
                rhs,
                restrictions,
            } => {
                self.expr(lhs);
                self.expr(rhs);
                self.exprs(restrictions);
            }
            StmtKind::Action { actions } => {
                for (target, value) in actions {
                    match &*target.kind {
                        ExprKind::Ident(name) => self.write(name),
                        _ => self.expr(target),
                    }
                    self.expr(value);
                }
            }
            StmtKind::Let { name, value } => {
                self.expr(value);
                self.scope.push(name.clone());
            }
            StmtKind::If {
                cond,
                then,
                otherwise,
            } => {
                self.expr(cond);
                self.block(then);
                match otherwise {
                    Some(ElseBranch::Block(b)) => self.block(b),
                    Some(ElseBranch::If(s)) => self.stmt(s),
                    None => {}
                }
            }
            StmtKind::For { var, iter, body } => {
                self.expr(iter);
                self.with([var.clone()], |w| w.block(body));
            }
            StmtKind::While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            StmtKind::Fn { params, body, .. } => {
                // The name itself is hoisted by `stmts`.
                self.with(params.iter().cloned(), |w| w.block(body));
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
        }
    }

    /// Walk a statement list in its own scope with hoisted `fn` names.
    /// Returns the hoisted names.
    fn stmts(&mut self, stmts: &[Stmt]) -> Vec<String> {
        let mark = self.scope.len();
        let fns: Vec<String> = stmts
            .iter()
            .filter_map(|s| match &s.kind {
                StmtKind::Fn { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        self.scope.extend(fns.iter().cloned());
        for s in stmts {
            self.stmt(s);
        }
        self.scope.truncate(mark);
        fns
    }

    fn block(&mut self, b: &Block) {
        self.stmts(&b.stmts);
    }

    fn program(&mut self, p: &Program) -> Vec<String> {
        self.stmts(&p.stmts)
    }
}

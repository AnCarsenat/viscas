//! Per-node analysis: parse every source field, then extract the names a
//! node defines and the free identifiers it references.
//!
//! Scoping rules for free identifiers:
//!
//! - Function parameters are bound in the body; regression parameters are
//!   bound in the regression; table column names are bound in the table's
//!   cells and formulas (outside, columns are reached as `T.x1`).
//! - `let`, `for`, script `fn` parameters, local functions `g(x) = …`
//!   (name and parameters, so they may recurse), comprehension bindings
//!   (`[k^2 for k = L]`, later bindings see earlier ones), `∑`/`∏` indices,
//!   `d/dx` and `∫ … dx` variables bind in their bodies only (bounds and
//!   ranges are outside the binder).
//! - Plots bind `x`, `y` and `θ`/`theta`; `t` when the relation is a
//!   parametric tuple; `r` when the relation is polar (`r = …` or it
//!   mentions `θ`).
//! - Plain action targets (`a` in `a -> a + 1`) and script assignments
//!   (`a = 5`) are *writes* ([`Analysis::writes`]), not references and
//!   not definitions.
//! - A multi-letter name made only of letters (`ab`) is also recorded
//!   letter by letter ([`Reference::split`]), because the evaluator reads
//!   an undefined `ab` as `a·b`.
//!
//! Each occurrence records the names bound around it
//! ([`Reference::scope`]) so rewrites can detect name capture.

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

/// An identifier occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub name: String,
    pub field: SourceField,
    /// Byte span of the identifier inside the field's text.
    pub span: Span,
    /// Letter-wise reading for multi-letter names (`ab` → `a·b`).
    pub split: Option<Vec<Letter>>,
    /// Names bound by enclosing binders at this occurrence (sorted,
    /// deduplicated). Rewriting the occurrence to one of these would be
    /// captured.
    pub scope: Vec<String>,
}

/// Everything derived from a node's content without looking at other
/// nodes. Cached by the scene; not serialized.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    pub sources: Vec<ParsedSource>,
    /// Names this node defines.
    pub defines: Vec<String>,
    /// Free identifier occurrences (reads), in source order per field.
    pub references: Vec<Reference>,
    /// Write targets (`a` in `a -> a + 1`, script `a = 5`).
    pub writes: Vec<Reference>,
    /// Occurrences, inside this node, of names the node itself defines and
    /// binds internally: regression parameters and script top-level `fn`
    /// names (including the name at the `fn` definition site).
    pub defined_uses: Vec<Reference>,
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

    /// Distinct written names, in first-occurrence order.
    pub fn written_names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = Vec::new();
        for w in &self.writes {
            if !v.contains(&w.name.as_str()) {
                v.push(&w.name);
            }
        }
        v
    }

    /// Every name the node mentions in a way that depends on other nodes:
    /// read names, their split letters and write targets.
    pub fn dependency_names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = Vec::new();
        for r in self.references.iter().chain(&self.writes) {
            v.push(&r.name);
            if let Some(letters) = &r.split {
                v.extend(letters.iter().filter(|l| !l.bound).map(|l| l.name.as_str()));
            }
        }
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

    let table_columns: Vec<String> = match kind {
        NodeKind::Table { columns } => columns.iter().map(|c| c.name.clone()).collect(),
        _ => Vec::new(),
    };

    for parsed in &out.sources {
        let Some(ast) = &parsed.ast else { continue };
        let text = kind.source(parsed.field).unwrap_or("");
        let mut w = Walker::new(parsed.field, text);
        match (kind, ast) {
            (NodeKind::Function { params, .. }, Parsed::Expr(e)) => {
                w.bind_all(params, false);
                w.expr(e);
            }
            (NodeKind::Regression { params, .. }, Parsed::Stmt(s)) => {
                w.bind_all(params, true);
                w.stmt(s);
            }
            (NodeKind::Table { .. }, Parsed::Expr(e)) => {
                w.bind_all(&table_columns, false);
                w.expr(e);
            }
            (NodeKind::Plot { .. }, Parsed::Stmt(s)) => {
                w.bind_all(&["x", "y", "θ", "theta"].map(String::from), false);
                if is_parametric(s) {
                    w.bind("t", false);
                }
                w.stmt(s);
                if lhs_is(s, "r") || w.theta_seen {
                    w.bind_after("r");
                }
            }
            (NodeKind::Script { .. }, Parsed::Program(p)) => {
                w.script = true;
                let top = w.program(p, true);
                for name in top {
                    if !out.defines.contains(&name) {
                        out.defines.push(name);
                    }
                }
            }
            (_, Parsed::Expr(e)) => w.expr(e),
            (_, Parsed::Stmt(s)) => w.stmt(s),
            (_, Parsed::Program(p)) => {
                w.program(p, false);
            }
        }
        out.references.append(&mut w.refs);
        out.writes.append(&mut w.writes);
        out.defined_uses.append(&mut w.defined_uses);
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

/// Is `name` splittable into letters (`ab`, not `a1`, `x_1`; `km` is)?
fn split_letters(name: &str) -> Option<Vec<String>> {
    let mut chars = name.chars();
    chars.next()?;
    chars.next()?;
    name.chars()
        .all(char::is_alphabetic)
        .then(|| name.chars().map(String::from).collect())
}

/// Byte span of the function name in `fn name(...)` starting at `start`.
fn fn_name_span(text: &str, start: usize, name: &str) -> Option<Span> {
    let rest = text.get(start..)?.strip_prefix("fn")?;
    let trimmed = rest.trim_start();
    let offset = start + 2 + (rest.len() - trimmed.len());
    trimmed
        .starts_with(name)
        .then(|| Span::new(offset, offset + name.len()))
}

struct Binder {
    name: String,
    /// Defined by the node itself (regression parameter, script `fn`).
    node_defined: bool,
}

struct Walker<'a> {
    field: SourceField,
    text: &'a str,
    scope: Vec<Binder>,
    refs: Vec<Reference>,
    writes: Vec<Reference>,
    defined_uses: Vec<Reference>,
    script: bool,
    theta_seen: bool,
}

impl<'a> Walker<'a> {
    fn new(field: SourceField, text: &'a str) -> Self {
        Walker {
            field,
            text,
            scope: Vec::new(),
            refs: Vec::new(),
            writes: Vec::new(),
            defined_uses: Vec::new(),
            script: false,
            theta_seen: false,
        }
    }

    fn bind(&mut self, name: &str, node_defined: bool) {
        self.scope.push(Binder {
            name: name.to_string(),
            node_defined,
        });
    }

    fn bind_all(&mut self, names: &[String], node_defined: bool) {
        for n in names {
            self.bind(n, node_defined);
        }
    }

    fn binder(&self, name: &str) -> Option<&Binder> {
        self.scope.iter().rev().find(|b| b.name == name)
    }

    fn bound(&self, name: &str) -> bool {
        self.binder(name).is_some()
    }

    fn scope_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.scope.iter().map(|b| b.name.clone()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    fn occurrence(&self, name: &str, span: Span) -> Reference {
        let split = split_letters(name).map(|letters| {
            letters
                .into_iter()
                .map(|l| Letter {
                    bound: self.bound(&l),
                    name: l,
                })
                .collect()
        });
        Reference {
            name: name.to_string(),
            field: self.field,
            span,
            split,
            scope: self.scope_names(),
        }
    }

    fn ident(&mut self, name: &str, span: Span) {
        if name == "θ" || name == "theta" {
            self.theta_seen = true;
        }
        match self.binder(name) {
            Some(b) if b.node_defined => {
                let r = self.occurrence(name, span);
                self.defined_uses.push(r);
            }
            Some(_) => {}
            None => {
                let r = self.occurrence(name, span);
                self.refs.push(r);
            }
        }
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
            if let Err(i) = r.scope.binary_search_by(|s| s.as_str().cmp(name)) {
                r.scope.insert(i, name.to_string());
            }
        }
    }

    fn with<R>(&mut self, names: &[String], f: impl FnOnce(&mut Self) -> R) -> R {
        let mark = self.scope.len();
        self.bind_all(names, false);
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
                    self.bind(&b.name, false);
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
                self.with(std::slice::from_ref(var), |w| w.expr(expr));
            }
            ExprKind::Integral { bounds, body, var } => {
                if let Some((lo, hi)) = bounds {
                    self.expr(lo);
                    self.expr(hi);
                }
                self.with(std::slice::from_ref(var), |w| w.expr(body));
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
                self.with(std::slice::from_ref(var), |w| w.expr(body));
            }
        }
    }

    /// A plain write target (`a` in `a -> …`, script `a = …`).
    fn write(&mut self, name: &str, span: Span) {
        if self.bound(name) {
            return;
        }
        let r = self.occurrence(name, span);
        self.writes.push(r);
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
                // `a = 5` in a script assigns (writes) the scene variable `a`
                // — or a local `let` binding when one is in scope.
                ExprKind::Ident(name) => {
                    self.expr(rhs);
                    self.exprs(restrictions);
                    self.write(name, lhs.span);
                }
                // `g(x) = …` defines a local function; it may recurse.
                ExprKind::Call { callee, args }
                    if matches!(&*callee.kind, ExprKind::Ident(_))
                        && args.iter().all(|a| matches!(&*a.kind, ExprKind::Ident(_))) =>
                {
                    let ExprKind::Ident(f) = &*callee.kind else {
                        unreachable!()
                    };
                    self.bind(f, false);
                    let params: Vec<String> = args
                        .iter()
                        .filter_map(|a| match &*a.kind {
                            ExprKind::Ident(p) => Some(p.clone()),
                            _ => None,
                        })
                        .collect();
                    self.with(&params, |w| {
                        w.expr(rhs);
                        w.exprs(restrictions);
                    });
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
                        ExprKind::Ident(name) => self.write(name, target.span),
                        _ => self.expr(target),
                    }
                    self.expr(value);
                }
            }
            StmtKind::Let { name, value } => {
                self.expr(value);
                self.bind(name, false);
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
                self.with(std::slice::from_ref(var), |w| w.block(body));
            }
            StmtKind::While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            StmtKind::Fn { params, body, .. } => {
                // The name itself is hoisted by `stmts`.
                self.with(params, |w| w.block(body));
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
        }
    }

    /// Walk a statement list in its own scope with hoisted `fn` names.
    /// At the top level of a script (`top`), the hoisted names are defined
    /// by the node: their definition sites and uses are recorded.
    fn stmts(&mut self, stmts: &[Stmt], top: bool) -> Vec<String> {
        let mark = self.scope.len();
        let mut fns: Vec<String> = Vec::new();
        for s in stmts {
            if let StmtKind::Fn { name, .. } = &s.kind {
                if top && let Some(span) = fn_name_span(self.text, s.span.start, name) {
                    let r = self.occurrence(name, span);
                    self.defined_uses.push(r);
                }
                if !fns.contains(name) {
                    fns.push(name.clone());
                }
            }
        }
        for f in &fns {
            self.bind(f, top);
        }
        for s in stmts {
            self.stmt(s);
        }
        self.scope.truncate(mark);
        fns
    }

    fn block(&mut self, b: &Block) {
        self.stmts(&b.stmts, false);
    }

    fn program(&mut self, p: &Program, top: bool) -> Vec<String> {
        self.stmts(&p.stmts, top)
    }
}

//! The console: one-line evaluation sharing the scene's definitions, with
//! its own definitions, session locals, `ans`/`ans2`, commands and input
//! history.
//!
//! Name rules:
//! - Scene nodes are visible by name and are never shadowed: `a := 1`
//!   when a node defines `a` redefines that node (one undo step).
//! - New names stay console-local until added to the board (`:add`,
//!   alias `:pin`). A console definition whose name a node later takes
//!   yields to the node; it comes back if the node goes away (e.g. undo
//!   of the add).
//! - `:add` turns the last input into a node, adding the console-only
//!   definitions it depends on first, as one undo step. `:plot <input>`
//!   evaluates the input and adds it in one go.
//! - [`Engine::console_eval_with`] can add inputs to the board as they
//!   are evaluated ([`AutoAdd`]): things that draw (relations, points)
//!   and/or new definitions.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::Ordering;

use viscas_core::eval::{
    AngleUnit, BUILTINS, ErrorKind, EvalError, Outcome, Session, StatementKind, Value,
    canonical_name, format_value,
};
use viscas_core::syntax::{ExprKind, Span, StmtKind, parse_program_with, parse_statement_with};
use viscas_scene::{NodeContent, NodeId, NodeKind, analyze, definition_line_with};

use crate::complete::{candidate_names, suggest};
use crate::engine::Engine;
use crate::eval::{env_with, stmt_spans};
use crate::help::{command_help, constant_help, function_help};
use crate::lines::line_parse_options;

/// Maximum number of remembered console inputs.
pub const HISTORY_LIMIT: usize = 1000;

/// What a console result line is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// The value of an expression.
    Value,
    /// A console definition (`a := 3`).
    Definition { name: String },
    /// `a := …` redefined the scene node `a` ("updated node a"); `exact`
    /// and `approx` show its new value.
    UpdatedNode { node: NodeId, name: String },
    /// A plot relation (`y = x^2`) that stayed in the console: no value;
    /// `:add` turns it into a plot.
    Relation { kind: String },
    /// The input was added to the board as this node (dependencies
    /// first, the input itself last).
    Added { node: NodeId },
    /// A `:vars` entry.
    Variable { name: String, scene: bool },
    /// Help or status text (in `exact`).
    Info,
}

/// One result line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleItem {
    /// Exact-style text (`1/3`, `≈1.414…`), or the message for `Info`.
    pub exact: String,
    /// Decimal text (empty when not applicable).
    pub approx: String,
    pub kind: ItemKind,
}

impl ConsoleItem {
    fn info(text: impl Into<String>) -> ConsoleItem {
        ConsoleItem {
            exact: text.into(),
            approx: String::new(),
            kind: ItemKind::Info,
        }
    }

    /// The primary text for the mode (falls back to `exact`).
    pub fn display(&self, approx: bool) -> &str {
        if approx && !self.approx.is_empty() {
            &self.approx
        } else {
            &self.exact
        }
    }
}

/// A console error with the byte span in the input line, if known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleError {
    pub message: String,
    pub span: Option<Span>,
}

/// Result of [`Engine::console_eval`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConsoleOutput {
    /// The input as entered (trimmed).
    pub input_echo: String,
    pub results: Vec<ConsoleItem>,
    pub error: Option<ConsoleError>,
    /// "Did you mean" names for unknown names / commands.
    pub suggestions: Vec<String>,
    /// `:clear` — the UI should clear its transcript.
    pub cleared: bool,
}

/// Which console inputs [`Engine::console_eval_with`] adds to the board
/// as soon as they evaluate (each statement is one undo step).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutoAdd {
    /// Things that draw: relations (`y = x^2`, `x^2 + y^2 = 4`, `r = θ`,
    /// parametric curves, inequalities) and points (`(1, 2)`, `P := (1, 2)`).
    pub plots: bool,
    /// New definitions (`a := 3`, `f(x) := x^2`).
    pub definitions: bool,
    /// Plain values too (`2 + 3`); used by `:plot`.
    pub values: bool,
}

impl AutoAdd {
    /// Add everything that can become a node.
    pub const ALL: AutoAdd = AutoAdd {
        plots: true,
        definitions: true,
        values: true,
    };
}

/// Console state kept by the engine.
#[derive(Debug, Default)]
pub(crate) struct ConsoleState {
    session: Session,
    /// Console definitions (including `ans`, `ans2`).
    defs: BTreeMap<String, Value>,
    /// Source line of each console definition (for `:pin`).
    sources: HashMap<String, String>,
    history: Vec<String>,
    /// The last successful non-command statement (for `:pin`).
    last: Option<String>,
}

impl ConsoleState {
    pub(crate) fn forget_pin(&mut self) {
        self.last = None;
    }
}

const MODES: &[&str] = &["exact", "approx", "deg", "rad", "real", "complex"];

/// Is `span` a usable range of `text`?
fn span_in(text: &str, span: Span) -> bool {
    span.start <= span.end
        && span.end <= text.len()
        && text.is_char_boundary(span.start)
        && text.is_char_boundary(span.end)
}

fn shift(span: Span, by: usize) -> Span {
    Span {
        start: span.start + by,
        end: span.end + by,
    }
}

impl Engine {
    /// Evaluate one console line (expressions, definitions, relations, or
    /// a `:command`), statement by statement. Nothing is added to the
    /// board automatically; see [`Engine::console_eval_with`].
    pub fn console_eval(&mut self, line: &str) -> ConsoleOutput {
        self.console_eval_with(line, AutoAdd::default())
    }

    /// [`Engine::console_eval`], adding the statements `auto` selects to
    /// the board as they evaluate (their results become
    /// [`ItemKind::Added`]).
    pub fn console_eval_with(&mut self, line: &str, auto: AutoAdd) -> ConsoleOutput {
        let input = line.trim();
        self.push_history(input);
        let mut out = ConsoleOutput {
            input_echo: input.to_string(),
            ..ConsoleOutput::default()
        };
        if input.is_empty() {
            return out;
        }
        if let Some(cmd) = input.strip_prefix(':') {
            self.console_command(cmd, input.len() - cmd.len(), &mut out);
            return out;
        }
        self.eval_statements(input, 0, auto, &mut out);
        out
    }

    /// Evaluates `src` (found at byte `base` of the echoed input).
    fn eval_statements(&mut self, src: &str, base: usize, auto: AutoAdd, out: &mut ConsoleOutput) {
        self.interrupt_flag().store(false, Ordering::Relaxed);
        let settings = *self.settings();
        let program = match parse_program_with(src, &settings.parse_options()) {
            Ok(p) => p,
            Err(errs) => {
                let e = &errs[0];
                out.error = Some(ConsoleError {
                    message: e.message.clone(),
                    span: span_in(src, e.span).then(|| shift(e.span, base)),
                });
                return;
            }
        };
        for stmt in &program.stmts {
            let (start, end) = (stmt.span.start, stmt.span.end);
            if !span_in(src, stmt.span) {
                continue;
            }
            let text = &src[start..end];
            let before = out.results.len();
            if !self.console_statement(text, base + start, out) {
                return;
            }
            self.console.last = Some(text.to_string());
            if self.wants_auto_add(text, &out.results[before..], auto)
                && !self.auto_add(before, out)
            {
                return;
            }
        }
    }

    /// Should the statement `text` with results `items` go on the board?
    fn wants_auto_add(&self, text: &str, items: &[ConsoleItem], auto: AutoAdd) -> bool {
        if items.is_empty()
            || items
                .iter()
                .any(|i| matches!(i.kind, ItemKind::UpdatedNode { .. }))
        {
            // Nothing to add, or already on the board (node redefined).
            return false;
        }
        let draws = self
            .line_content(text)
            .is_ok_and(|c| matches!(c.kind, NodeKind::Plot { .. } | NodeKind::Point { .. }));
        if draws {
            return auto.plots || auto.values;
        }
        items.iter().all(|i| match i.kind {
            ItemKind::Definition { .. } => auto.definitions,
            ItemKind::Value => auto.values,
            ItemKind::Relation { .. } => auto.plots,
            _ => false,
        })
    }

    /// Adds the last statement to the board, replacing its results (from
    /// index `from`) with the added nodes; `false` after an error.
    fn auto_add(&mut self, from: usize, out: &mut ConsoleOutput) -> bool {
        match self.pin_plan().and_then(|plan| self.apply_pin_plan(plan)) {
            Ok(ids) => {
                out.results.truncate(from);
                let items = self.added_items(ids);
                out.results.extend(items);
                self.console.last = None;
                true
            }
            Err(message) => {
                out.error = Some(ConsoleError {
                    message: format!("could not add to the board: {message}"),
                    span: None,
                });
                false
            }
        }
    }

    /// Forgets the last input, so `:add` has nothing to add (a UI added
    /// it from a [`Engine::pin_plan`] snapshot already).
    pub fn forget_console_last(&mut self) {
        self.console.forget_pin();
    }

    /// [`ItemKind::Added`] results for new nodes, with their values.
    pub fn added_items(&self, ids: Vec<NodeId>) -> Vec<ConsoleItem> {
        ids.into_iter()
            .map(|node| {
                let (exact, approx) = self
                    .value(node)
                    .map(|v| (v.exact.clone(), v.approx.clone()))
                    .unwrap_or_default();
                ConsoleItem {
                    exact,
                    approx,
                    kind: ItemKind::Added { node },
                }
            })
            .collect()
    }

    /// One statement; `false` after an error (reported in `out`).
    fn console_statement(&mut self, text: &str, offset: usize, out: &mut ConsoleOutput) -> bool {
        let settings = *self.settings();
        let popts = line_parse_options(text, &settings);
        let stmt = parse_statement_with(text, &popts).ok();
        // Scene nodes are never shadowed: redefine the node instead.
        if let Some(s) = &stmt
            && let StmtKind::Definition { lhs, .. } = &s.kind
            && let Some(name) = defined_name(lhs)
            && let Some(id) = self.scene().lookup(&name)
        {
            return self.redefine_node(id, &name, text, out);
        }
        let defs = self.defs_snapshot();
        let mut env = env_with(&defs);
        for (n, v) in &self.console.defs {
            if !self.evaluator().defines(n) {
                let _ = env.define(n, v.clone());
            }
        }
        let opts = self.evaluator().eval_options();
        let session = &mut self.console.session;
        session.env = env;
        session.opts = opts;
        session.parse = popts;
        let result = session.run(text);
        let globals: Vec<(String, Value)> = session
            .env
            .names()
            .filter_map(|n| session.env.get(n).map(|v| (n.to_string(), v.clone())))
            .collect();
        self.console.defs.extend(globals);
        match result {
            Ok(outcomes) => {
                for o in outcomes {
                    let item = match o {
                        Outcome::Value(v) => {
                            self.set_ans(&v);
                            value_item(&v, &settings, ItemKind::Value)
                        }
                        Outcome::Defined { name, value } => {
                            self.console.sources.insert(name.clone(), text.to_string());
                            value_item(&value, &settings, ItemKind::Definition { name })
                        }
                        Outcome::Relation(info) => ConsoleItem {
                            exact: String::new(),
                            approx: String::new(),
                            kind: ItemKind::Relation {
                                kind: relation_label(&info.kind).to_string(),
                            },
                        },
                    };
                    out.results.push(item);
                }
                true
            }
            Err(e) => {
                self.report(e, text, offset, stmt.as_ref(), out);
                false
            }
        }
    }

    fn report(
        &self,
        e: EvalError,
        text: &str,
        offset: usize,
        stmt: Option<&viscas_core::syntax::Stmt>,
        out: &mut ConsoleOutput,
    ) {
        if let ErrorKind::UnknownName(name) = &e.kind {
            out.suggestions = suggest(name, &candidate_names(self));
        }
        // Keep the span only if it points into this statement's own tree
        // (errors inside a called function's body carry spans into that
        // body's text).
        let own = match (&e.kind, stmt) {
            (ErrorKind::Parse, _) => true,
            (_, Some(s)) => {
                let mut spans = Vec::new();
                stmt_spans(s, &mut spans);
                spans.contains(&e.span)
            }
            (_, None) => e.span != Span::default(),
        };
        let span = (own && span_in(text, e.span)).then(|| shift(e.span, offset));
        out.error = Some(ConsoleError {
            message: e.message,
            span,
        });
    }

    fn redefine_node(
        &mut self,
        id: NodeId,
        name: &str,
        text: &str,
        out: &mut ConsoleOutput,
    ) -> bool {
        let content = match self.line_content(text) {
            Ok(c) => c,
            Err(errs) => {
                out.error = Some(ConsoleError {
                    message: errs[0].message.clone(),
                    span: None,
                });
                return false;
            }
        };
        if let Err(e) = self.scene_mut().set_kind(id, content.kind) {
            out.error = Some(ConsoleError {
                message: e.to_string(),
                span: None,
            });
            return false;
        }
        self.update();
        if let viscas_scene::Status::Error { message, .. } = self.scene().status(id) {
            out.error = Some(ConsoleError {
                message: format!("updated node `{name}`, which now has an error: {message}"),
                span: None,
            });
            return false;
        }
        let (exact, approx) = self
            .value(id)
            .map(|v| (v.exact.clone(), v.approx.clone()))
            .unwrap_or_default();
        out.results.push(ConsoleItem {
            exact,
            approx,
            kind: ItemKind::UpdatedNode {
                node: id,
                name: name.to_string(),
            },
        });
        true
    }

    fn set_ans(&mut self, v: &Value) {
        let defs = &mut self.console.defs;
        if let Some(prev) = defs.get("ans").cloned() {
            defs.insert("ans2".into(), prev);
        }
        defs.insert("ans".into(), v.clone());
    }

    fn push_history(&mut self, input: &str) {
        if input.is_empty() || self.console.history.last().is_some_and(|l| l == input) {
            return;
        }
        self.console.history.push(input.to_string());
        let n = self.console.history.len();
        if n > HISTORY_LIMIT {
            self.console.history.drain(..n - HISTORY_LIMIT);
        }
    }

    /// Console input history, oldest first (persist it with the app
    /// settings; restore with [`Engine::set_history`]).
    pub fn history(&self) -> &[String] {
        &self.console.history
    }

    pub fn set_history(&mut self, history: Vec<String>) {
        self.console.history = history;
        let n = self.console.history.len();
        if n > HISTORY_LIMIT {
            self.console.history.drain(..n - HISTORY_LIMIT);
        }
    }

    /// Active console definitions (not `ans`/`ans2`, not names a scene
    /// node currently provides), sorted.
    pub fn console_names(&self) -> Vec<String> {
        self.console
            .defs
            .keys()
            .filter(|n| *n != "ans" && *n != "ans2" && !self.evaluator().defines(n))
            .cloned()
            .collect()
    }

    /// `cmd` is the command text after `:`, found at byte `base` of the
    /// echoed input.
    fn console_command(&mut self, cmd: &str, base: usize, out: &mut ConsoleOutput) {
        let mut words = cmd.split_whitespace();
        let name = words.next().unwrap_or("");
        let args: Vec<&str> = words.collect();
        match name {
            "help" => self.cmd_help(&args, out),
            "mode" => self.cmd_mode(&args, out),
            "add" | "pin" | "plot" if args.is_empty() => self.cmd_pin(out),
            "plot" => {
                let rest = cmd.trim_start()["plot".len()..].trim();
                let at = base + (cmd.trim_end().len() - rest.len());
                self.eval_statements(rest, at, AutoAdd::ALL, out);
            }
            "add" | "pin" => {
                out.error = Some(ConsoleError {
                    message: format!(
                        "`:{name}` adds the last result; `:plot {}` evaluates and adds",
                        args.join(" ")
                    ),
                    span: None,
                });
            }
            "clear" => {
                let history = std::mem::take(&mut self.console.history);
                self.console = Default::default();
                self.console.history = history;
                out.cleared = true;
            }
            "vars" => self.cmd_vars(out),
            _ => {
                let cmds: Vec<String> = crate::help::COMMAND_HELP
                    .iter()
                    .map(|c| c.name[1..].to_string())
                    .collect();
                out.suggestions = suggest(name, &cmds)
                    .into_iter()
                    .map(|c| format!(":{c}"))
                    .collect();
                out.error = Some(ConsoleError {
                    message: format!("unknown command `:{name}` (try `:help`)"),
                    span: None,
                });
            }
        }
    }

    fn cmd_help(&self, args: &[&str], out: &mut ConsoleOutput) {
        let Some(&topic) = args.first() else {
            out.results.push(ConsoleItem::info(
                "Type an expression (`2^10`), a definition (`a := 3`, `f(x) := x^2`) or something to draw (`y = sin x`, `(1, 2)`). `:add` puts the last input on the board. Commands:",
            ));
            for c in crate::help::COMMAND_HELP {
                out.results.push(ConsoleItem::info(format!(
                    "{} — {}",
                    c.signature, c.summary
                )));
            }
            return;
        };
        if let Some(h) = command_help(topic) {
            out.results.push(ConsoleItem::info(format!(
                "{} — {}",
                h.signature, h.summary
            )));
            return;
        }
        if let Some(h) = function_help(topic) {
            let sig = if h.signature.is_empty() {
                topic
            } else {
                h.signature
            };
            let alias = if BUILTINS.iter().all(|b| b.name != topic) {
                format!(" (alias of `{}`)", sig.split('(').next().unwrap_or(sig))
            } else {
                String::new()
            };
            out.results
                .push(ConsoleItem::info(format!("{sig} — {}{alias}", h.summary)));
            return;
        }
        if let Some(h) = constant_help(topic) {
            out.results.push(ConsoleItem::info(format!(
                "{} — {}",
                h.signature, h.summary
            )));
            return;
        }
        let canon = canonical_name(topic);
        if let Some(id) = self.scene().lookup(canon) {
            let node = self.scene().get(id);
            let line = node
                .and_then(|n| definition_line_with(&n.content(), self.settings().parse_options()))
                .unwrap_or_else(|| topic.to_string());
            let value = self
                .value(id)
                .map(|v| format!(" = {}", v.display(self.settings().approx)))
                .unwrap_or_default();
            out.results
                .push(ConsoleItem::info(format!("scene node: {line}{value}")));
            return;
        }
        if let Some(v) = self.console.defs.get(canon) {
            out.results.push(ConsoleItem::info(format!(
                "console definition: {canon} = {}",
                format_value(v, &self.settings().exact_format())
            )));
            return;
        }
        out.suggestions = suggest(topic, &candidate_names(self));
        out.error = Some(ConsoleError {
            message: format!("no help for `{topic}`"),
            span: None,
        });
    }

    fn cmd_mode(&mut self, args: &[&str], out: &mut ConsoleOutput) {
        let mut s = *self.settings();
        if args.is_empty() {
            out.results
                .push(ConsoleItem::info(format!("mode: {}", s.mode_summary())));
            return;
        }
        for a in args {
            match *a {
                "exact" => s.approx = false,
                "approx" => s.approx = true,
                "deg" | "degrees" => s.angle = AngleUnit::Degrees,
                "rad" | "radians" => s.angle = AngleUnit::Radians,
                "real" => s.complex = false,
                "complex" => s.complex = true,
                other => {
                    let modes: Vec<String> = MODES.iter().map(|m| m.to_string()).collect();
                    out.suggestions = suggest(other, &modes);
                    out.error = Some(ConsoleError {
                        message: format!(
                            "unknown mode `{other}` (exact, approx, deg, rad, real, complex)"
                        ),
                        span: None,
                    });
                    return;
                }
            }
        }
        self.set_settings(s);
        out.results
            .push(ConsoleItem::info(format!("mode: {}", s.mode_summary())));
    }

    /// Console-only definitions `content` depends on, dependencies first.
    fn collect_pin_deps(
        &self,
        content: &NodeContent,
        seen: &mut HashSet<String>,
        plan: &mut Vec<NodeContent>,
    ) -> Result<(), String> {
        let analysis = analyze(content);
        for raw in analysis.dependency_names() {
            let name = canonical_name(raw).to_string();
            if self.evaluator().defines(&name)
                || self.scene().lookup(&name).is_some()
                || !seen.insert(name.clone())
            {
                continue;
            }
            match self.console.sources.get(&name) {
                Some(src) => {
                    let dep = self
                        .line_content(src)
                        .map_err(|e| format!("cannot add `{name}`: {}", e[0].message))?;
                    self.collect_pin_deps(&dep, seen, plan)?;
                    plan.push(dep);
                }
                None if self.console.defs.contains_key(&name) => {
                    return Err(format!(
                        "cannot add: `{name}` is a console value without a definition to add"
                    ));
                }
                None => {}
            }
        }
        Ok(())
    }

    /// The node contents `:pin` would add for the last input: the
    /// console-only definitions it depends on first, the input last.
    /// Computing it right after an evaluation lets a UI pin that result
    /// later (with [`Engine::apply_pin_plan`]) without re-evaluating.
    pub fn pin_plan(&self) -> Result<Vec<NodeContent>, String> {
        let line = self
            .console
            .last
            .clone()
            .ok_or_else(|| "nothing to add yet: evaluate something first".to_string())?;
        let content = self.line_content(&line).map_err(|e| e[0].message.clone())?;
        if let Some(name) = content
            .kind
            .defines_by_name()
            .then_some(())
            .and(content.name.as_ref())
            && self.scene().is_name_taken(name, None)
        {
            return Err(format!("a node named `{name}` already exists"));
        }
        let mut plan = Vec::new();
        let mut seen = HashSet::new();
        if let Some(n) = &content.name {
            seen.insert(canonical_name(n).to_string());
        }
        self.collect_pin_deps(&content, &mut seen, &mut plan)?;
        plan.push(content);
        Ok(plan)
    }

    /// Adds a [`Engine::pin_plan`] to the scene as one undo step and
    /// returns the new nodes. Dependencies whose name a scene node has
    /// taken since the plan was made are skipped (that node provides
    /// them now). All or nothing: on error the scene is unchanged.
    pub fn apply_pin_plan(&mut self, plan: Vec<NodeContent>) -> Result<Vec<NodeId>, String> {
        let last = plan.len().saturating_sub(1);
        let scene = self.scene_mut();
        scene.begin_group("Add to board from console");
        let mut ids = Vec::new();
        let mut error = None;
        for (i, c) in plan.into_iter().enumerate() {
            let provided = i < last
                && c.kind.defines_by_name()
                && c.name
                    .as_ref()
                    .is_some_and(|n| scene.is_name_taken(n, None));
            if provided {
                continue;
            }
            match scene.add(None, c) {
                Ok(id) => ids.push(id),
                Err(e) => {
                    error = Some(e.to_string());
                    break;
                }
            }
        }
        scene.end_group();
        if let Some(e) = error {
            if !ids.is_empty() {
                self.scene_mut().undo();
            }
            self.update();
            return Err(e);
        }
        for &id in &ids {
            self.assign_auto_color(id);
        }
        self.update();
        Ok(ids)
    }

    fn cmd_pin(&mut self, out: &mut ConsoleOutput) {
        let ids = match self.pin_plan().and_then(|plan| self.apply_pin_plan(plan)) {
            Ok(ids) => ids,
            Err(message) => {
                out.error = Some(ConsoleError {
                    message,
                    span: None,
                });
                return;
            }
        };
        let items = self.added_items(ids);
        out.results.extend(items);
        self.console.last = None;
    }

    fn cmd_vars(&self, out: &mut ConsoleOutput) {
        let settings = *self.settings();
        let mut scene_vars: Vec<(String, NodeId)> = self
            .scene()
            .iter()
            .filter_map(|n| n.defined_name().map(|name| (name.to_string(), n.id)))
            .collect();
        scene_vars.sort();
        for (name, id) in scene_vars {
            let (exact, approx) = self
                .value(id)
                .map(|v| (v.exact.clone(), v.approx.clone()))
                .unwrap_or_else(|| ("(error)".into(), String::new()));
            out.results.push(ConsoleItem {
                exact,
                approx,
                kind: ItemKind::Variable { name, scene: true },
            });
        }
        for name in self.console_names() {
            if let Some(v) = self.console.defs.get(&name) {
                let item = value_item(v, &settings, ItemKind::Variable { name, scene: false });
                out.results.push(item);
            }
        }
        if out.results.is_empty() {
            out.results.push(ConsoleItem::info("no definitions yet"));
        }
    }
}

fn defined_name(lhs: &viscas_core::syntax::Expr) -> Option<String> {
    match &*lhs.kind {
        ExprKind::Ident(n) => Some(canonical_name(n).to_string()),
        ExprKind::Call { callee, .. } => match &*callee.kind {
            ExprKind::Ident(n) => Some(canonical_name(n).to_string()),
            _ => None,
        },
        _ => None,
    }
}

fn value_item(v: &Value, s: &crate::EngineSettings, kind: ItemKind) -> ConsoleItem {
    ConsoleItem {
        exact: format_value(v, &s.exact_format()),
        approx: format_value(v, &s.approx_format()),
        kind,
    }
}

fn relation_label(k: &StatementKind) -> &'static str {
    match k {
        StatementKind::ExplicitY => "explicit y",
        StatementKind::ExplicitX => "explicit x",
        StatementKind::Polar => "polar",
        StatementKind::PolarAngle => "polar angle",
        StatementKind::PolarInequality { .. } => "polar inequality",
        StatementKind::Implicit => "implicit",
        StatementKind::Equation => "equation",
        StatementKind::Inequality { .. } => "inequality",
        StatementKind::Parametric { .. } => "parametric",
        _ => "relation",
    }
}

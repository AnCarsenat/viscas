//! The console: one-line evaluation sharing the scene's definitions, with
//! its own session locals, `ans`/`ans2`, commands and input history.

use std::sync::atomic::Ordering;

use viscas_core::eval::{
    AngleUnit, BUILTINS, ErrorKind, Outcome, Session, StatementKind, Value, canonical_name,
    format_value,
};
use viscas_core::syntax::Span;
use viscas_scene::{NodeId, definition_line_with};

use crate::complete::{candidate_names, suggest};
use crate::engine::Engine;
use crate::eval::env_with;
use crate::help::{command_help, constant_help, function_help};

/// Maximum number of remembered console inputs.
pub const HISTORY_LIMIT: usize = 1000;

/// What a console result line is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// The value of an expression.
    Value,
    /// A console definition (`a := 3`).
    Definition { name: String },
    /// A plot relation (`y = x^2`): no value; `:pin` turns it into a plot.
    Relation { kind: String },
    /// `:pin` created this node.
    Pinned { node: NodeId },
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

/// Console state kept by the engine.
#[derive(Debug, Default)]
pub(crate) struct ConsoleState {
    pub(crate) session: Session,
    history: Vec<String>,
    /// The last successful non-command input (for `:pin`).
    last: Option<String>,
}

impl ConsoleState {
    pub(crate) fn forget_pin(&mut self) {
        self.last = None;
    }

    pub(crate) fn session_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .session
            .env
            .names()
            .filter(|n| *n != "ans" && *n != "ans2")
            .map(String::from)
            .collect();
        v.sort();
        v
    }
}

const MODES: &[&str] = &["exact", "approx", "deg", "rad", "real", "complex"];

impl Engine {
    /// Evaluate one console line (expression, definition, relation, or a
    /// `:command`). Console definitions live in the console session and
    /// shadow scene nodes of the same name; scene nodes are visible by
    /// name.
    pub fn console_eval(&mut self, line: &str) -> ConsoleOutput {
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
            self.console_command(cmd, &mut out);
            return out;
        }
        self.interrupt_flag().store(false, Ordering::Relaxed);
        let settings = *self.settings();
        let defs = self.evaluator().defs().clone();
        let opts = self.evaluator().eval_options();
        let session = &mut self.console.session;
        session
            .env
            .set_resolver(env_with(&defs).resolver().cloned());
        session.opts = opts;
        session.parse = settings.parse_options();
        match session.run(input) {
            Ok(outcomes) => {
                for o in outcomes {
                    let item = match o {
                        Outcome::Value(v) => {
                            self.set_ans(&v);
                            value_item(&v, &settings, ItemKind::Value)
                        }
                        Outcome::Defined { name, value } => {
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
                self.console.last = Some(input.to_string());
            }
            Err(e) => {
                if let ErrorKind::UnknownName(name) = &e.kind {
                    out.suggestions = suggest(name, &candidate_names(self));
                }
                let span = (e.span.end <= input.len() && e.span != Span::default()
                    || e.kind == ErrorKind::Parse)
                    .then_some(e.span);
                out.error = Some(ConsoleError {
                    message: e.message,
                    span,
                });
            }
        }
        out
    }

    fn set_ans(&mut self, v: &Value) {
        let env = &mut self.console.session.env;
        if let Some(prev) = env.get("ans").cloned() {
            let _ = env.define("ans2", prev);
        }
        let _ = env.define("ans", v.clone());
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

    /// Names defined in the console session (not in the scene), sorted.
    pub fn console_names(&self) -> Vec<String> {
        self.console.session_names()
    }

    fn console_command(&mut self, cmd: &str, out: &mut ConsoleOutput) {
        let mut words = cmd.split_whitespace();
        let name = words.next().unwrap_or("");
        let args: Vec<&str> = words.collect();
        match name {
            "help" => self.cmd_help(&args, out),
            "mode" => self.cmd_mode(&args, out),
            "pin" => self.cmd_pin(out),
            "clear" => {
                self.console.session = Default::default();
                self.console.last = None;
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
                "Type an expression (`2^10`), a definition (`a := 3`, `f(x) := x^2`) or a relation (`y = sin x`). Commands:",
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
        if let Some(v) = self.console.session.env.get(canon) {
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

    fn cmd_pin(&mut self, out: &mut ConsoleOutput) {
        let Some(line) = self.console.last.clone() else {
            out.error = Some(ConsoleError {
                message: "nothing to pin yet: evaluate something first".into(),
                span: None,
            });
            return;
        };
        let content =
            match viscas_scene::parse_definition_with(&line, self.settings().parse_options()) {
                Ok(c) => c,
                Err(e) => {
                    out.error = Some(ConsoleError {
                        message: e
                            .first()
                            .map_or_else(|| "parse error".into(), |e| e.message.clone()),
                        span: None,
                    });
                    return;
                }
            };
        let defined = content
            .kind
            .defines_by_name()
            .then(|| content.name.clone())
            .flatten();
        if let Some(name) = &defined
            && self.scene().is_name_taken(name, None)
        {
            out.error = Some(ConsoleError {
                message: format!("a node named `{name}` already exists"),
                span: None,
            });
            return;
        }
        match self.add_node(None, content) {
            Ok(id) => {
                // The node now provides the name; drop the console shadow.
                if let Some(name) = &defined {
                    self.console.session.env.remove(name);
                }
                let shown = self
                    .value(id)
                    .map(|v| (v.exact.clone(), v.approx.clone()))
                    .unwrap_or_default();
                out.results.push(ConsoleItem {
                    exact: shown.0,
                    approx: shown.1,
                    kind: ItemKind::Pinned { node: id },
                });
                self.console.last = None;
            }
            Err(e) => {
                out.error = Some(ConsoleError {
                    message: e.to_string(),
                    span: None,
                });
            }
        }
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
        for name in self.console.session_names() {
            if let Some(v) = self.console.session.env.get(&name) {
                let item = value_item(v, &settings, ItemKind::Variable { name, scene: false });
                out.results.push(item);
            }
        }
        if out.results.is_empty() {
            out.results.push(ConsoleItem::info("no definitions yet"));
        }
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

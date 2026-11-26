//! Editor assistance for math input: name completion with rich items,
//! signature help, live diagnostics, hover docs, inline value preview
//! and "did you mean" suggestions.
//!
//! - [`Engine::completions`]: candidates for the identifier before the
//!   cursor (functions, constants, scene nodes with their value, console
//!   locals, and `:commands` in the console), each with a kind, signature,
//!   documentation, examples and a [`Snippet`] to insert. Filtering is a
//!   loose case-insensitive subsequence test; the UI ranks the items.
//! - [`Engine::signature_at`]: the call around the cursor with its
//!   signatures and the active parameter ([`signature`]).
//! - [`Engine::diagnostics`]: parse errors and unknown names with spans
//!   and suggestions ([`diagnose`]).
//! - [`Engine::describe`]: the documentation item of one name (hover).
//! - [`Engine::preview_task`]: a `Send` evaluation of a cheap pure input
//!   for a worker thread, with a step budget ([`preview`]).
//!
//! Function documentation comes from [`crate::help::catalog`].

mod diagnose;
mod preview;
mod signature;

pub use diagnose::{Diagnostic, Severity};
pub use preview::PreviewTask;
pub use signature::{SignatureHelp, call_at};

use viscas_core::eval::{ALIASES, BUILTINS, CONSTANTS, Function, PLOT_VARS, Value, format_value};
use viscas_scene::NodeKind;

use crate::engine::Engine;
use crate::help::{
    ASCII_ALIASES, COMMAND_HELP, FunctionDoc, Signature, catalog, constant_help, function_help,
};

/// What a completion inserts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CompletionKind {
    /// A scene node name.
    Node,
    /// A console-session definition.
    Console,
    Builtin,
    Constant,
    Command,
}

/// Where the input lives: commands are only offered in the console.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    Console,
    /// An expression row or inspector field.
    Expression,
}

/// Text to insert for a completion. `placeholders` are byte ranges of
/// `text` the user fills in turn (Tab jumps); `cursor` is where the caret
/// goes when there are none.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Snippet {
    pub text: String,
    pub cursor: usize,
    pub placeholders: Vec<(usize, usize)>,
}

impl Snippet {
    /// Plain text with the caret at its end.
    pub fn plain(text: &str) -> Snippet {
        Snippet {
            text: text.to_string(),
            cursor: text.len(),
            placeholders: Vec::new(),
        }
    }

    /// A call: `f()` with the caret inside for one parameter, `f(a, b)`
    /// with each parameter a placeholder for more.
    pub fn call(name: &str, params: &[String]) -> Snippet {
        let mut text = format!("{name}(");
        let mut placeholders = Vec::new();
        if params.len() > 1 {
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    text.push_str(", ");
                }
                placeholders.push((text.len(), text.len() + p.len()));
                text.push_str(p);
            }
        }
        let cursor = placeholders.first().map_or(text.len(), |p| p.0);
        text.push(')');
        Snippet {
            text,
            cursor,
            placeholders,
        }
    }

    /// Whether this inserts a call (`f(`…`)`).
    pub fn is_call(&self) -> bool {
        self.text.ends_with(')') && self.text.contains('(')
    }
}

/// One completion candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// The full name to insert in place of the typed fragment.
    pub text: String,
    pub kind: CompletionKind,
    /// Signature + summary for builtins, the value for nodes.
    pub detail: String,
    /// Main call form (`log(x, b)`), if a function.
    pub signature: Option<String>,
    /// Every call form, fewest parameters first.
    pub signatures: Vec<Signature>,
    /// Full documentation.
    pub doc: String,
    pub examples: Vec<String>,
    /// Grouping (`trigonometry`, `node`, `command`…).
    pub category: String,
    pub snippet: Snippet,
}

impl Completion {
    /// A candidate with no documentation beyond `detail`.
    pub fn new(text: &str, kind: CompletionKind, detail: &str) -> Completion {
        Completion {
            text: text.to_string(),
            kind,
            detail: detail.to_string(),
            signature: None,
            signatures: Vec::new(),
            doc: String::new(),
            examples: Vec::new(),
            category: String::new(),
            snippet: Snippet::plain(text),
        }
    }

    /// Whether accepting it inserts a call.
    pub fn is_function(&self) -> bool {
        self.snippet.is_call()
    }

    fn function(text: &str, kind: CompletionKind, f: &FunctionDoc) -> Completion {
        let full = f.full_signature().cloned();
        let mut summary = f
            .doc
            .split(". ")
            .next()
            .unwrap_or("")
            .trim_end_matches('.')
            .to_string();
        if let Some(first) = summary.get_mut(0..1) {
            first.make_ascii_lowercase();
        }
        let sig = full.as_ref().map(|s| s.label.clone()).unwrap_or_default();
        Completion {
            text: text.to_string(),
            kind,
            detail: format!("{sig} — {summary}"),
            signature: full.as_ref().map(|s| s.label.replacen(&f.name, text, 1)),
            signatures: f.signatures.clone(),
            doc: f.doc.clone(),
            examples: f.examples.clone(),
            category: f.category.clone(),
            snippet: Snippet::call(text, full.as_ref().map_or(&[][..], |s| &s.params)),
        }
    }
}

/// Candidates for the identifier being typed ([`Engine::completions`]).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CompletionSet {
    /// Byte offset of the fragment the accepted item replaces.
    pub start: usize,
    /// The typed fragment (`line[start..cursor]`).
    pub fragment: String,
    pub items: Vec<Completion>,
}

/// Byte offset where the identifier fragment before the end of `line`
/// starts (the part completion matches and the UI replaces). A leading
/// `:` command marker is included; leading digits are not (`2si` → `si`).
pub fn completion_start(line: &str) -> usize {
    let mut start = line.len();
    for (i, c) in line.char_indices().rev() {
        if c.is_alphanumeric() || c == '_' {
            start = i;
        } else {
            break;
        }
    }
    if line[..start].ends_with(':') && line[..start - 1].trim().is_empty() {
        return start - 1;
    }
    let digits = line[start..].bytes().take_while(u8::is_ascii_digit).count();
    start + digits
}

/// Whether `query`'s chars appear in `name` in order, case-insensitively,
/// with the first char matching `name`'s first char.
fn loose_match(query: &str, name: &str) -> bool {
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
    let mut q = query.chars().map(fold);
    let mut n = name.chars().map(fold);
    match (q.next(), n.next()) {
        (Some(a), Some(b)) if a == b => {}
        (None, _) => return true,
        _ => return false,
    }
    q.all(|qc| n.any(|nc| nc == qc))
}

impl Engine {
    /// Every name the input could use, as rich completion items (no
    /// filtering, no commands): scene nodes, console locals, builtins and
    /// aliases, constants.
    pub fn completion_candidates(&self) -> Vec<Completion> {
        let mut out: Vec<Completion> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut push = |c: Completion, out: &mut Vec<Completion>| {
            if seen.insert(c.text.clone()) {
                out.push(c);
            }
        };
        for n in self.scene().iter() {
            if let Some(name) = n.defined_name() {
                push(self.node_item(n, name), &mut out);
            }
        }
        let defs = self.console.defs();
        for name in self.console_names() {
            if let Some(v) = defs.get(&name) {
                push(self.value_item(&name, v), &mut out);
            }
        }
        for f in catalog().functions() {
            push(
                Completion::function(&f.name, CompletionKind::Builtin, &f),
                &mut out,
            );
        }
        for b in BUILTINS {
            // Builtins the catalog does not list.
            let detail = function_help(b.name)
                .map(|h| format!("{} — {}", h.signature, h.summary))
                .unwrap_or_default();
            let mut c = Completion::new(b.name, CompletionKind::Builtin, &detail);
            c.snippet = Snippet::call(b.name, &[]);
            push(c, &mut out);
        }
        for (alias, target) in ALIASES {
            if let Some(f) = catalog().function(target) {
                let mut c = Completion::function(alias, CompletionKind::Builtin, &f);
                c.detail = format!("alias of {target}");
                push(c, &mut out);
            }
        }
        for c in CONSTANTS.iter().chain(ASCII_ALIASES) {
            push(constant_item(c), &mut out);
        }
        out
    }

    fn node_item(&self, n: &viscas_scene::Node, name: &str) -> Completion {
        let value = self
            .value(n.id)
            .map(|v| v.display(self.settings().approx).to_string());
        let tag = n.kind.tag().to_string();
        let mut c = Completion::new(name, CompletionKind::Node, "");
        c.category = "node".into();
        if let NodeKind::Function { params, body } = &n.kind {
            let sig = Signature::from_params(name, params);
            c.detail = format!("{} = {body}", sig.label);
            c.doc = format!("Scene function `{}`.", sig.label);
            c.snippet = Snippet::call(name, params);
            c.signature = Some(sig.label.clone());
            c.signatures = vec![sig];
        } else {
            c.detail = value.clone().unwrap_or_else(|| tag.clone());
            c.doc = match value {
                Some(v) => format!("Scene {tag} `{name}` = {v}"),
                None => format!("Scene {tag} `{name}`"),
            };
        }
        c
    }

    fn value_item(&self, name: &str, v: &Value) -> Completion {
        let mut c = Completion::new(name, CompletionKind::Console, "");
        c.category = "console".into();
        if let Value::Function(f) = v
            && let Function::User(u) = &**f
        {
            let sig = Signature::from_params(name, &u.params);
            c.detail = sig.label.clone();
            c.doc = format!("Console function `{}`.", sig.label);
            c.snippet = Snippet::call(name, &u.params);
            c.signature = Some(sig.label.clone());
            c.signatures = vec![sig];
        } else {
            let shown = format_value(v, &self.settings().exact_format());
            c.detail = shown.clone();
            c.doc = format!("Console local `{name}` = {shown}");
        }
        c
    }

    /// Candidates for the identifier before byte `cursor` of `line`.
    /// Commands are offered for `:frag` at the start of a console line,
    /// and names after `:help `. Order: kind (nodes, console, builtins,
    /// constants), then shorter names; the UI re-ranks with its fuzzy
    /// matcher.
    pub fn completions(&self, line: &str, cursor: usize, scope: Scope) -> CompletionSet {
        let mut cursor = cursor.min(line.len());
        while !line.is_char_boundary(cursor) {
            cursor -= 1;
        }
        let before = &line[..cursor];
        let start = completion_start(before);
        let fragment = before[start..].to_string();
        let mut set = CompletionSet {
            start,
            fragment: fragment.clone(),
            items: Vec::new(),
        };
        if let Some(cmd) = fragment.strip_prefix(':') {
            if scope == Scope::Console {
                set.items = COMMAND_HELP
                    .iter()
                    .filter(|c| c.name[1..].starts_with(cmd))
                    .map(command_item)
                    .collect();
            }
            return set;
        }
        if fragment.is_empty() || in_string(before) {
            return set;
        }
        let mut items: Vec<Completion> = self
            .completion_candidates()
            .into_iter()
            .filter(|c| loose_match(&fragment, &c.text))
            .collect();
        if scope == Scope::Console && before.trim_start().starts_with(":help") {
            items.extend(
                COMMAND_HELP
                    .iter()
                    .filter(|c| loose_match(&fragment, &c.name[1..]))
                    .map(|c| {
                        let mut item = command_item(c);
                        item.text = c.name[1..].to_string();
                        item.snippet = Snippet::plain(&item.text);
                        item
                    }),
            );
        }
        items.sort_by(|a, b| (a.kind, a.text.len(), &a.text).cmp(&(b.kind, b.text.len(), &b.text)));
        set.items = items;
        set
    }

    /// Candidates for the identifier fragment at the end of `prefix` (pass
    /// the input up to the cursor), prefix matches only. Commands are
    /// offered for `:frag` at the start of a line. Order: kind (nodes,
    /// console, builtins, constants), then shorter names.
    pub fn complete(&self, prefix: &str) -> Vec<Completion> {
        let set = self.completions(prefix, prefix.len(), Scope::Console);
        set.items
            .into_iter()
            .filter(|c| c.text.starts_with(&set.fragment))
            .collect()
    }

    /// The documentation item for `name` exactly (hover docs): a node,
    /// console local, function, constant or `:command`.
    pub fn describe(&self, name: &str) -> Option<Completion> {
        if name.starts_with(':') {
            return COMMAND_HELP
                .iter()
                .find(|c| c.name == name)
                .map(command_item);
        }
        self.completion_candidates()
            .into_iter()
            .find(|c| c.text == name)
            .or_else(|| {
                PLOT_VARS.contains(&name).then(|| {
                    let mut c = Completion::new(name, CompletionKind::Constant, "plot variable");
                    c.doc =
                        format!("`{name}` is a plot variable: relations using it draw a curve.");
                    c
                })
            })
    }
}

fn constant_item(name: &str) -> Completion {
    let summary = constant_help(name)
        .map(|h| h.summary.to_string())
        .unwrap_or_else(|| "plot variable".into());
    let mut c = Completion::new(name, CompletionKind::Constant, &summary);
    c.category = "constant".into();
    let mut doc = summary;
    if let Some(first) = doc.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    c.doc = doc;
    c
}

fn command_item(c: &crate::help::HelpEntry) -> Completion {
    let mut item = Completion::new(
        c.name,
        CompletionKind::Command,
        &format!("{} — {}", c.signature, c.summary),
    );
    item.category = "command".into();
    item.signature = Some(c.signature.to_string());
    item.doc = c.summary.to_string();
    if c.signature.trim() != c.name {
        item.snippet = Snippet::plain(&format!("{} ", c.name));
    }
    item
}

/// Whether the end of `text` is inside a `"…"` string literal.
fn in_string(text: &str) -> bool {
    text.chars().filter(|c| *c == '"').count() % 2 == 1
}

/// Every name the console could resolve (for suggestions).
pub(crate) fn candidate_names(engine: &Engine) -> Vec<String> {
    let mut v: Vec<String> = engine
        .scene()
        .iter()
        .filter_map(|n| n.defined_name().map(String::from))
        .collect();
    v.extend(engine.console_names());
    v.extend(BUILTINS.iter().map(|b| b.name.to_string()));
    v.extend(ALIASES.iter().map(|(a, _)| a.to_string()));
    v.extend(CONSTANTS.iter().map(|c| c.to_string()));
    v.extend(ASCII_ALIASES.iter().map(|c| c.to_string()));
    v
}

/// Up to three candidates within a small edit distance of `name`
/// (closest first).
pub fn suggest(name: &str, candidates: &[String]) -> Vec<String> {
    let max = match name.chars().count() {
        0 => return Vec::new(),
        1..=3 => 1,
        _ => 2,
    };
    let mut scored: Vec<(usize, &String)> = candidates
        .iter()
        .filter(|c| c.as_str() != name)
        .map(|c| (levenshtein(name, c), c))
        .filter(|(d, _)| *d <= max)
        .collect();
    scored.sort();
    scored.dedup_by(|a, b| a.1 == b.1);
    scored.into_iter().take(3).map(|(_, c)| c.clone()).collect()
}

/// Optimal string alignment distance (edits plus adjacent transpositions).
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let w = b.len() + 1;
    let mut d = vec![0usize; (a.len() + 1) * w];
    for i in 0..=a.len() {
        d[i * w] = i;
    }
    for (j, cell) in d.iter_mut().enumerate().take(w) {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[(i - 1) * w + j - 1] + cost)
                .min(d[(i - 1) * w + j] + 1)
                .min(d[i * w + j - 1] + 1);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[(i - 2) * w + j - 2] + 1);
            }
            d[i * w + j] = v;
        }
    }
    d[a.len() * w + b.len()]
}

//! Name completion and "did you mean" suggestions.

use viscas_core::eval::{ALIASES, BUILTINS, CONSTANTS};

use crate::engine::Engine;
use crate::help::{ASCII_ALIASES, COMMAND_HELP, constant_help, function_help};

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

/// One completion candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// The full name to insert in place of the typed fragment.
    pub text: String,
    pub kind: CompletionKind,
    /// Signature + summary for builtins, the value for nodes.
    pub detail: String,
}

/// Byte offset where the identifier fragment before the end of `line`
/// starts (the part [`Engine::complete`] matches and the UI replaces). A
/// leading `:` command marker is included.
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
        start -= 1;
    }
    start
}

impl Engine {
    /// Candidates for the identifier fragment at the end of `prefix` (pass
    /// the input up to the cursor). Commands are offered for `:frag` at the
    /// start of a line. Order: exact-case prefix matches first, then by
    /// kind (nodes, console, builtins, constants), then shorter names.
    pub fn complete(&self, prefix: &str) -> Vec<Completion> {
        let frag = &prefix[completion_start(prefix)..];
        if let Some(cmd) = frag.strip_prefix(':') {
            return COMMAND_HELP
                .iter()
                .filter(|c| c.name[1..].starts_with(cmd))
                .map(|c| Completion {
                    text: c.name.to_string(),
                    kind: CompletionKind::Command,
                    detail: format!("{} — {}", c.signature, c.summary),
                })
                .collect();
        }
        if frag.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<Completion> = Vec::new();
        let mut push = |text: &str, kind, detail: String| {
            if text.starts_with(frag) && !out.iter().any(|c| c.text == text) {
                out.push(Completion {
                    text: text.to_string(),
                    kind,
                    detail,
                });
            }
        };
        for n in self.scene().iter() {
            if let Some(name) = n.defined_name() {
                let detail = self
                    .value(n.id)
                    .map(|v| v.display(self.settings().approx).to_string())
                    .unwrap_or_else(|| n.kind.tag().to_string());
                push(name, CompletionKind::Node, detail);
            }
        }
        for name in self.console_names() {
            push(&name, CompletionKind::Console, "console".into());
        }
        for b in BUILTINS {
            let d = function_help(b.name)
                .map(|h| format!("{} — {}", h.signature, h.summary))
                .unwrap_or_default();
            push(b.name, CompletionKind::Builtin, d);
        }
        for (alias, target) in ALIASES {
            push(alias, CompletionKind::Builtin, format!("alias of {target}"));
        }
        for c in CONSTANTS.iter().chain(ASCII_ALIASES) {
            let d = constant_help(c)
                .map(|h| h.summary.to_string())
                .unwrap_or_else(|| "plot variable".into());
            push(c, CompletionKind::Constant, d);
        }
        out.sort_by(|a, b| (a.kind, a.text.len(), &a.text).cmp(&(b.kind, b.text.len(), &b.text)));
        out
    }
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

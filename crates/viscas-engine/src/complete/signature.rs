//! Signature help: the call the cursor is in and its active parameter.

use viscas_core::eval::{Function, Value};
use viscas_scene::NodeKind;

use crate::engine::Engine;
use crate::help::{Signature, catalog};

/// The innermost call around a cursor ([`call_at`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub name: String,
    /// Byte offset of the name.
    pub name_start: usize,
    /// Byte offset of the `(`.
    pub open: usize,
    /// 0-based index of the argument the cursor is in.
    pub arg: usize,
}

/// Finds the unclosed `name(` before byte `cursor` of `line` and the
/// argument the cursor is in. Brackets and braces nest (`f([1, 2], |)`
/// is argument 1 of `f`); a cursor inside a non-call bracket looks
/// further out.
pub fn call_at(line: &str, cursor: usize) -> Option<Call> {
    let mut cursor = cursor.min(line.len());
    while !line.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let before = &line[..cursor];
    let mut depth = 0usize;
    let mut commas = 0usize;
    let mut in_str = before.chars().filter(|c| *c == '"').count() % 2 == 1;
    for (i, c) in before.char_indices().rev() {
        if c == '"' {
            in_str = !in_str;
            continue;
        }
        if in_str {
            continue;
        }
        match c {
            ')' | ']' | '}' => depth += 1,
            '(' | '[' | '{' if depth > 0 => depth -= 1,
            '(' => {
                let head = before[..i].trim_end();
                let name_start = head
                    .char_indices()
                    .rev()
                    .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
                    .last()
                    .map(|(j, _)| j);
                if let Some(s) = name_start {
                    let name = &head[s..];
                    if !name.starts_with(|c: char| c.is_ascii_digit()) {
                        return Some(Call {
                            name: name.to_string(),
                            name_start: s,
                            open: i,
                            arg: commas,
                        });
                    }
                }
                commas = 0;
            }
            '[' | '{' => commas = 0,
            ',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    None
}

/// What the signature hint shows ([`Engine::signature_at`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelp {
    pub name: String,
    /// Call forms, fewest parameters first.
    pub signatures: Vec<Signature>,
    /// The form that fits the argument count so far.
    pub active_signature: usize,
    /// The parameter the cursor is on, if the form has that many.
    pub active_param: Option<usize>,
    /// Short documentation.
    pub doc: String,
    /// Byte offset of the call's `(`.
    pub open: usize,
}

impl SignatureHelp {
    pub fn active(&self) -> &Signature {
        &self.signatures[self.active_signature]
    }
}

impl Engine {
    /// The signatures of the function called around byte `cursor` of
    /// `line`: scene functions (their parameters), console functions and
    /// catalog functions. `None` outside a call or for unknown names.
    pub fn signature_at(&self, line: &str, cursor: usize) -> Option<SignatureHelp> {
        let call = call_at(line, cursor)?;
        let (signatures, doc) = self.signatures_of(&call.name)?;
        if signatures.is_empty() {
            return None;
        }
        let active_signature = signatures
            .iter()
            .position(|s| s.param_for(call.arg).is_some())
            .unwrap_or(signatures.len() - 1);
        let active_param = signatures[active_signature].param_for(call.arg);
        Some(SignatureHelp {
            name: call.name,
            signatures,
            active_signature,
            active_param,
            doc,
            open: call.open,
        })
    }

    fn signatures_of(&self, name: &str) -> Option<(Vec<Signature>, String)> {
        if let Some(n) = self.scene().iter().find(|n| n.defined_name() == Some(name)) {
            return match &n.kind {
                NodeKind::Function { params, body } => Some((
                    vec![Signature::from_params(name, params)],
                    format!("Scene function = {body}"),
                )),
                _ => None,
            };
        }
        if let Some(Value::Function(f)) = self.console.defs().get(name)
            && let Function::User(u) = &**f
        {
            return Some((
                vec![Signature::from_params(name, &u.params)],
                "Console function".to_string(),
            ));
        }
        let f = catalog().function(name)?;
        let signatures = f
            .signatures
            .iter()
            .map(|s| Signature {
                label: s.label.replacen(&f.name, name, 1),
                ..s.clone()
            })
            .collect();
        Some((signatures, f.doc))
    }
}

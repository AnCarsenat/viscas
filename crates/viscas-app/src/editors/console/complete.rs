//! Completion and suggestion insertion math (byte and char offsets).
//! Pure, no GTK: GTK text buffers count chars, the engine counts bytes.

use viscas_core::syntax::Span;
use viscas_engine::{Completion, CompletionKind, completion_start};

/// Byte offset of char offset `chars` in `text` (clamped).
pub fn char_to_byte(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(b, _)| b)
}

/// Char offset of byte offset `byte` in `text` (clamped to a boundary).
pub fn byte_to_char(text: &str, byte: usize) -> usize {
    let mut b = byte.min(text.len());
    while !text.is_char_boundary(b) {
        b -= 1;
    }
    text[..b].chars().count()
}

/// Text appended after an accepted completion: `(` for functions (unless
/// one follows already), a space for commands that take arguments.
pub fn suffix(c: &Completion, rest: &str) -> &'static str {
    match c.kind {
        CompletionKind::Builtin if !rest.starts_with('(') => "(",
        CompletionKind::Command if command_takes_args(c) && !rest.starts_with(' ') => " ",
        _ => "",
    }
}

/// `:help f` / `:mode …` take arguments; `:pin` does not. The detail is
/// `"<signature> — <summary>"`.
fn command_takes_args(c: &Completion) -> bool {
    let sig = c.detail.split(" — ").next().unwrap_or("");
    sig.trim() != c.text
}

/// Accepts `c` for the input `text` with the cursor at byte `cursor`:
/// replaces the fragment before the cursor. Returns the new text and the
/// new cursor (bytes).
pub fn apply(text: &str, cursor: usize, c: &Completion) -> (String, usize) {
    let cursor = cursor.min(text.len());
    let start = completion_start(&text[..cursor]);
    let rest = &text[cursor..];
    let suffix = suffix(c, rest);
    let mut out = String::with_capacity(text.len() + c.text.len() + 1);
    out.push_str(&text[..start]);
    out.push_str(&c.text);
    out.push_str(suffix);
    let new_cursor = out.len();
    out.push_str(rest);
    (out, new_cursor)
}

/// Longest common prefix of every candidate's text.
pub fn common_prefix(items: &[Completion]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut len = first.text.len();
    for c in &items[1..] {
        len = first
            .text
            .char_indices()
            .zip(c.text.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map_or(0, |((i, a), _)| i + a.len_utf8())
            .min(len);
    }
    first.text[..len].to_owned()
}

/// Inserts the common prefix of `items` when it extends the fragment
/// before the cursor; `None` when it would not change anything.
pub fn extend_to_common(
    text: &str,
    cursor: usize,
    items: &[Completion],
) -> Option<(String, usize)> {
    let cursor = cursor.min(text.len());
    let start = completion_start(&text[..cursor]);
    let prefix = common_prefix(items);
    if prefix.len() <= cursor - start || !prefix.starts_with(&text[start..cursor]) {
        return None;
    }
    let mut out = String::from(&text[..start]);
    out.push_str(&prefix);
    let new_cursor = out.len();
    out.push_str(&text[cursor..]);
    Some((out, new_cursor))
}

/// The input after picking the "did you mean" `suggestion` for a failed
/// `input`: it replaces the error span when there is one, else the
/// leading command word (`:hlep` → `:help`), else the word most like it.
pub fn apply_suggestion(input: &str, span: Option<Span>, suggestion: &str) -> String {
    if let Some(span) = span {
        let (start, end) = super::render::clamp_span(input, span);
        if start < end {
            return format!("{}{suggestion}{}", &input[..start], &input[end..]);
        }
    }
    if suggestion.starts_with(':') && input.starts_with(':') {
        let end = input.find(char::is_whitespace).unwrap_or(input.len());
        return format!("{suggestion}{}", &input[end..]);
    }
    if input.starts_with(':') {
        // `:help sni`, `:mode dge`: replace the last word.
        let start = input.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        return format!("{}{suggestion}", &input[..start]);
    }
    // A word sharing the suggestion's first letter, else append.
    let mut best: Option<(usize, usize)> = None;
    let mut i = 0;
    for word in input.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if !word.is_empty()
            && word.chars().next() == suggestion.chars().next()
            && word != suggestion
            && best.is_none()
        {
            best = Some((i, i + word.len()));
        }
        i += word.len() + 1;
    }
    match best {
        Some((s, e)) if e <= input.len() => format!("{}{suggestion}{}", &input[..s], &input[e..]),
        _ => format!("{input}{suggestion}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(text: &str, kind: CompletionKind, detail: &str) -> Completion {
        Completion {
            text: text.into(),
            kind,
            detail: detail.into(),
        }
    }

    #[test]
    fn offsets_convert_both_ways() {
        let t = "θ + πx";
        assert_eq!(char_to_byte(t, 0), 0);
        assert_eq!(char_to_byte(t, 1), 2);
        assert_eq!(char_to_byte(t, 99), t.len());
        assert_eq!(byte_to_char(t, 2), 1);
        assert_eq!(byte_to_char(t, 1), 0, "snapped to a boundary");
        assert_eq!(byte_to_char(t, 99), 6);
    }

    #[test]
    fn function_completion_adds_paren() {
        let sin = c("sin", CompletionKind::Builtin, "sin(x) — sine");
        assert_eq!(apply("2 + si", 6, &sin), ("2 + sin(".into(), 8));
        // Cursor in the middle: only the fragment before it is replaced.
        assert_eq!(apply("si + 1", 2, &sin), ("sin( + 1".into(), 4));
        // A parenthesis already follows.
        assert_eq!(apply("si(x)", 2, &sin), ("sin(x)".into(), 3));
    }

    #[test]
    fn names_and_constants_insert_plainly() {
        let node = c("alpha", CompletionKind::Node, "3");
        assert_eq!(apply("2*alp", 5, &node), ("2*alpha".into(), 7));
        let pi = c("pi", CompletionKind::Constant, "π");
        assert_eq!(apply("p", 1, &pi), ("pi".into(), 2));
        // Unicode before the fragment.
        assert_eq!(apply("θ + al", 7, &node), ("θ + alpha".into(), 10));
    }

    #[test]
    fn commands_get_a_space_when_they_take_args() {
        let help = c(":help", CompletionKind::Command, ":help f — help on a name");
        let pin = c(
            ":pin",
            CompletionKind::Command,
            ":pin — pin the last result",
        );
        assert_eq!(apply(":he", 3, &help), (":help ".into(), 6));
        assert_eq!(apply(":p", 2, &pin), (":pin".into(), 4));
    }

    #[test]
    fn common_prefix_extension() {
        let items = vec![
            c("sinh", CompletionKind::Builtin, ""),
            c("sinc", CompletionKind::Builtin, ""),
            c("sin", CompletionKind::Builtin, ""),
        ];
        assert_eq!(common_prefix(&items), "sin");
        assert_eq!(
            extend_to_common("2*s", 3, &items),
            Some(("2*sin".into(), 5))
        );
        assert_eq!(extend_to_common("2*sin", 5, &items), None);
        assert_eq!(common_prefix(&[]), "");
        let uni = vec![
            c("θa", CompletionKind::Node, ""),
            c("θb", CompletionKind::Node, ""),
        ];
        assert_eq!(common_prefix(&uni), "θ");
    }

    #[test]
    fn suggestions_replace_the_right_text() {
        let span = Some(Span { start: 4, end: 7 });
        assert_eq!(apply_suggestion("1 + sni(2)", span, "sin"), "1 + sin(2)");
        assert_eq!(apply_suggestion(":hlep sin", None, ":help"), ":help sin");
        assert_eq!(apply_suggestion(":mode dge", None, "deg"), ":mode deg");
        assert_eq!(apply_suggestion("2 + sni(1)", None, "sin"), "2 + sin(1)");
        assert_eq!(apply_suggestion("2 + ", None, "sin"), "2 + sin");
    }
}

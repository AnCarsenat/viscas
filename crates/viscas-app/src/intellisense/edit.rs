//! Editing helpers of math input (pure, char offsets): bracket
//! auto-pairing, smart backspace, snippet placeholders and completion
//! insertion.

use viscas_engine::{Completion, Snippet};

/// A text change: replace chars `start..end` with `text`, then put the
/// caret at `caret` or select `select` (char offsets in the new text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub caret: usize,
    pub select: Option<(usize, usize)>,
}

fn closer(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

fn is_closer(c: char) -> bool {
    matches!(c, ')' | ']' | '}')
}

/// Whether a pair may open before `next`: at the end, before blanks,
/// closers, separators and operators — not glued to a word or number.
fn pair_allowed_before(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(c) => {
            c.is_whitespace()
                || is_closer(c)
                || matches!(
                    c,
                    ',' | ';' | '+' | '-' | '*' | '/' | '^' | '=' | '<' | '>' | '|'
                )
        }
    }
}

fn balanced(chars: &[char], open: char, close: char) -> bool {
    let o = chars.iter().filter(|c| **c == open).count();
    let c = chars.iter().filter(|c| **c == close).count();
    o == c
}

/// What typing `ch` does with auto-pairing, or `None` to type it plainly:
///
/// - `(` `[` `{` insert the pair with the caret inside (or wrap the
///   selection), unless glued to the next word;
/// - a closer right before the same closer steps over it when the
///   brackets are balanced;
/// - `|` opens a `||` pair where an operand is expected, steps over a
///   closing bar, and is plain after an operand (`|x|`).
pub fn on_char(text: &str, caret: usize, sel: Option<(usize, usize)>, ch: char) -> Option<Edit> {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    if let Some((s, e)) = sel.filter(|(s, e)| s != e) {
        let (s, e) = (s.min(e), s.max(e).min(chars.len()));
        let close = closer(ch).or((ch == '|').then_some('|'))?;
        let inner: String = chars[s..e].iter().collect();
        let n = e - s;
        return Some(Edit {
            start: s,
            end: e,
            text: format!("{ch}{inner}{close}"),
            caret: s + n + 1,
            select: Some((s + 1, s + 1 + n)),
        });
    }
    let next = chars.get(caret).copied();
    let prev = caret.checked_sub(1).and_then(|i| chars.get(i)).copied();
    let step_over = || Edit {
        start: caret,
        end: caret,
        text: String::new(),
        caret: caret + 1,
        select: None,
    };
    if let Some(close) = closer(ch) {
        if !pair_allowed_before(next) {
            return None;
        }
        return Some(Edit {
            start: caret,
            end: caret,
            text: format!("{ch}{close}"),
            caret: caret + 1,
            select: None,
        });
    }
    if is_closer(ch) {
        let open = match ch {
            ')' => '(',
            ']' => '[',
            _ => '{',
        };
        return (next == Some(ch) && balanced(&chars, open, ch)).then(step_over);
    }
    if ch == '|' {
        let bars = chars.iter().filter(|c| **c == '|').count();
        if next == Some('|') && bars % 2 == 0 {
            return Some(step_over());
        }
        let operand_before =
            prev.is_some_and(|p| p.is_alphanumeric() || is_closer(p) || matches!(p, '!' | '\''));
        if operand_before || !pair_allowed_before(next) {
            return None;
        }
        return Some(Edit {
            start: caret,
            end: caret,
            text: "||".into(),
            caret: caret + 1,
            select: None,
        });
    }
    None
}

/// Smart backspace: deletes an empty pair (`(|)`, `[|]`, `{|}`, `||`)
/// at once; `None` lets the key delete one char.
pub fn on_backspace(text: &str, caret: usize, sel: Option<(usize, usize)>) -> Option<Edit> {
    if sel.is_some_and(|(s, e)| s != e) || caret == 0 {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let prev = *chars.get(caret - 1)?;
    let next = *chars.get(caret)?;
    let pair = closer(prev) == Some(next) || (prev == '|' && next == '|');
    pair.then(|| Edit {
        start: caret - 1,
        end: caret + 1,
        text: String::new(),
        caret: caret - 1,
        select: None,
    })
}

/// The edit accepting `c` makes when the fragment spans chars
/// `start..caret` of `text`. Functions get their call snippet unless a
/// `(` follows already; `Some` placeholders come back as char ranges in
/// the new text.
pub fn accept(
    text: &str,
    start: usize,
    caret: usize,
    c: &Completion,
) -> (Edit, Vec<(usize, usize)>) {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    let start = start.min(caret);
    // Replace the rest of the identifier too (`si|n` + `sinh` → `sinh`).
    let mut end = caret;
    while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
        end += 1;
    }
    let paren_follows = chars[end..]
        .iter()
        .find(|c| !c.is_whitespace())
        .is_some_and(|c| *c == '(');
    let snippet = if c.is_function() && paren_follows {
        Snippet::plain(&c.text)
    } else {
        c.snippet.clone()
    };
    let to_chars = |byte: usize| snippet.text[..byte].chars().count();
    let holes: Vec<(usize, usize)> = snippet
        .placeholders
        .iter()
        .map(|(s, e)| (start + to_chars(*s), start + to_chars(*e)))
        .collect();
    let select = holes.first().copied();
    let edit = Edit {
        start,
        end,
        text: snippet.text.clone(),
        caret: select.map_or(start + to_chars(snippet.cursor), |s| s.1),
        select,
    };
    (edit, holes)
}

/// Placeholder navigation after inserting a snippet: the stops shift
/// with edits so Tab still finds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetSession {
    /// Placeholder char ranges, in order.
    pub stops: Vec<(usize, usize)>,
    /// Index of the placeholder being filled.
    pub current: usize,
    /// Char offset just after the snippet (`)`), where the last Tab goes.
    pub end: usize,
    /// Text length (chars) the stops refer to.
    pub len: usize,
}

impl SnippetSession {
    pub fn new(stops: Vec<(usize, usize)>, end: usize, len: usize) -> Option<Self> {
        (stops.len() > 1).then_some(SnippetSession {
            stops,
            current: 0,
            end,
            len,
        })
    }

    /// Follows a text change: the text is now `len` chars with the caret
    /// at `caret`. Insertions extend the placeholder they touch and shift
    /// later ones; deletions collapse what they removed. Returns `false`
    /// when the session no longer makes sense (the snippet was deleted).
    pub fn update(&mut self, len: usize, caret: usize) -> bool {
        if len >= self.len {
            let n = len - self.len;
            let p = caret.saturating_sub(n);
            for (s, e) in &mut self.stops {
                if *s <= p && p <= *e {
                    *e += n;
                } else if *s > p {
                    *s += n;
                    *e += n;
                }
            }
            if self.end >= p {
                self.end += n;
            }
        } else {
            let n = self.len - len;
            let p = caret;
            let map = |q: usize| {
                if q <= p {
                    q
                } else if q >= p + n {
                    q - n
                } else {
                    p
                }
            };
            for (s, e) in &mut self.stops {
                *s = map(*s);
                *e = map(*e);
            }
            self.end = map(self.end);
        }
        self.len = len;
        let first = self.stops.first().map_or(0, |s| s.0);
        self.end <= len && first <= self.end
    }

    /// Moves to the next placeholder: its range, or `None` when past the
    /// last one (the caret then goes to [`SnippetSession::end`]).
    pub fn next(&mut self) -> Option<(usize, usize)> {
        if self.current + 1 < self.stops.len() {
            self.current += 1;
            Some(self.stops[self.current])
        } else {
            None
        }
    }

    /// Moves to the previous placeholder (stays on the first).
    pub fn prev(&mut self) -> (usize, usize) {
        self.current = self.current.saturating_sub(1);
        self.stops[self.current]
    }

    /// Whether `caret` is still inside the snippet.
    pub fn contains(&self, caret: usize) -> bool {
        let first = self.stops.first().map_or(0, |s| s.0);
        (first.saturating_sub(1)..=self.end).contains(&caret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viscas_engine::CompletionKind;

    fn apply(text: &str, e: &Edit) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out: String = chars[..e.start].iter().collect();
        out.push_str(&e.text);
        out.extend(&chars[e.end..]);
        out
    }

    #[test]
    fn brackets_pair_up() {
        let e = on_char("2 + ", 4, None, '(').unwrap();
        assert_eq!(apply("2 + ", &e), "2 + ()");
        assert_eq!(e.caret, 5);
        let e = on_char("f()", 2, None, '[').unwrap();
        assert_eq!(apply("f()", &e), "f([])");
        // Glued to a word: plain.
        assert!(on_char("ab", 0, None, '(').is_none());
        // Before a closer or operator: paired.
        assert!(on_char("f()", 2, None, '(').is_some());
        assert!(on_char("a+b", 1, None, '{').is_some());
    }

    #[test]
    fn closers_step_over_balanced_pairs() {
        let e = on_char("sin(x)", 5, None, ')').unwrap();
        assert_eq!(e.text, "");
        assert_eq!(e.caret, 6);
        // Unbalanced: the closer is typed.
        assert!(on_char("x)", 1, None, ')').is_none());
        assert!(on_char("sin(x", 5, None, ')').is_none());
    }

    #[test]
    fn selection_is_wrapped() {
        let e = on_char("2x + 1", 0, Some((0, 2)), '(').unwrap();
        assert_eq!(apply("2x + 1", &e), "(2x) + 1");
        assert_eq!(e.select, Some((1, 3)));
        let e = on_char("x", 0, Some((0, 1)), '|').unwrap();
        assert_eq!(apply("x", &e), "|x|");
        assert!(on_char("x", 0, Some((0, 1)), 'a').is_none());
    }

    #[test]
    fn bars_pair_where_an_operand_is_expected() {
        let e = on_char("2 + ", 4, None, '|').unwrap();
        assert_eq!(apply("2 + ", &e), "2 + ||");
        // After an operand the bar closes.
        assert!(on_char("|x", 2, None, '|').is_none());
        // Step over the auto-inserted closing bar.
        let e = on_char("|x|", 2, None, '|').unwrap();
        assert_eq!((e.text.as_str(), e.caret), ("", 3));
    }

    #[test]
    fn backspace_removes_empty_pairs() {
        let e = on_backspace("f()", 2, None).unwrap();
        assert_eq!(apply("f()", &e), "f");
        assert_eq!(e.caret, 1);
        assert!(on_backspace("f(x)", 3, None).is_none());
        assert!(on_backspace("()", 0, None).is_none());
        assert!(on_backspace("()", 1, Some((0, 2))).is_none());
        assert_eq!(apply("||", &on_backspace("||", 1, None).unwrap()), "");
    }

    fn func(name: &str, params: &[&str]) -> Completion {
        let mut c = Completion::new(name, CompletionKind::Builtin, "");
        let p: Vec<String> = params.iter().map(|s| s.to_string()).collect();
        c.snippet = Snippet::call(name, &p);
        c
    }

    #[test]
    fn accepting_inserts_snippets() {
        let (e, holes) = accept("2 + lo", 4, 6, &func("log", &["x", "b"]));
        assert_eq!(apply("2 + lo", &e), "2 + log(x, b)");
        assert_eq!(holes, vec![(8, 9), (11, 12)]);
        assert_eq!(e.select, Some((8, 9)));
        let (e, holes) = accept("si", 0, 2, &func("sin", &["x"]));
        assert_eq!(apply("si", &e), "sin()");
        assert_eq!(e.caret, 4);
        assert!(holes.is_empty());
        // A parenthesis follows: only the name.
        let (e, _) = accept("si(1)", 0, 2, &func("sin", &["x"]));
        assert_eq!(apply("si(1)", &e), "sin(1)");
        assert_eq!(e.caret, 3);
        // The rest of the word is replaced too.
        let (e, _) = accept(
            "θ + al1",
            4,
            6,
            &Completion::new("alpha", CompletionKind::Node, ""),
        );
        assert_eq!(apply("θ + al1", &e), "θ + alpha");
    }

    #[test]
    fn snippet_stops_follow_typing() {
        // "log(x, b)": stops x=(4,5), b=(7,8), end 9.
        let mut s = SnippetSession::new(vec![(4, 5), (7, 8)], 9, 9).unwrap();
        // Typing "10" over the selected `x`: delete 1 then insert 2.
        assert!(s.update(8, 4));
        assert_eq!(s.stops, vec![(4, 4), (6, 7)]);
        assert!(s.update(10, 6));
        assert_eq!(s.stops, vec![(4, 6), (8, 9)]);
        assert_eq!(s.end, 10);
        assert_eq!(s.next(), Some((8, 9)));
        assert_eq!(s.next(), None);
        assert_eq!(s.prev(), (4, 6));
        assert!(s.contains(5));
        assert!(!s.contains(11));
        // Deleting the whole snippet ends the session.
        assert!(!s.update(0, 0) || s.end == 0);
        assert!(SnippetSession::new(vec![(1, 2)], 3, 3).is_none());
    }
}

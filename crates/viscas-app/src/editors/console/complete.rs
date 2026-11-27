//! Offset conversion and suggestion insertion math (byte and char offsets).
//! Pure, no GTK: GTK text buffers count chars, the engine counts bytes.

use viscas_core::syntax::Span;

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
        let start = input
            .char_indices()
            .rfind(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8());
        return format!("{}{suggestion}", &input[..start]);
    }
    // A word sharing the suggestion's first letter, else append.
    let first = suggestion.chars().next();
    let best = words(input).into_iter().find(|&(s, e)| {
        let word = &input[s..e];
        word.chars().next() == first && word != suggestion
    });
    match best {
        Some((s, e)) => format!("{}{suggestion}{}", &input[..s], &input[e..]),
        None => format!("{input}{suggestion}"),
    }
}

/// Byte ranges of the identifier-like words of `text`, whatever the
/// width of the separators (`·`, `√`, NBSP…).
pub fn words(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        let word = c.is_alphanumeric() || c == '_';
        match (word, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, text.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn suggestions_replace_the_right_text() {
        let span = Some(Span { start: 4, end: 7 });
        assert_eq!(apply_suggestion("1 + sni(2)", span, "sin"), "1 + sin(2)");
        assert_eq!(apply_suggestion(":hlep sin", None, ":help"), ":help sin");
        assert_eq!(apply_suggestion(":mode dge", None, "deg"), ":mode deg");
        assert_eq!(apply_suggestion("2 + sni(1)", None, "sin"), "2 + sin(1)");
        assert_eq!(apply_suggestion("2 + ", None, "sin"), "2 + sin");
    }

    #[test]
    fn suggestions_with_multibyte_separators() {
        // Regression: separators wider than one byte used to panic or
        // splice at the wrong offset.
        assert_eq!(apply_suggestion("2·sni", None, "sin"), "2·sin");
        assert_eq!(apply_suggestion("2√sni", None, "sin"), "2√sin");
        assert_eq!(
            apply_suggestion("π\u{a0}sni(1)", None, "sin"),
            "π\u{a0}sin(1)"
        );
        assert_eq!(apply_suggestion("√ + sni", None, "sin"), "√ + sin");
        assert_eq!(apply_suggestion("ππ·πx + sni", None, "sin"), "ππ·πx + sin");
        assert_eq!(
            apply_suggestion(":help\u{a0}sni", None, "sin"),
            ":help\u{a0}sin"
        );
        assert_eq!(words("a·b√c π"), vec![(0, 1), (3, 4), (7, 8), (9, 11)]);
    }
}

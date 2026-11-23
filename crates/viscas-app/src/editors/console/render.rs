//! Text → Pango markup for console and expression-list output.
//!
//! "Typeset-lite": simple exponents become superscripts (`x^2` → x²),
//! numeric fractions get a fraction slash with raised numerator and
//! lowered denominator, `sqrt(` becomes `√(` and `*` a centered dot.
//! Real math layout (own Cairo/Pango engine, REQUIREMENTS §14.4) will
//! replace this module behind the same functions. Pure, no GTK.

use viscas_core::syntax::Span;

/// Escapes `&`, `<`, `>`, `'` and `"` for Pango markup.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        push_escaped(&mut out, c);
    }
    out
}

fn push_escaped(out: &mut String, c: char) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '\'' => out.push_str("&#39;"),
        '"' => out.push_str("&quot;"),
        _ => out.push(c),
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Markup for a math result (`3x^2 + 1/2` → 3x² + ½-style).
pub fn math_markup(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    render_range(&chars, 0, chars.len(), false, &mut out);
    out
}

/// Renders `c[i..end]`; `in_sup` disables nested fractions (unreadable
/// at superscript size).
fn render_range(c: &[char], mut i: usize, end: usize, in_sup: bool, out: &mut String) {
    while i < end {
        let ch = c[i];
        let prev = i.checked_sub(1).map(|p| c[p]);
        if ch == '^'
            && let Some((a, b, next)) = exponent(c, i + 1, end)
        {
            out.push_str("<sup>");
            render_range(c, a, b, true, out);
            out.push_str("</sup>");
            i = next;
            continue;
        }
        if !in_sup
            && ch.is_ascii_digit()
            && prev.is_none_or(|p| !is_word(p) && !matches!(p, ')' | '.' | '/' | '^'))
            && let Some((num_end, den_end)) = fraction(c, i, end)
        {
            out.push_str("<sup>");
            out.extend(&c[i..num_end]);
            out.push_str("</sup>\u{2044}<sub>");
            out.extend(&c[num_end + 1..den_end]);
            out.push_str("</sub>");
            i = den_end;
            continue;
        }
        if ch == 's' && prev.is_none_or(|p| !is_word(p)) && starts_with(c, i, end, "sqrt(") {
            out.push_str("√(");
            i += 5;
            continue;
        }
        if ch == '*' {
            out.push('·');
            i += 1;
            continue;
        }
        push_escaped(out, ch);
        i += 1;
    }
}

fn starts_with(c: &[char], i: usize, end: usize, pat: &str) -> bool {
    let n = pat.chars().count();
    i + n <= end && c[i..i + n].iter().copied().eq(pat.chars())
}

/// The exponent after a `^` at `i`: `(…)` (parentheses dropped), an
/// optionally negative number, or an identifier. Returns the content
/// range and where rendering continues.
fn exponent(c: &[char], i: usize, end: usize) -> Option<(usize, usize, usize)> {
    if i >= end {
        return None;
    }
    if c[i] == '(' {
        let mut depth = 0usize;
        for (j, &ch) in c.iter().enumerate().take(end).skip(i) {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return (j > i + 1).then_some((i + 1, j, j + 1));
                    }
                }
                _ => {}
            }
        }
        return None;
    }
    let mut j = i;
    if c[j] == '-' {
        j += 1;
    }
    let start_digits = j;
    if j < end && c[j].is_ascii_digit() {
        while j < end && (c[j].is_ascii_digit() || c[j] == '.') {
            j += 1;
        }
        return Some((i, j, j));
    }
    if j == start_digits && j < end && c[j].is_alphabetic() {
        while j < end && is_word(c[j]) {
            j += 1;
        }
        return Some((i, j, j));
    }
    None
}

/// `digits/digits` starting at `i`, not glued to a following word,
/// parenthesis, exponent or decimal point. Returns the end of the
/// numerator (the `/`) and of the denominator.
fn fraction(c: &[char], i: usize, end: usize) -> Option<(usize, usize)> {
    let mut j = i;
    while j < end && c[j].is_ascii_digit() {
        j += 1;
    }
    if j >= end || c[j] != '/' {
        return None;
    }
    let num_end = j;
    j += 1;
    let den_start = j;
    while j < end && c[j].is_ascii_digit() {
        j += 1;
    }
    if j == den_start {
        return None;
    }
    if j < end && (is_word(c[j]) || matches!(c[j], '(' | '^' | '.')) {
        return None;
    }
    Some((num_end, j))
}

/// Byte range of `span` clamped to `text` and to char boundaries.
pub fn clamp_span(text: &str, span: Span) -> (usize, usize) {
    let mut start = span.start.min(text.len());
    let mut end = span.end.clamp(start, text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    while !text.is_char_boundary(end) {
        end += 1;
    }
    (start, end)
}

/// `text` escaped, with `span` wavy-underlined in `color` (a CSS hex
/// color). An empty span underlines the next character, or a trailing
/// placeholder at the end of the text ("something is missing here").
pub fn underline_span(text: &str, span: Option<Span>, color: &str) -> String {
    let Some(span) = span else {
        return escape(text);
    };
    let (start, mut end) = clamp_span(text, span);
    let mut placeholder = false;
    if start == end {
        match text[start..].chars().next() {
            Some(c) if c != '\n' => end = start + c.len_utf8(),
            _ => placeholder = true,
        }
    }
    let open = format!(
        "<span underline=\"error\" underline_color=\"{}\" foreground=\"{}\">",
        escape(color),
        escape(color)
    );
    let marked = if placeholder {
        "\u{2002}".to_owned()
    } else {
        escape(&text[start..end])
    };
    format!(
        "{}{open}{marked}</span>{}",
        escape(&text[..start]),
        escape(&text[end..])
    )
}

/// Help/info text: `` `code` `` segments in monospace.
pub fn info_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for (i, part) in s.split('`').enumerate() {
        if i % 2 == 1 {
            out.push_str("<tt>");
            out.push_str(&math_markup(part));
            out.push_str("</tt>");
        } else {
            out.push_str(&escape(part));
        }
    }
    out
}

/// Should the `≈` decimal be shown next to `exact`? Not when empty or
/// when it says the same thing (`3` / `3`, `≈0.84` / `0.84`).
pub fn shows_approx(exact: &str, approx: &str) -> bool {
    !approx.is_empty() && exact.trim_start_matches('≈') != approx.trim_start_matches('≈')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, end: usize) -> Option<Span> {
        Some(Span { start, end })
    }

    #[test]
    fn escapes_markup() {
        assert_eq!(escape("a<b & \"c\""), "a&lt;b &amp; &quot;c&quot;");
    }

    #[test]
    fn superscripts() {
        assert_eq!(math_markup("x^2"), "x<sup>2</sup>");
        assert_eq!(math_markup("3x^2 + 1"), "3x<sup>2</sup> + 1");
        assert_eq!(math_markup("2^-3"), "2<sup>-3</sup>");
        assert_eq!(math_markup("e^x"), "e<sup>x</sup>");
        assert_eq!(math_markup("4^(3/2)"), "4<sup>3/2</sup>");
        assert_eq!(math_markup("2^3^2"), "2<sup>3</sup><sup>2</sup>");
        // Unbalanced or empty: left alone.
        assert_eq!(math_markup("x^(1"), "x^(1");
        assert_eq!(math_markup("x^"), "x^");
    }

    #[test]
    fn fractions() {
        assert_eq!(math_markup("1/3"), "<sup>1</sup>\u{2044}<sub>3</sub>");
        assert_eq!(
            math_markup("[1, 3/2]"),
            "[1, <sup>3</sup>\u{2044}<sub>2</sub>]"
        );
        // Glued to a variable, a parenthesis or a decimal: plain.
        assert_eq!(math_markup("1/2x"), "1/2x");
        assert_eq!(math_markup("1/(1/x)"), "1/(1/x)");
        assert_eq!(math_markup("a1/2"), "a1/2");
        assert_eq!(math_markup("1.5/2"), "1.5/2");
        assert_eq!(math_markup("π/2"), "π/2");
    }

    #[test]
    fn roots_and_products() {
        assert_eq!(math_markup("sqrt(2)"), "√(2)");
        assert_eq!(math_markup("isqrt(2)"), "isqrt(2)");
        assert_eq!(math_markup("2 * 3"), "2 · 3");
        assert_eq!(math_markup("a < b"), "a &lt; b");
    }

    #[test]
    fn underline_marks_the_span() {
        let m = underline_span("1 + foo", span(4, 7), "#f00");
        assert!(m.starts_with("1 + <span underline=\"error\""));
        assert!(m.contains(">foo</span>"));
        assert!(m.ends_with("</span>"));
        // Markup in the text is escaped on every side.
        let m = underline_span("<a> & b", span(4, 5), "#f00");
        assert!(m.starts_with("&lt;a&gt; "));
        assert!(m.contains(">&amp;</span> b"));
        assert_eq!(underline_span("x<1", None, "#f00"), "x&lt;1");
    }

    #[test]
    fn underline_empty_and_out_of_range_spans() {
        // Empty span: the next char.
        assert!(underline_span("2 +", span(1, 1), "#f00").contains("> </span>+"));
        // At the end: a placeholder.
        assert!(underline_span("2 +", span(3, 3), "#f00").contains(">\u{2002}</span>"));
        // Clamped and snapped to char boundaries.
        assert_eq!(clamp_span("π+1", Span { start: 1, end: 99 }), (0, 4));
        assert!(underline_span("π", span(1, 2), "#f00").contains(">π</span>"));
    }

    #[test]
    fn info_code_segments() {
        assert_eq!(
            info_markup("Type `2^10` & go"),
            "Type <tt>2<sup>10</sup></tt> &amp; go"
        );
    }

    #[test]
    fn approx_shown_only_when_different() {
        assert!(shows_approx("1/3", "0.333333333333"));
        assert!(!shows_approx("3", "3"));
        assert!(!shows_approx("≈0.84", "0.84"));
        assert!(!shows_approx("x", ""));
    }
}

//! Pango markup of the assistance popups (pure).

use gtk::glib::markup_escape_text as esc;
use viscas_engine::{Completion, CompletionKind, Diagnostic, Severity, SignatureHelp};

/// Badge text and CSS class of a completion kind.
pub fn kind_badge(kind: CompletionKind, function: bool) -> (&'static str, &'static str) {
    match kind {
        CompletionKind::Node if function => ("ƒ", "node"),
        CompletionKind::Node => ("n", "node"),
        CompletionKind::Console => ("c", "console"),
        CompletionKind::Builtin => ("ƒ", "builtin"),
        CompletionKind::Constant => ("π", "constant"),
        CompletionKind::Command => (":", "command"),
    }
}

/// The item name with its matched chars in bold, followed by its
/// parameter list dimmed (`<b>lo</b>g<span…>(x, b)</span>`).
pub fn name_markup(c: &Completion, positions: &[usize]) -> String {
    let mut out = String::new();
    for (i, ch) in c.text.chars().enumerate() {
        let s = esc(&ch.to_string()).to_string();
        if positions.contains(&i) {
            out.push_str(&format!("<b>{s}</b>"));
        } else {
            out.push_str(&s);
        }
    }
    if let Some(sig) = &c.signature
        && let Some(open) = sig.find('(')
        && c.kind != CompletionKind::Command
    {
        out.push_str(&format!("<span alpha=\"60%\">{}</span>", esc(&sig[open..])));
    }
    out
}

/// The short text on the right of a row: the value of a name, the
/// summary of a function.
pub fn short_doc(c: &Completion) -> String {
    match c.detail.split_once(" — ") {
        Some((_, summary)) => summary.to_string(),
        None => c.detail.clone(),
    }
}

/// The detail pane: signature(s), documentation, examples, category.
pub fn doc_markup(c: &Completion) -> String {
    let mut out = String::new();
    let sigs: Vec<&str> = if c.signatures.is_empty() {
        c.signature.iter().map(String::as_str).collect()
    } else {
        c.signatures.iter().map(|s| s.label.as_str()).collect()
    };
    if sigs.is_empty() {
        out.push_str(&format!("<tt><b>{}</b></tt>", esc(&c.text)));
    } else {
        let lines: Vec<String> = sigs
            .iter()
            .map(|s| format!("<tt><b>{}</b></tt>", esc(&s.replacen(sig_name(s), &c.text, 1))))
            .collect();
        out.push_str(&lines.join("\n"));
    }
    let doc = if c.doc.is_empty() { &c.detail } else { &c.doc };
    if !doc.is_empty() {
        out.push_str(&format!("\n\n{}", esc(doc)));
    }
    if !c.examples.is_empty() {
        out.push_str("\n\n<span size=\"small\" alpha=\"70%\">Examples</span>");
        for e in &c.examples {
            out.push_str(&format!("\n<tt>  {}</tt>", esc(e)));
        }
    }
    if !c.category.is_empty() {
        out.push_str(&format!(
            "\n\n<span size=\"small\" alpha=\"55%\">{}</span>",
            esc(&c.category)
        ));
    }
    out
}

fn sig_name(label: &str) -> &str {
    label.split('(').next().unwrap_or(label)
}

/// The signature hint: the active form with the current parameter in
/// bold, a `1/2` counter for several forms, and the doc.
pub fn signature_markup(h: &SignatureHelp) -> String {
    let sig = h.active();
    let mut label = String::new();
    match h.active_param.and_then(|p| sig.param_range(p)) {
        Some((s, e)) => {
            label.push_str(&esc(&sig.label[..s]));
            label.push_str(&format!(
                "<b><u>{}</u></b>",
                esc(&sig.label[s..e])
            ));
            label.push_str(&esc(&sig.label[e..]));
        }
        None => label.push_str(&esc(&sig.label)),
    }
    let mut out = format!("<tt>{label}</tt>");
    if h.signatures.len() > 1 {
        out.push_str(&format!(
            "  <span size=\"small\" alpha=\"60%\">{}/{}</span>",
            h.active_signature + 1,
            h.signatures.len()
        ));
    }
    if !h.doc.is_empty() {
        let first = h.doc.split(". ").next().unwrap_or(&h.doc);
        out.push_str(&format!(
            "\n<span size=\"small\" alpha=\"75%\">{}</span>",
            esc(first.trim_end_matches('.'))
        ));
    }
    out
}

/// Hover tooltip for a name: signature or value, then the doc.
pub fn hover_markup(c: &Completion) -> String {
    let head = match &c.signature {
        Some(sig) if c.kind != CompletionKind::Command => sig.clone(),
        _ if matches!(c.kind, CompletionKind::Node | CompletionKind::Console)
            && !c.detail.is_empty() =>
        {
            format!("{} = {}", c.text, c.detail)
        }
        _ => c.text.clone(),
    };
    let mut out = format!("<tt><b>{}</b></tt>", esc(&head));
    let doc = if c.doc.is_empty() { &c.detail } else { &c.doc };
    if !doc.is_empty() {
        out.push_str(&format!("\n{}", esc(doc)));
    }
    if let Some(e) = c.examples.first() {
        out.push_str(&format!("\n<tt><span alpha=\"70%\">{}</span></tt>", esc(e)));
    }
    out
}

/// Tooltip of a squiggle: the message and the quick fixes.
pub fn diagnostic_markup(d: &Diagnostic) -> String {
    let mark = match d.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let mut out = format!("<b>{mark}:</b> {}", esc(&d.message));
    if !d.suggestions.is_empty() {
        let names: Vec<String> = d
            .suggestions
            .iter()
            .map(|s| format!("<tt>{}</tt>", esc(s)))
            .collect();
        out.push_str(&format!(
            "\ndid you mean {}?  <span alpha=\"65%\">Ctrl+. fixes</span>",
            names.join(", ")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use viscas_engine::help::Signature;
    use viscas_core::syntax::Span;
    use viscas_engine::Snippet;

    fn log() -> Completion {
        let mut c = Completion::new("log", CompletionKind::Builtin, "log(x, b) — logarithm");
        c.signature = Some("log(x, b)".into());
        c.signatures = vec![Signature::parse("log(x)"), Signature::parse("log(x, b)")];
        c.doc = "Logarithm, base 10 or base b.".into();
        c.examples = vec!["log(8, 2) = 3".into()];
        c.category = "elementary".into();
        c.snippet = Snippet::call("log", &["x".into(), "b".into()]);
        c
    }

    #[test]
    fn rows_bold_the_matched_letters() {
        let m = name_markup(&log(), &[0, 1]);
        assert_eq!(m, "<b>l</b><b>o</b>g<span alpha=\"60%\">(x, b)</span>");
        assert_eq!(short_doc(&log()), "logarithm");
        let lt = Completion::new("a<b", CompletionKind::Node, "1");
        assert_eq!(name_markup(&lt, &[]), "a&lt;b");
    }

    #[test]
    fn doc_pane_lists_forms_and_examples() {
        let m = doc_markup(&log());
        assert!(m.starts_with("<tt><b>log(x)</b></tt>\n<tt><b>log(x, b)</b></tt>"));
        assert!(m.contains("base 10"));
        assert!(m.contains("log(8, 2) = 3"));
        assert!(m.contains("elementary"));
        // Aliases show their own name.
        let mut alias = log();
        alias.text = "lg".into();
        assert!(doc_markup(&alias).contains("lg(x, b)"));
    }

    #[test]
    fn signature_hint_bolds_the_active_parameter() {
        let h = SignatureHelp {
            name: "log".into(),
            signatures: vec![Signature::parse("log(x)"), Signature::parse("log(x, b)")],
            active_signature: 1,
            active_param: Some(1),
            doc: "Logarithm. More.".into(),
            open: 3,
        };
        let m = signature_markup(&h);
        assert!(m.starts_with("<tt>log(x, <b><u>b</u></b>)</tt>"), "{m}");
        assert!(m.contains("2/2"));
        assert!(m.contains("Logarithm"));
        assert!(!m.contains("More"));
    }

    #[test]
    fn hover_and_diagnostic_tooltips() {
        assert!(hover_markup(&log()).starts_with("<tt><b>log(x, b)</b></tt>\nLogarithm"));
        let node = Completion::new("a", CompletionKind::Node, "1/2");
        assert!(hover_markup(&node).contains("a = 1/2"));
        let d = Diagnostic {
            span: Span { start: 0, end: 3 },
            message: "unknown function `sni`".into(),
            severity: Severity::Error,
            suggestions: vec!["sin".into()],
        };
        let m = diagnostic_markup(&d);
        assert!(m.contains("<b>error:</b> unknown function `sni`"));
        assert!(m.contains("<tt>sin</tt>"));
        assert!(m.contains("Ctrl+."));
    }
}

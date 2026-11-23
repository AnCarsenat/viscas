//! Pure logic of the inspector (GTK-free, unit tested): merging the
//! properties of several selected nodes, parsing typed values and the
//! field-level diagnostics shown under each entry.

use viscas_core::syntax::Span;
use viscas_scene::{
    KindTag, LineStyle, Node, NodeId, PropValue, Scene, SourceField, Status, Style,
};

/// A property over the selection: one shared value, or mixed values.
#[derive(Clone, Debug, PartialEq)]
pub enum Mixed<T> {
    /// Nothing selected.
    Empty,
    Same(T),
    Different,
}

impl<T: Clone> Mixed<T> {
    /// The shared value, if any.
    pub fn value(&self) -> Option<T> {
        match self {
            Mixed::Same(v) => Some(v.clone()),
            _ => None,
        }
    }

    pub fn is_mixed(&self) -> bool {
        matches!(self, Mixed::Different)
    }
}

/// Merges values: [`Mixed::Same`] when all are equal.
pub fn common<T: PartialEq>(values: impl IntoIterator<Item = T>) -> Mixed<T> {
    let mut iter = values.into_iter();
    let Some(first) = iter.next() else {
        return Mixed::Empty;
    };
    for v in iter {
        if v != first {
            return Mixed::Different;
        }
    }
    Mixed::Same(first)
}

/// Properties every node has, merged over the selection.
#[derive(Clone, Debug, PartialEq)]
pub struct CommonProps {
    pub kind: Mixed<KindTag>,
    pub visible: Mixed<bool>,
    pub locked: Mixed<bool>,
    /// `None` inside = automatic color.
    pub color: Mixed<Option<viscas_scene::Color>>,
    pub line_width: Mixed<f64>,
    pub line_style: Mixed<LineStyle>,
    pub opacity: Mixed<f64>,
    pub label: Mixed<String>,
    pub show_label: Mixed<bool>,
}

impl CommonProps {
    pub fn of(nodes: &[&Node]) -> CommonProps {
        let style = |f: fn(&Style) -> f64| common(nodes.iter().map(|n| f(&n.style)));
        CommonProps {
            kind: common(nodes.iter().map(|n| n.kind.tag())),
            visible: common(nodes.iter().map(|n| n.visible)),
            locked: common(nodes.iter().map(|n| n.locked)),
            color: common(nodes.iter().map(|n| n.style.color)),
            line_width: style(|s| s.line_width),
            line_style: common(nodes.iter().map(|n| n.style.line_style)),
            opacity: style(|s| s.opacity),
            label: common(
                nodes
                    .iter()
                    .map(|n| n.style.label.clone().unwrap_or_default()),
            ),
            show_label: common(nodes.iter().map(|n| n.style.show_label)),
        }
    }
}

/// Summary line of a multi-selection: `3 nodes · 2 plots, 1 slider`.
pub fn selection_summary(nodes: &[&Node]) -> String {
    let mut counts: Vec<(KindTag, usize)> = Vec::new();
    for n in nodes {
        let tag = n.kind.tag();
        match counts.iter_mut().find(|(k, _)| *k == tag) {
            Some((_, c)) => *c += 1,
            None => counts.push((tag, 1)),
        }
    }
    let parts: Vec<String> = counts
        .iter()
        .map(|(k, c)| {
            let s = if *c == 1 { "" } else { "s" };
            format!("{c} {k}{s}")
        })
        .collect();
    format!("{} nodes · {}", nodes.len(), parts.join(", "))
}

/// Parses a parameter list typed as `x, y` (spaces or commas).
pub fn parse_params(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Text of a free-form property for an entry (`""` when absent).
pub fn prop_text(value: Option<&PropValue>) -> String {
    match value {
        None => String::new(),
        Some(PropValue::Float(f)) => crate::theme::model::trim_float(*f, 6),
        Some(PropValue::Int(i)) => i.to_string(),
        Some(PropValue::Text(t)) => t.clone(),
        Some(PropValue::Bool(b)) => b.to_string(),
    }
}

/// Property value for typed text: a number when it parses as one, the
/// expression text otherwise; `None` for blank (remove the property).
pub fn prop_from_text(text: &str) -> Option<PropValue> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    match t.parse::<f64>() {
        Ok(v) if v.is_finite() => Some(PropValue::Float(v)),
        _ => Some(PropValue::Text(t.to_owned())),
    }
}

/// An error to show under one source field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub message: String,
    pub span: Option<Span>,
}

/// Errors of `field` in node `id`: blocking diagnostics, unknown names
/// (advisory) and an evaluation error attributed to that field. The
/// evaluation error of a node without field goes to [`SourceField::Main`].
pub fn field_errors(scene: &Scene, id: NodeId, field: SourceField) -> Vec<FieldError> {
    let mut out: Vec<FieldError> = scene
        .diagnostics(id)
        .iter()
        .filter(|d| d.field.unwrap_or(SourceField::Main) == field)
        .map(|d| FieldError {
            message: d.message.clone(),
            span: d.span,
        })
        .collect();
    if let Status::Error {
        message,
        field: f,
        span,
    } = scene.status(id)
        && f.unwrap_or(SourceField::Main) == field
        && !out.iter().any(|e| e.message == *message)
    {
        out.push(FieldError {
            message: message.clone(),
            span: *span,
        });
    }
    out
}

/// Clamps a byte span to `text` on char boundaries (spans may be stale
/// while typing).
pub fn clamp_span(text: &str, span: Span) -> Option<(usize, usize)> {
    let fix = |mut i: usize| {
        i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let (s, e) = (fix(span.start), fix(span.end));
    match (s, e) {
        (s, e) if e > s => Some((s, e)),
        // Zero-width (end of input): underline the last char.
        (s, _) if s > 0 => {
            let prev = text[..s].chars().next_back().map_or(1, char::len_utf8);
            Some((s - prev, s))
        }
        _ => None,
    }
}

/// Slider bound from its expression text, when it is a plain number.
pub fn numeric(text: &str) -> Option<f64> {
    text.trim()
        .replace('−', "-")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use viscas_scene::{Color, NodeContent};

    #[test]
    fn common_values() {
        assert_eq!(common(Vec::<i32>::new()), Mixed::Empty);
        assert_eq!(common([2, 2, 2]), Mixed::Same(2));
        assert_eq!(common([2, 3]), Mixed::Different);
        assert!(common([1.0, 1.5]).is_mixed());
        assert_eq!(common(["a"]).value(), Some("a"));
    }

    #[test]
    fn multi_edit_merges_common_properties() {
        let mut s = Scene::new();
        let a = s.add(None, NodeContent::plot("y = x")).unwrap();
        let b = s.add(None, NodeContent::plot("y = 2x")).unwrap();
        let c = s.add(None, NodeContent::variable("c", "1")).unwrap();
        s.update(b, |n| {
            n.style.line_width = 4.0;
            n.style.color = Some(Color::rgb(255, 0, 0));
        })
        .unwrap();
        s.set_locked(c, true).unwrap();
        let nodes: Vec<&Node> = [a, b].iter().map(|i| s.get(*i).unwrap()).collect();
        let p = CommonProps::of(&nodes);
        assert_eq!(p.kind, Mixed::Same(KindTag::Plot));
        assert_eq!(p.visible, Mixed::Same(true));
        assert!(p.line_width.is_mixed());
        assert!(p.color.is_mixed());
        assert_eq!(p.opacity, Mixed::Same(1.0));
        assert_eq!(p.line_style, Mixed::Same(LineStyle::Solid));
        assert_eq!(p.label, Mixed::Same(String::new()));

        let nodes: Vec<&Node> = [a, b, c].iter().map(|i| s.get(*i).unwrap()).collect();
        let p = CommonProps::of(&nodes);
        assert!(p.kind.is_mixed());
        assert!(p.locked.is_mixed());
        assert_eq!(selection_summary(&nodes), "3 nodes · 2 plots, 1 variable");
    }

    #[test]
    fn params_and_props() {
        assert_eq!(parse_params(" x, y  z,,"), vec!["x", "y", "z"]);
        assert!(parse_params("  ").is_empty());
        assert_eq!(prop_text(Some(&PropValue::Float(2.5))), "2.5");
        assert_eq!(prop_text(None), "");
        assert_eq!(prop_from_text(" 3 "), Some(PropValue::Float(3.0)));
        assert_eq!(prop_from_text("2pi"), Some(PropValue::Text("2pi".into())));
        assert_eq!(prop_from_text(""), None);
        assert_eq!(numeric("−5"), Some(-5.0));
        assert_eq!(numeric("a"), None);
    }

    #[test]
    fn errors_attach_to_fields() {
        let mut s = Scene::new();
        let k = s
            .add(None, NodeContent::slider("k", "1", "0 +", "10"))
            .unwrap();
        assert!(field_errors(&s, k, SourceField::Main).is_empty());
        let min = field_errors(&s, k, SourceField::Min);
        assert_eq!(min.len(), 1);
        assert!(min[0].span.is_some());
    }

    #[test]
    fn spans_are_clamped() {
        assert_eq!(clamp_span("abc", Span::new(1, 2)), Some((1, 2)));
        assert_eq!(clamp_span("abc", Span::new(3, 3)), Some((2, 3)));
        assert_eq!(clamp_span("aθ", Span::new(2, 9)), Some((1, 3)));
        assert_eq!(clamp_span("", Span::new(0, 0)), None);
    }
}

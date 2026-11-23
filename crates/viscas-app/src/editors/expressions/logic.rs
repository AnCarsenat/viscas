//! Pure rules of the Expressions editor: which nodes get a row, how a
//! node error span maps into the row's line, how an edited line merges
//! into the node, and where a dropped row lands. No GTK.

use viscas_core::syntax::Span;
use viscas_scene::{NodeContent, NodeId, NodeKind, Scene, SourceField};

/// Kinds shown as rows: everything an expression-list line can express.
pub fn listed(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Variable { .. }
            | NodeKind::Function { .. }
            | NodeKind::Plot { .. }
            | NodeKind::Point { .. }
            | NodeKind::Slider { .. }
            | NodeKind::List { .. }
            | NodeKind::Action { .. }
            | NodeKind::Regression { .. }
    )
}

/// Ids of the listed nodes in scene (depth-first) order.
pub fn rows_in_order(scene: &Scene) -> Vec<NodeId> {
    scene
        .iter()
        .filter(|n| listed(&n.kind))
        .map(|n| n.id)
        .collect()
}

/// Do the kind's values deserve an inline preview (`= 3 ≈ …`)?
pub fn shows_value(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Variable { .. }
            | NodeKind::Point { .. }
            | NodeKind::Slider { .. }
            | NodeKind::List { .. }
    )
}

/// The span of an error in `field` of `kind`, moved into the row's
/// `line` (which ends with the field's text for every listed kind:
/// `a := <expr>`, `f(x) := <body>`, `<relation>`). `None` when the field
/// is not part of the line (slider bounds) or the text does not match.
pub fn line_span(line: &str, kind: &NodeKind, field: SourceField, span: Span) -> Option<Span> {
    if field != SourceField::Main {
        return None;
    }
    let text = kind.source(SourceField::Main)?;
    let offset = if line.ends_with(text) {
        line.len() - text.len()
    } else {
        line.find(text)?
    };
    (span.end <= text.len() && span.start <= span.end).then_some(Span {
        start: span.start + offset,
        end: span.end + offset,
    })
}

/// The content after editing a row of `old` into `parsed` (the parsed
/// line): name and kind come from the line; visibility, lock, style and
/// properties stay. A slider stays a slider (with the new value) when the
/// line still defines a plain variable.
pub fn merged_content(old: &NodeContent, parsed: NodeContent) -> NodeContent {
    let mut out = old.clone();
    out.name = parsed.name;
    out.kind = match (&old.kind, parsed.kind) {
        (
            NodeKind::Slider {
                min,
                max,
                step,
                animation,
                ..
            },
            NodeKind::Variable { expr },
        ) if out.name.is_some() => NodeKind::Slider {
            value: expr,
            min: min.clone(),
            max: max.clone(),
            step: step.clone(),
            animation: animation.clone(),
        },
        (_, kind) => kind,
    };
    out
}

/// Numeric slider range `(min, max, step)` from its expression fields;
/// unparsable bounds fall back to `[-10, 10]`, an empty or invalid step
/// to a hundredth of the range.
pub fn slider_range(min: &str, max: &str, step: &str) -> (f64, f64, f64) {
    let num = |s: &str| s.trim().parse::<f64>().ok().filter(|v| v.is_finite());
    let mut lo = num(min).unwrap_or(-10.0);
    let mut hi = num(max).unwrap_or(10.0);
    if hi < lo {
        std::mem::swap(&mut lo, &mut hi);
    }
    if hi == lo {
        hi = lo + 1.0;
    }
    let step = num(step).filter(|s| *s > 0.0).unwrap_or((hi - lo) / 100.0);
    (lo, hi, step)
}

/// Decimal digits that show `step` exactly (0.25 → 2, 5 → 0), at most 6.
pub fn step_digits(step: f64) -> u32 {
    let mut digits = 0;
    let mut s = step.abs();
    while digits < 6 && (s - s.round()).abs() > 1e-9 {
        s *= 10.0;
        digits += 1;
    }
    digits
}

/// Slider value text for `v` snapped to `step` from `min`.
pub fn slider_text(v: f64, min: f64, step: f64) -> String {
    let snapped = if step > 0.0 {
        min + ((v - min) / step).round() * step
    } else {
        v
    };
    crate::theme::model::trim_float(snapped, step_digits(step) as usize)
}

/// Where a dragged row goes relative to the target row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Before,
    After,
}

/// `(parent, index)` to pass to [`Scene::move_node`] for dropping
/// `dragged` before/after `target`; `None` for a no-op or invalid move
/// (onto itself, into its own subtree).
pub fn drop_target(
    scene: &Scene,
    dragged: NodeId,
    target: NodeId,
    place: Place,
) -> Option<(Option<NodeId>, usize)> {
    if dragged == target || scene.is_ancestor(dragged, target) || !scene.contains(target) {
        return None;
    }
    let parent = scene.parent(target);
    let siblings: Vec<NodeId> = scene
        .children(parent)
        .iter()
        .copied()
        .filter(|&c| c != dragged)
        .collect();
    let at = siblings.iter().position(|&c| c == target)?;
    let index = match place {
        Place::Before => at,
        Place::After => at + 1,
    };
    let unchanged =
        scene.parent(dragged) == parent && scene.index_in_parent(dragged) == Some(index);
    (!unchanged).then_some((parent, index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_kinds() {
        assert!(listed(&NodeKind::Plot {
            relation: "y = x".into()
        }));
        assert!(listed(&NodeKind::slider("1")));
        assert!(!listed(&NodeKind::Folder));
        assert!(!listed(&NodeKind::viewport()));
        assert!(shows_value(&NodeKind::slider("1")));
        assert!(!shows_value(&NodeKind::Plot {
            relation: "y = x".into()
        }));
    }

    #[test]
    fn spans_move_into_the_line() {
        let kind = NodeKind::Variable {
            expr: "1 + foo".into(),
        };
        let span = Span { start: 4, end: 7 };
        assert_eq!(
            line_span("a := 1 + foo", &kind, SourceField::Main, span),
            Some(Span { start: 9, end: 12 })
        );
        let f = NodeKind::Function {
            params: vec!["x".into()],
            body: "x^2 + q".into(),
        };
        assert_eq!(
            line_span(
                "f(x) = x^2 + q",
                &f,
                SourceField::Main,
                Span { start: 6, end: 7 }
            ),
            Some(Span { start: 13, end: 14 })
        );
        // Slider bounds are not in the line; stale spans are dropped.
        let s = NodeKind::slider("3");
        assert_eq!(line_span("a := 3", &s, SourceField::Min, span), None);
        assert_eq!(
            line_span("a := 3", &s, SourceField::Main, Span { start: 0, end: 5 }),
            None
        );
    }

    #[test]
    fn edits_keep_style_and_slider_metadata() {
        let mut old = NodeContent::slider("a", "3", "0", "5");
        if let NodeKind::Slider { step, .. } = &mut old.kind {
            *step = "0.5".into();
        }
        old.visible = false;
        old.style.line_width = 4.0;
        let parsed = NodeContent::variable("a", "4");
        let merged = merged_content(&old, parsed);
        assert!(!merged.visible);
        assert_eq!(merged.style.line_width, 4.0);
        match &merged.kind {
            NodeKind::Slider {
                value, min, step, ..
            } => {
                assert_eq!(value, "4");
                assert_eq!(min, "0");
                assert_eq!(step, "0.5");
            }
            k => panic!("expected slider, got {k:?}"),
        }
        // A different kind replaces the slider.
        let merged = merged_content(&old, NodeContent::plot("y = x"));
        assert!(matches!(merged.kind, NodeKind::Plot { .. }));
        assert_eq!(merged.name, None);
    }

    #[test]
    fn slider_numbers() {
        assert_eq!(slider_range("0", "5", "0.5"), (0.0, 5.0, 0.5));
        assert_eq!(slider_range("a", "", ""), (-10.0, 10.0, 0.2));
        assert_eq!(slider_range("5", "1", "-1"), (1.0, 5.0, 0.04));
        assert_eq!(slider_range("2", "2", "1"), (2.0, 3.0, 1.0));
        assert_eq!(step_digits(0.25), 2);
        assert_eq!(step_digits(5.0), 0);
        assert_eq!(step_digits(0.1), 1);
        assert_eq!(slider_text(1.26, 0.0, 0.25), "1.25");
        assert_eq!(slider_text(3.9, 0.0, 1.0), "4");
    }

    #[test]
    fn drop_positions() {
        let mut scene = Scene::new();
        let a = scene.add(None, NodeContent::variable("a", "1")).unwrap();
        let b = scene.add(None, NodeContent::variable("b", "2")).unwrap();
        let c = scene.add(None, NodeContent::variable("c", "3")).unwrap();
        assert_eq!(drop_target(&scene, a, a, Place::Before), None);
        assert_eq!(
            drop_target(&scene, a, b, Place::Before),
            None,
            "already there"
        );
        assert_eq!(drop_target(&scene, a, b, Place::After), Some((None, 1)));
        assert_eq!(drop_target(&scene, c, a, Place::Before), Some((None, 0)));
        assert_eq!(drop_target(&scene, a, c, Place::After), Some((None, 2)));
        let (p, i) = drop_target(&scene, a, c, Place::After).unwrap();
        scene.move_node(a, p, i).unwrap();
        assert_eq!(scene.children(None), &[b, c, a]);
        // Into a folder, and never into one's own subtree.
        let f = scene.add(None, NodeContent::folder("F")).unwrap();
        let d = scene.add(Some(f), NodeContent::variable("d", "4")).unwrap();
        assert_eq!(drop_target(&scene, b, d, Place::Before), Some((Some(f), 0)));
        assert_eq!(drop_target(&scene, f, d, Place::After), None);
    }
}

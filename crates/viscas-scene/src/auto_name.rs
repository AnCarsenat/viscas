//! Godot-style automatic labels for drawable nodes: `Plot1`, `Circle1`,
//! `Curve1`, `Region1`, `Point1`.
//!
//! Display only: an unnamed drawable keeps `name = None` and is *shown*
//! with a label derived from its kind and tree order
//! ([`Scene::auto_label`]). Nothing is stored, so duplicates never share
//! a name, points never become identifiers and files stay clean; a node
//! gets a stored name only when the user renames it. Labels skip numbers
//! used by stored names of the same form, and are cached per scene
//! revision.

use std::collections::{HashMap, HashSet};

use crate::node::{NodeId, NodeKind};
use crate::scene::Scene;

/// The automatic-name family of a node kind: `Plot`, `Circle`, `Curve`,
/// `Region` or `Point`; `None` for kinds named by the user (variables,
/// functions, ...).
pub fn family(kind: &NodeKind) -> Option<&'static str> {
    match kind {
        NodeKind::Plot { relation } => Some(plot_family(relation)),
        NodeKind::Point { .. } => Some("Point"),
        _ => None,
    }
}

/// Classifies a relation for naming: inequalities are regions, top-level
/// tuples are parametric curves, `(x-a)^2 + (y-b)^2 = r^2` shapes are
/// circles, anything else is a plot.
pub fn plot_family(relation: &str) -> &'static str {
    let flat: String = relation.chars().filter(|c| !c.is_whitespace()).collect();
    if flat.contains(['<', '>', '≤', '≥']) {
        return "Region";
    }
    if flat.starts_with('(') && has_top_level_comma(&flat) {
        return "Curve";
    }
    if is_circle(&flat) {
        return "Circle";
    }
    "Plot"
}

/// Whether a parenthesized tuple `(a, b)` makes up the whole text.
fn has_top_level_comma(s: &str) -> bool {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 && i + 1 < s.len() {
                    return false; // `(…)…`: not a bare tuple
                }
            }
            ',' if depth == 1 => return true,
            _ => {}
        }
    }
    false
}

/// `x^2 + y^2 = …` with optional shifts (`(x-1)^2`) on one side.
fn is_circle(flat: &str) -> bool {
    let Some((lhs, rhs)) = flat.split_once('=') else {
        return false;
    };
    if rhs.contains('=') {
        return false;
    }
    let squares = |side: &str| {
        let terms = split_top_plus(side);
        terms.len() == 2
            && terms.iter().any(|t| squared_of(t, 'x'))
            && terms.iter().any(|t| squared_of(t, 'y'))
    };
    (squares(lhs) && !rhs.contains(['x', 'y'])) || (squares(rhs) && !lhs.contains(['x', 'y']))
}

/// Splits at `+` signs outside parentheses.
fn split_top_plus(s: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (Vec::new(), 0i32, 0);
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '+' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// `v^2` or `(v±c)^2`.
fn squared_of(term: &str, v: char) -> bool {
    let Some(base) = term.strip_suffix("^2").or_else(|| term.strip_suffix('²')) else {
        return false;
    };
    if base.len() == 1 {
        return base.starts_with(v);
    }
    let Some(inner) = base.strip_prefix('(').and_then(|b| b.strip_suffix(')')) else {
        return false;
    };
    inner.starts_with(v)
        && inner[1..].starts_with(['+', '-'])
        && !inner[1..].contains(['x', 'y', '('])
}

/// Every label family.
const FAMILIES: [&str; 5] = ["Plot", "Circle", "Curve", "Region", "Point"];

/// Splits `Plot12` into (`Plot`, 12) when the name is an automatic name
/// of a known family.
pub fn parse_auto_name(name: &str) -> Option<(&str, u32)> {
    let digits = name.trim_start_matches(|c: char| !c.is_ascii_digit());
    let base = &name[..name.len() - digits.len()];
    if digits.is_empty() || digits.starts_with('0') {
        return None;
    }
    let n = digits.parse().ok()?;
    FAMILIES.contains(&base).then_some((base, n))
}

impl Scene {
    /// Labels of every unnamed drawable, computed once per revision: in
    /// tree order, each takes the smallest number of its family not used
    /// by a stored name or an earlier label.
    pub fn auto_labels(&self) -> &HashMap<NodeId, String> {
        self.auto_labels.get_or_init(|| {
            let mut used: HashMap<&'static str, HashSet<u32>> = HashMap::new();
            for node in self.iter() {
                if let Some((base, n)) = node.name.as_deref().and_then(parse_auto_name)
                    && let Some(fam) = FAMILIES.into_iter().find(|f| *f == base)
                {
                    used.entry(fam).or_default().insert(n);
                }
            }
            let mut labels = HashMap::new();
            for node in self.iter() {
                if node.name.is_some() {
                    continue;
                }
                if let Some(fam) = family(&node.kind) {
                    let set = used.entry(fam).or_default();
                    let n = (1..).find(|n| !set.contains(n)).unwrap_or(1);
                    set.insert(n);
                    labels.insert(node.id, format!("{fam}{n}"));
                }
            }
            labels
        })
    }

    /// The automatic label shown for an unnamed drawable node (`Plot2`);
    /// `None` for named nodes and kinds without automatic labels.
    pub fn auto_label(&self, id: NodeId) -> Option<String> {
        self.auto_labels().get(&id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::NodeContent;

    #[test]
    fn families() {
        assert_eq!(plot_family("y = x^2"), "Plot");
        assert_eq!(plot_family("x^2 + y^2 = 4"), "Circle");
        assert_eq!(plot_family("(x-1)^2 + (y+2)^2 = 9"), "Circle");
        assert_eq!(plot_family("x^2 + y^2 = x"), "Plot");
        assert_eq!(plot_family("(cos t, sin t)"), "Curve");
        assert_eq!(plot_family("(x+1)(x-1) = y"), "Plot");
        assert_eq!(plot_family("y < x"), "Region");
        assert_eq!(plot_family("x^2+y^2 <= 4"), "Region");
    }

    #[test]
    fn parse_names() {
        assert_eq!(parse_auto_name("Plot12"), Some(("Plot", 12)));
        assert_eq!(parse_auto_name("Circle1"), Some(("Circle", 1)));
        assert_eq!(parse_auto_name("Plot"), None);
        assert_eq!(parse_auto_name("Plot01"), None);
        assert_eq!(parse_auto_name("a1"), None);
    }

    #[test]
    fn labels_skip_stored_names_and_follow_changes() {
        let mut s = Scene::new();
        let a = s.add(None, NodeContent::plot("y = x")).unwrap();
        let mut named = NodeContent::plot("y = 2x");
        named.name = Some("Plot1".into());
        s.add(None, named).unwrap();
        let c = s.add(None, NodeContent::plot("y = 3x")).unwrap();
        assert_eq!(s.auto_label(a).as_deref(), Some("Plot2"));
        assert_eq!(s.auto_label(c).as_deref(), Some("Plot3"));
        let circle = s.add(None, NodeContent::plot("x^2+y^2=1")).unwrap();
        assert_eq!(s.auto_label(circle).as_deref(), Some("Circle1"));
        // Labels are display only: nothing is stored.
        assert!(s.get(circle).unwrap().name.is_none());
        // Duplicates get their own label (in tree order: right after `a`).
        let d = s.duplicate(a).unwrap();
        assert_eq!(s.auto_label(d).as_deref(), Some("Plot3"));
        assert_eq!(s.auto_label(c).as_deref(), Some("Plot4"));
        // The cache follows edits (rename stores a name; label goes).
        let r = s.revision();
        s.rename(a, Some("Mine")).unwrap();
        assert!(s.revision() > r);
        assert_eq!(s.auto_label(a), None);
        assert_eq!(s.auto_label(d).as_deref(), Some("Plot2"));
        assert_eq!(s.auto_label(c).as_deref(), Some("Plot3"));
        let v = s.add(None, NodeContent::variable("k", "1")).unwrap();
        assert_eq!(s.auto_label(v), None);
    }
}

//! Godot-style automatic names for drawable nodes: `Plot1`, `Circle1`,
//! `Curve1`, `Region1`, `Point1`.
//!
//! New plot-like nodes get such a name stored when they are added (see
//! [`Scene::next_auto_name`]); older unnamed nodes are *displayed* with an
//! automatic label ([`Scene::auto_label`]) that is only stored once the
//! user edits the name.

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

/// Splits `Plot12` into (`Plot`, 12) when the name is an automatic name
/// of a known family.
pub fn parse_auto_name(name: &str) -> Option<(&str, u32)> {
    let digits = name.trim_start_matches(|c: char| !c.is_ascii_digit());
    let base = &name[..name.len() - digits.len()];
    if digits.is_empty() || digits.starts_with('0') {
        return None;
    }
    let n = digits.parse().ok()?;
    ["Plot", "Circle", "Curve", "Region", "Point"]
        .contains(&base)
        .then_some((base, n))
}

impl Scene {
    /// Numbers used per family by stored names and by the labels of
    /// unnamed nodes (in tree order, each taking the smallest free number).
    fn auto_labels(&self) -> (HashMap<NodeId, String>, HashMap<&'static str, HashSet<u32>>) {
        let mut used: HashMap<&'static str, HashSet<u32>> = HashMap::new();
        for node in self.iter() {
            if let Some((base, n)) = node.name.as_deref().and_then(parse_auto_name)
                && let Some(fam) = ["Plot", "Circle", "Curve", "Region", "Point"]
                    .into_iter()
                    .find(|f| *f == base)
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
        (labels, used)
    }

    /// The automatic label shown for an unnamed drawable node (`Plot2`);
    /// `None` for named nodes and kinds without automatic names.
    pub fn auto_label(&self, id: NodeId) -> Option<String> {
        let node = self.get(id)?;
        if node.name.is_some() {
            return None;
        }
        family(&node.kind)?;
        self.auto_labels().0.remove(&id)
    }

    /// The name a new node of `kind` gets: the next free number of its
    /// family (never one shown by an unnamed node's label, nor a name
    /// defined elsewhere). `None` for kinds without automatic names.
    pub fn next_auto_name(&self, kind: &NodeKind) -> Option<String> {
        let fam = family(kind)?;
        let (_, used) = self.auto_labels();
        let set = used.get(fam);
        (1..)
            .map(|n: u32| (n, format!("{fam}{n}")))
            .find(|(n, name)| {
                !set.is_some_and(|s| s.contains(n)) && !self.is_name_taken(name, None)
            })
            .map(|(_, name)| name)
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
    fn labels_and_next_names_never_collide() {
        let mut s = Scene::new();
        let a = s.add(None, NodeContent::plot("y = x")).unwrap();
        let mut named = NodeContent::plot("y = 2x");
        named.name = Some("Plot1".into());
        s.add(None, named).unwrap();
        let c = s.add(None, NodeContent::plot("y = 3x")).unwrap();
        assert_eq!(s.auto_label(a).as_deref(), Some("Plot2"));
        assert_eq!(s.auto_label(c).as_deref(), Some("Plot3"));
        let kind = NodeKind::Plot {
            relation: "y = 4x".into(),
        };
        assert_eq!(s.next_auto_name(&kind).as_deref(), Some("Plot4"));
        let circle = NodeKind::Plot {
            relation: "x^2+y^2=1".into(),
        };
        assert_eq!(s.next_auto_name(&circle).as_deref(), Some("Circle1"));
        let var = NodeKind::Variable { expr: "1".into() };
        assert_eq!(s.next_auto_name(&var), None);
    }
}

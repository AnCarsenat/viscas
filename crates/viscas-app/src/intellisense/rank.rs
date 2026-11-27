//! Ranking of completion items with the palette's fuzzy matcher (pure).

use viscas_engine::{Completion, CompletionKind};

use crate::fuzzy::fuzzy_match;

/// Most items the popup lists.
pub const MAX_ITEMS: usize = 60;

/// A ranked item: the completion and the char positions of its matched
/// letters (for bold markup).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ranked {
    pub item: Completion,
    pub positions: Vec<usize>,
    pub score: i32,
}

/// Everyday names, most common first: a tie-break so `s` offers `sin`
/// before `sec`.
const COMMON: [&str; 24] = [
    "sin", "cos", "tan", "sqrt", "ln", "log", "exp", "abs", "pi", "e", "x", "y", "floor", "ceil",
    "round", "min", "max", "sum", "asin", "acos", "atan", "fact", "gcd", "theta",
];

fn common_bonus(name: &str) -> i32 {
    COMMON
        .iter()
        .position(|c| *c == name)
        .map_or(0, |i| 48 - i32::try_from(i).unwrap_or(48))
}

fn kind_bonus(kind: CompletionKind) -> i32 {
    match kind {
        CompletionKind::Node => 30,
        CompletionKind::Console => 25,
        CompletionKind::Builtin => 10,
        CompletionKind::Constant => 8,
        CompletionKind::Command => 0,
    }
}

/// Orders `items` for the typed `fragment`: fuzzy score, with strong
/// bonuses for case-sensitive and case-insensitive prefix matches, then
/// the item kind (scene names first) and shorter names. Items the
/// fragment does not match are dropped; an empty fragment keeps all.
pub fn rank(fragment: &str, items: Vec<Completion>) -> Vec<Ranked> {
    let query = fragment.strip_prefix(':').unwrap_or(fragment);
    let lower = query.to_lowercase();
    let mut out: Vec<Ranked> = items
        .into_iter()
        .filter_map(|item| {
            let name = item.text.strip_prefix(':').unwrap_or(&item.text);
            let offset = item.text.len() - name.len();
            let m = fuzzy_match(query, name)?;
            let len = name.chars().count();
            let scene = matches!(item.kind, CompletionKind::Node | CompletionKind::Console);
            // For a one-letter fragment, scene names only win when short
            // and spelled with exactly that letter (`a` → `a1`, not
            // `alpha` over `abs`).
            let short_query = query.chars().count() < 2;
            let kind = match (scene && short_query, name.starts_with(query) && len <= 3) {
                (true, true) => kind_bonus(item.kind) + 40,
                (true, false) => 0,
                _ => kind_bonus(item.kind),
            };
            let mut score = m.score + kind + common_bonus(name);
            if !query.is_empty() {
                if name.starts_with(query) {
                    score += 300;
                } else if name.to_lowercase().starts_with(&lower) {
                    score += 200;
                }
                if name == query {
                    score += 80;
                }
            }
            score -= i32::try_from(len).unwrap_or(i32::MAX / 2);
            let positions = m.positions.iter().map(|p| p + offset).collect();
            Some(Ranked {
                item,
                positions,
                score,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.item.text.cmp(&b.item.text))
    });
    out.truncate(MAX_ITEMS);
    out
}

/// Whether the popup has nothing to offer beyond what is typed: a single
/// non-function item spelled exactly like the fragment.
pub fn is_redundant(fragment: &str, ranked: &[Ranked]) -> bool {
    match ranked {
        [] => true,
        [only] => only.item.text == fragment && !only.item.is_function(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(text: &str, kind: CompletionKind) -> Completion {
        Completion::new(text, kind, "")
    }

    fn names(r: &[Ranked]) -> Vec<&str> {
        r.iter().map(|r| r.item.text.as_str()).collect()
    }

    #[test]
    fn prefix_matches_come_first() {
        let items = vec![
            c("asin", CompletionKind::Builtin),
            c("sinh", CompletionKind::Builtin),
            c("sin", CompletionKind::Builtin),
            c("sign", CompletionKind::Builtin),
        ];
        let r = rank("sin", items);
        assert_eq!(names(&r)[..2], ["sin", "sinh"]);
        // `sign` and `asin` match loosely, after the prefix matches.
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn common_functions_first_for_one_letter() {
        let items = vec![
            c("sec", CompletionKind::Builtin),
            c("sum", CompletionKind::Builtin),
            c("sqrt", CompletionKind::Builtin),
            c("sin", CompletionKind::Builtin),
            c("speed", CompletionKind::Node),
        ];
        let r = rank("s", items);
        assert_eq!(names(&r)[..3], ["sin", "sqrt", "sum"]);
        // A short node spelled with the letter still ranks high.
        let r = rank(
            "a",
            vec![
                c("abs", CompletionKind::Builtin),
                c("a1", CompletionKind::Node),
            ],
        );
        assert_eq!(names(&r)[0], "a1");
        let r = rank(
            "a",
            vec![
                c("abs", CompletionKind::Builtin),
                c("alpha", CompletionKind::Node),
            ],
        );
        assert_eq!(names(&r)[0], "abs");
    }

    #[test]
    fn scene_names_beat_builtins_on_ties() {
        let items = vec![
            c("sinh", CompletionKind::Builtin),
            c("sigma", CompletionKind::Node),
        ];
        assert_eq!(names(&rank("si", items))[0], "sigma");
    }

    #[test]
    fn fuzzy_matches_are_kept_and_marked() {
        let r = rank("sqt", vec![c("sqrt", CompletionKind::Builtin)]);
        assert_eq!(r[0].positions, vec![0, 1, 3]);
        let r = rank("SI", vec![c("sin", CompletionKind::Builtin)]);
        assert_eq!(names(&r), ["sin"]);
    }

    #[test]
    fn commands_match_without_the_colon() {
        let r = rank(
            ":he",
            vec![
                c(":help", CompletionKind::Command),
                c(":vars", CompletionKind::Command),
            ],
        );
        assert_eq!(names(&r), [":help"]);
        assert_eq!(r[0].positions, vec![1, 2]);
    }

    #[test]
    fn redundant_popups() {
        let pi = rank("pi", vec![c("pi", CompletionKind::Constant)]);
        assert!(is_redundant("pi", &pi));
        let mut sin = Completion::new("sin", CompletionKind::Builtin, "");
        sin.snippet = viscas_engine::Snippet::call("sin", &["x".into()]);
        let sin = rank("sin", vec![sin]);
        assert!(!is_redundant("sin", &sin), "accepting adds the parentheses");
        assert!(is_redundant("x", &[]));
    }
}

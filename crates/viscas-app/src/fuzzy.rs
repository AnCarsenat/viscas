//! Fuzzy matching for the command palette (pure, GTK-free).
//!
//! A query matches a candidate when its characters appear in the candidate
//! in order (case-insensitive). Matches are scored so that word starts,
//! consecutive runs and early matches rank first.

/// A successful match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Higher is better.
    pub score: i32,
    /// Char indices (not byte offsets) of the matched candidate characters.
    pub positions: Vec<usize>,
}

const MATCH: i32 = 16;
const WORD_START: i32 = 24;
const CONSECUTIVE: i32 = 20;
const FIRST_CHAR: i32 = 12;
const GAP: i32 = 2;
const MAX_LEADING_PENALTY: i32 = 12;

fn is_word_start(chars: &[char], i: usize) -> bool {
    i == 0 || {
        let prev = chars[i - 1];
        let cur = chars[i];
        !prev.is_alphanumeric() || (prev.is_lowercase() && cur.is_uppercase())
    }
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Matches `query` against `candidate`. Whitespace in the query is ignored,
/// and an empty query matches everything with score 0.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<Match> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(fold)
        .collect();
    if query.is_empty() {
        return Some(Match {
            score: 0,
            positions: Vec::new(),
        });
    }
    let chars: Vec<char> = candidate.chars().collect();
    let folded: Vec<char> = chars.iter().copied().map(fold).collect();

    // Try every occurrence of the first query char as a starting point and
    // keep the best greedy match; candidates are short, so this is cheap.
    let mut best: Option<Match> = None;
    for start in (0..folded.len()).filter(|&i| folded[i] == query[0]) {
        let Some(positions) = greedy(&query, &folded, &chars, start) else {
            break; // later starts cannot succeed if this one did not
        };
        let score = score(&positions, &chars);
        if best.as_ref().is_none_or(|b| score > b.score) {
            best = Some(Match { score, positions });
        }
    }
    best
}

/// Matches the rest of the query after `start`, preferring word starts over
/// plain occurrences when the next query char appears at both.
fn greedy(query: &[char], folded: &[char], chars: &[char], start: usize) -> Option<Vec<usize>> {
    let mut positions = vec![start];
    let mut from = start + 1;
    for &q in &query[1..] {
        let mut rest = from..folded.len();
        let next = rest.clone().find(|&i| folded[i] == q)?;
        // Keep consecutive runs; otherwise jump to a word start if one exists.
        let pick = if next == from {
            next
        } else {
            rest.find(|&i| folded[i] == q && is_word_start(chars, i))
                .unwrap_or(next)
        };
        positions.push(pick);
        from = pick + 1;
    }
    Some(positions)
}

fn score(positions: &[usize], chars: &[char]) -> i32 {
    let mut score = 0;
    let mut prev: Option<usize> = None;
    for &p in positions {
        score += MATCH;
        if is_word_start(chars, p) {
            score += WORD_START;
        }
        match prev {
            Some(q) if p == q + 1 => score += CONSECUTIVE,
            Some(q) => score -= GAP * i32::try_from(p - q - 1).unwrap_or(i32::MAX / 4).min(20),
            None => {
                if p == 0 {
                    score += FIRST_CHAR;
                }
                score -= (i32::try_from(p).unwrap_or(i32::MAX)).min(MAX_LEADING_PENALTY);
            }
        }
        prev = Some(p);
    }
    score
}

/// Filters and ranks `candidates`, best first; ties keep the input order.
/// Returns indices into `candidates` with their matches.
pub fn rank<'a>(query: &str, candidates: impl IntoIterator<Item = &'a str>) -> Vec<(usize, Match)> {
    let mut out: Vec<(usize, Match)> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(i, c)| fuzzy_match(query, c).map(|m| (i, m)))
        .collect();
    out.sort_by(|(ia, a), (ib, b)| b.score.cmp(&a.score).then(ia.cmp(ib)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        let m = fuzzy_match("", "Quit").unwrap();
        assert_eq!(m.score, 0);
        assert!(m.positions.is_empty());
        assert!(fuzzy_match("   ", "").is_some());
    }

    #[test]
    fn subsequence_is_required() {
        assert!(fuzzy_match("spl", "Split Area").is_some());
        assert!(fuzzy_match("xyz", "Split Area").is_none());
        assert!(fuzzy_match("lps", "Split Area").is_none());
        assert!(fuzzy_match("a", "").is_none());
    }

    #[test]
    fn case_and_spaces_are_ignored() {
        let m = fuzzy_match("SA", "split area").unwrap();
        assert_eq!(m.positions, vec![0, 6]);
        assert!(fuzzy_match("split  area", "Split Area").is_some());
    }

    #[test]
    fn prefers_word_starts() {
        // "sa" should hit the S of Split and the A of Area, not "sp-l-i-t a".
        let m = fuzzy_match("sa", "Split Area Horizontally").unwrap();
        assert_eq!(m.positions, vec![0, 6]);
        let m = fuzzy_match("wg", "Switch Workspace: Graph").unwrap();
        assert_eq!(m.positions, vec![7, 18]);
    }

    #[test]
    fn positions_are_char_indices() {
        let m = fuzzy_match("θ", "Set θ range").unwrap();
        assert_eq!(m.positions, vec![4]);
    }

    #[test]
    fn ranking_orders_by_quality() {
        let items = ["Close Area", "Console", "Change Editor: Console", "Quit"];
        let ranked: Vec<usize> = rank("con", items).into_iter().map(|(i, _)| i).collect();
        // Prefix match first, word-start match next, scattered match last.
        assert_eq!(ranked, vec![1, 2]);
        let ranked: Vec<usize> = rank("ca", items).into_iter().map(|(i, _)| i).collect();
        assert_eq!(ranked[0], 0);
    }

    #[test]
    fn consecutive_beats_scattered() {
        let a = fuzzy_match("max", "Maximize Area").unwrap();
        let b = fuzzy_match("max", "Make Axes").unwrap();
        assert!(a.score > b.score);
    }

    #[test]
    fn ties_keep_input_order() {
        let ranked: Vec<usize> = rank("", ["b", "a", "c"])
            .into_iter()
            .map(|(i, _)| i)
            .collect();
        assert_eq!(ranked, vec![0, 1, 2]);
    }
}

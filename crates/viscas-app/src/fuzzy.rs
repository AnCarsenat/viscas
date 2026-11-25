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
    best_alignment(&query, &folded, &chars)
}

/// Exhaustive best alignment by dynamic programming over (query index,
/// candidate index): `best[i][j]` is the best score of matching
/// `query[..=i]` with `query[i]` placed at candidate char `j`. Candidates
/// are short (command titles), so the O(m·n²) cost is negligible, and unlike
/// a greedy scan this never misses a valid match.
fn best_alignment(query: &[char], folded: &[char], chars: &[char]) -> Option<Match> {
    let (m, n) = (query.len(), folded.len());
    if m > n {
        return None;
    }
    let gain = |j: usize| {
        MATCH
            + if is_word_start(chars, j) {
                WORD_START
            } else {
                0
            }
    };
    let mut best: Vec<Vec<Option<i32>>> = vec![vec![None; n]; m];
    let mut from: Vec<Vec<usize>> = vec![vec![0; n]; m];

    for j in (0..n).filter(|&j| folded[j] == query[0]) {
        let leading = i32::try_from(j)
            .unwrap_or(i32::MAX)
            .min(MAX_LEADING_PENALTY);
        let first = if j == 0 { FIRST_CHAR } else { 0 };
        best[0][j] = Some(gain(j) + first - leading);
    }
    for i in 1..m {
        for j in (i..n).filter(|&j| folded[j] == query[i]) {
            let mut cell: Option<(i32, usize)> = None;
            let prev_row = &best[i - 1];
            for (k, prev) in prev_row.iter().enumerate().take(j).skip(i - 1) {
                let Some(prev) = *prev else {
                    continue;
                };
                let link = if k + 1 == j {
                    CONSECUTIVE
                } else {
                    -GAP * i32::try_from(j - k - 1).unwrap_or(i32::MAX).min(20)
                };
                let score = prev + link;
                if cell.is_none_or(|(s, _)| score > s) {
                    cell = Some((score, k));
                }
            }
            if let Some((score, k)) = cell {
                best[i][j] = Some(score + gain(j));
                from[i][j] = k;
            }
        }
    }

    // Best end position (earliest on ties), then walk the back pointers.
    let (mut j, score) = (0..n).filter_map(|j| best[m - 1][j].map(|s| (j, s))).fold(
        None,
        |acc: Option<(usize, i32)>, (j, s)| match acc {
            Some((_, bs)) if bs >= s => acc,
            _ => Some((j, s)),
        },
    )?;
    let mut positions = vec![0; m];
    for i in (0..m).rev() {
        positions[i] = j;
        j = from[i][j];
    }
    Some(Match { score, positions })
}

/// Lowest score [`rank`] keeps for `query`: a bit more than one plain
/// match per query char, so word-start and contiguous matches pass while
/// letters scattered across a title (gaps, late start) drop out.
pub fn threshold(query: &str) -> i32 {
    const PER_CHAR: i32 = MATCH + CONSECUTIVE / 2;
    let len = query.chars().filter(|c| !c.is_whitespace()).count();
    PER_CHAR * i32::try_from(len).unwrap_or(i32::MAX / PER_CHAR)
}

/// Filters and ranks `candidates`, best first; ties keep the input order.
/// Matches under [`threshold`] are dropped. Returns indices into
/// `candidates` with their matches.
pub fn rank<'a>(query: &str, candidates: impl IntoIterator<Item = &'a str>) -> Vec<(usize, Match)> {
    let min = threshold(query);
    let mut out: Vec<(usize, Match)> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(i, c)| fuzzy_match(query, c).map(|m| (i, m)))
        .filter(|(_, m)| m.score >= min)
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
    fn finds_matches_a_greedy_scan_misses() {
        // Greedy word-start jumping would take the second "b" and fail.
        let m = fuzzy_match("abc", "axbc b").unwrap();
        assert_eq!(m.positions, vec![0, 2, 3]);
        assert!(fuzzy_match("abc", "a b c").is_some());
        assert!(fuzzy_match("aab", "ab ab").is_some());
    }

    #[test]
    fn match_exists_iff_subsequence() {
        let cands = ["axbc b", "Split Area", "abcabc", "cba", "a-b_c", ""];
        let queries = ["abc", "sa", "cab", "aa", "bcb", "x"];
        for c in cands {
            for q in queries {
                let mut it = c.to_lowercase().chars().collect::<Vec<_>>().into_iter();
                let is_subseq = q.chars().all(|qc| it.any(|cc| cc == qc));
                assert_eq!(fuzzy_match(q, c).is_some(), is_subseq, "{q:?} in {c:?}");
            }
        }
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
    fn threshold_drops_scattered_matches() {
        let items = [
            "Show Roots of Selected",
            "Toggle Radians / Degrees",
            "prod(L) — product of a list",
            "Clear Console Transcript",
        ];
        let ranked: Vec<usize> = rank("root", items).into_iter().map(|(i, _)| i).collect();
        assert_eq!(ranked, vec![0]);
        assert!(fuzzy_match("sa", "Split Area").is_some_and(|m| m.score >= threshold("sa")));
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

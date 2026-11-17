//! Matching for Quick Open (fuzzy) and Quick Select (patterns).

use crate::sort::fold;

/// A fuzzy match: how good it is, and which characters of the candidate
/// matched (as character indices, for highlighting).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i64,
    pub positions: Vec<u32>,
}

/// Subsequence fuzzy match of `query` in `candidate`, ignoring case and
/// accents: every query character must appear in order. Higher scores are
/// better; contiguous runs, word starts and a match at the very beginning
/// score more. `None` if the query does not match. An empty query matches
/// everything with score 0.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    // Fold character by character so positions map back to the original.
    let cand: Vec<char> = candidate.chars().collect();
    let folded: Vec<String> = cand.iter().map(|c| fold(&c.to_string())).collect();
    let needle: Vec<char> = fold(query).chars().filter(|c| !c.is_whitespace()).collect();
    if needle.is_empty() {
        return Some(FuzzyMatch { score: 0, positions: Vec::new() });
    }

    let is_word_start = |i: usize| {
        i == 0 || {
            let prev = cand[i - 1];
            !prev.is_alphanumeric() || (prev.is_lowercase() && cand[i].is_uppercase())
        }
    };

    let matches = |q: usize, i: usize| folded[i].starts_with(needle[q]);
    let bonus = |i: usize| 10 + if is_word_start(i) { 12 } else { 0 } + if i == 0 { 10 } else { 0 };

    // Best alignment by dynamic programming: best[q][i] is the top score
    // for matching the first q+1 query characters with the last of them
    // at candidate position i. A greedy scan would grab the "S" of "Sort"
    // for the query "size" and miss the word "Size" further on.
    const NO: i64 = i64::MIN / 2;
    let (n, m) = (needle.len(), cand.len());
    let mut best = vec![vec![NO; m]; n];
    let mut back = vec![vec![usize::MAX; m]; n];
    for (i, first) in best[0].iter_mut().enumerate() {
        if matches(0, i) {
            // Characters skipped before the first match cost a little.
            *first = bonus(i) - (i.min(10) as i64);
        }
    }
    for q in 1..n {
        for i in q..m {
            if !matches(q, i) {
                continue;
            }
            for k in (q - 1)..i {
                if best[q - 1][k] == NO {
                    continue;
                }
                // Contiguous characters score extra; gaps cost a little.
                let link = if k + 1 == i { 15 } else { -((i - k - 1).min(10) as i64) };
                let total = best[q - 1][k] + link + bonus(i);
                if total > best[q][i] {
                    best[q][i] = total;
                    back[q][i] = k;
                }
            }
        }
    }
    let (end, top) = (0..m).map(|i| (i, best[n - 1][i])).max_by_key(|(i, score)| (*score, std::cmp::Reverse(*i)))?;
    if top == NO {
        return None;
    }
    let mut positions = vec![0u32; n];
    let mut at = end;
    for q in (0..n).rev() {
        positions[q] = at as u32;
        at = back[q][at];
    }
    // Shorter candidates win ties: "Size" before "Resize Columns".
    let score = top - (m as i64) / 8;
    Some(FuzzyMatch { score, positions })
}

/// A Quick Select pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// Name contains the text (case- and accent-insensitive).
    Contains(String),
    /// Extension equals (written `.ext` or `*.ext`).
    Extension(String),
    /// Whole-name wildcard with `*` and `?`.
    Wildcard(String),
}

impl Pattern {
    /// Work out which kind of pattern the user typed.
    pub fn parse(text: &str) -> Pattern {
        let text = text.trim();
        let ext = text.strip_prefix("*.").or_else(|| text.strip_prefix('.'));
        if let Some(ext) = ext {
            if !ext.is_empty() && !ext.contains(['*', '?', '.', '/']) {
                return Pattern::Extension(fold(ext));
            }
        }
        if text.contains(['*', '?']) {
            Pattern::Wildcard(fold(text))
        } else {
            Pattern::Contains(fold(text))
        }
    }

    /// Does `name` match? An empty "contains" pattern matches nothing, so
    /// an empty field selects nothing.
    pub fn matches(&self, name: &str) -> bool {
        let folded = fold(name);
        match self {
            Pattern::Contains(text) => !text.is_empty() && folded.contains(text.as_str()),
            Pattern::Extension(ext) => crate::entry::extension_of(&folded) == *ext,
            Pattern::Wildcard(pattern) => wildcard(pattern, &folded),
        }
    }
}

/// `*` matches any run of characters, `?` exactly one.
pub(crate) fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            // Let the last star swallow one more character and retry.
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank<'a>(query: &str, candidates: &[&'a str]) -> Vec<&'a str> {
        let mut scored: Vec<(i64, &str)> =
            candidates.iter().filter_map(|c| fuzzy_match(query, c).map(|m| (m.score, *c))).collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        scored.into_iter().map(|(_, c)| c).collect()
    }

    #[test]
    fn spec_qo01_subsequence_matches_and_ranks_the_obvious_first() {
        let ranked = rank("prj", &["Projects", "Pictures", "Program Files", "My Private Journal", "Desktop"]);
        assert_eq!(ranked[0], "Projects");
        assert!(!ranked.contains(&"Desktop"));
        assert!(!ranked.contains(&"Pictures"));
    }

    #[test]
    fn spec_qo02_menu_paths_match_by_their_words() {
        let ranked = rank(
            "size",
            &[
                "View › Sort by › Size",
                "View › Resize Columns to Fit",
                "File › Calculate Folder Size",
                "Edit › Select All",
            ],
        );
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0], "View › Sort by › Size");
        let m = fuzzy_match("size", "View › Sort by › Size").unwrap();
        assert_eq!(m.positions, vec![17, 18, 19, 20]); // the word "Size" itself
    }

    #[test]
    fn matching_ignores_case_and_accents_and_needs_order() {
        assert!(fuzzy_match("RESUME", "résumé.pdf").is_some());
        assert!(fuzzy_match("docs", "Documents").is_some());
        assert!(fuzzy_match("scod", "Documents").is_none()); // wrong order
        assert!(fuzzy_match("xyz", "Documents").is_none());
        assert_eq!(fuzzy_match("", "anything").unwrap().score, 0);
        assert!(fuzzy_match("a", "").is_none());
    }

    #[test]
    fn better_matches_score_higher() {
        let score = |q: &str, c: &str| fuzzy_match(q, c).unwrap().score;
        assert!(score("doc", "Documents") > score("doc", "My Old Documents")); // prefix
        assert!(score("doc", "Documents") > score("doc", "Dropbox Cache")); // contiguous
        assert!(score("nt", "New Tab") > score("nt", "Enter Full Screen")); // word starts
        assert!(score("copy", "Copy") > score("copy", "Copy Path for Terminal"));
        // shorter
    }

    #[test]
    fn positions_point_at_the_matched_characters() {
        let m = fuzzy_match("nt", "New Tab").unwrap();
        assert_eq!(m.positions, vec![0, 4]);
        // Accented candidate: positions count characters, not bytes.
        let m = fuzzy_match("sum", "résumé").unwrap();
        assert_eq!(m.positions, vec![2, 3, 4]);
    }

    #[test]
    fn spec_qo08_qo09_quick_select_patterns() {
        assert_eq!(Pattern::parse("resume"), Pattern::Contains("resume".into()));
        assert!(Pattern::parse("resume").matches("résumé.pdf"));
        assert!(!Pattern::parse("resume").matches("notes.txt"));

        assert_eq!(Pattern::parse(".jpg"), Pattern::Extension("jpg".into()));
        assert_eq!(Pattern::parse("*.JPG"), Pattern::Extension("jpg".into()));
        assert!(Pattern::parse(".jpg").matches("Photo.JPG"));
        assert!(!Pattern::parse(".jpg").matches("jpg-notes.txt"));
        assert!(!Pattern::parse(".jpg").matches("archive.jpg.zip"));

        assert_eq!(Pattern::parse("img_*.p?g"), Pattern::Wildcard("img_*.p?g".into()));
        assert!(Pattern::parse("IMG_*.p?g").matches("img_0001.png"));
        assert!(!Pattern::parse("img_*.p?g").matches("img_0001.jpeg"));
        assert!(Pattern::parse("*").matches("anything"));
        assert!(Pattern::parse("a*b*c").matches("a-x-b-y-c"));
        assert!(!Pattern::parse("a*b*c").matches("a-x-c-y-b"));
        assert!(Pattern::parse("?").matches("x") && !Pattern::parse("?").matches("xy"));

        // An empty field selects nothing.
        assert!(!Pattern::parse("").matches("anything"));
        assert!(!Pattern::parse("  ").matches("anything"));
    }
}

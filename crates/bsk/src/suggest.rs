//! "Did you mean" support.

/// Returns the candidate closest to `word`, if one is close enough to be a plausible typo.
///
/// Ties go to the candidate that comes first, so the answer is deterministic.
///
/// ```
/// let keys = ["description", "skill"];
/// assert_eq!(bsk::closest("skil", keys), Some("skill"));
/// assert_eq!(bsk::closest("desc", keys), Some("description"));
/// assert_eq!(bsk::closest("zzz", keys), None);
/// ```
pub fn closest<'a, I>(word: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let word = word.to_lowercase();
    let length = word.chars().count();
    let limit = match length {
        0..=2 => 0,
        3..=4 => 1,
        5..=7 => 2,
        _ => 3,
    };
    let mut best: Option<(&'a str, usize)> = None;
    for candidate in candidates {
        let lowered = candidate.to_lowercase();
        let distance = if length >= 3 && lowered.starts_with(&word) {
            0
        } else {
            distance(&word, &lowered)
        };
        if distance <= limit && best.is_none_or(|(_, d)| distance < d) {
            best = Some((candidate, distance));
        }
    }
    best.map(|(candidate, _)| candidate)
}

/// Optimal string alignment distance: insertions, deletions, substitutions and
/// swaps of adjacent characters each cost one.
///
/// Row `i` of the table depends only on rows `i - 1` and `i - 2`, so three rows are enough.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut two_back = vec![0usize; b.len() + 1];
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(two_back[j - 2] + 1);
            }
            current[j] = best;
        }
        // Row i - 1 becomes "two back", row i becomes "previous", and the old row is reused.
        std::mem::swap(&mut two_back, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_counts_edits() {
        assert_eq!(distance("skill", "skill"), 0);
        assert_eq!(distance("skil", "skill"), 1);
        assert_eq!(distance("sikll", "skill"), 1);
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance("", "abc"), 3);
    }

    /// The textbook version with a full grid, to check the rolling rows against.
    fn distance_with_a_full_grid(a: &str, b: &str) -> usize {
        let a: Vec<char> = a.chars().collect();
        let b: Vec<char> = b.chars().collect();
        let mut grid = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for (i, row) in grid.iter_mut().enumerate() {
            row[0] = i;
        }
        for (j, cell) in grid[0].iter_mut().enumerate() {
            *cell = j;
        }
        for i in 1..=a.len() {
            for j in 1..=b.len() {
                let cost = usize::from(a[i - 1] != b[j - 1]);
                let mut best = (grid[i - 1][j] + 1)
                    .min(grid[i][j - 1] + 1)
                    .min(grid[i - 1][j - 1] + cost);
                if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                    best = best.min(grid[i - 2][j - 2] + 1);
                }
                grid[i][j] = best;
            }
        }
        grid[a.len()][b.len()]
    }

    #[test]
    fn distance_matches_the_full_grid_on_every_short_string() {
        let mut words = vec![String::new()];
        for _ in 0..4 {
            let longer: Vec<String> = words
                .iter()
                .flat_map(|w| ["a", "b", "c"].map(|c| format!("{w}{c}")))
                .collect();
            words.extend(longer);
            words.sort();
            words.dedup();
        }
        assert_eq!(words.len(), 1 + 3 + 9 + 27 + 81);
        for a in &words {
            for b in &words {
                assert_eq!(
                    distance(a, b),
                    distance_with_a_full_grid(a, b),
                    "{a:?} and {b:?}"
                );
            }
        }
        for (a, b) in [
            ("skill", "sikll"),
            ("kitten", "sitting"),
            ("éa", "aé"),
            ("", "é"),
        ] {
            assert_eq!(distance(a, b), distance_with_a_full_grid(a, b));
        }
    }

    #[test]
    fn finds_typos_and_prefixes() {
        let keys = ["library", "registry", "on-conflict"];
        assert_eq!(closest("libary", keys), Some("library"));
        assert_eq!(closest("lib", keys), Some("library"));
        assert_eq!(closest("Registry", keys), Some("registry"));
        assert_eq!(closest("on-confict", keys), Some("on-conflict"));
    }

    #[test]
    fn rejects_unrelated_words() {
        let keys = ["library", "registry"];
        assert_eq!(closest("banana", keys), None);
        assert_eq!(closest("", keys), None);
        assert_eq!(closest("ab", keys), None);
    }

    #[test]
    fn earliest_candidate_wins_a_tie() {
        assert_eq!(closest("cat", ["car", "cap"]), Some("car"));
    }
}

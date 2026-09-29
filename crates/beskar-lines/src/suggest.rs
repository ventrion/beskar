/// Picks the candidate closest to `word`, if any is close enough to be a
/// plausible typo. Used for "did you mean" hints.
pub fn closest<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    if word.is_empty() {
        return None;
    }
    let word_len = word.chars().count();
    let allowed = (word_len / 3).clamp(1, 3);
    let mut best: Option<(usize, &'a str)> = None;
    for &candidate in candidates {
        let mut distance = edit_distance(word, candidate);
        // "desc" is not a typo of "description", and "repository" is not one of
        // "repo", but in both cases the intent is clear.
        let shorter = word_len.min(candidate.chars().count());
        if shorter >= 3 && (candidate.starts_with(word) || word.starts_with(candidate)) {
            distance = distance.min(1);
        }
        if distance <= allowed && best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// Levenshtein distance over chars.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, &ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, &cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            let deletion = previous[j + 1] + 1;
            let insertion = current[j] + 1;
            current.push(substitution.min(deletion).min(insertion));
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_single_typo() {
        assert_eq!(closest("skils", &["description", "skill"]), Some("skill"));
    }

    #[test]
    fn finds_prefix() {
        assert_eq!(closest("desc", &["description", "skill"]), Some("description"));
    }

    #[test]
    fn finds_a_longer_spelling_of_a_key() {
        assert_eq!(closest("repository", &["repo"]), Some("repo"));
        assert_eq!(closest("profiles", &["profile"]), Some("profile"));
    }

    #[test]
    fn short_keys_do_not_match_everything_that_starts_with_them() {
        assert_eq!(closest("skill", &["s"]), None);
    }

    #[test]
    fn rejects_distant_words() {
        assert_eq!(closest("banana", &["description", "skill"]), None);
    }

    #[test]
    fn prefers_the_nearest() {
        assert_eq!(closest("profil", &["profile", "profiles"]), Some("profile"));
    }

    #[test]
    fn empty_word_suggests_nothing() {
        assert_eq!(closest("", &["a"]), None);
    }
}

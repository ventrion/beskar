/// The candidate closest to `input` by edit distance, if one is close enough
/// to be a plausible typo. Used for "did you mean" suggestions.
pub fn closest<'a>(input: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let mut best: Option<(usize, &'a str)> = None;
    for candidate in candidates {
        let distance = distance(input, candidate);
        let longest = input.chars().count().max(candidate.chars().count());
        let limit = (longest / 3).max(1);
        if distance <= limit && best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// Optimal string alignment distance: insertions, deletions, substitutions
/// and swaps of adjacent characters each cost 1.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let width = b.len() + 1;
    let mut d = vec![0usize; (a.len() + 1) * width];
    for i in 0..=a.len() {
        d[i * width] = i;
    }
    for (j, cell) in d.iter_mut().enumerate().take(width) {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut value = (d[(i - 1) * width + j] + 1)
                .min(d[i * width + j - 1] + 1)
                .min(d[(i - 1) * width + j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                value = value.min(d[(i - 2) * width + j - 2] + 1);
            }
            d[i * width + j] = value;
        }
    }
    d[a.len() * width + b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances() {
        assert_eq!(distance("", ""), 0);
        assert_eq!(distance("skill", "skill"), 0);
        assert_eq!(distance("skils", "skill"), 1);
        assert_eq!(distance("sklil", "skill"), 1);
        assert_eq!(distance("kitten", "sitting"), 3);
    }

    #[test]
    fn suggestions_prefer_the_nearest_plausible_candidate() {
        let keys = ["library", "registry", "skills-dir", "on-conflict"];
        assert_eq!(closest("libary", keys), Some("library"));
        assert_eq!(closest("skill-dir", keys), Some("skills-dir"));
        assert_eq!(closest("colour", keys), None);
        assert_eq!(closest("x", ["y"]), Some("y"));
        assert_eq!(closest("statsu", ["status", "stats"]), Some("status"));
    }
}

//! Small wording helpers shared by every front end.

pub use bsk::closest;

/// "1 skill", "2 skills", "3 repositories".
pub fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        return format!("1 {noun}");
    }
    match noun.strip_suffix('y') {
        Some(stem) if !stem.ends_with(['a', 'e', 'i', 'o', 'u']) => format!("{n} {stem}ies"),
        _ => format!("{n} {noun}s"),
    }
}

/// Replaces every control character except tab and newline with U+FFFD.
///
/// Names and descriptions come from files that may have been written by a stranger. Printed as they
/// are, an escape sequence in one could rewrite the terminal it is shown on.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\t' && c != '\n' {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

/// Quotes text for a POSIX shell when it needs quoting, so a suggested command can be pasted.
pub fn shell_quote(text: &str) -> String {
    let plain = !text.is_empty()
        && text.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '-' | '.' | '/' | ':' | '=' | '@' | '%' | '+' | ',')
        });
    if plain {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// Joins names for display, or "(none)" when there are none.
pub fn names<I, S>(items: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let list: Vec<String> = items.into_iter().map(|s| s.as_ref().to_string()).collect();
    if list.is_empty() {
        "(none)".to_string()
    } else {
        list.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_pluralise_regular_and_y_words() {
        assert_eq!(count(1, "skill"), "1 skill");
        assert_eq!(count(0, "skill"), "0 skills");
        assert_eq!(count(3, "repository"), "3 repositories");
        assert_eq!(count(1, "repository"), "1 repository");
        assert_eq!(count(2, "key"), "2 keys");
    }

    #[test]
    fn sanitize_replaces_control_characters_and_keeps_the_rest() {
        assert_eq!(
            sanitize("plain text é ✓\tand\nlines"),
            "plain text é ✓\tand\nlines"
        );
        assert_eq!(
            sanitize("a\u{1b}[2Jb\u{7}c\u{85}d\u{7f}e\rf"),
            "a\u{fffd}[2Jb\u{fffd}c\u{fffd}d\u{fffd}e\u{fffd}f"
        );
    }

    #[test]
    fn shell_quote_only_quotes_when_needed() {
        assert_eq!(shell_quote("/home/me/project"), "/home/me/project");
        assert_eq!(shell_quote("/home/me/second app"), "'/home/me/second app'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("$(rm -rf /)"), "'$(rm -rf /)'");
        assert_eq!(shell_quote("a;b"), "'a;b'");
    }

    #[test]
    fn names_are_joined_or_marked_empty() {
        assert_eq!(names(Vec::<String>::new()), "(none)");
        assert_eq!(names(["a", "b"]), "a, b");
    }
}

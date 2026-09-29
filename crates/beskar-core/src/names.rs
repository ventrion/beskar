//! Skill and profile names.
//!
//! Both follow the Agent Skills naming rule: 1 to 64 characters of lowercase
//! ASCII letters, digits and hyphens, not starting or ending with a hyphen,
//! and without consecutive hyphens. Such a name is safe as a directory name,
//! as a file name and as a BSK value.

use std::borrow::Borrow;
use std::fmt;

use crate::{Error, Result};

pub const MAX_NAME_LEN: usize = 64;

/// Why `name` is not a valid skill or profile name, if it is not.
pub fn problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        Some("it is empty")
    } else if name.len() > MAX_NAME_LEN {
        Some("it is longer than 64 characters")
    } else if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        Some("names use lowercase letters, digits and hyphens only")
    } else if name.starts_with('-') || name.ends_with('-') {
        Some("names cannot start or end with a hyphen")
    } else if name.contains("--") {
        Some("names cannot contain two hyphens in a row")
    } else {
        None
    }
}

/// The closest valid name to `raw`: lowercased, with spaces, underscores and
/// dots turned into hyphens and other characters dropped.
pub fn suggest(raw: &str) -> Option<String> {
    let mut out = String::new();
    for c in raw
        .chars()
        .flat_map(|c| fold_accent(c).chars().collect::<Vec<_>>())
    {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if matches!(c, '-' | '_' | ' ' | '.') && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out = out.trim_matches('-').to_string();
    if out.len() > MAX_NAME_LEN {
        out.truncate(MAX_NAME_LEN);
        out = out.trim_end_matches('-').to_string();
    }
    (problem(&out).is_none() && out != raw).then_some(out)
}

/// Advice for an invalid name, recognising habits from other formats:
/// trailing comments, bracketed lists and quotes.
pub fn fix_hint(raw: &str) -> Option<String> {
    if let Some((name, _)) = raw.split_once(" #") {
        let name = name.trim();
        return Some(format!(
            "BSK has no trailing comments: write `{name}` and put the comment on a line of its own"
        ));
    }
    if raw.starts_with('[') {
        return Some(
            "BSK has no `[a, b]` lists: write one line per name, repeating the key".to_string(),
        );
    }
    let unquoted = raw.trim_matches(['"', '\'']);
    if unquoted != raw && problem(unquoted).is_none() {
        return Some(format!("names are written without quotes: `{unquoted}`"));
    }
    suggest(raw).map(|better| format!("try `{better}`"))
}

/// A plain ASCII spelling for common accented Latin letters, so a
/// directory named `café` suggests `cafe` rather than `caf`.
fn fold_accent(c: char) -> String {
    let lower = c.to_lowercase().collect::<String>();
    let folded = match lower.as_str() {
        "à" | "á" | "â" | "ã" | "ä" | "å" | "ā" => "a",
        "ç" | "č" | "ć" => "c",
        "è" | "é" | "ê" | "ë" | "ē" | "ě" => "e",
        "ì" | "í" | "î" | "ï" | "ī" => "i",
        "ñ" | "ń" | "ň" => "n",
        "ò" | "ó" | "ô" | "õ" | "ö" | "ø" | "ō" => "o",
        "ù" | "ú" | "û" | "ü" | "ū" | "ů" => "u",
        "ý" | "ÿ" => "y",
        "ž" | "ź" | "ż" => "z",
        "š" | "ś" => "s",
        "ř" => "r",
        "ß" => "ss",
        "æ" => "ae",
        "œ" => "oe",
        _ => return c.to_string(),
    };
    folded.to_string()
}

macro_rules! name_type {
    ($(#[$doc:meta])* $name:ident, $what:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Validate a name.
            pub fn new(name: &str) -> Result<Self> {
                match problem(name) {
                    None => Ok($name(name.to_string())),
                    Some(reason) => {
                        let error = Error::invalid(format!(concat!("`{}` is not a valid ", $what, " name: {}"), name, reason));
                        Err(match fix_hint(name) {
                            Some(hint) => error.hint(hint),
                            None => error,
                        })
                    }
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
    };
}

name_type!(
    /// The identity of a skill: its directory name in the library and in
    /// every workspace.
    SkillId,
    "skill"
);

name_type!(
    /// The identity of a profile: its file name (without `.bsk`) in the
    /// library's `profiles/` directory.
    ProfileName,
    "profile"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names() {
        for name in ["pdf", "code-review", "a", "web-research-2", &"x".repeat(64)] {
            assert_eq!(problem(name), None, "{name}");
        }
    }

    #[test]
    fn invalid_names() {
        for name in [
            "",
            "PDF",
            "code_review",
            "-pdf",
            "pdf-",
            "a--b",
            "my skill",
            "../x",
            ".hidden",
            &"x".repeat(65),
        ] {
            assert!(problem(name).is_some(), "{name}");
        }
    }

    #[test]
    fn suggestions() {
        assert_eq!(suggest("My Skill").as_deref(), Some("my-skill"));
        assert_eq!(suggest("code_review").as_deref(), Some("code-review"));
        assert_eq!(suggest("--pdf--").as_deref(), Some("pdf"));
        assert_eq!(suggest("pdf"), None);
        assert_eq!(suggest("Unicodé Café").as_deref(), Some("unicode-cafe"));
        assert_eq!(suggest("!!!"), None);
    }

    #[test]
    fn habits_from_other_formats_get_specific_advice() {
        assert_eq!(
            fix_hint("tdd # the tdd skill").as_deref(),
            Some(
                "BSK has no trailing comments: write `tdd` and put the comment on a line of its own"
            )
        );
        assert!(
            fix_hint("[git, pdf]")
                .unwrap()
                .starts_with("BSK has no `[a, b]` lists")
        );
        assert_eq!(
            fix_hint("\"git\"").as_deref(),
            Some("names are written without quotes: `git`")
        );
    }

    #[test]
    fn errors_suggest_a_fix() {
        let error = SkillId::new("Code Review").unwrap_err();
        assert_eq!(
            error.message,
            "`Code Review` is not a valid skill name: names use lowercase letters, digits and hyphens only"
        );
        assert_eq!(error.hints, ["try `code-review`"]);
    }
}

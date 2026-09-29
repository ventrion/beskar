//! Names that identify things in the library.
//!
//! A skill id is a directory name in the library. A profile name is a file
//! name. Both end up in filesystem paths, so they are validated once, when
//! they enter the program, and can be trusted afterwards. A name can never
//! contain a path separator or start with a dot.

use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

use crate::error::{Error, Result};

const MAX_LENGTH: usize = 100;

fn check(what: &str, text: &str) -> Result<()> {
    let problem = if text.is_empty() {
        Some("it is empty")
    } else if text.chars().count() > MAX_LENGTH {
        Some("it is longer than 100 characters")
    } else if !text.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        Some("it must start with a letter or digit")
    } else if text.ends_with('.') {
        Some("it must not end with '.'")
    } else if !text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        Some("it may only contain letters, digits, '.', '-' and '_'")
    } else {
        None
    };
    match problem {
        None => Ok(()),
        Some(reason) => {
            let mut error = Error::invalid(format!("invalid {what} '{text}': {reason}"));
            if let Some(hint) = hint_for(text) {
                error = error.with_hint(hint);
            }
            Err(error)
        }
    }
}

/// A specific suggestion for the mistakes people actually make, or `None`.
fn hint_for(text: &str) -> Option<String> {
    if crate::config::has_trailing_comment(text) {
        return Some("bsk has no trailing comments; put the comment on its own line".to_string());
    }
    if text.contains(',') || text.starts_with('[') {
        return Some("give one name per line".to_string());
    }
    if text.contains('/') || text.contains('\\') {
        return Some("use just the name, not a path".to_string());
    }
    let only_spacing_differs = text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ' | '\t'));
    if only_spacing_differs {
        let cleaned: String = text
            .trim()
            .chars()
            .map(|c| if c.is_whitespace() { '-' } else { c })
            .collect();
        let cleaned = cleaned
            .trim_start_matches(['-', '_', '.'])
            .trim_end_matches('.')
            .to_string();
        if cleaned != text && !cleaned.is_empty() && cleaned.chars().count() <= MAX_LENGTH {
            return Some(format!("did you mean '{cleaned}'?"));
        }
    }
    None
}

macro_rules! name_type {
    ($(#[$meta:meta])* $name:ident, $what:literal) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// Validates `text` and wraps it.
            pub fn parse(text: &str) -> Result<Self> {
                check($what, text)?;
                Ok($name(text.to_string()))
            }

            /// The name as text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({:?})"), self.0)
            }
        }

        impl FromStr for $name {
            type Err = Error;

            fn from_str(text: &str) -> Result<Self> {
                $name::parse(text)
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
    /// The identity of a skill: the name of its directory in the library.
    SkillId,
    "skill id"
);

name_type!(
    /// The identity of a profile: the name of its file in the library, without the extension.
    ProfileName,
    "profile name"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names() {
        for name in [
            "git",
            "code-review",
            "web_research",
            "pdf2",
            "v1.2",
            "A",
            "a.b-c_d",
        ] {
            assert!(SkillId::parse(name).is_ok(), "{name}");
            assert!(ProfileName::parse(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn rejects_names_that_could_escape_a_directory() {
        for name in [
            "",
            ".",
            "..",
            ".hidden",
            "a/b",
            "a\\b",
            "../x",
            "/abs",
            "a b",
            "trailing.",
            "é",
            "a\nb",
            "-x",
            "_x",
        ] {
            assert!(SkillId::parse(name).is_err(), "{name:?}");
        }
        assert!(SkillId::parse(&"a".repeat(101)).is_err());
        assert!(SkillId::parse(&"a".repeat(100)).is_ok());
    }

    #[test]
    fn errors_say_what_is_wrong_and_may_suggest_a_fix() {
        let e = SkillId::parse("Code Review").unwrap_err();
        assert_eq!(
            e.message(),
            "invalid skill id 'Code Review': it may only contain letters, digits, '.', '-' and '_'"
        );
        assert_eq!(e.hint(), Some("did you mean 'Code-Review'?"));
        let e = ProfileName::parse("").unwrap_err();
        assert_eq!(e.message(), "invalid profile name '': it is empty");
        assert_eq!(e.hint(), None);
    }

    #[test]
    fn hints_target_the_mistakes_people_make() {
        let hint = |text: &str| SkillId::parse(text).unwrap_err().hint().map(str::to_string);
        assert_eq!(
            hint("git # the vcs").as_deref(),
            Some("bsk has no trailing comments; put the comment on its own line")
        );
        assert_eq!(hint("[a, b]").as_deref(), Some("give one name per line"));
        assert_eq!(hint("a,b").as_deref(), Some("give one name per line"));
        assert_eq!(
            hint("skills/git").as_deref(),
            Some("use just the name, not a path")
        );
        assert_eq!(hint("= git"), None);
        assert_eq!(
            hint("Code Review").as_deref(),
            Some("did you mean 'Code-Review'?")
        );
        assert_eq!(
            hint("C# tools"),
            None,
            "a hash without a space before it is not a comment"
        );
    }

    #[test]
    fn the_two_kinds_of_name_are_distinct_types_that_order_like_text() {
        let mut ids = [SkillId::parse("b").unwrap(), SkillId::parse("a").unwrap()];
        ids.sort();
        assert_eq!(ids[0].as_str(), "a");
        assert_eq!(format!("{:?}", ids[0]), "SkillId(\"a\")");
        assert_eq!(ids[1].to_string(), "b");
    }

    #[test]
    fn can_be_looked_up_by_str() {
        let mut map = std::collections::BTreeMap::new();
        map.insert(SkillId::parse("git").unwrap(), 1);
        assert_eq!(map.get("git"), Some(&1));
    }
}

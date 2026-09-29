use std::fmt;
use std::str::FromStr;

use crate::error::{Error, Result};

const MAX_LEN: usize = 64;

/// Checks a name that becomes a directory name and a whitespace-free token in
/// Beskar files.
///
/// Names are lowercase on purpose: on a case-insensitive filesystem `Git` and
/// `git` would be one directory.
fn validate(what: &str, name: &str) -> Result<()> {
    let ok_start = name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit());
    let ok_rest = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
    if name.is_empty() || name.chars().count() > MAX_LEN || !ok_start || !ok_rest {
        let mut error =
            Error::invalid(format!("`{name}` is not a valid {what} name")).with_hint(format!(
                "use lowercase letters, digits, `-`, `_` and `.`, starting with a letter or digit, \
                 at most {MAX_LEN} characters"
            ));
        let lower = name.to_ascii_lowercase();
        if lower != name && validate(what, &lower).is_ok() {
            error = error.with_hint(format!("names are lowercase, try `{lower}`"));
        }
        return Err(error);
    }
    Ok(())
}

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident, $what:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(name: impl Into<String>) -> Result<Self> {
                let name = name.into();
                validate($what, &name)?;
                Ok($name(name))
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

        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self> {
                $name::new(s)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

id_type!(
    /// The name of a skill: its directory name in the library and in every
    /// repository it is installed into.
    SkillId,
    "skill"
);
id_type!(
    /// The name of a profile: the stem of its file in the library.
    ProfileId,
    "profile"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names() {
        for name in ["git", "code-review", "web_research", "python3.12", "0day", "a"] {
            assert!(SkillId::new(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn rejects_unsafe_or_awkward_names() {
        for name in [
            "",
            ".hidden",
            "-lead",
            "_lead",
            "a b",
            "a/b",
            "a\\b",
            "..",
            ".",
            "Code",
            "tab\t",
            "ünï",
            &"x".repeat(65),
        ] {
            assert!(SkillId::new(name).is_err(), "{name:?}");
        }
        assert!(SkillId::new("x".repeat(64)).is_ok());
    }

    #[test]
    fn uppercase_gets_a_lowercase_suggestion() {
        let error = SkillId::new("Code-Review").unwrap_err();
        assert!(error.hint().unwrap().contains("`code-review`"));
    }

    #[test]
    fn ids_of_different_kinds_do_not_mix() {
        let skill = SkillId::new("git").unwrap();
        let profile = ProfileId::new("git").unwrap();
        assert_eq!(skill.as_str(), profile.as_str());
    }

    #[test]
    fn ids_sort_by_name() {
        let mut ids = [SkillId::new("b").unwrap(), SkillId::new("a").unwrap()];
        ids.sort();
        assert_eq!(ids[0].as_str(), "a");
    }
}

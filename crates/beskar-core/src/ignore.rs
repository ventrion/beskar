//! Names left out when Beskar copies, fingerprints or compares a skill.
//!
//! These are version-control metadata, operating-system litter and build
//! artifacts that appear when an agent runs a skill's scripts (Python
//! writes `__pycache__` next to the script it runs). Counting them would
//! make a skill look locally modified the first time it was used.
//!
//! Ignored entries are not part of a skill, but they may matter to whoever
//! put them there. Replacing a copy moves them into the new copy, and
//! removing a copy only deletes the disposable ones: regenerable caches
//! and operating-system litter.

use std::ffi::OsStr;

/// Version-control metadata. A skill directory holding one of these is a
/// checkout with its own history.
pub const VCS_PATTERNS: &[&str] = &[".git", ".hg", ".svn"];

/// Litter and caches that can be deleted without asking.
pub const DISPOSABLE_PATTERNS: &[&str] = &[
    ".DS_Store",
    "Thumbs.db",
    "__pycache__",
    "*.pyc",
    "node_modules",
    ".venv",
];

/// A set of name patterns. A pattern matches a single file or directory
/// name at any depth; `*` matches any run of characters and `?` matches one
/// character. An ignored directory is skipped with everything inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ignore {
    extra: Vec<String>,
}

impl Default for Ignore {
    fn default() -> Self {
        Ignore::new(&[])
    }
}

impl Ignore {
    /// The built-in patterns plus `extra`.
    pub fn new(extra: &[String]) -> Self {
        Ignore {
            extra: extra.to_vec(),
        }
    }

    pub fn matches(&self, name: &OsStr) -> bool {
        let name = name.to_string_lossy();
        VCS_PATTERNS
            .iter()
            .chain(DISPOSABLE_PATTERNS)
            .copied()
            .chain(self.extra.iter().map(String::as_str))
            .any(|pattern| glob(pattern, &name))
    }

    /// Whether an ignored entry can be deleted without asking: a built-in
    /// cache or litter pattern matches it.
    pub fn is_disposable(&self, name: &OsStr) -> bool {
        let name = name.to_string_lossy();
        DISPOSABLE_PATTERNS
            .iter()
            .any(|pattern| glob(pattern, &name))
    }
}

/// Wildcard match of a whole name. `*` matches any run of characters, `?`
/// matches exactly one.
pub fn glob(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            backtrack = Some((p, n));
            p += 1;
        } else if let Some((star, matched)) = backtrack {
            p = star + 1;
            n = matched + 1;
            backtrack = Some((star, matched + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(glob("*.pyc", "module.pyc"));
        assert!(!glob("*.pyc", "module.py"));
        assert!(glob("*", ""));
        assert!(glob("a*b*c", "aXXbYYc"));
        assert!(!glob("a*b*c", "aXXbYY"));
        assert!(glob("?.txt", "a.txt"));
        assert!(!glob("?.txt", "ab.txt"));
        assert!(glob(".git", ".git"));
        assert!(!glob(".git", ".gitignore"));
    }

    #[test]
    fn defaults_and_extras() {
        let ignore = Ignore::new(&["*.log".to_string()]);
        assert!(ignore.matches(OsStr::new("__pycache__")));
        assert!(ignore.matches(OsStr::new(".git")));
        assert!(ignore.matches(OsStr::new("debug.log")));
        assert!(!ignore.matches(OsStr::new(".gitignore")));
        assert!(!ignore.matches(OsStr::new("SKILL.md")));
    }

    #[test]
    fn only_caches_and_litter_are_disposable() {
        let ignore = Ignore::new(&[".env".to_string()]);
        assert!(ignore.is_disposable(OsStr::new("node_modules")));
        assert!(ignore.is_disposable(OsStr::new("x.pyc")));
        assert!(!ignore.is_disposable(OsStr::new(".git")));
        assert!(!ignore.is_disposable(OsStr::new(".env")));
    }
}

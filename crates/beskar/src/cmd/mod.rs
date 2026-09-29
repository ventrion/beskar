//! Command handlers, one module per command group.

pub mod library;
pub mod profile;
pub mod registry;
pub mod repo;
pub mod setup;

use beskar_core::{Beskar, ConflictPolicy, Error};

use crate::app::{App, Failure};
use crate::args::Matches;
use crate::output::render_error;

/// `--on-conflict`, falling back to the configured policy.
pub fn conflict_policy(m: &Matches, beskar: &Beskar) -> Result<ConflictPolicy, Failure> {
    match m.value("on-conflict") {
        None => Ok(beskar.config.on_conflict),
        Some(text) => ConflictPolicy::parse(text).ok_or_else(|| {
            Failure::usage(format!("unknown conflict policy `{text}`"))
                .hint("use ask, keep, replace or abort")
        }),
    }
}

/// Plural helper: `count(3, "skill")` is "3 skills".
pub fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A count with a verb that agrees: `counted(1, "workspace", "has", "have")`
/// is "1 workspace has".
pub fn counted(n: usize, noun: &str, one: &str, many: &str) -> String {
    format!("{} {}", count(n, noun), if n == 1 { one } else { many })
}

/// Join names as `a, b and c`.
pub fn join_and<S: AsRef<str>>(items: &[S]) -> String {
    match items {
        [] => String::new(),
        [one] => one.as_ref().to_string(),
        [rest @ .., last] => format!(
            "{} and {}",
            rest.iter()
                .map(|s| s.as_ref())
                .collect::<Vec<_>>()
                .join(", "),
            last.as_ref()
        ),
    }
}

/// Print an error indented under a heading (used when one workspace of
/// many fails).
pub fn print_nested_error(app: &mut App, error: &Error) {
    let style = app.out.style();
    let home = app.env.user_home.clone();
    for line in render_error(error, style, home.as_deref(), "error") {
        app.out.line(format!("  {line}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins() {
        assert_eq!(join_and::<&str>(&[]), "");
        assert_eq!(join_and(&["a"]), "a");
        assert_eq!(join_and(&["a", "b"]), "a and b");
        assert_eq!(join_and(&["a", "b", "c"]), "a, b and c");
        assert_eq!(count(1, "skill"), "1 skill");
        assert_eq!(count(2, "skill"), "2 skills");
    }
}

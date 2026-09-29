//! Asking a person questions at the terminal.
//!
//! Questions go to standard error, so results on standard output stay clean when piped.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use beskar_core::Ignore;
use beskar_core::config::shorten;
use beskar_core::diff;
use beskar_core::error::{Error, Result};
use beskar_core::reconcile::{
    Conflict, ConflictKind, Decision, PolicyResolver, PromotionRisk, Resolution, Resolver,
    promotion_risk,
};

use crate::context::Style;

/// Reads answers from `input` and writes questions to `out`.
pub struct Prompter<'a> {
    input: &'a mut dyn BufRead,
    out: &'a mut dyn Write,
}

impl<'a> Prompter<'a> {
    /// A prompter over the given streams.
    pub fn new(input: &'a mut dyn BufRead, out: &'a mut dyn Write) -> Self {
        Prompter { input, out }
    }

    /// Prints `text` and a line break.
    pub fn say(&mut self, text: &str) {
        let _ = writeln!(self.out, "{text}");
    }

    /// Prints `prompt` and reads one line. `None` means the input ended.
    pub fn ask(&mut self, prompt: &str) -> Option<String> {
        let _ = write!(self.out, "{prompt}");
        let _ = self.out.flush();
        let mut line = String::new();
        match self.input.read_line(&mut line) {
            Ok(0) | Err(_) => {
                let _ = writeln!(self.out);
                None
            }
            Ok(_) => Some(line.trim().to_string()),
        }
    }

    /// Asks a yes or no question. Pressing Enter picks the default. End of input means no.
    pub fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
        loop {
            let Some(answer) = self.ask(&format!("{question} {hint} ")) else {
                return false;
            };
            match answer.to_lowercase().as_str() {
                "" => return default_yes,
                "y" | "yes" => return true,
                "n" | "no" => return false,
                _ => self.say("Please answer y or n."),
            }
        }
    }
}

/// Asks a person how to resolve each conflict.
pub struct TerminalResolver<'a> {
    prompter: Prompter<'a>,
    style: Style,
    user_home: Option<PathBuf>,
}

impl<'a> TerminalResolver<'a> {
    /// A resolver that asks on the given streams.
    pub fn new(
        input: &'a mut dyn BufRead,
        out: &'a mut dyn Write,
        style: Style,
        user_home: Option<PathBuf>,
    ) -> Self {
        TerminalResolver {
            prompter: Prompter::new(input, out),
            style,
            user_home,
        }
    }

    fn explain(kind: ConflictKind) -> &'static str {
        match kind {
            ConflictKind::LocalDrift => "The workspace copy has local modifications.",
            ConflictKind::Diverged => {
                "The workspace copy has local modifications, and the library version has changed since it was installed."
            }
            ConflictKind::ModifiedRemoval => {
                "No enabled profile selects this skill any more, but the workspace copy has local modifications."
            }
            ConflictKind::Untracked => {
                "A folder with this name exists, but Beskar did not install it, and it differs from the library version."
            }
        }
    }
}

/// What a typed answer means. Only whole letters and whole words count: "local" and "leave" must
/// not be read as "l" (replace), because `k` and `l` are neighbours on a keyboard and the second
/// one destroys work.
fn choice(answer: &str) -> Option<char> {
    match answer.trim().to_lowercase().as_str() {
        "k" | "keep" => Some('k'),
        "l" | "replace" => Some('l'),
        "p" | "promote" => Some('p'),
        "d" | "diff" => Some('d'),
        "q" | "quit" | "abort" => Some('q'),
        _ => None,
    }
}

impl Resolver for TerminalResolver<'_> {
    fn resolve(&mut self, conflict: &Conflict<'_>) -> Result<Decision> {
        let style = self.style;
        let removal = conflict.kind == ConflictKind::ModifiedRemoval;
        let repo = shorten(conflict.repo, self.user_home.as_deref());
        self.prompter.say("");
        self.prompter
            .say(&style.bold(&format!("Conflict: {}", conflict.skill)));
        self.prompter
            .say(&style.dim(&format!("Repository: {repo}")));
        self.prompter.say("");
        self.prompter.say(Self::explain(conflict.kind));
        self.prompter.say("");
        self.prompter.say("  [k] keep local");
        self.prompter.say(if removal {
            "  [l] remove it (discards the local changes)"
        } else {
            "  [l] replace with library"
        });
        self.prompter.say(if removal {
            "  [p] promote to library, then remove the workspace copy"
        } else {
            "  [p] promote to library"
        });
        self.prompter.say("  [d] show diff");
        self.prompter.say("  [q] abort (nothing is changed)");
        loop {
            let Some(answer) = self.prompter.ask("Choice [k/l/p/d/q]: ") else {
                return Ok(Decision::Abort);
            };
            match choice(&answer) {
                Some('k') => return Ok(Decision::Resolve(Resolution::Keep)),
                Some('l') => {
                    let question = if removal {
                        format!("Remove {} and discard your changes?", conflict.skill)
                    } else {
                        format!("Discard your changes to {}?", conflict.skill)
                    };
                    if self.prompter.confirm(&question, false) {
                        return Ok(Decision::Resolve(Resolution::Replace));
                    }
                }
                Some('p') => {
                    let why = match promotion_risk(&conflict.facts) {
                        PromotionRisk::None => None,
                        PromotionRisk::OverwritesLibraryChanges => Some(
                            "This replaces changes that were made in the library since this copy was installed.",
                        ),
                        PromotionRisk::UnrelatedToLibrary => Some(
                            "This replaces the library's version with a folder Beskar did not install.",
                        ),
                    };
                    match why {
                        None => return Ok(Decision::Resolve(Resolution::Promote)),
                        Some(why) => {
                            self.prompter.say(why);
                            if self.prompter.confirm("Promote anyway?", false) {
                                return Ok(Decision::Resolve(Resolution::PromoteOverwriting));
                            }
                        }
                    }
                }
                Some('d') => {
                    let left = conflict
                        .library
                        .clone()
                        .unwrap_or_else(|| conflict.workspace.join(".beskar-no-library-copy"));
                    let text = diff::render(
                        &left,
                        &conflict.workspace,
                        "library",
                        "workspace",
                        &Ignore::library(),
                        &Ignore::workspace(),
                    )
                    .map_err(|e| Error::invalid(format!("cannot show the diff: {e}")))?;
                    if text.is_empty() {
                        self.prompter.say("No differences.");
                    } else {
                        self.prompter.say(text.trim_end());
                    }
                }
                Some(_) => return Ok(Decision::Abort),
                None if answer.is_empty() => self.prompter.say("Please answer k, l, p, d or q."),
                None => self
                    .prompter
                    .say(&format!("'{answer}' is not one of k, l, p, d or q.")),
            }
        }
    }
}

/// Whichever resolver the situation calls for.
pub enum AnyResolver<'a> {
    /// Ask a person.
    Terminal(TerminalResolver<'a>),
    /// Apply a fixed policy.
    Policy(PolicyResolver),
}

impl Resolver for AnyResolver<'_> {
    fn resolve(&mut self, conflict: &Conflict<'_>) -> Result<Decision> {
        match self {
            AnyResolver::Terminal(terminal) => terminal.resolve(conflict),
            AnyResolver::Policy(policy) => policy.resolve(conflict),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beskar_core::reconcile::Facts;
    use beskar_core::testing::TempDir;
    use beskar_core::{Fingerprint, SkillId};
    use std::io::Cursor;
    use std::path::Path;

    fn print(seed: u8) -> Fingerprint {
        format!("sha256:{}", format!("{seed:02x}").repeat(32))
            .parse()
            .unwrap()
    }

    /// Runs the resolver with typed `answers`, for a conflict whose library, recorded and workspace
    /// fingerprints are `library`, `recorded` and 9 (so the copy always differs).
    fn run_facts(
        answers: &str,
        kind: ConflictKind,
        with_library: bool,
        library: Option<u8>,
        recorded: Option<u8>,
    ) -> (Decision, String) {
        let dir = TempDir::new("prompt");
        dir.write("ws/SKILL.md", "local\n");
        dir.write("lib/SKILL.md", "library\n");
        let skill = SkillId::parse("code-review").unwrap();
        let repo = PathBuf::from("/home/me/projects/foo");
        let conflict = Conflict {
            repo: &repo,
            skill: &skill,
            kind,
            workspace: dir.path().join("ws"),
            library: with_library.then(|| dir.path().join("lib")),
            facts: Facts {
                library: library.map(print),
                recorded: recorded.map(print),
                workspace: Some(print(9)),
            },
        };
        let mut input = Cursor::new(answers.as_bytes().to_vec());
        let mut out = Vec::new();
        let decision = {
            let mut resolver = TerminalResolver::new(
                &mut input,
                &mut out,
                Style::new(false),
                Some(PathBuf::from("/home/me")),
            );
            resolver.resolve(&conflict).unwrap()
        };
        (decision, String::from_utf8(out).unwrap())
    }

    /// A plain local edit: the library is as it was when the copy was installed.
    fn run(answers: &str, kind: ConflictKind, with_library: bool) -> (Decision, String) {
        run_facts(answers, kind, with_library, Some(1), Some(1))
    }

    #[test]
    fn the_menu_matches_the_brief() {
        let (decision, shown) = run("k\n", ConflictKind::LocalDrift, true);
        assert_eq!(decision, Decision::Resolve(Resolution::Keep));
        assert!(shown.contains("Conflict: code-review\n"));
        assert!(shown.contains("Repository: ~/projects/foo\n"));
        assert!(shown.contains("The workspace copy has local modifications.\n"));
        for line in [
            "[k] keep local",
            "[l] replace with library",
            "[p] promote to library",
            "[d] show diff",
        ] {
            assert!(shown.contains(line), "missing {line}");
        }
    }

    #[test]
    fn each_answer_maps_to_a_decision() {
        assert_eq!(
            run("l\ny\n", ConflictKind::LocalDrift, true).0,
            Decision::Resolve(Resolution::Replace)
        );
        assert_eq!(
            run("p\n", ConflictKind::LocalDrift, true).0,
            Decision::Resolve(Resolution::Promote)
        );
        assert_eq!(
            run("q\n", ConflictKind::LocalDrift, true).0,
            Decision::Abort
        );
        assert_eq!(
            run("KEEP\n", ConflictKind::LocalDrift, true).0,
            Decision::Resolve(Resolution::Keep)
        );
        assert_eq!(
            run("replace\ny\n", ConflictKind::LocalDrift, true).0,
            Decision::Resolve(Resolution::Replace)
        );
        assert_eq!(
            run("quit\n", ConflictKind::LocalDrift, true).0,
            Decision::Abort
        );
    }

    #[test]
    fn only_whole_letters_and_words_count_so_no_typo_can_destroy_work() {
        // "local", "leave", "look" and "let me see" all start with l, the destructive choice.
        for word in [
            "local",
            "leave",
            "look",
            "let me see",
            "lk",
            "kl",
            "kk",
            "kee",
            "x",
        ] {
            let (decision, shown) = run(&format!("{word}\nk\n"), ConflictKind::LocalDrift, true);
            assert_eq!(decision, Decision::Resolve(Resolution::Keep), "{word}");
            assert!(
                shown.contains(&format!("'{word}' is not one of k, l, p, d or q.")),
                "{word}: {shown}"
            );
            assert!(
                !shown.contains("Discard your changes"),
                "{word} must not reach the destructive question"
            );
        }
    }

    #[test]
    fn replacing_needs_a_second_yes_and_the_default_is_no() {
        let (decision, shown) = run("l\nn\nk\n", ConflictKind::LocalDrift, true);
        assert_eq!(
            decision,
            Decision::Resolve(Resolution::Keep),
            "declining goes back to the menu"
        );
        assert!(
            shown.contains("Discard your changes to code-review? [y/N]"),
            "{shown}"
        );
        assert_eq!(
            run("l\n\nk\n", ConflictKind::LocalDrift, true).0,
            Decision::Resolve(Resolution::Keep),
            "Enter means no"
        );
        assert_eq!(
            run("l\n", ConflictKind::LocalDrift, true).0,
            Decision::Abort,
            "no answer at all aborts"
        );
        let (_, shown) = run("l\nn\nq\n", ConflictKind::ModifiedRemoval, true);
        assert!(
            shown.contains("Remove code-review and discard your changes? [y/N]"),
            "{shown}"
        );
    }

    #[test]
    fn end_of_input_aborts_rather_than_guessing() {
        assert_eq!(run("", ConflictKind::LocalDrift, true).0, Decision::Abort);
        assert_eq!(
            run("\n\n", ConflictKind::LocalDrift, true).0,
            Decision::Abort
        );
    }

    #[test]
    fn show_diff_prints_the_diff_and_asks_again() {
        let (decision, shown) = run("d\nk\n", ConflictKind::LocalDrift, true);
        assert_eq!(decision, Decision::Resolve(Resolution::Keep));
        assert!(
            shown.contains("--- library/SKILL.md\n+++ workspace/SKILL.md\n"),
            "{shown}"
        );
        assert!(shown.contains("-library\n+local\n"));
        assert_eq!(shown.matches("Choice [k/l/p/d/q]:").count(), 2);
    }

    #[test]
    fn show_diff_without_a_library_copy_shows_everything_as_added() {
        let (_, shown) = run("d\nq\n", ConflictKind::Untracked, false);
        assert!(shown.contains("+++ workspace/SKILL.md"), "{shown}");
    }

    #[test]
    fn an_empty_or_unknown_answer_is_explained() {
        let (decision, shown) = run("\nk\n", ConflictKind::LocalDrift, true);
        assert_eq!(decision, Decision::Resolve(Resolution::Keep));
        assert!(shown.contains("Please answer k, l, p, d or q."));
    }

    #[test]
    fn promoting_over_library_changes_asks_a_second_question_and_says_so_in_the_decision() {
        // The library (fingerprint 5) differs from what was installed (fingerprint 1).
        let diverged = |answers: &str, kind| run_facts(answers, kind, true, Some(5), Some(1));
        let (decision, shown) = diverged("p\nn\nk\n", ConflictKind::Diverged);
        assert_eq!(
            decision,
            Decision::Resolve(Resolution::Keep),
            "declining the second question returns to the menu"
        );
        assert!(shown.contains("replaces changes that were made in the library"));
        assert_eq!(
            diverged("p\ny\n", ConflictKind::Diverged).0,
            Decision::Resolve(Resolution::PromoteOverwriting)
        );
        assert_eq!(
            run("p\n", ConflictKind::LocalDrift, true).0,
            Decision::Resolve(Resolution::Promote),
            "a plain local edit needs no second question"
        );
    }

    #[test]
    fn a_no_longer_wanted_edit_gets_the_same_warning_when_the_library_has_newer_content() {
        let (decision, shown) = run_facts(
            "p\ny\n",
            ConflictKind::ModifiedRemoval,
            true,
            Some(5),
            Some(1),
        );
        assert_eq!(decision, Decision::Resolve(Resolution::PromoteOverwriting));
        assert!(
            shown.contains("replaces changes that were made in the library"),
            "{shown}"
        );
        let (decision, _) = run_facts("p\n", ConflictKind::ModifiedRemoval, true, Some(1), Some(1));
        assert_eq!(
            decision,
            Decision::Resolve(Resolution::Promote),
            "and none when the library is as it was"
        );
    }

    #[test]
    fn a_folder_beskar_did_not_install_asks_before_replacing_the_librarys_version() {
        let (decision, shown) = run_facts("p\ny\n", ConflictKind::Untracked, true, Some(5), None);
        assert_eq!(decision, Decision::Resolve(Resolution::PromoteOverwriting));
        assert!(shown.contains("a folder Beskar did not install"), "{shown}");
        let (decision, _) = run_facts("p\n", ConflictKind::Untracked, false, None, None);
        assert_eq!(
            decision,
            Decision::Resolve(Resolution::Promote),
            "a skill the library lacks needs no warning"
        );
    }

    #[test]
    fn removal_conflicts_use_removal_wording() {
        let (_, shown) = run("q\n", ConflictKind::ModifiedRemoval, true);
        assert!(shown.contains("No enabled profile selects this skill any more"));
        assert!(shown.contains("[l] remove it (discards the local changes)"));
        assert!(shown.contains("then remove the workspace copy"));
    }

    #[test]
    fn confirm_understands_defaults_and_bad_input() {
        let ask = |answers: &str, default: bool| {
            let mut input = Cursor::new(answers.as_bytes().to_vec());
            let mut out = Vec::new();
            let result = Prompter::new(&mut input, &mut out).confirm("Go?", default);
            (result, String::from_utf8(out).unwrap())
        };
        assert!(ask("\n", true).0);
        assert!(!ask("\n", false).0);
        assert!(ask("y\n", false).0);
        assert!(!ask("No\n", true).0);
        assert!(!ask("", true).0, "end of input is a no");
        let (result, shown) = ask("maybe\ny\n", false);
        assert!(result);
        assert!(shown.contains("Please answer y or n."));
        assert!(shown.starts_with("Go? [y/N] "));
    }

    #[test]
    fn a_path_helper_is_not_needed_here() {
        assert_eq!(Path::new("a").to_str(), Some("a"));
    }
}

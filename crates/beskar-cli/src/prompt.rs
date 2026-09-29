//! Talking to a person at a terminal: yes/no questions and conflict choices.
//! Generic over the input and output streams so it can be tested.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use beskar_core::diff::diff_trees;
use beskar_core::reconcile::{Choice, ConflictResolver, Item, Plan, Resolution};
use beskar_core::{Error, Library, Result};

use crate::render::{conflict_text, resolution_label, show_path};

const TITLE_WIDTH: usize = 22;

/// How many unclear answers to a conflict prompt are put up with before
/// giving up. A script that feeds junk (`yes | beskar ...`) must not loop forever.
const MAX_UNCLEAR: usize = 5;

/// Asks a yes/no question. An empty answer takes the default; end of input
/// counts as no, so a closed stdin never confirms anything.
pub fn confirm(
    input: &mut impl BufRead,
    output: &mut impl Write,
    question: &str,
    default_yes: bool,
) -> io::Result<bool> {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    loop {
        write!(output, "{question} {hint} ")?;
        output.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            writeln!(output)?;
            return Ok(false);
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "" => return Ok(default_yes),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => writeln!(output, "Please answer y or n.")?,
        }
    }
}

/// Settles conflicts by asking. Nothing has been changed when it is asked, and
/// answering `q` (or closing the input) stops the whole repository untouched.
pub struct Interactive<R: BufRead, W: Write> {
    input: R,
    output: W,
    library: Library,
    user_home: Option<PathBuf>,
}

impl<R: BufRead, W: Write> Interactive<R, W> {
    pub fn new(input: R, output: W, library: Library, user_home: Option<PathBuf>) -> Self {
        Interactive { input, output, library, user_home }
    }

    fn show_diff(&mut self, plan: &Plan, item: &Item) -> io::Result<()> {
        let library_copy = self.library.skill_path(&item.skill);
        let installed = plan.skills_dir.join(item.skill.as_str());
        if !library_copy.is_dir() {
            return writeln!(self.output, "\nThe library no longer has `{}`.\n", item.skill);
        }
        if !installed.is_dir() {
            return writeln!(self.output, "\nThere is no skill directory to compare.\n");
        }
        match diff_trees(&library_copy, &installed) {
            Ok(diff) if diff.is_empty() => writeln!(self.output, "\nNo differences.\n"),
            Ok(diff) => {
                writeln!(self.output, "\nComparing the library (-) with this repository (+):\n")?;
                write!(self.output, "{}", diff.render())
            }
            Err(error) => writeln!(self.output, "\nCannot compare: {error}\n"),
        }
    }

    fn ask(&mut self, plan: &Plan, item: &Item) -> io::Result<Choice> {
        let options = item.resolutions();
        writeln!(
            self.output,
            "\nConflict: {} in {}\n\n{}\n",
            item.skill,
            show_path(self.user_home.as_deref(), &plan.repo),
            conflict_text(item)
        )?;
        let letter = |r: Resolution| match r {
            Resolution::Keep => 'k',
            Resolution::Replace => 'l',
            Resolution::Promote => 'p',
        };
        for option in &options {
            let (title, detail) = resolution_label(item, *option);
            writeln!(self.output, "  [{}] {title:<TITLE_WIDTH$}{detail}", letter(*option))?;
        }
        writeln!(
            self.output,
            "  [d] {:<TITLE_WIDTH$}compare the library's copy with this one",
            "show diff"
        )?;
        writeln!(self.output, "  [q] {:<TITLE_WIDTH$}change nothing in this repository", "quit")?;
        let mut letters: Vec<String> = options.iter().map(|o| letter(*o).to_string()).collect();
        letters.extend(["d".to_string(), "q".to_string()]);
        let mut unclear = 0;
        loop {
            if unclear >= MAX_UNCLEAR {
                writeln!(self.output, "\nToo many unclear answers, so nothing was changed.")?;
                return Ok(Choice::Abort);
            }
            write!(self.output, "\nChoice [{}]: ", letters.join("/"))?;
            self.output.flush()?;
            let mut line = String::new();
            if self.input.read_line(&mut line)? == 0 {
                writeln!(self.output, "\nNo answer, so nothing was changed.")?;
                return Ok(Choice::Abort);
            }
            let answer = line.trim().to_ascii_lowercase();
            match answer.as_str() {
                "q" => return Ok(Choice::Abort),
                "d" => self.show_diff(plan, item)?,
                other => {
                    let chosen = options
                        .iter()
                        .find(|o| other.len() == 1 && other.starts_with(letter(**o)))
                        .copied();
                    match chosen {
                        Some(Resolution::Promote) if item.promote_overwrites_library() => {
                            let question = "The library's copy changed since this one was installed. \
                                            Promoting replaces those changes. Promote anyway?";
                            if confirm(&mut self.input, &mut self.output, question, false)? {
                                return Ok(Choice::Use(Resolution::Promote));
                            }
                        }
                        Some(option) => return Ok(Choice::Use(option)),
                        None => {
                            unclear += 1;
                            writeln!(
                                self.output,
                                "Please answer with one of: {}",
                                letters.join(", ")
                            )?;
                        }
                    }
                }
            }
        }
    }
}

impl<R: BufRead, W: Write> ConflictResolver for Interactive<R, W> {
    fn resolve(&mut self, plan: &Plan, item: &Item) -> Result<Choice> {
        self.ask(plan, item).map_err(|e| Error::io("cannot talk to the terminal", &e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beskar_core::reconcile::{Status, Workspace};
    use beskar_core::{Fingerprint, SkillId};
    use std::io::Cursor;

    fn fp(n: u8) -> Fingerprint {
        Fingerprint::parse(&format!("fp1:{}", format!("{n:02x}").repeat(32))).unwrap()
    }

    fn item(status: Status, library: u8, recorded: u8) -> Item {
        Item {
            skill: SkillId::new("code-review").unwrap(),
            status,
            via: Vec::new(),
            library: Some(fp(library)),
            recorded: Some(fp(recorded)),
            workspace: Workspace::Tree(fp(9)),
        }
    }

    fn plan() -> Plan {
        Plan {
            repo: "/home/ana/proj".into(),
            skills_dir: "/home/ana/proj/.agents/skills".into(),
            items: Vec::new(),
            problems: Vec::new(),
            unmanaged: Vec::new(),
        }
    }

    fn ask(answers: &str, item: &Item) -> (Choice, String) {
        let mut output = Vec::new();
        let mut resolver = Interactive::new(
            Cursor::new(answers.to_string()),
            &mut output,
            Library::new("/nonexistent"),
            Some("/home/ana".into()),
        );
        let choice = resolver.resolve(&plan(), item).unwrap();
        drop(resolver);
        (choice, String::from_utf8(output).unwrap())
    }

    #[test]
    fn confirm_understands_answers_and_defaults() {
        let run = |answer: &str, default: bool| {
            confirm(&mut Cursor::new(answer.to_string()), &mut Vec::new(), "Go?", default).unwrap()
        };
        assert!(run("y\n", false));
        assert!(run("YES\n", false));
        assert!(!run("n\n", true));
        assert!(run("\n", true));
        assert!(!run("\n", false));
        assert!(!run("", true), "end of input never confirms");
        assert!(run("maybe\ny\n", false), "unclear answers are asked again");
    }

    #[test]
    fn the_prompt_names_the_skill_and_the_repository() {
        let (_, output) = ask("k\n", &item(Status::Diverged, 1, 2));
        assert!(output.contains("Conflict: code-review in ~/proj"), "{output}");
        assert!(output.contains("[k] keep local"), "{output}");
        assert!(output.contains("[l] replace with library"), "{output}");
        assert!(output.contains("[p] promote to library"), "{output}");
    }

    #[test]
    fn letters_map_to_resolutions() {
        assert_eq!(ask("k\n", &item(Status::Diverged, 1, 2)).0, Choice::Use(Resolution::Keep));
        assert_eq!(ask("L\n", &item(Status::Diverged, 1, 2)).0, Choice::Use(Resolution::Replace));
        assert_eq!(ask("q\n", &item(Status::Diverged, 1, 2)).0, Choice::Abort);
    }

    #[test]
    fn promoting_over_newer_library_work_needs_a_second_yes() {
        let diverged = item(Status::Diverged, 1, 2);
        assert!(diverged.promote_overwrites_library());
        let (choice, output) = ask("p\ny\n", &diverged);
        assert_eq!(choice, Choice::Use(Resolution::Promote));
        assert!(output.contains("Promoting replaces those changes"), "{output}");
        // Declining goes back to the choices instead of promoting.
        let (choice, _) = ask("p\nn\nk\n", &diverged);
        assert_eq!(choice, Choice::Use(Resolution::Keep));
        // The default answer is no.
        let (choice, _) = ask("p\n\nq\n", &diverged);
        assert_eq!(choice, Choice::Abort);
    }

    #[test]
    fn promoting_where_the_library_did_not_move_needs_no_confirmation() {
        let removal = item(Status::RemoveModified, 2, 2);
        assert!(!removal.promote_overwrites_library());
        let (choice, output) = ask("p\n", &removal);
        assert_eq!(choice, Choice::Use(Resolution::Promote));
        assert!(output.contains("then remove it here"), "{output}");
    }

    #[test]
    fn promote_is_not_offered_for_a_directory_beskar_did_not_install() {
        let unmanaged = item(Status::Unmanaged, 1, 2);
        assert!(!unmanaged.can_promote());
        let (choice, output) = ask("p\nk\n", &unmanaged);
        assert_eq!(choice, Choice::Use(Resolution::Keep));
        assert!(output.contains("Please answer with one of"), "{output}");
        assert!(!output.contains("[p]"), "{output}");
    }

    #[test]
    fn end_of_input_aborts_instead_of_guessing() {
        let (choice, output) = ask("", &item(Status::Diverged, 1, 2));
        assert_eq!(choice, Choice::Abort);
        assert!(output.contains("nothing was changed"), "{output}");
    }

    #[test]
    fn asking_for_the_diff_does_not_consume_the_choice() {
        let (choice, output) = ask("d\nk\n", &item(Status::Diverged, 1, 2));
        assert_eq!(choice, Choice::Use(Resolution::Keep));
        assert!(output.contains("The library no longer has `code-review`"), "{output}");
    }

    #[test]
    fn endless_junk_gives_up_instead_of_looping() {
        let junk = "what\n".repeat(1000);
        let (choice, output) = ask(&junk, &item(Status::Diverged, 1, 2));
        assert_eq!(choice, Choice::Abort);
        assert!(output.contains("Too many unclear answers"), "{output}");
    }

    #[test]
    fn junk_input_is_asked_again() {
        let (choice, _) = ask("what\n\nkeep\nl\n", &item(Status::Diverged, 1, 2));
        assert_eq!(choice, Choice::Use(Resolution::Replace));
    }
}

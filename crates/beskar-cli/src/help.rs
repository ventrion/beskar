//! Help text, generated from the command table.

use crate::args::{Flag, GLOBAL};
use crate::ui::{self, Ui};
use crate::{COMMANDS, Command, Exit, GROUPS, Group};

pub fn main(ui: &Ui) -> String {
    let mut out = format!(
        "{} — Better Skill Arrangement\n\n\
         Keep one curated library of agent skills, group them into profiles, and\n\
         materialize exactly the skills each workspace needs into .agents/skills/.\n\n\
         {}\n  beskar <command> [options]\n\n{}\n",
        ui.bold("beskar"),
        ui.bold("Usage:"),
        ui.bold("Commands:"),
    );
    let top: Vec<Vec<String>> = COMMANDS
        .iter()
        .filter(|c| c.path.len() == 1)
        .map(|c| vec![format!("  {}", c.path[0]), c.about.to_string()])
        .collect();
    out.push_str(&ui::table(&top));
    for g in GROUPS {
        out.push_str(&format!("\n{} {}\n", ui.bold(&format!("{}:", g.name)), ui.dim(g.about)));
        let rows: Vec<Vec<String>> = COMMANDS
            .iter()
            .filter(|c| c.path[0] == g.name)
            .map(|c| vec![format!("  {} {}", c.path[1], c.usage).trim_end().to_string(), c.about.to_string()])
            .collect();
        out.push_str(&ui::table(&rows));
    }
    out.push_str(&format!(
        "\n{}\n{}\
         \n{}\n\
         \x20 beskar help <command>    details for one command\n\
         \x20 beskar help format       the Plate file format used for all Beskar files\n\
         \x20 beskar help workflow     a first session, start to finish\n\
         \n\
         Exit status: 0 ok, 1 error, 2 bad usage, 3 stopped for a decision.\n",
        ui.bold("Global options:"),
        flags_table(GLOBAL),
        ui.bold("More:"),
    ));
    out
}

pub fn group(ui: &Ui, g: &Group) -> String {
    let mut out = format!("{} — {}\n\n{}\n", ui.bold(&format!("beskar {}", g.name)), g.about, ui.bold("Commands:"));
    let rows: Vec<Vec<String>> = COMMANDS
        .iter()
        .filter(|c| c.path[0] == g.name)
        .map(|c| vec![format!("  {} {}", c.path[1], c.usage).trim_end().to_string(), c.about.to_string()])
        .collect();
    out.push_str(&ui::table(&rows));
    out.push_str(&format!("\nSee `beskar {} <command> --help` for details.\n", g.name));
    out
}

pub fn usage_line(c: &Command) -> String {
    let opts = if c.flags.is_empty() { "" } else { " [options]" };
    format!("beskar {}{opts} {}", c.path.join(" "), c.usage).trim_end().to_string()
}

pub fn command(ui: &Ui, c: &Command) -> String {
    let mut out = format!("{}\n\n{} {}\n", c.about, ui.bold("Usage:"), usage_line(c));
    if !c.help.is_empty() {
        out.push_str(&format!("\n{}\n", c.help));
    }
    if !c.flags.is_empty() {
        out.push_str(&format!("\n{}\n{}", ui.bold("Options:"), flags_table(c.flags)));
    }
    out
}

fn flags_table(flags: &[Flag]) -> String {
    let rows: Vec<Vec<String>> = flags
        .iter()
        .map(|f| {
            let short = f.short.map(|s| format!("-{s}, ")).unwrap_or_default();
            let value = f.value.map(|v| format!(" <{v}>")).unwrap_or_default();
            vec![format!("  {short}--{}{value}", f.long), f.help.to_string()]
        })
        .collect();
    ui::table(&rows)
}

pub fn topic(ui: &Ui, words: &[&str]) -> Exit {
    match words {
        [] => print!("{}", main(ui)),
        ["format"] => print!("{FORMAT}"),
        ["workflow"] => print!("{WORKFLOW}"),
        [g] if GROUPS.iter().any(|x| x.name == *g) => {
            print!("{}", group(ui, GROUPS.iter().find(|x| x.name == *g).expect("checked")));
        }
        _ => match COMMANDS.iter().find(|c| c.path == words) {
            Some(c) => print!("{}", command(ui, c)),
            None => {
                eprintln!("error: no help for `{}`\n  hint: see `beskar help`", words.join(" "));
                return Exit::Usage;
            }
        },
    }
    Exit::Ok
}

const FORMAT: &str = "\
Plate — the file format of every Beskar file

Each line is exactly one of these, recognizable by its first character:

  # comment              whole-line comment (there are no end-of-line comments)
  [kind label]           section header, e.g. [repo /home/me/api]
  key = value            a single value: everything after `=`, trimmed
  key:                   a list; its items follow on their own lines
    - item               a list item: everything after `- `, trimmed

Rules that keep it unambiguous:

  * Values are text, taken verbatim. No quotes, no escapes, no types:
    `on = no` is the two letters n and o, `x = 007` keeps its zeros.
  * Quoting is an error, not a feature: `key = \"x\"` is rejected so a stray
    TOML/YAML habit cannot sneak quote characters into your data.
  * `#` only starts a comment at the beginning of a line; `a = b # c` has
    the value `b # c` (Beskar warns about this).
  * Indentation never matters; indent list items for readability.
  * Keys are lowercase: a-z, 0-9, `-`, `_`. A key appears once per section.
  * Anything else is an error with a line number and a hint.

Example (a profile, library/profiles/coding.plate):

  # Everything I want when writing code.
  description = Everyday software engineering

  skills:
    - code-review
    - git
    - testing

Beskar keeps your comments and layout when it edits a profile.
";

const WORKFLOW: &str = "\
A first session

  beskar init                              # config, library, registry
  beskar library scan ~/my-skills          # import skills (--yes when scripted)
  beskar profile create coding code-review git testing

  cd ~/projects/app
  beskar repo add . --enable coding        # register + enable in one go
  beskar repo update --dry-run             # see the plan
  beskar repo update                       # materialize .agents/skills/

Later, after improving a skill in the library:

  beskar update --all                      # propagate to every workspace

After improving a skill inside a workspace:

  beskar skill diff code-review            # review the local changes
  beskar skill promote code-review         # make them canonical
  beskar update --all                      # hand them to other workspaces

Keep an eye on things:

  beskar status                            # this workspace
  beskar registry status                   # every workspace
  beskar registry status --skill git       # where is `git` installed, and why
  beskar doctor                            # full health check
";

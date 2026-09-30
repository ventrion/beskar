//! Help text.

use crate::commands::{COMMANDS, Command, GLOBAL_FLAGS, GROUPS, Group, subcommands};
use crate::output::Style;

pub fn usage_line(command: &Command) -> String {
    let mut line = format!("beskar {}", command.path.join(" "));
    if !command.usage.is_empty() {
        line.push(' ');
        line.push_str(command.usage);
    }
    if !command.flags.is_empty() {
        line.push_str(" [flags]");
    }
    line
}

fn entry(command: &Command) -> (String, &'static str) {
    let mut name = command.path.join(" ");
    if !command.usage.is_empty() {
        name.push(' ');
        name.push_str(command.usage);
    }
    (name, command.summary)
}

fn listing(entries: &[(String, &str)], style: Style) -> String {
    let width = entries
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    entries
        .iter()
        .map(|(name, summary)| {
            let pad = " ".repeat(width - name.chars().count() + 3);
            format!("  {}{pad}{summary}", style.bold(name))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn overview(style: Style) -> String {
    let mut out = format!(
        "{} {}: Better Skill Arrangement\n\
         One curated library of agent skills; each workspace gets the skills its profiles name.\n\n\
         {} beskar <command> [arguments] [flags]\n",
        style.bold("beskar"),
        env!("CARGO_PKG_VERSION"),
        style.bold("Usage:"),
    );
    let top = |names: &[&str]| -> Vec<(String, &'static str)> {
        names
            .iter()
            .map(|name| {
                entry(
                    COMMANDS
                        .iter()
                        .find(|c| c.path == [*name])
                        .expect("command exists"),
                )
            })
            .collect()
    };
    out += &format!(
        "\n{}\n{}\n",
        style.bold("Setup"),
        listing(&top(&["init", "doctor"]), style)
    );
    for group in GROUPS {
        let entries: Vec<(String, &str)> = subcommands(group.name).into_iter().map(entry).collect();
        out += &format!(
            "\n{}\n{}\n",
            style.bold(group.title),
            listing(&entries, style)
        );
    }
    let mut shortcuts = top(&["status", "update"]);
    shortcuts[0].0 = "status [--all]".into();
    shortcuts[1].0 = "update [--all]".into();
    out += &format!(
        "\n{}\n{}\n",
        style.bold("Shortcuts"),
        listing(&shortcuts, style)
    );
    let more = [
        ("help <command>".to_string(), "Show a command's help"),
        (
            "help format".to_string(),
            "The .bsk file format of profiles, config and registry",
        ),
        (
            "help json".to_string(),
            "The JSON that --json prints, for scripts and agents",
        ),
    ];
    out += &format!("\n{}\n{}\n", style.bold("Help"), listing(&more, style));
    let globals: Vec<(String, &str)> = GLOBAL_FLAGS
        .iter()
        .map(|flag| (format!("--{}", flag.long), flag.help))
        .collect();
    out += &format!(
        "\n{}\n{}\n",
        style.bold("Flags for every command"),
        listing(&globals, style)
    );
    out += "\nFiles live in ~/.beskar (set BESKAR_HOME to move them). BESKAR_LOCK_TIMEOUT sets how many\n\
            seconds to wait for another beskar process. NO_COLOR turns off colors.\n\
            Exit status: 0 success, 1 error, 2 usage error, 3 a decision is needed (a conflict,\n\
            or a confirmation that --yes gives without a terminal).";
    out
}

pub fn group(group: &Group, style: Style) -> String {
    let entries: Vec<(String, &str)> = subcommands(group.name).into_iter().map(entry).collect();
    format!(
        "{}\n\n{}\n\n{}\n{}\n\nRun `beskar help {} <command>` for a command's details.",
        style.bold(group.title),
        group.about,
        style.bold("Commands"),
        listing(&entries, style),
        group.name
    )
}

pub fn command(command: &Command, style: Style) -> String {
    let mut out = format!(
        "{}: {}\n\n{} {}\n\n{}\n",
        style.bold(&format!("beskar {}", command.path.join(" "))),
        command.summary,
        style.bold("Usage:"),
        usage_line(command),
        command.about
    );
    let mut flags: Vec<(String, &str)> = command
        .flags
        .iter()
        .map(|flag| {
            let short = flag
                .short
                .map(|c| format!("-{c}, "))
                .unwrap_or_else(|| "    ".to_string());
            let value = flag.value.map(|v| format!(" {v}")).unwrap_or_default();
            (format!("{short}--{}{value}", flag.long), flag.help)
        })
        .collect();
    flags.push(("-h, --help".to_string(), "Show this help"));
    flags.extend(
        GLOBAL_FLAGS
            .iter()
            .map(|flag| (format!("    --{}", flag.long), flag.help)),
    );
    out += &format!("\n{}\n{}", style.bold("Flags:"), listing(&flags, style));
    out
}

pub fn format(style: Style) -> String {
    format!(
        "{title}

Beskar's files use BSK, a line-based format: config.bsk and registry.bsk in
~/.beskar, and profiles/<name>.bsk in the library. Every line is one of:

  # comment         a comment line; `#` is the first character on the line
  key: value        an entry; the value is the rest of the line, as written
  [name label]      a section header; the entries below it belong to it
                    a blank line, which is ignored

{rules}
  - The first non-blank character decides what a line is. A line that is not
    a comment, a section header or an entry is an error.
  - Keys are lowercase letters, digits and `-`, starting with a letter.
  - Values are taken verbatim: no quotes, no escapes, no types. Spaces around
    a value are dropped. A `#` inside a value is part of the value.
  - A key that takes several values is repeated, one line per value. Adding
    or removing an item means adding or removing a line.
  - Indentation is allowed and means nothing. Nothing nests beyond sections.
  - Files are UTF-8, with LF or CRLF line endings.

{example} a profile, profiles/coding.bsk

  # Skills for everyday software work.
  description: Everyday software development
  skill: code-review
  skill: git
  skill: testing

{example} the config, ~/.beskar/config.bsk

  library: ~/.beskar/library
  registry: ~/.beskar/registry.bsk
  skills-dir: .agents/skills
  on-conflict: ask
  ignore: *.log

When Beskar edits a profile or the config, it changes only the lines
involved and keeps your comments. The registry is Beskar's own state and is
rewritten whole. Mistakes are reported with file, line and column, usually
with the corrected line:

  error: use `:` between a key and its value
   --> ~/.beskar/config.bsk:3:9
    |
  3 | library = ~/lib
    |         ^
  help: write `library: ~/lib`",
        title = style.bold("BSK: the Beskar file format"),
        rules = style.bold("Rules"),
        example = style.bold("Example:"),
    )
}

pub fn json(style: Style) -> String {
    format!(
        "{title}

With --json every command prints exactly one JSON document on standard
output and never asks a question. Conflicts follow --on-conflict, where
`ask` stops like `abort`, and confirmations need --yes. Notices also reach
standard error, for a person watching.

  {{
    \"ok\": true,                  exit == 0
    \"command\": \"repo update\",    what ran; null if the command line was not understood
    \"exit\": 0,                   the exit status: 0, 1, 2 or 3
    \"data\": {{ ... }},             the result, whenever the command produced one
    \"error\": {{ ... }},            why it failed, when it did
    \"notices\": [ ... ]           waits for the lock, leftovers of interrupted runs
  }}

{errors} {{\"kind\", \"message\", \"hints\": [...]}}, plus \"file\", \"line\" and
\"column\" for a mistake in one of Beskar's files. Kinds: usage, not_initialized,
not_found, already_exists, invalid, conflict, locked, io.

{plans} (status, update, enable, disable, toggle) list one step per skill:
  skill, action, conflict, profiles, library, recorded, kept, present,
  blocked, stays
Actions: install, restore, update, remove, release, forget, record,
unchanged, keep_local, conflict (diverged, untracked or orphaned),
missing_source, unmanaged. Fingerprints are 64 hexadecimal digits, and
paths are absolute.

An update reports each workspace with a result (up_to_date, planned,
stopped, applied, failed or error), its steps, and outcomes such as
{{\"skill\": \"git\", \"done\": \"updated\"}} or {{\"skill\": ..., \"error\": {{...}}}}.

Field names are stable; new fields may be added. docs/JSON.md in the
source describes every command's data.",
        title = style.bold("JSON output (--json)"),
        errors = style.bold("Errors:"),
        plans = style.bold("Plans"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_mentions_every_command() {
        let text = overview(Style::new(false));
        for command in COMMANDS {
            assert!(text.contains(&command.path.join(" ")), "{:?}", command.path);
        }
    }

    #[test]
    fn command_help_lists_flags() {
        let update = COMMANDS
            .iter()
            .find(|c| c.path == ["repo", "update"])
            .unwrap();
        let text = command(update, Style::new(false));
        assert!(text.contains("Usage: beskar repo update [flags]"), "{text}");
        assert!(text.contains("-n, --dry-run"), "{text}");
        assert!(text.contains("--on-conflict POLICY"), "{text}");
    }
}

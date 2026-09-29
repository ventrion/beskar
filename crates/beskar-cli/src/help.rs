//! Help text, generated from the command table so it cannot drift from it.

use std::fmt::Write as _;

use crate::app::{Command, VERSION};
use crate::commands::{COMMANDS, GROUPS};

pub fn root() -> String {
    let mut out = format!(
        "beskar {VERSION}, better skill arrangement\n\n\
         One curated library of agent skills, profiles that group them, and repositories\n\
         that receive exactly the skills they need in .agents/skills.\n\n\
         Usage: beskar <command> [options]\n\n\
         Commands:\n"
    );
    let mut rows: Vec<(String, &str)> = Vec::new();
    for command in COMMANDS.iter().filter(|c| c.path.len() == 1) {
        rows.push((
            format!("{} {}", command.path[0], command.synopsis).trim().to_string(),
            command.about,
        ));
    }
    for (group, about) in GROUPS {
        rows.push((format!("{group} <command>"), about));
    }
    rows.sort_by_key(|(name, _)| order(name));
    rows.push(("help [format]".to_string(), "Show help, or describe the file format"));
    let width = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (name, about) in rows {
        let _ = writeln!(out, "  {name:<width$}  {about}");
    }
    out.push_str(
        "\nGlobal options:\n\
         \x20 --home <dir>   Use this Beskar home (default: $BESKAR_HOME, else ~/.beskar)\n\
         \x20 -h, --help     Show help for a command\n\
         \x20 -V, --version  Show the version\n\n\
         A typical session:\n\
         \x20 beskar init\n\
         \x20 beskar library scan ~/my-skills\n\
         \x20 beskar profile create coding\n\
         \x20 beskar profile add coding git code-review testing\n\
         \x20 cd ~/projects/app\n\
         \x20 beskar repo add .\n\
         \x20 beskar repo enable coding\n\
         \x20 beskar repo update\n\n\
         Files use one-fact-per-line `.bsk` files. `beskar help format` explains them.\n",
    );
    out
}

fn order(name: &str) -> usize {
    ["init", "doctor", "library", "profile", "repo", "registry", "status", "update"]
        .iter()
        .position(|first| name.starts_with(first))
        .unwrap_or(usize::MAX)
}

pub fn group(name: &str) -> String {
    let about = GROUPS.iter().find(|(g, _)| *g == name).map_or("", |(_, about)| *about);
    let mut out = format!(
        "beskar {name}: {about}\n\nUsage: beskar {name} <command> [options]\n\nCommands:\n"
    );
    let commands: Vec<&Command> =
        COMMANDS.iter().filter(|c| c.path.len() == 2 && c.path[0] == name).collect();
    let rows: Vec<(String, &str)> = commands
        .iter()
        .map(|c| (format!("{} {}", c.path[1], c.synopsis).trim().to_string(), c.about))
        .collect();
    let width = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (usage, about) in rows {
        let _ = writeln!(out, "  {usage:<width$}  {about}");
    }
    let _ = write!(out, "\nRun `beskar {name} <command> --help` for a command's options.\n");
    out
}

pub fn command(command: &Command) -> String {
    let options = if command.flags.is_empty() { "" } else { " [options]" };
    let usage = format!("beskar {} {}{options}", command.name(), command.synopsis);
    let mut out = format!("Usage: {}\n\n{}\n", usage.replace("  ", " ").trim_end(), command.about);
    if !command.notes.is_empty() {
        let _ = write!(out, "\n{}\n", command.notes);
    }
    if !command.flags.is_empty() {
        out.push_str("\nOptions:\n");
        let labels: Vec<String> = command
            .flags
            .iter()
            .map(|flag| {
                let short = flag.short.map_or_else(|| "    ".to_string(), |c| format!("-{c}, "));
                let value = flag.value.map_or(String::new(), |v| format!(" <{v}>"));
                format!("{short}--{}{value}", flag.name)
            })
            .collect();
        let width = labels.iter().map(String::len).max().unwrap_or(0);
        for (label, flag) in labels.iter().zip(command.flags) {
            let _ = writeln!(out, "  {label:<width$}  {}", flag.help);
        }
    }
    out
}

/// `beskar help [topic]`.
pub fn topic(topic: Option<&str>) -> i32 {
    match topic {
        None => {
            put!("{}", root());
            0
        }
        Some("format") => {
            say!("{}", beskar_lines::SPEC.trim_end());
            say!("\n{}", beskar_core::FILE_KINDS.trim_end());
            0
        }
        Some(name) => {
            if let Some(command) = COMMANDS.iter().find(|c| c.path == [name]) {
                put!("{}", self::command(command));
                return 0;
            }
            if GROUPS.iter().any(|(g, _)| *g == name) {
                put!("{}", group(name));
                return 0;
            }
            complain!(
                "error: no help topic `{name}`\n\nTry `beskar help` or `beskar help format`."
            );
            2
        }
    }
}

//! One command table drives validation and help, adapted from PR #9's command specification.
use crate::args::Args;
use beskar_core::Result;

pub struct Command {
    pub path: &'static str,
    pub arguments: &'static str,
    pub about: &'static str,
    pub min: usize,
    pub max: usize,
    pub flags: &'static [&'static str],
    pub values: &'static [&'static str],
}
macro_rules! command {
    ($path:literal, $args:literal, $about:literal, $min:expr, $max:expr, [$($flag:literal),*], [$($value:literal),*]) => {
        Command { path: $path, arguments: $args, about: $about, min: $min, max: $max, flags: &[$($flag),*], values: &[$($value),*] }
    }
}
pub const COMMANDS: &[Command] = &[
    command!(
        "init",
        "[--library PATH] [--registry PATH] [--agent-skills PATH]",
        "Initialize settings, library and registry",
        0,
        0,
        [],
        ["library", "registry", "agent-skills"]
    ),
    command!(
        "doctor",
        "[--recover]",
        "Check state and safely recover interrupted transactions",
        0,
        0,
        ["recover"],
        []
    ),
    command!("config show", "", "Show configuration", 0, 0, [], []),
    command!(
        "config path",
        "",
        "Print the configuration file path",
        0,
        0,
        [],
        []
    ),
    command!(
        "config set",
        "KEY VALUE",
        "Edit a setting while preserving comments",
        2,
        2,
        [],
        []
    ),
    command!(
        "library init",
        "",
        "Create the library directories",
        0,
        0,
        [],
        []
    ),
    command!("library list", "", "List canonical skills", 0, 0, [], []),
    command!(
        "library show",
        "NAME",
        "Show content, fingerprint and usage",
        1,
        1,
        [],
        []
    ),
    command!(
        "library add",
        "PATH [--name NAME] [--requires SKILL,...] [--dry-run]",
        "Import a skill directory",
        1,
        1,
        ["dry-run"],
        ["name", "requires"]
    ),
    command!(
        "library scan",
        "PATH [--yes] [--dry-run]",
        "Discover and import a batch of skills",
        1,
        1,
        ["yes", "dry-run"],
        []
    ),
    command!(
        "library remove",
        "NAME [--dry-run]",
        "Remove an unreferenced library skill",
        1,
        1,
        ["dry-run"],
        []
    ),
    command!(
        "library require",
        "NAME SKILL...",
        "Install SKILLs wherever NAME is installed",
        2,
        usize::MAX,
        [],
        []
    ),
    command!(
        "library unrequire",
        "NAME SKILL...",
        "Stop installing SKILLs because of NAME",
        2,
        usize::MAX,
        [],
        []
    ),
    command!(
        "profile create",
        "NAME [SKILL...]",
        "Create a reusable set of skills",
        1,
        usize::MAX,
        [],
        []
    ),
    command!(
        "profile delete",
        "NAME",
        "Delete a profile that no repository enables",
        1,
        1,
        [],
        []
    ),
    command!(
        "profile list",
        "",
        "List profiles and skill counts",
        0,
        0,
        [],
        []
    ),
    command!(
        "profile show",
        "NAME",
        "Show a profile and its repositories",
        1,
        1,
        [],
        []
    ),
    command!(
        "profile add",
        "NAME SKILL...",
        "Add skills to a profile",
        2,
        usize::MAX,
        [],
        []
    ),
    command!(
        "profile remove",
        "NAME SKILL...",
        "Remove skills from a profile",
        2,
        usize::MAX,
        [],
        []
    ),
    command!(
        "repo add",
        "[PATH]",
        "Register an existing workspace",
        0,
        1,
        [],
        ["repo"]
    ),
    command!(
        "repo remove",
        "[PATH]",
        "Unregister and preserve all workspace files",
        0,
        1,
        [],
        ["repo"]
    ),
    command!(
        "repo list",
        "",
        "List registered repositories",
        0,
        0,
        [],
        []
    ),
    command!(
        "repo status",
        "[--all] [--repo PATH]",
        "Inspect installed and desired skills",
        0,
        0,
        ["all"],
        ["repo"]
    ),
    command!(
        "repo enable",
        "PROFILE... [--repo PATH]",
        "Enable profiles without installing files",
        1,
        usize::MAX,
        [],
        ["repo"]
    ),
    command!(
        "repo disable",
        "PROFILE... [--repo PATH]",
        "Disable profiles without removing files",
        1,
        usize::MAX,
        [],
        ["repo"]
    ),
    command!(
        "repo toggle",
        "PROFILE... [--repo PATH]",
        "Toggle profile selections",
        1,
        usize::MAX,
        [],
        ["repo"]
    ),
    command!(
        "repo update",
        "[--all] [--repo PATH] [--dry-run] [--conflict POLICY]",
        "Reconcile copies with enabled profiles",
        0,
        0,
        ["all", "dry-run"],
        ["repo", "conflict"]
    ),
    command!(
        "registry list",
        "[--profile NAME | --skill NAME]",
        "List repositories, optionally filtered by usage",
        0,
        0,
        [],
        ["profile", "skill"]
    ),
    command!(
        "registry where",
        "--profile NAME | --skill NAME",
        "Find where a profile or skill is used",
        0,
        0,
        [],
        ["profile", "skill"]
    ),
    command!(
        "registry status",
        "",
        "Inspect every registered repository",
        0,
        0,
        [],
        []
    ),
    command!(
        "registry stats",
        "",
        "Count repositories, profiles and skills",
        0,
        0,
        [],
        []
    ),
    command!(
        "registry update",
        "--all [--dry-run] [--conflict POLICY]",
        "Update every repository as one transaction",
        0,
        0,
        ["all", "dry-run"],
        ["conflict"]
    ),
    command!(
        "registry prune",
        "[--dry-run]",
        "Forget repositories whose paths no longer exist",
        0,
        0,
        ["dry-run"],
        []
    ),
    command!(
        "skill diff",
        "NAME [--repo PATH]",
        "Compare the library with a workspace copy",
        1,
        1,
        [],
        ["repo"]
    ),
    command!(
        "skill promote",
        "NAME [--repo PATH] [--dry-run] [--conflict abort|replace]",
        "Copy a tracked local improvement into the library",
        1,
        1,
        ["dry-run"],
        ["repo", "conflict"]
    ),
    command!(
        "status",
        "[--all] [--repo PATH]",
        "Inspect the current repository or all repositories",
        0,
        0,
        ["all"],
        ["repo"]
    ),
    command!(
        "update",
        "[--all] [--repo PATH] [--dry-run] [--conflict POLICY]",
        "Update the current repository or all repositories",
        0,
        0,
        ["all", "dry-run"],
        ["repo", "conflict"]
    ),
];
pub fn find(args: &Args) -> Result<(&'static Command, usize)> {
    for command in COMMANDS {
        let words: Vec<_> = command.path.split_whitespace().collect();
        if args
            .words
            .iter()
            .map(String::as_str)
            .take(words.len())
            .eq(words.iter().copied())
            && args.words.len() >= words.len()
        {
            let size = words.len();
            args.count(
                size + command.min,
                command.max.saturating_add(size),
                &format!("{} {}", command.path, command.arguments),
            )?;
            args.check(command.flags, command.values)?;
            if args.value("conflict").is_some() {
                args.policy()?;
            }
            if args.flag("all") && args.value("repo").is_some() {
                return Err("--all and --repo cannot be combined".into());
            }
            if command.path == "registry update" && !args.flag("all") {
                return Err("usage: beskar registry update --all".into());
            }
            if args.value("profile").is_some() && args.value("skill").is_some() {
                return Err("choose exactly one of --profile NAME and --skill NAME".into());
            }
            if args.words[size..].iter().any(|w| w.is_empty()) {
                return Err("arguments cannot be empty".into());
            }
            return Ok((command, size));
        }
    }
    Err(format!(
        "unknown command {:?}; use beskar --help",
        args.words.join(" ")
    ))
}
pub fn help(topic: &[String]) -> Result<String> {
    let topic = topic.join(" ");
    if topic == "format" {
        return Ok(include_str!("../../../docs/FORMAT.md").into());
    }
    let selected: Vec<_> = COMMANDS
        .iter()
        .filter(|c| topic.is_empty() || c.path == topic || c.path.starts_with(&format!("{topic} ")))
        .collect();
    if selected.is_empty() {
        return Err(format!("unknown help topic {topic:?}"));
    }
    let mut text = String::from(
        "Beskar: reusable skills for local workspaces\n\nUsage: beskar [--home PATH] [--json] <command>\n\n",
    );
    for c in selected {
        text.push_str(&format!(
            "  {} {}\n      {}\n",
            c.path, c.arguments, c.about
        ));
    }
    text.push_str("\nConflict policies: ask, abort, keep, replace. --on-conflict is an alias.\nInteractive updates ask; scripts fail unless a policy is explicit.\nRepository commands use the nearest registered ancestor.\nUse beskar help format for the .bsk grammar. No Git or network required.\n");
    Ok(text)
}

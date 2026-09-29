//! The command table. Each command is a row: where it lives in the tree, what
//! arguments it takes, its options and the function that runs it.

use crate::app::Command;
use crate::args::{FlagSpec, option, switch};

mod library;
mod profile;
mod registry;
mod repo;
mod setup;

pub const GROUPS: &[(&str, &str)] = &[
    ("library", "Manage the skills you own"),
    ("profile", "Group skills by use case"),
    ("repo", "Choose profiles for a repository and install their skills"),
    ("registry", "See and update every registered repository"),
];

const REPO: FlagSpec = option(
    "repo",
    "path",
    "Repository to act on (default: the registered repository containing the current directory)",
);
const ON_CONFLICT: FlagSpec = option(
    "on-conflict",
    "policy",
    "What to do when an update would overwrite or delete local changes: ask, abort, keep or replace",
);
const DRY_RUN: FlagSpec = switch("dry-run", "Show what would change and change nothing").short('n');
const ALL_REPOS: FlagSpec = switch("all", "Act on every registered repository");
const YES: FlagSpec = switch("yes", "Do not ask for confirmation").short('y');
const FORCE: FlagSpec = switch("force", "Go ahead even though something else depends on it");

const CONFLICT_NOTES: &str = "\
A conflict is a skill that was modified in the repository and that the update would
overwrite or delete. Beskar never resolves one silently. The policy decides:
  ask      prompt for each one on a terminal (the default there)
  abort    change nothing in that repository and report the conflicts (the default elsewhere)
  keep     leave the modified copies alone and update the rest
  replace  overwrite or delete the modified copies, discarding the local changes
Set a default with `on-conflict` in the config file.";

pub const COMMANDS: &[Command] = &[
    Command {
        path: &["init"],
        synopsis: "",
        about: "Set up Beskar: config, registry and library",
        notes: "Creates what is missing and leaves what exists alone, so it is safe to run again.\n\
                Files live in ~/.beskar (or $BESKAR_HOME, or --home).",
        flags: &[option("library", "dir", "Keep the library here instead of in the Beskar home")],
        run: setup::init,
    },
    Command {
        path: &["doctor"],
        synopsis: "",
        about: "Check the config, library, registry, repositories and installed skills",
        notes: "Exits 1 when it finds an error. Warnings and notes do not change the exit code.",
        flags: &[],
        run: setup::doctor,
    },
    Command {
        path: &["status"],
        synopsis: "",
        about: "Shortcut for `repo status`",
        notes: "",
        flags: &[REPO, ALL_REPOS],
        run: repo::status,
    },
    Command {
        path: &["update"],
        synopsis: "",
        about: "Shortcut for `repo update`",
        notes: CONFLICT_NOTES,
        flags: &[REPO, ALL_REPOS, DRY_RUN, ON_CONFLICT],
        run: repo::update,
    },
    // ---- library ----
    Command {
        path: &["library", "init"],
        synopsis: "",
        about: "Create the library directories if they are missing",
        notes: "`beskar init` already does this. Use this to repair a library whose directories were deleted.",
        flags: &[],
        run: library::init,
    },
    Command {
        path: &["library", "add"],
        synopsis: "<path>",
        about: "Import one skill directory into the library",
        notes: "The skill is named after its directory. It is copied, so the original can go.\n\
                A skill that is already there is left alone when identical, and refused when different\n\
                unless you pass --replace.",
        flags: &[
            option("name", "name", "Import under this name instead of the directory's"),
            switch("replace", "Overwrite a different skill of the same name"),
        ],
        run: library::add,
    },
    Command {
        path: &["library", "scan"],
        synopsis: "<path>",
        about: "Find skills under a directory and offer to import them",
        notes: "A skill is a directory containing SKILL.md. On a terminal it asks before importing.\n\
                Elsewhere it only lists what it found, unless you pass --yes.",
        flags: &[YES, switch("replace", "Also overwrite skills that exist with different content")],
        run: library::scan,
    },
    Command {
        path: &["library", "list"],
        synopsis: "",
        about: "List the skills in the library",
        notes: "",
        flags: &[],
        run: library::list,
    },
    Command {
        path: &["library", "show"],
        synopsis: "<skill>",
        about: "Show a skill: metadata, files, profiles and where it is installed",
        notes: "",
        flags: &[],
        run: library::show,
    },
    Command {
        path: &["library", "remove"],
        synopsis: "<skill>",
        about: "Remove a skill from the library",
        notes: "Refuses while a profile lists the skill. With --force it is removed from those\n\
                profiles too. Repositories that already have it keep their copy until the next update.",
        flags: &[FORCE],
        run: library::remove,
    },
    // ---- profile ----
    Command {
        path: &["profile", "create"],
        synopsis: "<name>",
        about: "Create an empty profile",
        notes: "",
        flags: &[option("description", "text", "One line saying what the profile is for")],
        run: profile::create,
    },
    Command {
        path: &["profile", "delete"],
        synopsis: "<name>",
        about: "Delete a profile",
        notes: "Refuses while a repository has it enabled. With --force it is disabled there too.",
        flags: &[FORCE],
        run: profile::delete,
    },
    Command {
        path: &["profile", "list"],
        synopsis: "",
        about: "List profiles",
        notes: "",
        flags: &[],
        run: profile::list,
    },
    Command {
        path: &["profile", "show"],
        synopsis: "<name>",
        about: "Show a profile's skills and the repositories that use it",
        notes: "",
        flags: &[],
        run: profile::show,
    },
    Command {
        path: &["profile", "add"],
        synopsis: "<profile> <skill>...",
        about: "Add skills from the library to a profile",
        notes: "",
        flags: &[],
        run: profile::add,
    },
    Command {
        path: &["profile", "remove"],
        synopsis: "<profile> <skill>...",
        about: "Remove skills from a profile",
        notes: "",
        flags: &[],
        run: profile::remove,
    },
    // ---- repo ----
    Command {
        path: &["repo", "add"],
        synopsis: "[path]",
        about: "Register a directory as a repository (default: the current directory)",
        notes: "It does not have to be a Git repository. Registering installs nothing; enable\n\
                profiles and run `beskar repo update` for that.",
        flags: &[],
        run: repo::add,
    },
    Command {
        path: &["repo", "remove"],
        synopsis: "[path]",
        about: "Stop managing a repository",
        notes: "Installed skills stay on disk by default and simply stop being managed.\n\
                With --purge the skills Beskar installed are deleted first.\n\n\
                Locally modified skills are conflicts under --purge:\n\
                pass --on-conflict to decide, as with update.",
        flags: &[switch("purge", "Delete the skills Beskar installed here"), ON_CONFLICT],
        run: repo::remove,
    },
    Command {
        path: &["repo", "list"],
        synopsis: "",
        about: "List registered repositories and their profiles",
        notes: "",
        flags: &[],
        run: registry::list_plain,
    },
    Command {
        path: &["repo", "status"],
        synopsis: "",
        about: "Show what is installed in a repository and what an update would do",
        notes: "Reads only. Every skill gets a line: = clean, ~ library changed, + not installed,\n\
                - no longer wanted, * modified locally, ! conflict.",
        flags: &[REPO, ALL_REPOS],
        run: repo::status,
    },
    Command {
        path: &["repo", "enable"],
        synopsis: "<profile>...",
        about: "Enable profiles in a repository",
        notes: "This changes what the repository should have. `beskar repo update` installs it.",
        flags: &[REPO],
        run: repo::enable,
    },
    Command {
        path: &["repo", "disable"],
        synopsis: "<profile>...",
        about: "Disable profiles in a repository",
        notes: "This changes what the repository should have. `beskar repo update` removes what is no longer wanted.",
        flags: &[REPO],
        run: repo::disable,
    },
    Command {
        path: &["repo", "toggle"],
        synopsis: "<profile>...",
        about: "Enable each profile that is off and disable each that is on",
        notes: "",
        flags: &[REPO],
        run: repo::toggle,
    },
    Command {
        path: &["repo", "update"],
        synopsis: "",
        about: "Make a repository's skills match its enabled profiles",
        notes: CONFLICT_NOTES,
        flags: &[REPO, ALL_REPOS, DRY_RUN, ON_CONFLICT],
        run: repo::update,
    },
    Command {
        path: &["repo", "diff"],
        synopsis: "<skill>",
        about: "Show how the installed copy of a skill differs from the library's",
        notes: "Lines starting with - are the library's, lines starting with + are the repository's.",
        flags: &[REPO],
        run: repo::diff,
    },
    Command {
        path: &["repo", "promote"],
        synopsis: "<skill>",
        about: "Copy a locally modified skill into the library",
        notes: "Other repositories pick the change up on their next update. Refuses when the library's\n\
                copy changed since this one was installed, because that would overwrite it; --force\n\
                overrides. Look at `beskar repo diff <skill>` first.",
        flags: &[
            REPO,
            switch("force", "Overwrite library changes made since this copy was installed"),
        ],
        run: repo::promote,
    },
    // ---- registry ----
    Command {
        path: &["registry", "list"],
        synopsis: "",
        about: "List registered repositories, or ask where a profile or skill is used",
        notes: "",
        flags: &[
            option("profile", "name", "Show the repositories that have this profile enabled"),
            option("skill", "name", "Show the repositories where this skill is installed, and why"),
        ],
        run: registry::list,
    },
    Command {
        path: &["registry", "status"],
        synopsis: "",
        about: "One line per repository: up to date, changes pending, conflicts",
        notes: "Reads only.",
        flags: &[],
        run: registry::status,
    },
    Command {
        path: &["registry", "stats"],
        synopsis: "",
        about: "Count repositories, profiles and skills, and find unused skills",
        notes: "",
        flags: &[],
        run: registry::stats,
    },
    Command {
        path: &["registry", "update"],
        synopsis: "",
        about: "Update every registered repository",
        notes: CONFLICT_NOTES,
        flags: &[
            switch(
                "all",
                "Accepted for symmetry with `repo update`; this command always covers every repository",
            ),
            DRY_RUN,
            ON_CONFLICT,
        ],
        run: registry::update,
    },
    Command {
        path: &["registry", "prune"],
        synopsis: "",
        about: "Forget repositories whose directory is gone",
        notes: "",
        flags: &[DRY_RUN],
        run: registry::prune,
    },
];

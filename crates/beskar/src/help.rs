//! Help text. Kept as plain strings so `beskar help <topic>` and `--help`
//! read the same everywhere.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const MAIN: &str = "\
beskar - Better Skill Arrangement

Keep one curated library of agent skills, group them into profiles, and
materialise exactly the right skills into each repository's .agents/skills/.

Usage: beskar [--home <dir>] <command> [args] [options]

Commands:
  init            Set up config, library and registry (run this first)
  doctor          Check config, library, registry and every repository
  status          Show every registered repository's sync state
  update          Reconcile the current repository (--all: every repository)

  library         Manage the skill library      (list, add, scan, show, remove, init)
  profile         Manage profiles               (create, delete, list, show, add, remove)
  repo            Manage a repository           (add, remove, list, status, enable, disable,
                                                 toggle, update, diff, promote)
  registry        Inspect all repositories      (list, status, stats, update, prune)
  config          Show or change configuration  (show, set)
  help            Help for a command, or 'help format' for the file format

Options:
  --home <dir>    Beskar home (default: $BESKAR_HOME, else ~/.beskar)
  -h, --help      Show help
  -V, --version   Show version

Start here:
  beskar init
  beskar library scan ~/my-skills
  beskar profile create coding
  beskar profile add coding git code-review
  cd ~/projects/app && beskar repo add . && beskar repo enable coding
  beskar repo update
";

pub const LIBRARY: &str = "\
Manage the skill library: the canonical, curated copies of every skill.

Usage:
  beskar library list                     List skills with their descriptions
  beskar library show <skill>             Details, plus where it is used
  beskar library add <dir> [--name <n>]   Copy one skill directory into the library
                        [--replace]       Overwrite an existing skill of that name
  beskar library scan <dir> [--yes]       Find every directory with a SKILL.md and import them
  beskar library remove <skill> [--force] Delete a skill (--force also drops it from profiles)
  beskar library init [<dir>]             Create the library structure (beskar init does this)

A skill is any directory. When it has a SKILL.md with front matter, the
description shown in listings comes from there.
";

pub const PROFILE: &str = "\
Manage profiles: named sets of skills that describe a use case.

Usage:
  beskar profile list                              List profiles
  beskar profile show <profile>                    Skills in the profile and repositories using it
  beskar profile create <profile> [-d <text>]      Create an empty profile
  beskar profile delete <profile> [--force]        Delete (--force also disables it in repositories)
  beskar profile add <profile> <skill>...          Add skills (they must exist in the library)
  beskar profile remove <profile> <skill>...       Remove skills

Profiles live in <library>/profiles/<name>.slate and travel with the library.
";

pub const REPO: &str = "\
Manage one repository (any directory; Git is not required).

The repository is the current directory's registered ancestor, unless a path
is given or --repo <path> is passed.

Usage:
  beskar repo add [<path>]                     Register a repository
  beskar repo remove [<path>] [--purge]        Unregister (--purge deletes the skills Beskar installed)
  beskar repo list                             List registered repositories
  beskar repo status [<path>]                  Compare desired and actual skills, change nothing
  beskar repo enable <profile>...              Enable profiles (then run update)
  beskar repo disable <profile>...             Disable profiles
  beskar repo toggle <profile>...              Flip profiles on or off
  beskar repo update [<path>] [--dry-run]      Bring .agents/skills in line with enabled profiles
                     [--on-conflict <policy>]  ask | keep | replace | fail
                     [--all]                   Every registered repository
  beskar repo diff <skill>                     Diff the installed copy against the library
  beskar repo promote <skill>                  Make the installed copy the new library version

Markers in status and update output:
  +  install      ~  update from library      -  remove       =  up to date
  M  modified locally (kept)   !  conflict, needs a decision   ?  not in the library
";

pub const REGISTRY: &str = "\
Inspect and maintain every registered repository at once.

Usage:
  beskar registry list                         Repositories with their enabled profiles
  beskar registry status                       One-line sync summary per repository
  beskar registry stats                        Counts across library and repositories
  beskar registry update [--dry-run]           Reconcile every repository
                         [--on-conflict <p>]   ask | keep | replace | fail
  beskar registry prune [--dry-run]            Forget repositories whose directory is gone
";

pub const CONFIG: &str = "\
Show or change Beskar's configuration file.

Usage:
  beskar config show                 Print the effective configuration
  beskar config set <key> <value>    Change a key

Keys: library, registry, skills_dir, on_conflict
";

pub const INIT: &str = "\
Set up Beskar: a config file, a library and an empty registry.

Usage:
  beskar init [--library <dir>]

Nothing existing is overwritten; running it twice is safe.
";

pub const DOCTOR: &str = "\
Check configuration, library, profiles, registry and every repository.

Usage:
  beskar doctor

Exit status is 1 when anything fails outright; warnings do not change it.
";

pub const FORMAT: &str = "\
Slate: the file format Beskar uses for config, profiles and the registry

Slate is line-oriented. There are four kinds of line and nothing else:

  # a comment            first non-blank character is '#'
  [kind name]            a section header; the name is optional and may contain spaces
  key = value            an entry; the value runs to the end of the line
                         a blank line

Rules:

  * Every line stands alone. Indentation is ignored; nothing spans lines.
  * No quotes, no escapes. The value is the text after the first '=', trimmed.
    A '#' inside a value is part of the value, not a comment.
  * Every value is text. The program reading the file decides what 'true' or
    '007' mean; the parser never guesses a type.
  * A repeated key inside one section is a list, one item per line. Where a
    single value is expected, a repeat is an error that names the line.
  * Keys and section kinds use letters, digits, '_', '-' and '.'. Two sections
    may not share the same kind and name.
  * Unknown keys are errors. A typo is reported with its line number instead
    of being ignored.

Beskar edits these files in place: comments, blank lines and ordering survive.

Files:

  <home>/config.slate                 library, registry, skills_dir, on_conflict
  <library>/library.slate             format marker
  <library>/profiles/<name>.slate     description = ...   skill = <name> (repeated)
  <home>/registry.slate               [repo <path>] sections with profile = ...,
                                      installed = <skill> <fingerprint>, synced = ...

Example profile:

  # Everyday coding work.
  description = Coding in any language
  skill = git
  skill = code-review
  skill = testing
";

pub fn for_group(group: &str) -> Option<&'static str> {
    Some(match group {
        "library" => LIBRARY,
        "profile" => PROFILE,
        "repo" => REPO,
        "registry" => REGISTRY,
        "config" => CONFIG,
        "init" => INIT,
        "doctor" => DOCTOR,
        "format" | "slate" => FORMAT,
        "status" => REGISTRY,
        "update" => REPO,
        _ => return None,
    })
}

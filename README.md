# Beskar

Beskar, short for Better Skill Arrangement, manages agent skills across local workspaces. You keep one curated library of skills and group them into profiles. Each repository then gets a copy of exactly the skills its enabled profiles list, in `.agents/skills/`.

It is a command line tool written in Rust. The crates in this workspace depend on nothing but the standard library. It works offline and does not need Git.

## Build

```bash
cargo build --release
```

The binary lands in `target/release/beskar`. Rust 1.89 or newer.

## Quick start

```bash
beskar init                          # config, registry and library in ~/.beskar
beskar library scan ~/my-skills      # find skills and import them

beskar profile create coding
beskar profile add coding git code-review testing

cd ~/projects/app
beskar repo add .                    # register this directory
beskar repo enable coding            # say which profiles it should have
beskar repo update --dry-run         # preview
beskar repo update                   # install
```

Later, after improving `code-review` in the library:

```bash
beskar registry update               # every repository that needs it gets the new copy
```

`beskar --help` lists the commands, and `beskar <command> --help` lists the options.

## How the pieces fit

```text
library      the skills you own, and the profiles that group them
   |
registry     which repositories exist and which profiles each has enabled
   |
repository   .agents/skills/ holds a copy of each wanted skill
```

The library is never read by agents. The registry lives outside the library because it holds absolute paths. Skills are copied rather than linked, so a repository keeps working if the library moves.

## Local changes are not overwritten

Beskar records a fingerprint of every skill it installs. That is enough to tell three situations apart:

- The library changed and the installed copy did not. `update` replaces it.
- The installed copy was edited and the library did not change. `update` leaves it alone, and `status` shows it as modified. `beskar repo diff <skill>` shows the edits and `beskar repo promote <skill>` moves them into the library.
- Both changed, or an update would delete an edited skill. That is a conflict, and Beskar needs a decision.

On a terminal, a conflict prompts for keep, replace, promote or diff. Without one, the update aborts and leaves the repository untouched. Pass `--on-conflict keep` or `--on-conflict replace` to choose ahead of time, or set `on-conflict` in the config. Directories in `.agents/skills` that Beskar did not install are never removed, and neither is an installed skill that has its own `.git` inside.

Before it deletes or replaces anything, Beskar moves the old copy aside and checks that it is still what the plan saw. If someone edited it in the meantime, the copy goes back and that skill is reported as failed. It refuses to touch a skills directory that is a symlink leading outside the repository, or into the library.

[docs/adr/0002](docs/adr/0002-installed-skills-are-copies-tracked-by-fingerprint.md) has the reasoning.

## Files

Everything Beskar writes uses one small format, described in [crates/beskar-lines/SPEC.md](crates/beskar-lines/SPEC.md) and printed by `beskar help format`. A profile is a list of lines:

```text
# Everyday software engineering.
description Everyday software engineering
skill code-review
skill git
skill testing
```

One fact per line, no quoting, no types, `#` comments on their own line. Appending `skill pdf` with `echo` is a valid edit, and when Beskar edits a profile it keeps your comments and layout. The registry is Beskar's own state and is rewritten whenever it changes. [docs/adr/0001](docs/adr/0001-line-oriented-file-format.md) explains why it is not YAML or TOML.

```text
~/.beskar/
    config.bsk           settings
    registry.bsk         machine-local state
    library/
        skills/<name>/   one directory per skill
        profiles/<name>.bsk
```

Set `BESKAR_HOME`, or pass `--home <dir>`, to keep it somewhere else.

## Commands

| Command | What it does |
| --- | --- |
| `init`, `doctor` | Set everything up, and check that it all agrees |
| `library add`, `scan`, `list`, `show`, `remove` | Manage the skills you own |
| `profile create`, `delete`, `list`, `show`, `add`, `remove` | Group skills by use case |
| `repo add`, `remove`, `list`, `status` | Register directories and see their state |
| `repo enable`, `disable`, `toggle` | Choose a repository's profiles |
| `repo update`, `diff`, `promote` | Install, compare and save local changes |
| `registry list`, `status`, `stats`, `update`, `prune` | Look across every repository |
| `status`, `update` | Shortcuts for `repo status` and `repo update` |

`enable` and `disable` change what a repository should have. Only `update` touches files.

## Layout of this repository

```text
crates/beskar-lines   the file format: parser, lossless editor, spec
crates/beskar-core    the domain: library, profiles, registry, reconciliation
crates/beskar-cli     the `beskar` binary: arguments, prompts, output
docs/BRIEF.md         what Beskar is for
docs/adr/             decisions that are hard to reverse
CONTEXT-MAP.md        vocabulary, one glossary per context
```

The core never prints or prompts. A TUI or another front end can sit on it the way the CLI does.

## Development

```bash
cargo test
cargo clippy --all-targets
cargo fmt --all
```

The CLI tests run the real binary against sandboxed homes under the system temp directory.

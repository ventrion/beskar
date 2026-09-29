# Beskar

**Better Skill Arrangement.** One curated library of agent skills, grouped
into profiles, materialised into exactly the repositories that need them.

```text
LIBRARY     "What skills do I own?"                   ~/.beskar/library/skills/
PROFILE     "Which skills belong together?"           ~/.beskar/library/profiles/*.slate
REGISTRY    "Where should those profiles be active?"  ~/.beskar/registry.slate
REPOSITORY  "Materialise exactly those skills here."  <repo>/.agents/skills/
```

Beskar is a single static binary written in Rust with **no dependencies
outside the standard library**. It works offline, never needs Git, and never
silently overwrites a skill you have edited locally.

## Install

```bash
cargo install --path crates/beskar
```

or `cargo build --release` and copy `target/release/beskar` onto your `PATH`.

## Five-minute tour

```bash
beskar init                                  # config, library and registry under ~/.beskar

beskar library scan ~/Downloads/agent-skills # find every directory with a SKILL.md, import them
beskar library list

beskar profile create coding -d "Everyday coding"
beskar profile add coding git code-review testing

cd ~/projects/app
beskar repo add .                            # register this workspace
beskar repo enable coding                    # desired state
beskar repo update --dry-run                 # what would change
beskar repo update                           # materialise into .agents/skills/
```

Later, improve `code-review` in the library and roll it out everywhere:

```bash
beskar registry update                       # reconciles every registered repository
```

## What `update` does

For every skill Beskar compares three SHA-256 fingerprints: the library copy,
what Beskar recorded when it last installed, and what is in the workspace now.

| Marker | Meaning | Action |
| --- | --- | --- |
| `+` | wanted, not installed | copy from library |
| `~` | library changed, workspace untouched | replace copy |
| `-` | no longer wanted, workspace untouched | delete copy |
| `=` | identical to library | nothing |
| `M` | edited locally, library unchanged | keep, report |
| `!` | edited locally **and** library changed, or unwanted but edited | ask |
| `?` | a profile wants it but the library lacks it | report |
| (blank) | in `.agents/skills` but never installed by Beskar | leave alone |

A `!` conflict is resolved interactively:

```text
Conflict: code-review

  The workspace copy is modified locally and changed in the library.

  [k] keep local  [l] replace with library  [p] promote to library  [d] show diff  [a] abort
```

Without a terminal, `update` refuses to guess. Pass `--on-conflict keep`,
`--on-conflict replace` or `--on-conflict fail`, or set `on_conflict` in the
config. Nothing is written until every conflict has an answer.

## Commands

```text
beskar init [--library <dir>]        beskar doctor
beskar status                        beskar update [--all]

beskar library   list | show <skill> | add <dir> [--name n] [--replace] |
                 scan <dir> [--yes] | remove <skill> [--force] | init [<dir>]
beskar profile   list | show <p> | create <p> [-d text] | delete <p> [--force] |
                 add <p> <skill>... | remove <p> <skill>...
beskar repo      add [<path>] | remove [<path>] [--purge] | list | status [<path>] |
                 enable <p>... | disable <p>... | toggle <p>... |
                 update [<path>] [--dry-run] [--on-conflict ask|keep|replace|fail] [--all] |
                 diff <skill> | promote <skill>
beskar registry  list | status | stats | update [--dry-run] [--on-conflict ...] | prune [--dry-run]
beskar config    show | set <key> <value>
beskar help [<command> | format]
```

Repository commands act on the registered repository containing the current
directory. Pass a path or `--repo <path>` to pick another.

Exit codes: `0` success, `1` failure, `2` usage error.

## Files

Everything Beskar writes is either a plain directory of files (skills) or a
[Slate](docs/slate.md) document: a line-oriented format with four kinds of
line, no quoting and no type inference, designed to be edited by people and
coding agents alike. `beskar help format` prints the rules.

```text
~/.beskar/
  config.slate              library, registry, skills_dir, on_conflict
  registry.slate            [repo <path>] sections: profiles, installed fingerprints, sync time
  library/
    library.slate           format marker
    skills/<name>/          canonical skill directories
    profiles/<name>.slate   description = ...  skill = <name> (one line per skill)
```

Set `BESKAR_HOME` (or pass `--home`) to keep state elsewhere. The library
location is configurable and may be a Git repository; Beskar does not care.
The registry contains absolute paths and stays on the machine.

## Layout

| Crate | Role |
| --- | --- |
| [`crates/slate`](crates/slate) | The Slate format: parser, typed views, comment-preserving editor |
| [`crates/beskar-core`](crates/beskar-core) | Domain model: config, library, skills, profiles, registry, fingerprints, reconciliation, diff, doctor |
| [`crates/beskar`](crates/beskar) | The CLI: argument parsing, prompts, output |

The core knows nothing about terminals, so a TUI or editor integration can
sit on the same engine. See [`CONTEXT-MAP.md`](CONTEXT-MAP.md) for the
vocabulary of each crate and [`docs/BRIEF.md`](docs/BRIEF.md) for the design
brief this implements.

## Development

```bash
cargo test            # unit tests plus end-to-end CLI tests in an isolated BESKAR_HOME
cargo build --release
```

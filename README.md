# Beskar: Better Skill Arrangement

Beskar keeps one curated library of agent skills, groups them into
profiles, and copies exactly the skills each workspace needs into its
`.agents/skills/` directory. It tracks what it installed, so local edits
are never overwritten or deleted without a decision.

```text
LIBRARY     what skills do I own?              ~/.beskar/library/skills/
PROFILE     which skills belong together?      ~/.beskar/library/profiles/*.plate
REGISTRY    where should profiles be active?   ~/.beskar/registry.plate
REPOSITORY  materialize exactly those skills   <repo>/.agents/skills/
```

Beskar is written in Rust with no dependencies outside the standard library.

## Build

```bash
cargo build --release
```

The binary is `target/release/beskar`.

## A first session

```bash
beskar init                                   # config, library, registry
beskar library scan ~/my-skills               # find and import skills
beskar profile create coding code-review git testing

cd ~/projects/app
beskar repo add . --enable coding             # register and enable
beskar repo update --dry-run                  # preview
beskar repo update                            # materialize .agents/skills/
```

After improving a skill in the library, run `beskar update --all` to push
it to every workspace that uses it. After improving one inside a
workspace, run `beskar skill diff <skill>`, then `beskar skill promote <skill>`,
then `beskar update --all`.

`beskar help` lists every command. `beskar help workflow` walks through
this session, and `beskar help format` explains the file format.

## Commands

```text
beskar init | doctor | status | update [--all]
beskar library  init | add | scan | list | show | remove
beskar profile  create | delete | list | show | add | remove
beskar repo     add | remove | list | status | enable | disable | toggle | update
beskar registry list | status | stats | update | prune
beskar skill    diff | promote
```

`enable`/`disable` change desired state only. `update` reconciles the
filesystem with it. Commands that act on a repository use the registered
repository containing the current directory, or the one given with `--repo`.

## Local changes are safe

For every installed skill, Beskar compares three fingerprints: the
library's current content, what Beskar installed (recorded in the
registry), and what is on disk now.

| Library | Workspace | Result |
|---|---|---|
| unchanged | unchanged | `clean` |
| changed | unchanged | `update`: replaced with the new library version |
| unchanged | edited | `modified`: left alone |
| changed | edited | `conflict`: needs a decision |
| no longer wanted | edited | `conflict`: needs a decision |
| directory Beskar didn't create | — | never touched (`untracked`), or a `conflict` if it blocks an install |

Conflicts are settled with keep / replace / promote. In a terminal you
are asked, with a diff on request. Everywhere else, `--on-conflict`
(or `on-conflict` in the config) decides: `keep`, `replace`, or
`fail`. The default, `ask`, stops without making changes when nobody
can be asked.

Installs are staged under a hidden name and renamed into place, so an
interrupted run never leaves a half-copied skill behind.

## Files

All Beskar files use [Plate](crates/plate/SPEC.md), a small line-oriented
format. It has no quoting, no escaping and no type guessing, and it keeps
comments when Beskar edits a file.

| File | Portable? | Written by |
|---|---|---|
| `~/.beskar/config.plate` | machine | `init`; edit freely |
| `<library>/library.plate` | yes | `init` (marker and format version) |
| `<library>/profiles/<name>.plate` | yes | you or `beskar profile ...`; comments survive |
| `~/.beskar/registry.plate` | machine | `beskar repo ...` |

Set `BESKAR_HOME` (or pass `--home`) to keep Beskar somewhere other than
`~/.beskar`. The library can be any directory, for example a Git clone
shared between machines: `beskar init --library ~/src/my-skills`.

## Exit status

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | error |
| 2 | invalid command line |
| 3 | stopped for a decision: unresolved conflicts, skills missing from the library, or a confirmation that needs `--yes` |

## Layout

```text
crates/plate         the Plate format: parser, lossless editor, writer
crates/beskar-core   domain: library, profiles, registry, reconciliation
crates/beskar-cli    the `beskar` binary: argument parsing and presentation
```

The core crate never prints or prompts, so another front end (a TUI, an
editor plugin) can drive the same plans and resolutions.

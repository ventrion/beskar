# Beskar

**Better Skill Arrangement.** One library of agent skills, installed per repository.

An agent should see only the skills that fit the workspace and the task it is working in, while you keep one curated collection. Beskar holds that collection, lets you group skills into profiles, and copies the skills of the profiles you pick into each repository's `.agents/skills/`.

```text
   Library ──── profiles select skills ────▶ Registry ──── update ────▶ .agents/skills/
   skills/                                   repo → profiles             what the agent sees
   profiles/
```

Beskar is written in Rust with no dependencies beyond the standard library. It works offline, needs no Git, and reads and writes only plain files.

## Install

```bash
cargo install --path crates/beskar-cli
```

That puts a `beskar` binary in `~/.cargo/bin`. To build without installing, run `cargo build --release` and use `target/release/beskar`. Building needs Rust 1.89 or newer.

## Quick start

```bash
beskar init                                   # settings, registry and an empty library in ~/.beskar
beskar library scan ~/my-skills               # import every folder that contains a SKILL.md
beskar profile create coding git code-review testing

cd ~/projects/app
beskar repo add .                             # register the folder
beskar repo enable coding                     # record what you want here
beskar repo update --dry-run                  # see the plan
beskar repo update                            # install it
```

The dry run prints the plan and changes nothing:

```text
Repository: ~/projects/app

+ code-review
+ git
+ testing

No files changed.
```

When you later improve `code-review` in the library, one command brings every repository up to date:

```bash
beskar registry update --all
```

## How it fits together

| Concept | What it is |
| --- | --- |
| **Library** | Your curated collection: `skills/<id>/` folders and `profiles/<name>.bsk` files. The source of truth. Safe to keep in Git. |
| **Profile** | A named set of skills, such as `coding` or `research`. A repository can enable several; its skills are their union, and a skill in two profiles is installed once. |
| **Repository** | Any folder where agents run. It does not have to be a Git repository. |
| **Registry** | Machine-local record of repositories, their enabled profiles, and what Beskar installed in each. It holds absolute paths, so it lives outside the library. |

`enable` and `disable` change what you want. `update` changes the files. Installed skills are copies, so a repository keeps working if the library moves.

## Commands

Every command has `--help`. `beskar help format` describes the file format.

| Group | Commands |
| --- | --- |
| Setup | `init`, `doctor`, `config` (`show`, `set`, `path`) |
| `library` | `init`, `add`, `scan`, `list`, `show`, `remove` |
| `profile` | `create`, `delete`, `list`, `show`, `add`, `remove` |
| `repo` | `add`, `remove`, `list`, `status`, `enable`, `disable`, `toggle`, `update` |
| `registry` | `list`, `status`, `stats`, `update`, `prune` |
| `skill` | `diff`, `promote` |
| Shortcuts | `status` and `update`, each with `--all` for every repository |

Questions the registry answers:

```bash
beskar registry list --profile coding        # where is this profile used?
beskar registry list --skill playwright      # where is this skill installed, and through which profile?
beskar registry status                       # which repositories need updating?
beskar registry stats                        # counts, including skills nothing uses
```

## Local edits are safe

Beskar records a fingerprint of every skill it installs. For each skill it compares the library's version, the recorded one and what is in the repository now, so it can tell four situations apart:

| `repo status` says | `--json` id | Meaning |
| --- | --- | --- |
| up to date | `clean` | The repository's copy and the library agree. |
| library has a newer version | `library-changed` | The copy is untouched. The update replaces it. |
| modified locally | `local-drift` | Someone edited the copy. The library is as it was. |
| modified locally and changed in the library | `diverged` | Both changed. |

Beskar never overwrites or removes an edited copy silently. An update that would do so is a **conflict**. At a terminal it asks:

```text
Conflict: code-review
Repository: ~/projects/app

The workspace copy has local modifications.

  [k] keep local
  [l] replace with library
  [p] promote to library
  [d] show diff
  [q] abort (nothing is changed)
Choice [k/l/p/d/q]:
```

Only a whole letter or word counts, so "local" is not read as "l". Replacing asks a second question, "Discard your changes to code-review? [y/N]", and promoting over changes made in the library asks "Promote anyway?".

Without a terminal, `--on-conflict` decides: `fail` stops before changing anything in that repository, `keep` leaves the edited skills and updates the rest, `replace` overwrites them with the library version. The default is `ask`, which behaves like `fail` when nobody can answer. Set your own default with `beskar config set on-conflict keep`.

To send an improvement back, review it with `beskar skill diff code-review`, then run `beskar skill promote code-review`. Other repositories pick it up with `beskar registry update --all`.

Beskar removes only skills it installed. Folders you made yourself inside `.agents/skills/` are left alone.

An edit is any change to a file's name, contents or executable bit. Timestamps do not count, and neither do `__pycache__`, `*.pyc` and `.DS_Store`. A `.git` folder inside an installed skill does count, so a clone you made there is a local change and no update removes it silently. A link inside a skill is followed while it points at a file in the same skill. A link that leaves the skill, or points at nothing, is an error that names the link, so a skill cannot pull a file such as `~/.ssh/id_rsa` into the library.

## Files

Everything Beskar owns is in `~/.beskar` (set `BESKAR_HOME` or pass `--home` to use another folder). All of it is text in the bsk format: one `key value` per line, `#` comments on their own lines, no quoting. Beskar edits `config.bsk` and the profile files in place and keeps your comments. `registry.bsk` is machine state that Beskar rewrites whole, so put no notes in it.

```text
~/.beskar/
  config.bsk        settings
  registry.bsk      machine-local state
  library/          skills/ and profiles/, the part to keep in Git
```

`config.bsk`:

```bsk
# Where your skills and profiles live. Point this at a Git checkout to sync it between machines.
library ~/.beskar/library

# Machine-local record of repositories and installed skills. Keep it out of the library.
registry ~/.beskar/registry.bsk

# Where skills are installed inside each repository, relative to the repository root.
agent-skills .agents/skills

on-conflict ask
```

`library/profiles/coding.bsk`, where the file name is the profile name:

```bsk
description Everyday software engineering
skill code-review
skill git
skill testing
```

`registry.bsk`:

```bsk
version 1

[repo /home/me/projects/app]
profile  coding
synced   2026-09-29T11:56:06Z
skill    git  sha256:9399dea134c699425b5be195c838fe85e793d1c1af05f3108e307a74bc44f102
```

To install into a different folder, such as `.claude/skills`, run `beskar config set agent-skills .claude/skills`. Copies already installed under the old folder stay there and Beskar stops managing them; `config set` says how many repositories are affected, and `beskar registry update --all` installs into the new folder. The library cannot live inside a folder that agents read skills from, and `init` refuses to put it there. Beskar also refuses to update a repository whose skills folder is, or lies inside, the library.

## Using Beskar from scripts and coding agents

- Commands never prompt when there is no terminal. A decision that needs an answer stops the command and the message names the flag to pass (`--yes`, `--force`, `--on-conflict`).
- `--dry-run` shows what `update` and `library scan` would do. An update preview exits with the status the real run would have: 1 when a conflict would stop it and nobody can answer.
- `--json` prints the result of `doctor`, `config show`, `library list|show|scan`, `profile list|show`, `repo list|status|update`, `registry list|status|stats|update` and the shortcuts.
- Exit status is 0 for success, 1 when the work failed (or `doctor` found an error), and 2 when the command line itself is wrong: an unknown command or option, a missing or malformed argument, an empty path.
- Errors go to standard error, usually with a `hint:` line that names the next thing to try. With `--json`, a failure also prints `{"error": {"kind", "message", "hint"}}` on standard output. Mistyped commands, options, skills and profiles get "did you mean" suggestions.
- Mistakes in a `.bsk` file are reported with the line, the column and a fix, in the style of a compiler.
- `beskar doctor` checks the settings, the library, the registry and every repository, and says what to run for each problem.
- Several Beskar processes can run at once. Each takes a lock on the files it changes and waits up to ten seconds for it, then fails with a "busy" error.

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

| Crate | Role |
| --- | --- |
| `crates/bsk` | The file format: parser, in-place editor, schema checks, diagnostics. Specification in `SPEC.md`. |
| `crates/beskar-core` | The domain: library, profiles, registry, reconciliation, fingerprints, diffs. Never prints or prompts. |
| `crates/beskar-cli` | The `beskar` binary: argument parsing, help, prompts, text and JSON output. |

Domain vocabulary is in `CONTEXT-MAP.md` and the `CONTEXT.md` next to each crate. Decisions that are costly to reverse are in `docs/adr/`.

The original brief is `docs/BRIEF.md`. Classification of skills by a model, which the brief describes as optional enrichment, is not implemented; nothing in Beskar depends on it.

# Beskar

Keep one skill library. Choose profiles for each workspace. Copy their skills into `.agents/skills` without losing local edits.

Beskar is written in Rust and has **no third-party dependencies**, including development dependencies. It runs offline and does not require Git, an AI model, a database, or a configuration parser package.

## Build

Rust 1.89 or newer is required for standard-library file locking.

```sh
cargo build --release --offline
./target/release/beskar --help
```

To install the binary on your Cargo path:

```sh
cargo install --path crates/beskar --offline
```

## Start here

```sh
beskar init
beskar library scan ~/my-skills --dry-run
beskar library scan ~/my-skills --yes

beskar profile create coding
beskar profile add coding code-review testing

cd ~/projects/my-app
beskar repo add .
beskar repo enable coding
beskar repo update --dry-run
beskar repo update
```

The library owns the original directories. Profiles reference skill names. The registry records which profiles each workspace wants and what Beskar last installed. Workspace files are independent copies.

`enable`, `disable`, and `toggle` change the desired profiles. They do not copy or delete files. `update` reconciles that intent with the filesystem. Several active profiles produce a deduplicated union of skills.

Workspaces can be ordinary directories. Beskar also resolves the nearest registered workspace when run from a subdirectory. Use `--repo /absolute/workspace` to select one explicitly.

## Configuration people can read

Every `.bsk` document starts with `beskar 1`. A record is a key, a space, and a literal value. Blank lines and full-line comments are allowed. There are no strings to quote, implicit booleans, escape sequences, environment substitutions, or indentation rules.

`~/.beskar/config.bsk`:

```text
beskar 1

# Spaces and # are part of the path.
library /home/ada/My skills # curated
registry /home/ada/.beskar/registry.bsk
agent-skills .agents/skills
```

A profile, `library/profiles/coding.bsk`:

```text
beskar 1

# Review before changing code.
skill code-review
skill testing
```

A profile's filename is its name. `skill` records reference directories under `library/skills/`. Unknown records, duplicate entries, unsupported versions, and invalid names produce errors. Profile membership commands preserve comments and the order of existing entries.

Only whole lines beginning with `#` after indentation are comments. In `library /data/skills # personal`, the path includes ` # personal`. Surrounding whitespace is ignored. The [format specification](docs/format.md) defines the grammar and all records.

The default state directory is `~/.beskar`. Override it with `BESKAR_HOME` or `--home PATH`. Choose a library when initializing with `beskar init --library /path/to/library`. Existing configuration is not overwritten by another `init`.

The registry stays outside the portable library. Configured paths must be absolute, except `agent-skills`, which is relative to each workspace. Beskar rejects library locations that would expose the whole collection through an agent's skills directory. If Git manages your library, ignore `.beskar.lock` and `.beskar-import-*` in that library's `.gitignore`.

## Daily operations

```sh
beskar library add /path/to/skill
beskar library add /path/to/another-skill --name code-review
beskar library list
beskar library show code-review

beskar profile list
beskar profile show coding
beskar profile remove coding testing

beskar repo enable research
beskar repo disable research
beskar repo toggle coding
beskar repo status

beskar registry list --profile coding
beskar registry list --skill code-review
beskar registry stats
beskar registry update --all --dry-run
beskar registry update --all
beskar doctor
```

`beskar update` and `beskar status` are short forms of their `repo` commands. Add `--all` to cover the registry. Global updates validate every workspace before replacing any installed skill. A conflict or an unreadable workspace stops the default batch.

`library add` accepts any ordinary directory. `library scan` discovers directories containing `SKILL.md`, stops descending when it finds one, skips symlink directories, and skips `.git`, `target`, and `node_modules`. Importing requires a terminal confirmation or `--yes`. Existing names and duplicate discoveries stop the batch before anything enters the library.

`library remove` refuses skills referenced by a profile or tracked installation. `profile delete` refuses enabled profiles. `repo remove` unregisters a workspace and leaves its files intact. Re-registering does not adopt those files automatically. `registry prune` removes only missing workspace records; `--dry-run` previews them.

Registry skill queries and installed counts describe tracked installations. `status` and `doctor` check the actual files. An unused skill is neither installed nor selected by an enabled profile.

## Local edits

A SHA-256 tree fingerprint includes relative filenames, file bytes, empty directories, and executable permission bits on Unix. It ignores timestamps. Beskar records the fingerprint of the copy it installed and compares that baseline with both the current workspace and the current library.

| State | Update behavior |
| --- | --- |
| Library and workspace match the baseline | Leave the copy untouched |
| Library changed, workspace still matches the baseline | Replace with the new library copy |
| Workspace changed, including deleted files or permission changes | Refuse by default |
| Workspace and library independently reached the same content | Record their common fingerprint without rewriting files |
| Managed directory is completely missing | Reinstall it if desired; otherwise forget the missing installation |
| Unmanaged path occupies a desired skill name | Refuse under every policy |
| Unrelated unmanaged skills exist | Leave them in place |

A conflict stops an update before it changes workspaces. Inspect the edits, then choose a policy explicitly:

```sh
beskar repo update --conflict keep
beskar repo update --conflict replace
```

`keep` preserves local copies and their original baselines, so drift remains visible. It applies other safe changes in the batch. `replace` discards local edits to managed skills, including when removing a disabled skill. It cannot overwrite unmanaged paths.

For a manual promotion, review the workspace copy and copy the chosen edits into the library yourself. Once both copies match, update records the new baseline. Automated promotion, diff presentation, classification, and a TUI are future work.

## For coding agents

Use `--json` for one JSON document on stdout. Diagnostic errors also have a JSON representation. No command prompts in JSON mode. A scan needs `--yes` to import or `--dry-run` to inspect.

```sh
beskar repo status --json
beskar repo update --dry-run --json
beskar registry list --skill testing --json
```

Plans expose each skill's `action`, `baseline`, `current`, and `desired` fingerprints. The `unmanaged` array lists unrelated entries that will remain in place. For an unmanaged name collision, `current` is null because Beskar does not treat those contents as an installation. Update reports describe the plan that was applied, or would be applied during a dry-run. A separate status call reports the resulting state.

Exit codes:

| Code | Meaning |
| --- | --- |
| `0` | Command succeeded |
| `1` | Operational error, conflict during update, or failed doctor check |
| `2` | Invalid command syntax or option |
| `3` | Status needs attention, or dry-run found conflicts |

Records and output use deterministic ordering. A clean update does not rewrite registry state or installed files.

## Filesystem safety and recovery

Beskar uses standard-library process locks, stages and verifies copies, saves the original registry, and swaps skill directories before committing registry state. Ordinary update errors roll back changes across the batch. Failed transactions retain staged copies so concurrent edits can be recovered.

An interrupted process leaves a `registry.pending.bsk` marker beside the registry. Further mutations stop, and `doctor` reports the recovery location. Recovery after interruption is manual; see [recovery instructions](docs/format.md#recovering-an-interrupted-update).

Skills may contain regular files and directories. Symlinks, devices, sockets, and FIFOs inside a skill are rejected. Managed filesystem boundaries cannot be symlinks. Filenames must be UTF-8. Executable bits are preserved on Unix; directory permissions, timestamps, ownership, ACLs, and extended attributes are not part of the portable skill model.

Process locks coordinate Beskar commands. They do not lock editors, Git, or other tools. Beskar rechecks content around swaps, but this is not a filesystem snapshot or a guarantee against hostile concurrent filesystem changes or power loss. Finish editing or synchronizing skills before updating.

To change `agent-skills` while installations exist, first disable profiles and update using the old path. Beskar refuses a path change that would strand tracked copies. Then edit the path, enable the desired profiles, and update again.

## Implementation and checks

The `beskar` library contains application operations, strict record parsing, filesystem checks, and a reusable reconciliation planner and executor. The binary owns argument parsing, prompts, and text or JSON output. The [implementation brief](docs/BRIEF.md) describes the domain.

```sh
cargo fmt --all -- --check
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
```

Tests exercise the real CLI in isolated temporary directories, along with hash vectors, format validation, process locking, stale plans, and rollback after a failed registry write.

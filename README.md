# Beskar

Beskar manages a global skill library, groups skills into profiles, and copies enabled profiles into local workspaces. It runs offline and uses Rust's standard library. There are no third-party dependencies.

The library contains canonical skill directories and portable profile files. The machine-local registry records workspace paths, enabled profiles, and the fingerprints of installed copies. Agents see only the copies in each workspace's `.agents/skills/` directory.

## Build and install

Rust 1.89 or newer is required.

```sh
cargo build --release --offline
cargo install --path crates/beskar --offline
beskar --help
```

To run directly from the checkout, use `cargo run --offline -- <arguments>`.

## First use

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

Enabling, disabling, or toggling a profile changes desired configuration. `update` applies that configuration to the filesystem. Multiple enabled profiles produce a union of skills, with one copy of each skill.

Workspaces can be ordinary directories. Beskar does not require Git. Copies retain binary files, empty directories, and file permissions. Changing or moving the library does not change already installed copies.

## Configuration you can edit

Beskar uses `.bsk` files. Each line contains a named record. Quote paths with spaces, use `#` for comments, and list one skill per line. There are no indentation rules, implicit types, includes, or variable substitutions. Unknown records and duplicates are errors with file and line information.

The default `~/.beskar/config.bsk` looks like this:

```text
beskar 1
library "/home/user/.beskar/library"
registry "/home/user/.beskar/registry.bsk"
agent-skills ".agents/skills"
```

A profile named `coding` lives at `library/profiles/coding.bsk`:

```text
beskar 1
# Check changes before shipping.
skill code-review
skill testing
```

Profile commands preserve existing comments and formatting. They append new skills and remove the requested records. Generated registry files use deterministic ordering.

Set `BESKAR_HOME` or pass `--home PATH` to use a different configuration directory. Initialize custom locations with:

```sh
beskar init --library ~/skill-library --registry ~/.local/state/beskar/registry.bsk
```

The shell expands `~` in that command. Paths inside `.bsk` files are literal. Library and registry paths must be absolute; `agent-skills` must be relative to a workspace. Keep the registry outside the library, and keep the library outside automatically discovered agent skill directories.

See [the complete format reference](docs/FORMAT.md) for the grammar, schemas, and editing rules.

## Commands

| Area | Commands |
| --- | --- |
| Setup | `init`, `doctor`, `doctor --recover` |
| Library | `library init`, `add PATH`, `scan PATH`, `list`, `show NAME`, `remove NAME` |
| Profiles | `profile create NAME`, `delete NAME`, `list`, `show NAME`, `add NAME SKILL...`, `remove NAME SKILL...` |
| Workspaces | `repo add [PATH]`, `remove [PATH]`, `list`, `status`, `enable PROFILE...`, `disable PROFILE...`, `toggle PROFILE...`, `update` |
| Registry | `registry list`, `status`, `stats`, `where --profile NAME`, `where --skill NAME`, `update --all`, `prune` |
| Local edits | `skill promote NAME` |
| Shortcuts | `status`, `status --all`, `update`, `update --all` |

Repository commands use the nearest registered ancestor of the current directory. Pass `--repo PATH` to target another workspace. `repo update --all` and `registry update --all` reconcile all registered workspaces.

`library add` imports any directory, even without `SKILL.md`. It uses the directory name unless you pass `--name NAME`. `library scan` recursively discovers directories containing `SKILL.md`, stops descending when it finds a skill, and skips `.git` directories. It previews a batch before importing. Scripts must pass `--yes`; `--dry-run` previews without asking.

`library show` prints the fingerprint, profile membership, workspace usage, and optional `SKILL.md` contents. Beskar treats the skill's internal format as opaque.

`registry where` reports profile usage and tracked or desired skill installations, including which enabled profiles select each skill. `registry stats` counts tracked installations. A library skill is unused if neither a profile nor a tracked installation refers to it.

`repo remove` unregisters a workspace and preserves all its files. Re-registering it makes those existing copies unmanaged. `registry prune` removes only entries whose workspace paths no longer exist. Both library removals and pruning support `--dry-run`. A skill referenced by a profile, or a profile enabled in a workspace, must have its references removed before deletion.

## Updating without losing local work

```sh
beskar status --all
beskar update --all --dry-run
beskar update --all
```

An update compares the current workspace fingerprint with the fingerprint recorded when Beskar installed that skill. A change in the library updates clean workspace copies. A change in the workspace, including deletion or a changed executable bit, is local drift.

The default conflict policy is `abort`. Beskar preflights every selected workspace and refuses the whole batch if any conflict or invalid source exists. Use an explicit policy after reviewing the local changes:

```sh
# Keep conflicting copies and update other skills.
beskar repo update --conflict keep

# Replace or remove conflicting copies according to the enabled profiles.
beskar repo update --conflict replace
```

Keeping a copy retains its original baseline, so future updates still report drift. If the workspace already matches the current library, Beskar can refresh its baseline without copying files. Updates leave unchanged skills alone.

Beskar preserves unrelated unmanaged skills. An unmanaged directory at a desired skill's destination blocks an update even with `--conflict replace`. Move it aside or import it under another name before proceeding.

To make a local improvement canonical:

```sh
beskar skill promote code-review --dry-run
beskar skill promote code-review
beskar update --all
```

Promotion copies the tracked workspace skill back to the library and records its new baseline for that workspace. If the library also changed since installation, promotion fails until you review the changes and explicitly pass `--conflict replace`.

## Interrupted operations

Imports, promotion, and reconciliation stage replacements on each destination's filesystem. A journal records replacements and backups. The registry participates in the same transaction as installed skills. Filesystem errors trigger recovery; interrupted processes leave a journal for the next run to detect.

Run `beskar doctor --recover` to finish a fully applied transaction or restore the originals from a partial transaction. Recovery checks fingerprints first. If you edited a target or backup after interruption, it preserves both and asks for manual recovery. The journal contains the exact target and backup paths.

A machine-local `.lock` prevents simultaneous Beskar processes from writing state. After a terminated process, verify that no Beskar process is running before removing the stale lock. Do not edit profiles or deployed files while an update is running.

Beskar rejects symlinks and special files in managed skills and state paths, and requires UTF-8 paths. It never follows links into another workspace or library. Directory entries are synced on Unix; interruption recovery on other platforms depends on their filesystem's rename durability. File ownership, extended attributes, and directory permissions are not replicated.

Beskar records the deployment path and blocks updates or promotion if `agent-skills` changes while installations are tracked. Unregister and resolve the old copies first, then change the path and register again. Moving a library means moving its contents and editing the library path; Beskar does not synchronize it through Git.

## Implementation and checks

The `beskar` library crate separates format parsing, domain records, filesystem operations, storage, reconciliation, and transactions. CLI presentation lives in its own module. The same planner serves single-workspace and global updates. SHA-256 is implemented locally and tested against standard vectors; fingerprints include sorted relative paths, directory markers, file sizes, file bytes, and Unix executable bits.

```sh
cargo test --workspace --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo fmt --all --check
```

Tests exercise CLI workflows in isolated temporary directories, drift and ownership protection, dry-run behavior, global preflight, promotion, and interrupted transactions. Classification and network synchronization remain optional future integrations.

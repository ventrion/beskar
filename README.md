# Beskar

Beskar keeps one curated library of agent skills, groups them into profiles, and copies selected profiles into local workspaces. It works offline, requires no Git repository, and uses Rust's standard library with no third-party dependencies.

The library is portable. The registry is machine-local. Agents see the copies in each workspace's `.agents/skills/` directory.

## Install

[Releases](https://github.com/ventrion/beskar/releases/latest) include prebuilt archives for Linux, macOS, and Windows on x86_64 and ARM64. The Linux builds are statically linked. Unpack the archive for your platform and put `beskar` (`beskar.exe` on Windows) on your `PATH`. `SHA256SUMS` lists the checksum of each archive. The binaries are unsigned. If macOS blocks one downloaded through a browser, run `xattr -d com.apple.quarantine beskar`.

To build from source, Rust 1.89 or newer is required.

```sh
cargo install --path crates/beskar-cli --offline
```

Or run `cargo build --release --offline` and use `target/release/beskar`.

## First use

```sh
beskar init
beskar library scan ~/my-skills --dry-run
beskar library scan ~/my-skills --yes
beskar profile create coding code-review testing

cd ~/projects/my-app
beskar repo add .
beskar repo enable coding
beskar repo update --dry-run
beskar repo update
```

Enabling a profile records what you want. Updating installs its skills. Several enabled profiles produce a union, with one copy of each skill. Copies retain binary content, empty directories, and file permissions, so a workspace keeps working when the library moves or becomes unavailable.

After editing a library skill, update all registered workspaces with:

```sh
beskar update --all --dry-run
beskar update --all
```

## Commands

Every command supports `--help` and `--json`. `beskar help format` describes the file format.

| Area | Commands |
| --- | --- |
| Setup | `init`, `doctor`, `doctor --recover` |
| Configuration | `config show`, `path`, `set KEY VALUE` |
| Library | `library init`, `add PATH`, `scan PATH`, `list`, `show NAME`, `remove NAME` |
| Profiles | `profile create NAME [SKILL...]`, `delete`, `list`, `show`, `add NAME SKILL...`, `remove NAME SKILL...` |
| Workspaces | `repo add [PATH]`, `remove [PATH]`, `list`, `status`, `enable PROFILE...`, `disable PROFILE...`, `toggle PROFILE...`, `update` |
| Registry | `registry list`, `status`, `stats`, `where`, `update --all`, `prune` |
| Local edits | `skill diff NAME`, `skill promote NAME` |
| Shortcuts | `status [--all]`, `update [--all]`, `repo diff`, `repo promote` |

Repository commands use the nearest registered ancestor of the current directory. `--repo PATH` targets another registered workspace. Registered workspaces cannot contain one another; `repo add` rejects nesting before changing the registry. Global updates plan every selected workspace before changing any of them.

`library add` accepts a directory with any contents; `SKILL.md` is optional. Use `--name NAME` when its directory name is unsuitable. `library scan` discovers directories containing `SKILL.md`, stops descending at each skill, and skips `.git` directories while searching. Scan previews a batch before import and requires `--yes` when no terminal is attached. `--dry-run` performs validation without importing.

Names use lowercase letters, digits, hyphens, or underscores. `library show` displays content, a fingerprint, profile membership, and workspace usage. Removing a referenced library skill or enabled profile requires removing its references first.

```sh
beskar registry list --profile coding
beskar registry where --skill code-review
beskar registry stats
```

`repo remove` unregisters a workspace and preserves its copies. Re-registering makes those existing copies unmanaged. `registry prune` removes only records whose workspace paths no longer exist. Library imports and removals, promotion, updates, and pruning support dry runs.

## Local changes

Beskar compares three fingerprints for each installed skill: the last installed version, the current workspace copy, and the current library version. Status distinguishes clean copies, library changes, local drift, and divergence. Deleting a tracked copy also counts as local drift.

When an update encounters drift, a terminal user can keep the copy, replace or remove it, inspect a diff, or abort. Replacing requires confirmation. All choices are collected before any files change. Without a terminal, the default stops the whole selected batch.

Scripts choose a policy explicitly:

```sh
beskar update --conflict keep
beskar update --conflict replace
beskar update --conflict abort
```

`--on-conflict` is an alias; `fail` means `abort`. `--conflict ask` requests terminal choices and fails safely when no terminal is available. JSON mode never prompts.

Keeping drift retains its recorded baseline, so future updates still report it. Unmanaged destinations are never overwritten or automatically adopted, even when their contents match the library. Unrelated unmanaged skills remain untouched.

Review and promote a local improvement with:

```sh
beskar skill diff code-review
beskar skill promote code-review --dry-run
beskar skill promote code-review
beskar update --all
```

Promotion updates both the library and that workspace's baseline in one transaction. If the library also changed, it requires `--conflict replace` after review. Promotion is a separate command, so aborting an update cannot leave a promotion partially applied.

## Configuration

State defaults to `~/.beskar`. `--home PATH` overrides `BESKAR_HOME`, which overrides the default.

```text
~/.beskar/
  config.bsk
  registry.bsk
  library/
    skills/
    profiles/
```

Beskar uses versioned `.bsk` records. Paths with spaces or special characters are quoted. There are no inferred types, indentation rules, or environment substitutions.

```text
beskar 1
library "/home/alex/.beskar/library"
registry "/home/alex/.beskar/registry.bsk"
agent-skills ".agents/skills"
```

A profile at `library/profiles/coding.bsk`:

```text
beskar 1
# Review changes before shipping.
skill code-review
skill testing
```

CLI edits preserve config and profile comments, record order, and existing line endings. The registry is generated bookkeeping and is rewritten deterministically. See [FORMAT.md](docs/FORMAT.md) for the complete grammar.

Custom initial locations:

```sh
beskar init --library ~/skill-library --registry ~/.local/state/beskar/registry.bsk
```

Use `config set agent-skills .claude/skills` before installing copies. If installations are already tracked, Beskar refuses to abandon their old destination. Unregister and resolve those copies before switching paths.

`config set library PATH` selects an existing library whose enabled profiles are valid. Prepare the new location before switching. `config set registry PATH` copies the current registry to a new file, or accepts an existing identical registry; its parent directory must exist. It refuses to replace unrelated state. Both changes are journaled. Alternatively, edit config while Beskar is idle.

## Scripts and agents

`--json` emits exactly one result document on standard output:

```json
{"ok": true, "data": {}, "error": null}
```

`data` contains the command's structured result. Update reports include repository paths, skill states, proposed actions, fingerprints, and profile membership. Failed updates retain their conflict reports. Errors include a `kind` and `message`; human-readable diagnostics also go to standard error.

Exit codes are 0 for success, 1 for an operation failure or unresolved conflict, and 2 for invalid command syntax. Dry runs fail when the corresponding update would be blocked. Piping output into a program that closes early does not panic.

## Recovery and concurrency

Updates, imports, promotion, registry changes, and configuration changes stage replacements and keep originals in backups. One journal covers every selected workspace and the registry. Ordinary filesystem failures roll back partial work.

After an interrupted operation, run:

```sh
beskar doctor --recover
```

Recovery removes temporary copies from interrupted staging, restores originals from a partial apply, or finishes cleanup when all new targets are already in place. Before recovering an apply, it checks fingerprints. If a target or backup was edited after interruption, both are preserved for manual recovery; the journal records their locations. New empty deployment directories are removed during rollback, while directories containing unrelated files survive.

Kernel locks coordinate the home, library, registry, and each registered repository, including across different Beskar homes. Locks release when the process exits or is killed. Their empty files remain: `.lock` in the home, `.beskar.lock` in library and repository roots, and `<registry-file>.lock`. Do not delete these files while Beskar runs. Exclude them from Git when applicable.

Skills contain regular files and directories. Beskar fingerprints all their contents, including `.git` and caches, and rejects symlinks and special files. Symlinks in the directories that contain the home, library, registry, a workspace, or an import source, such as `/var` on macOS, are resolved once and Beskar records the resolved path. Paths must be UTF-8. File ownership, extended attributes, and directory permissions are not replicated. Directory entries are synced on Unix; rename durability on other systems depends on their filesystem. The test suite runs on Linux, macOS, and Windows.

## Development

```sh
cargo test --workspace --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --offline
```

To release, set `version` in the root `Cargo.toml`, merge the change, then tag that commit `v<version>` and push the tag. The [release workflow](.github/workflows/release.yml) rejects a tag that does not match the version. It runs the tests natively on Linux, macOS, and Windows, builds all six archives, and publishes them with `SHA256SUMS`. Pull requests that change the workflow build the same archives without publishing.

The [architecture](docs/ARCHITECTURE.md) explains the three crates, immutable plans, locking, and recovery. The [consolidation review](docs/CONSOLIDATION.md) records all nine source PRs and why their designs were selected or adapted. Domain vocabulary is in [CONTEXT-MAP.md](CONTEXT-MAP.md).

The original brief is [docs/BRIEF.md](docs/BRIEF.md). Classification, Git synchronization, and a TUI remain outside this implementation.

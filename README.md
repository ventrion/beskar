# Beskar — Better Skill Arrangement

`beskar` is a skill library and installation manager for [Agent Skills](https://agentskills.io):
curate skills in one Git-backed Library, compose them into Profiles, and install the exact union
of skills a project needs into its harness directories — with drift detection, protection of
local modifications, and reversible, plan-first reconciliation.

The name expands to **Better Skill Arrangement**; see the spec for the branding note (§134).

- Spec (normative): [`docs/SPEC.md`](docs/SPEC.md)
- Implementation notes & status: [`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md)

## How it fits together

```text
Library      one Git repository holding skills/, profiles/, catalog.toml
Profile      an ordered, immutable-ID skill set; profiles compose freely
Installation one (workspace, target) pair with one shared source ref
Desired set  union of all attached profiles — one physical copy per skill
Registry     machine-local state: attachments, last-applied membership
Reconciler   converges the target toward the desired set, never destroying
             anything Beskar does not own (modified files, extra files)
```

## Downloads and releases

Download the CLI (including `beskar tui`) and desktop GUI from
[GitHub Releases](https://github.com/ventrion/beskar/releases). Archives are available
for Linux x86-64, Windows x86-64, and macOS Intel / Apple Silicon, with SHA-256
checksum files. Extract the archive and put `beskar` (`beskar.exe` on Windows) on
`PATH`; launch `beskar-gui` for the desktop interface. Git must also be on `PATH`.

Maintainers can run the **Create release** GitHub Actions workflow or ask an agent
to use `$create-release`. See [the release guide](docs/RELEASING.md) for automation,
platform requirements, and recovery instructions.

## Build

Rust 1.98.1 (pinned in `rust-toolchain.toml`), `git` on `PATH`. Once dependencies
are downloaded, tests need no network access.

```sh
cargo build --workspace          # CLI + TUI + desktop GUI
cargo build -p beskar-gui        # desktop GUI (x11/glow by default)
cargo test --workspace           # hermetic: tempdirs + local-path remotes only
```

The GUI optionally builds against Wayland instead of X11:

```sh
cargo build -p beskar-gui --features wayland
```

## CLI overview

Every reading/mutating command accepts `--json` (only JSON on stdout, stable
snake_case identifiers — the automation API, §130) and `--dry-run` where it
mutates something. Exit codes: `0` ok, `1` typed error, `2` usage, `3` action
required/blocked, `4` partial success (§94).

```text
# Library setup (§87)
beskar init                          create a new Library here
beskar init --remote <git-url>       clone an existing Library
beskar init --library <path>         adopt an existing local Library (read-only)

# Curating the Library (§69-§77)
beskar ingest --bucket <bucket> <path> [--recursive]
beskar skill list|show|move|rename|remove|tag|rank
beskar profile list|show|create|delete|rename|add|remove|move|validate
beskar library status|branch|switch

# Installations (§20-§59)
beskar add <profile> <workspace>     attach a profile and reconcile
beskar remove <profile> <workspace>  detach a profile and reconcile
beskar installation profiles|attach|detach|profile-order   same engine (§78-§79)
beskar status [--all]                read-only drift report
beskar why <skill> <workspace>       which profiles require this skill
beskar update [--all]                converge from local refs — never fetches
beskar ref set <workspace> <ref>     move the whole installation to another ref

# Library ↔ remotes (§62-§68)
beskar fetch                         fast-forward local branches — never merges
beskar push [branch]                 never force-pushes, refuses when dirty

# Maintenance (§83-§84)
beskar doctor                        read-only diagnostics, never auto-repairs
beskar registry list|show|prune|repair|move
beskar unregister [--keep-files]     forget an installation

# First-run / legacy (§87, §120-§124)
beskar migrate skm --home <legacy-home> [--registry <file>]
                                     convert a skill-manager Home

# Interactive
beskar tui                           terminal UI (same core as everything else)
```

`beskar-gui` is the desktop GUI over the same core services; it refuses to
start without a usable display instead of degrading silently.

## Environment overrides (§86)

| Variable           | Meaning                                                |
| ------------------ | ------------------------------------------------------ |
| `BESKAR_HOME`      | relocates the whole Beskar home (registry, state dir)  |
| `BESKAR_LIBRARY`   | pins the active Library path, skipping discovery       |
| `RUST_LOG`         | tracing filter; `-v/-vv` flags also raise verbosity    |

Without the first two, Beskar discovers the Library from the current
directory upward (`beskar.toml`) and uses the platform's standard
config/state directories. `beskar migrate skm --registry <path>` redirects
the Registry a migration writes to.

## Platform notes

Linux, macOS, and Windows are supported targets (CI runs the full test suite
on all three). Serialized paths inside Library/Registry files always use `/`
(§119); native paths are converted at the filesystem boundary. Tests are
hermetic and offline: tempdirs, local-path Git remotes, no network (§125).

## Status

v1 implementation is complete; see
[`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md) for the phase-by-phase
summary, design decisions, and the honest list of remaining limitations, and
[`docs/COVERAGE.md`](docs/COVERAGE.md) for the step-by-step §137 v1
product-contract coverage report.

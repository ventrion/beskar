# beskar

Better skill arrangement. beskar keeps agent skills in one global
library, groups them into named profiles, and copies the right profiles
into each repository's `.agents/skills/` directory.

One skill, many projects. Fix the skill once in the library, and every
project picks the fix up on its next `beskar repo update`. Local
changes in a project are detected by fingerprint, never silently
overwritten, and can be promoted back into the library when they turn
out to be the better version.

Written in Rust with zero dependencies. No serde, no clap, no sha2
crate. The configuration format is a small in-tree format called
[bsk](docs/bsk-format.md), and SHA-256 is about 150 lines next door to
the fingerprint code. `cargo build` needs nothing but the compiler.

## Quick start

```sh
beskar init                       # ~/.beskar with config, registry, library
beskar --yes library scan ~/my-skills
beskar profile create coding -d "Everyday skills"
beskar profile add coding code-review git-tools

cd ~/work/api
beskar repo add .
beskar repo enable coding
beskar repo update                # copies skills into .agents/skills/
```

After that, the daily loop is two commands:

```sh
beskar repo status                # what drifted, what is pending
beskar repo update                # reconcile with the library
```

## How state is organized

Three places, each with one job.

**The library** (`~/.beskar/library/`) holds skills and profiles. Skills
are plain directories, portable and syncable, happy to live in a git
repo of their own. Profiles are text files listing skill ids, one per
line, and you can edit them by hand.

**The registry** (`~/.beskar/registry.bsk`) is machine-local state:
which repos beskar manages, which profiles are enabled in each, and the
fingerprints of what was installed. It is rebuildable from the library
plus the workspaces, so it never needs to be synced anywhere.

**The workspace** (`<repo>/.agents/skills/`) holds materialized copies.
Copies, not symlinks: a repo directory stays self-contained and works
in CI, in a container, and on a machine that never heard of beskar.
beskar does not read or write git metadata, so a workspace does not have
to be a git repo at all.

Each installed skill is fingerprinted with SHA-256 over a canonical
manifest of its files (paths, sizes, content hashes; symlinks recorded
as links). Three fingerprints per install tell the whole story:

| workspace copy | library version | meaning |
|---|---|---|
| unchanged | unchanged | clean |
| unchanged | changed | pending update |
| changed | unchanged | local modifications |
| changed | changed | conflict |

## Conflict policy

When an update would clobber local changes, beskar asks rather than
guesses. Interactively it prompts per conflict: keep local, take the
library version, promote local into the library, show a diff, or abort.
In scripts and CI there is no one to ask, so the default is to keep
local files, announce it, and exit non-zero.

Explicit policies for non-interactive runs:

```sh
beskar repo update --conflict skip      # keep local, change nothing
beskar repo update --conflict replace   # library wins
beskar repo update --conflict promote   # local wins, and becomes the library
beskar repo update --conflict abort     # stop at the first conflict
```

The same rule shows up everywhere. A drifted skill that a profile no
longer wants is kept on disk, not deleted. `repo remove --purge` removes
clean installs but refuses to delete files it did not write. Deleting a
profile in use, or a skill referenced by a profile, requires `--force`.

## Inspecting things

`beskar doctor` checks the whole deployment: config paths, library
integrity, registry sanity, and per-repo state, and exits non-zero on
real problems. `beskar repo status` explains each skill in one line.
`beskar registry stats` counts repos, profiles, installs, and library
skills nothing uses anymore.

```sh
$ beskar repo status
Repository: /home/u/work/api
profiles: coding
last sync: 2026-09-29T11:28:25Z (3 hours ago)

 = code-review                  clean [via coding]
 ~ git-tools                    library has a newer version [via coding]
 + lint-runner                  wanted, not installed

2 pending — run `beskar repo update` to reconcile
```

## Building and testing

```sh
cargo build            # no dependencies to fetch, because there are none
cargo test             # unit tests plus an end-to-end test of the binary
```

Rust edition 2021. The binary is `target/debug/beskar` or
`target/release/beskar`.

## Layout

```
crates/beskar/src/
  main.rs         entry point, exit codes
  cli.rs          hand-rolled argument parser, help text
  commands/       one module per command family
  bsk.rs          the configuration format (parse, write, round trip)
  config.rs       config.bsk
  profile.rs      profiles/*.bsk
  registry.rs     registry.bsk
  library.rs      the on-disk library
  reconcile.rs    planner + applier (pure plan, side effects at the edge)
  fingerprint.rs  SHA-256 directory manifests
  sha256.rs       SHA-256
  diff.rs         small line diff for conflict previews
  ui.rs           colors, symbols, prompts
  util.rs         fs helpers, atomic writes, ISO timestamps
```

The reconciliation logic is a pure function of desired state, recorded
installs, and workspace fingerprints. It returns a plan of actions; a
separate applier performs them. This split is what makes the drift
matrix testable without a filesystem, and it keeps conflicts from being
resolved deep inside some copy routine.

## Environment

`BESKAR_HOME` overrides the beskar home directory. `NO_COLOR` and
non-terminal stdout disable color. Exit codes: 0 success, 1 something
did not fully succeed, 2 usage error.

## What beskar does not do

No daemon, no lockfile negotiation, no network. It does not sync the
library between machines; keep the library in git or syncthing or a
shared drive, and run beskar locally. It does not track skills by
upstream URL; the library is where skills live, and `library scan` is
how they get in.

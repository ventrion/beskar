# Releasing Beskar

The public repository is https://github.com/ventrion/beskar. Releases use stable
`vX.Y.Z` tags and one shared version in `Cargo.toml` / `Cargo.lock`.

## Create a release

Ask a repository-aware agent to use `.agents/skills/create-release/SKILL.md`, or:

1. Review changes since the latest release. Commit curated user-facing notes to
   `docs/releases/vX.Y.Z.md` on `main`. The initial `v0.1.0` notes are supplied.
   If the file is absent, GitHub generates notes using `.github/release.yml` and
   merged pull requests. Write curated notes for direct-commit releases, since
   GitHub's automatic notes primarily describe merged pull requests.
2. Run `gh workflow run release.yml --repo ventrion/beskar --ref main -f version=X.Y.Z`.
   `major`, `minor`, or `patch` are also accepted; these increment the workspace
   version. For the initial release, explicitly use `0.1.0`.
3. Follow the run with `gh run list --repo ventrion/beskar --workflow release.yml`,
   then `gh run watch RUN_ID --repo ventrion/beskar --exit-status`.

The workflow updates every local workspace package in the lockfile without
upgrading dependencies, writes a dated changelog entry identical to the release
notes, commits the changes, and pushes `main`. It then runs formatting, Clippy,
the full test suite on all four runners, release-script tests, and the Linux
Wayland feature check. Native release builds run against that exact commit.
Only after all checks and builds pass does it create an annotated tag and publish
a GitHub Release containing the notes and all four archives and checksums.
Uploads happen in a draft so partially uploaded releases stay unpublished.

Only repository writers can dispatch releases. The workflow uses the built-in
`GITHUB_TOKEN` with `contents: write` in preparation/publication jobs; no personal
access token or separate release secret is needed. It must run from `main` and
must be allowed to push release commits to that branch. If branch protection is
added later, adapt the preparation step to the organization's approved merge
process. A release run directly calls reusable CI because pushes made with
`GITHUB_TOKEN` do not trigger another push workflow.

## Artifacts

| Runner | Target | Archive |
| --- | --- | --- |
| Ubuntu 24.04 | `x86_64-unknown-linux-gnu` | `.tar.gz` |
| Windows 2022 | `x86_64-pc-windows-msvc` | `.zip` |
| macOS 15 Intel | `x86_64-apple-darwin` | `.tar.gz` |
| macOS 15 Apple Silicon | `aarch64-apple-darwin` | `.tar.gz` |

Each archive contains the CLI, integrated TUI, desktop GUI, README, changelog,
and this guide. Each `.sha256` file contains a SHA-256 digest and archive filename.
On Linux use `sha256sum -c ARCHIVE.sha256`; on macOS use
`shasum -a 256 -c ARCHIVE.sha256`; on Windows compare
`(Get-FileHash ARCHIVE.zip -Algorithm SHA256).Hash` to the checksum file.

Git is a runtime requirement. Linux builds require glibc 2.39+ and the GUI uses
X11/XWayland and OpenGL. Windows binaries target Windows 10+ and require the
[Microsoft Visual C++ v14 x64 Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist)
(`VCRUNTIME140.dll`). macOS binaries target macOS 15+. The archive contains executables rather than installers or macOS app
bundles. Windows code signing and macOS signing/notarization are not configured.

## Local scripts

Python 3.11+ is required; CI uses 3.12. Rust is pinned by `rust-toolchain.toml`.
Run commands from the repository root. Preparation requires a clean working tree
and fetched tags (`git fetch origin --tags`). Commit curated notes before running
it. It edits files only; it does not commit, tag, push, or publish.

```sh
python scripts/release.py prepare --version 0.2.0
python scripts/release.py validate --tag v0.2.0
cargo build --release --locked --target x86_64-unknown-linux-gnu -p beskar-cli -p beskar-gui
python scripts/release.py package --tag v0.2.0 --target x86_64-unknown-linux-gnu
python -m unittest discover -s scripts/tests -v
```

Use the GitHub workflow for publication. Its `publish` script requires all four
archives and checksums and a tag pointing at the checked-out commit. Version
strings cannot contain prerelease/build suffixes; support for prereleases can be
added separately when the project needs them.

## Recover a failed release

- If preparation succeeded, use **Re-run failed jobs** on that run. This retains
  the prepared version and commit, including when `version=patch` was used.
  Dispatching `patch` again would prepare a different version.
- If code needs fixing after preparation, commit the fix on `main` and dispatch
  the same explicit `X.Y.Z`. Preparation reuses an existing matching changelog
  entry. This is permitted only while its tag does not yet exist.
- If publication failed after tagging, rerun the failed publication job. It
  verifies that the existing tag points to the original commit and can resume
  uploading to its draft release. It never moves an existing tag or changes assets
  on an already published release. Inspect the release first if the runner lost
  its connection during the final publish step.
- A concurrent update to `main` causes preparation's ordinary push to fail.
  Start a new run from the updated `main`; no force push is performed.

GitHub's concurrency group prevents overlapping active release runs. Avoid
queuing multiple releases: GitHub may replace a pending run with a newer request.

---
name: create-release
description: Prepare and publish a Beskar release with a version bump, curated release notes, changelog entry, annotated tag, and Linux, Windows, and macOS builds. Use when asked to create or prepare a release in this repository.
---

# Create release

Read [the release guide](../../../docs/RELEASING.md) for commands, supported targets,
and failure recovery. Work from this repository's root; the destination is
`ventrion/beskar`. Python 3.11+, Rust via rustup, Git, and authenticated `gh` are
required for local preparation and publication.

Inspect the working tree, current workspace version, fetched `v*` tags, and changes
since the latest released tag. Include merged PRs and direct commits. Preserve
unrelated user edits. Choose the user-requested version; otherwise infer a stable
patch/minor/major bump from the compatibility impact and state your choice. For a
first release, the existing untagged workspace version is valid.

Write `docs/releases/vX.Y.Z.md` with concise user-facing changes, meaningful fixes,
breaking changes/migration steps when present, download guidance, and known
limitations supported by the code. Link relevant PRs/issues when available. Do not
invent features or turn raw commit messages into the entire release narrative.
These notes become both the GitHub release body and the dated `CHANGELOG.md` entry.

For a preparation-only request, stop with reviewable local changes: commit notes
only if requested or needed within authorized scope, then use
`python scripts/release.py prepare --version X.Y.Z` from a clean tree, or explain
that preparation requires committing existing changes. Do not publish for a
preparation-only request. Do not add approval steps when the user already asked
you to create/publish the release.

For an authorized release, commit and push the notes and intended source changes
to `main` using normal non-force operations. Dispatch `release.yml` with the
explicit version, then monitor that run. Let the workflow handle version/lockfile
updates, changelog generation, checks, packaging, tagging, and publication; do not
manually pre-create its tag or bypass failing checks.

Verify all required jobs succeeded, the published tag resolves to the prepared
commit, and the release contains four platform archives plus four checksum files.
Report the release URL, version, and any material limitation. On failure, inspect
logs and follow the recovery guide; do not repeatedly dispatch a relative bump,
move tags, or overwrite a published release. If recovery requires missing access
or signing credentials, report the concrete blocker.

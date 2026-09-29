# AGENTS.md

## Agent skills

### Issue tracker

Issues live in Rohrpost, the git-native tracker under `.rohrpost/` (use the `rp` CLI with `--json`). See `docs/agents/issue-tracker.md`.

### Triage labels

The five default labels: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Multi-context: a root `CONTEXT-MAP.md` points to `crates/<name>/CONTEXT.md`. See `docs/agents/domain.md`.

## Code

Rust workspace on the standard library alone. A new dependency needs an ADR that supersedes `docs/adr/0002-standard-library-only.md`.

Read and write Beskar's own files (settings, profiles, registry) through the `bsk` crate, which edits in place and keeps the user's comments.

`reconcile::decide` in `crates/beskar-core/src/reconcile.rs` is the only place that chooses to overwrite or remove installed files, and `crates/beskar-core/tests/safety.rs` fuzzes that promise. Send every destructive step through it.

Test a command by calling `beskar_cli::run` with the `Sandbox` in `crates/beskar-cli/tests/common/mod.rs`.

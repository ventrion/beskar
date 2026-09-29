# AGENTS.md

## Agent skills

### Issue tracker

Issues live in Rohrpost, the git-native tracker under `.rohrpost/` (use the `rp` CLI with `--json`). See `docs/agents/issue-tracker.md`.

### Triage labels

The five default labels: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Multi-context: a root `CONTEXT-MAP.md` points to `crates/<name>/CONTEXT.md`. See `docs/agents/domain.md`.

## Code

A Rust workspace with no crates from outside it (`docs/adr/0002-no-external-dependencies.md`). A change is finished when `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check` pass. `beskar-core` returns data and takes decisions as arguments; printing and prompting belong in `crates/beskar`.

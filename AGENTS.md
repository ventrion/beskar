# AGENTS.md

## Agent skills

### Issue tracker

Issues live in Rohrpost, the git-native tracker under `.rohrpost/` (use the `rp` CLI with `--json`). See `docs/agents/issue-tracker.md`.

### Triage labels

The five default labels: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Multi-context: a root `CONTEXT-MAP.md` points to `crates/<name>/CONTEXT.md`. See `docs/agents/domain.md`.

## Code

A Rust workspace with no crates from outside it (`docs/adr/0002-no-external-dependencies.md`). A change is finished when `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` pass.

Every rule of an operation (what it checks, locks and records) lives in `beskar_core::ops`, one method per command returning a report (`docs/adr/0003-operations-live-in-the-core.md`). `crates/beskar` only parses, asks and renders, as text and as JSON; a new command gets both renderings and an entry in `docs/JSON.md`. Changes to the library, the registry or a workspace go through `Beskar::transact` (`crates/beskar-core/docs/adr/0003-locking-and-recovery.md`). The safety rule of reconciliation is the decision table in `crates/beskar-core/src/reconcile.rs`; `crates/beskar-core/tests/safety.rs` checks it on random histories.

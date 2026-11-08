# Reconciliation: design_draft.md against concepts_brief.md (round 2)

Date: 2026-11-08. Every contradiction below was raised and decided by the
author during drafting. None is open.

## Contradictions (brief decided X, draft decides Y)

1.  Brief decision 11: "Third-party format libraries are allowed only in the harness layer." Draft Non-Goals: "Third-party crates anywhere in v1." Decided in session (D3, follow-up gate).
2.  Brief decision 12: "A separate harness crate ... covers skill and agent locations, rendering, headless invocation, model listing and the vendor map." Draft D3: std-only `beskar-targets` holds only the target table and renderers. Decided in session (D3; decision 14 had already removed runners).
3.  Brief decision 3: "Per-harness skill text where needed. Enabled harnesses get their own directories." Draft G5: "One target-neutral `SKILL.md` ... is the default. Per-target skill variants ... only for targets where Stage 0 shows neutral dispatch text fails." Decided by approving Goals; flagged at the time.
4.  Brief decision 15: "`curation/` for provenance". Draft: `adaptation/<upstream>.bsk`, `beskar adapt`, `beskar-adapt`. Decided in session (Q9).
5.  Brief decision 15: "the repository needs a naming rule (for example a prefix per upstream)." Draft: upstream names kept; a clash fails `adapt check` and is resolved by a recorded `rename`. Decided in session (Q5).
6.  Brief decision 18: "The maintainer, detected by the repository being writable or by a config flag, is asked to re-curate." Draft Part E: "Every reader gets the same wording. Beskar does not try to detect who maintains the adapted repository." Decided in session (Q6).
7.  Brief decision 2 / B5: "Older binaries refuse it cleanly" and "document the exact message an old binary prints". Draft G6: 0.4.x stops with its existing `line 1: expected 'beskar 1' as the first record`; only later binaries name a newer Beskar. Decided in session (D1; G6 reworded because released binaries cannot print a new message).
8.  Brief decision 14: helpers "can be std-only `beskar` commands or scripts shipped with the skill." Draft G9: `beskar adapt` commands. Decided by approving Goals; flagged at the time.
9.  Brief Open Question 11 / Major risks: "Beskar must detect and explain this, and offer to replace the link as one grouped change, never silently." Draft Part F: grouped, confirmed migration, limited to links resolving to the repository's own skill destination; all other symlinks refused. Consistent in intent; scope narrowed. Decided in session (Q7).

## Additions that change architecture, an external interface or a dependency

1.  New crate `beskar-targets` and dependency direction `cli → core → targets → bsk`; the JSON string encoder moves there and the CLI depends on it.
2.  `ls-remote` URL and protocol allowlist (`https`, `ssh`, scp-style, absolute `file://`; `ext::` and others blocked; `GIT_PROTOCOL_FROM_USER=0`), and `remote sync --no-upstream-check`.
3.  Format upgrade policy: a file becomes `beskar 2` on first need and is never downgraded.
4.  `agent.bsk` `override TARGET PROMPT-HASH` record, `library agent override-ack`, and stale-override checks in `adapt check` and `doctor`.
5.  Journal record `unlink LINK TARGET`, `--replace-link PATH`, and an ADR 0007 section allowing one symlink operation (amends ADR 0003).
6.  `beskar adapt init | upstream add | upstream update | check | record | status`, all store-free and run on `--dir`.
7.  CI step that fails when `cargo tree` lists any non-workspace package.
8.  Agents follow skills: an agent is deployed only when a skill it names is desired; an agent naming no skill is an error.
9.  A target that cannot express `readonly` refuses to render a `readonly` agent; overrides on `readonly` agents become manual access-review items in the PR.
10. Registry ownership keyed by output path (`agent-file PATH AGENT HASH TARGET...`), with shared paths between targets.
11. Tier vocabulary `fast | balanced | deep`.

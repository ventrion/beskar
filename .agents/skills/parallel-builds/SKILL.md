---
name: parallel-builds
description: Use when orchestrating multiple builder agents in parallel (workflow tool or raw subagents), setting up git worktrees for builders, assigning migrations or shared files across branches, or planning the merge order of many feature branches.
---

# Parallel builds

Builders in isolated worktrees cannot see each other. Everything they cannot
know — the gate verdict on the base commit, sibling migration revisions, who
owns which file — must be written into their briefs by the orchestrator, and
everything they produce must be verified against main, not against their own
worktree.

**Orchestrate with the workflow tool; verify before diagnosing; gate on main.**

## Before spawning

1. Run the full gate suite on the base commit. Pre-existing failures go to the
   human, and the accepted-failure list goes into **every** builder brief —
   otherwise a builder "fixes" something unrelated or hides behind it.
2. Create each builder's worktree from a **committed** baseline, then run the
   unit suite once in a fresh worktree. Gates green in a worktree for the
   wrong reason (untracked helpers, host leftovers) are worse than red.
3. Disk preflight: every worktree that runs `uv run` (or any package manager)
   materializes a full environment (~250 MB here). `df -h` the target
   filesystem, `git worktree list`, and clean stale worktrees and env dirs
   first. A full disk kills the host session mid-run (EDQUOT), not just the
   workflow.
4. Partition files: never assign the same edit to two builders. Files several
   builders will touch (container wiring, registries, task registration) get
   one of: a single owner builder, a pinned contract in every brief (exact
   names, registration shape), or sequential builds that rebase.

## Briefs

- Assign migration revisions explicitly ("you create 0007, down_revision
  0006"): builders in isolated worktrees cannot see sibling branches, and
  "chain off the current head" guarantees multiple heads when two builders
  each add one.
- State the workflow-script API contract in the brief or follow it yourself:
  in workflow scripts `agent`, `parallel`, `phase`, and `args` are provided
  globals — never shadow them, and pass thunks to `parallel([...])`, never to
  `Promise.all` (which does not invoke them).
- When results look empty, inspect what the script actually returned before
  theorizing: agents that never ran leave no transcript, empty results, and
  no error. "The runtime cannot do X" is a last-resort claim, verified, never
  narrated.

## Merging

1. Merge in dependency order (foundation first), one branch at a time.
2. After **each** merge: unit suite, full gates, smoke — on main. Fix forward
   on main, never inside a merged branch. Verify single alembic head at every
   merge, not at deploy.
3. After resolving any wiring conflict, re-read the resolved file whole:
   auto-merged hunks silently drop lines above the conflict, and each side's
   registrations and constructor calls must all survive.
4. Diff the merge result for stacked duplicates when two branches were
   flagged as touching identical lines.
5. The integrator removes each builder's worktree and env dir immediately
   after merging.

## Housekeeping

Prefer `worktrunk` (`wt`) over raw `git worktree` for housekeeping where it
is installed; the commands map 1:1 and it handles pruning.

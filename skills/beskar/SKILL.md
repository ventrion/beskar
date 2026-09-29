---
name: beskar
description: Use when changing, adding or removing a skill in this workspace's skills directory, when a skill the task needs is missing, or when a `beskar` command reports local changes, conflicts or exit status 3.
---

# Working with Beskar-managed skills

Beskar copies this workspace's skills, this one included, from the user's library into the directory this skill sits in, following the profiles enabled for the workspace. `beskar update` refreshes those copies from the library. A copy edited here keeps its edits: Beskar detects the local change and asks before replacing it.

## 1. Check the state

Run `beskar status` in the workspace.

Done when you know the row of every skill the task touches: `✓` up to date, `+` not installed yet, `~` the library has a newer version, `*` changed here, `!` conflict, `✗` missing from the library or blocked, `?` not managed by Beskar.

## 2. Act on the task

- **Change a skill for this workspace only**: edit its directory here. It shows as `*`, and updates leave it alone until the library version changes.
- **Change a skill for every workspace**: edit it here, show the user `beskar repo diff <skill>`, and once they approve, run `beskar repo promote <skill>` and then `beskar update --all`.
- **Discard local edits**: `beskar repo restore <skill> --yes`.
- **Get a missing skill**: `beskar profile list` shows the profiles and `beskar library list` the skills. Ask the user which profile to enable, then run `beskar repo enable <profile>` and `beskar repo update`.
- **Settle a conflict** (`!`): show the user `beskar repo diff <skill>`, then run `beskar repo update --on-conflict keep` or `--on-conflict replace`, as they choose.
- **Unblock a skill** (`✗ ... but it has its own .git`, or files that are not part of the skill): follow the `help:` line `beskar status` prints for it, with the user's approval.
- **Exit status 3** means Beskar needs a decision: a conflict (`--on-conflict`) or a confirmation (`--yes`). Ask the user before passing either.

Done when `beskar status` shows each touched skill in the state the task asked for.

## Reference

- Profiles are files in the library's `profiles/` directory, one `skill: <name>` line per skill. `beskar help format` describes the syntax.
- Every command takes `--help`, and `--dry-run` previews `update`, `repo remove` and `library scan`.

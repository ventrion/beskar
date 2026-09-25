---
name: resolving-merge-conflicts
description: "Use when you need to resolve an in-progress git merge/rebase conflict."
---

1. **See the current state** of the merge/rebase. Check git history, and the conflicting files.

2. **Find the primary sources** for each conflict. Understand deeply why each change was made, and what the original intent was. Read the commit messages, check the PRs, check original issues/tickets.

3. **Resolve each hunk.** Preserve both intents where possible. Where incompatible, pick the one matching the merge's stated goal and note the trade-off. Do **not** invent new behaviour. Always resolve; never `--abort`. When uniting branches that touched the same wiring (container, registry, task registration), re-read the resolved file whole: auto-merged hunks silently drop lines above the conflict, and each side's registrations and constructor calls must all survive.

4. Discover the project's **automated checks** and run them, typically typecheck, then tests, then format. Fix anything the merge broke.

4b. **Sweep the whole repository for conflict markers**, not only the files you edited: `rg -n '^(<<<<<<<|=======|>>>>>>>)' -g '!.venv*'` from the repo root. No automated gate covers files no test imports (e.g. `migrations/env.py`), so a marker there reaches production with every gate green.

5. **Finish the merge/rebase.** Stage everything and commit. If rebasing, continue the rebase process until all commits are rebased.

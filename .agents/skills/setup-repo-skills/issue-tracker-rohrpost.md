# Issue tracker: Rohrpost

Issues and specs for this repo live in **[Rohrpost](https://github.com/code-factorio/rohrpost)**, a git-native tracker. Tickets are events in `.rohrpost/log.jsonl`, committed with the code — there is no external service to reach for and no daemon to run, so the tracker branches and merges exactly like the code does.

## Invoking `rp`

`rp` walks up from the current directory to find `.rohrpost/`, so it works from anywhere in the repo. Every command accepts `--json`; **always pass `--json`** — it is the agent-facing interface, and the plain output is for human terminals.

Ticket ids are bare (`a1b2c3`) or rendered with the project's display prefix (`RP-a1b2c3`); `rp` accepts either. Prefer the bare id in scripts. Exit codes: `0` success, `1` domain failure (no such ticket, bad status), `2` usage error.

## Conventions

- **Create a ticket**: `rp new "<title>" --body "<markdown>" --json`. Add `--type task|bug|spike|epic` (default `task`), `-p 0..4` (0 highest), `--label` (repeatable), `--blocked-by <id>` (repeatable), `--parent <epic-id>`.
- **Read a ticket**: `rp show <id> --include body,deps,notes --json`. Notes are the comment thread; `rp comments <id> --json` fetches them alone.
- **List tickets**: `rp list --status open --label <label> --json`. Also filters on `--type`, `--parent`, and `--match <text>` for a case-insensitive substring of the title. Filters compose.
- **Find work**: `rp ready --json` — open, unblocked, non-epic tickets, highest priority first. This is the queue an agent picks from.
- **Comment**: `rp comment <id> "<note>"` (or `--body-file` for long notes). Notes are local and never leave the repo.
- **Apply / remove labels**: `rp set <id> labels+=a,b` / `rp set <id> labels-=a`. Set fields use `+=` / `-=` so two concurrent runners compose instead of clobbering.
- **Update any scalar field**: `rp set <id> status=review priority=1` (`title`, `type`, `status`, `priority`, `assignee`, `parent`, `body`).
- **Claim**: `rp claim <id>` — moves to `in_progress` and stamps the actor as assignee.
- **Close**: `rp close <id> --reason "<why>"`. **Abandon**: `rp drop <id> --reason "<why>"`.

All mutations are idempotent: re-running `rp close` on a done ticket appends nothing and still exits `0`.

Multi-line bodies go through `--body-file` — a path, or `-` for stdin: `rp new "<title>" --body-file body.md --json`, or pipe a heredoc into `--body-file -`. `--body` and `--body-file` are mutually exclusive; `set` takes `--body-file` too (exclusive with `body=`).

## Statuses

`open` → `in_progress` → `review` → `done`, with `waiting` for "stalled on a human" and `dropped` as the other terminal. `ready` is **derived, never set**: a ticket is ready when it is `open`, not an epic, and every `blocked_by` ticket is `done`. Closing a blocker unblocks its dependents with no extra write.

Note the asymmetry: a **`dropped` blocker does not unblock** its dependents. If a blocker is abandoned, remove the edge explicitly with `rp set <dependent> blocked_by-=<id>`.

## Actors

Agents should identify themselves so the log distinguishes them from humans: set `ROHRPOST_RUNNER=<agent>` and `ROHRPOST_BATCH=<batch>` in the environment (yielding `runner/<agent>@<batch>`), or pass `--actor` per command. Without either, events are attributed to `user/<git config user.email>`.

## When a skill says "publish to the issue tracker"

Run `rp new`, putting the ticket prose in `--body`.

## When a skill says "fetch the relevant ticket"

Run `rp show <id> --include body,deps,notes --json`.

## Pull requests as a request surface

**PRs as a request surface: no.** _(Set to `yes` if this repo should pull external PRs into the triage queue; `/triage` reads this flag.)_

When set to `yes`, read the PR with the host's own CLI, then record each one as a Rohrpost ticket (`rp new "..." --label needs-triage --body "PR: <url>\n\n..."`) so triage state stays in one place. Keep the conversation on the host; keep the triage decision in Rohrpost.

## Wayfinding operations

Used by `/wayfinder`. The **map** is an epic; its **child tickets** are the epic's children. Rohrpost has native parent and blocking relationships, so no body conventions are needed.

- **Map**: an epic labelled `wayfinder:map`, holding the Destination / Notes / Decisions-so-far / Fog body: `rp new "<destination>" --type epic --label wayfinder:map --body-file map.md --json`. Update it in place — read the current body first (`rp show <map-id> --include body --json`), edit it, write it back whole with `rp set <map-id> --body-file map.md --json`.
- **Child ticket**: `rp new "<question>" --parent <map-id> --label wayfinder:<type> --json --body-file - <<'EOF'` with the `## Question` prose on stdin, where `<type>` is `research`, `prototype`, `grilling`, or `task`. Epics nest one level: children of the map, never grandchildren.
- **Blocking**: native `blocked_by`. Set at creation with `--blocked-by <id>` (repeatable) or later with `rp set <child> blocked_by+=<id>` / `blocked_by-=<id>`. A ticket is unblocked when every blocker is `done`.
- **Frontier query**: `rp ready --json`, then keep tickets whose `parent` is the map id. `ready` already excludes blocked tickets, epics, and anything claimed (a claim moves the ticket to `in_progress`). First in the list wins.
- **Claim**: `rp claim <id>` — the session's first write, before any work.
- **Resolve**: `rp comment <id> "<answer>"`, then `rp close <id> --reason "<one-line gist>"`, then append the gist plus a link to the map's Decisions-so-far via `rp set <map-id> --body-file ...`.
- **Rule out of scope**: `rp drop <id> --reason "<why it sits past the destination>"`, then add a line to the map's **Out of scope** section. Use `drop`, not `close` — `done` would file a mis-scoped ticket among the map's decisions, and `dropped` is the terminal for work abandoned rather than resolved.
- **Whole-map view**: `rp tree <map-id> --json` renders the epic and its children.

### Handles

Ticket ids are random, so they are hard to type and impossible to remember. Give every map a short **prefix** — a single unbroken word, no dashes, agreed with the user when the map is charted — and carry a **handle** as a leading bracketed prefix in each title:

```
[addr]    Human-addressable tickets: how a person points at a ticket   ← the map
[addr-1]  Which human moments actually require typing a ticket id?     ← a child
```

The bare `[addr]` names the map itself, so a human can say `/wayfinder addr` instead of quoting an id. The sequence is allocated by the charting session, which creates the tickets serially.

The number is **identity, not order**. Maps grow as fog graduates into tickets and shrink as tickets are ruled out of scope, so allocate monotonically, never reuse a number, and never renumber to close a gap. `[addr-3]` does not run before `[addr-5]`; the `blocked_by` graph is the only order there is.

**`rp` knows nothing about this convention.** There is no handle field, no uniqueness contract, and no `doctor` check — the id remains the only identity and the handle is a search key. That is what makes it safe: two branches both allocating `[addr-7]` produce two titles sharing a string, not a corrupt id. Nothing is lost, and renumbering one repairs it.

#### Searching for a handle

Three query shapes, each exact. The brackets and the dash are the delimiters that make them so:

```bash
rp list --match "[addr]"   --json     # the map epic, and nothing else
rp list --match "[addr-"   --json     # every child of that map
rp list --match "[addr-2]" --json     # exactly one ticket
```

Each is safe against a longer prefix, because the next character always differs: `[addr]` cannot match `[address]` (`]` vs `e`), `[addr-` cannot match `[address-1]` (`-` vs `e`), and `[addr-2]` cannot match `[addr-20]` (`]` vs `0`). **Never search the bare `[addr`** — dropping the trailing delimiter is what drags in every prefix that merely starts the same way.

One naming rule follows: **a map prefix must not contain a dash.** A prefix like `addr-x` puts `[addr-x-1]` in range of `[addr-`, which is the one way to reintroduce the collision. Keep prefixes a single unbroken word.

`--match` is a substring, never a regex, and that is deliberate: as a regex, `[addr]` is a character class matching `a`, `d` or `r`, so it would silently match nearly every title in the repo. A dumb matcher cannot produce that answer.

A handle is a search key, not an address: `rp show`, `rp claim`, `rp close`, `rp comment` and `rp set` all take ids only. Resolve first, then mutate with the id that comes back.

For the whole map in one call, prefer the native parent edges over a title search:

```bash
rp list --match "[addr]" --json       # -> the map's id
rp tree <map-id> --json               # -> the map and every child
```

`rp tree` is authoritative where a title search is not: it still finds a child whose handle was renumbered, dropped, or never applied in the first place.

Titles are mutable and fold under per-field last-write-wins, so any later edit can drop a handle. Do not treat a handle as a durable external reference — it is a convenience for typing, not an identifier to cite in a commit message.

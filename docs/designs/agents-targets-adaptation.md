# Agent Definitions, Targets and Skill Adaptation

*   **Author:** Vinzenz Feenstra
*   **Status:** Proposed
*   **Date:** 2026-11-08

--------------------------------------------------------------------------------

## Overview

Beskar copies skills from a user's library into `.agents/skills/` inside each
repository (Beskar's word for a local workspace). Most coding tools read that
directory. Some skill collections also need **agent definitions**: files such
as `.claude/agents/zoom_out.md` that describe a named helper agent with its own
prompt, tool access and model. A skill hands work to such an agent by name.
Google's elephant-goldfish-model (EGM) collection is the motivating case. Its
review skills send work to seven reviewer agents.

Beskar cannot install agent definitions today. Even if it could, each coding
tool (a **target**) keeps agent files in its own directory, in its own format,
and gives skills its own way to call them. This design covers six targets:
Claude Code, Cursor, OpenCode, GitHub Copilot CLI, GitHub Copilot in VS Code,
and Codex.

The design has three parts:

1.  **Agent definitions as library items.** One agent definition is a shared
    prompt (`PROMPT.md`), a short `agent.bsk` record, and optional native files
    for individual targets. Beskar writes a native agent file for each target
    a repository enables, with the same ownership and fingerprint rules it
    already applies to skills.
2.  **Adapted collections.** Upstream collections such as EGM are written for
    a plugin system. They locate their files through `${CLAUDE_PLUGIN_ROOT}`
    and dispatch in ways that only some targets understand. A `beskar-adapt`
    skill, run inside the user's own coding agent, rewrites such a collection
    into a separate Git repository: portable skills, agent definitions, the
    upstream originals, and a provenance record. A small council of reviewer
    agents checks the rewrite. The user approves it through a pull request.
    Beskar then tracks that repository as an ordinary remote, so people who
    subscribe to it never run a model.
3.  **Dirty detection in sync.** When a tracked remote carries adaptation
    provenance, `beskar remote sync` reports which adapted items have fallen
    behind their upstream or been edited by hand. It never changes what is
    deployed.

Stages 3 to 6 of this design rest on facts about the six targets that nobody
has checked against vendor documentation (directory paths, frontmatter fields,
dispatch syntax). A Stage 0 spike, with no code, settles them first. Those
sections stay provisional until it does.

## The Problem

A user who tracks EGM with `beskar remote add` today gets its skills and none
of its reviewer agents. The review skills then fail, or quietly fall back to
doing the review in the main agent's context. That defeats the reason for
separate reviewers: a reviewer that shares the author's context is not
independent.

Placing EGM's agent files by hand does not scale, for four reasons:

*   **Every target wants a different file.** Claude Code reads Markdown with
    YAML frontmatter from `.claude/agents/`. Other targets use other
    directories, other fields and sometimes TOML. One prompt has to become up
    to six files, and they must stay in step.
*   **Beskar would not own them.** Files placed by hand fall outside Beskar's
    fingerprints. Beskar cannot update them, detect drift in them, or remove
    them when the skill that needed them goes away.
*   **The upstream text is not portable.** EGM's skills reference
    `${CLAUDE_PLUGIN_ROOT}/agents/...` and assume Claude Code's plugin loader.
    Outside that loader the paths point nowhere. Fixing this at install time
    would mean path substitution, which the author rejected as a bad design.
*   **Upstream keeps moving.** Any hand conversion goes stale on the next
    upstream commit, and nothing tells the user it went stale.

The proposal splits the work by who pays for it. Rendering agent files and
detecting staleness is deterministic, std-only Beskar code that runs on every
machine. Rewriting upstream text needs a model, so it runs once per upstream
change, in the adapted repository's maintainer's own agent. Every subscriber
reuses the result.

## Goals

*   **G1. Agent definitions are library items.** An agent definition lives in
    `agents/NAME/` in a library or collection: `PROMPT.md`, `agent.bsk`
    (the skills it serves, `readonly` or `readwrite`, model tier), and
    optional whole-file native overrides per target. One agent can serve
    several skills.
*   **G2. Six targets.** Claude Code, Cursor, OpenCode, Copilot CLI and
    Copilot VS Code each receive a native agent file. Codex counts as
    supported through a fallback: its skill text tells a general-purpose
    subagent to read the persona file at a literal path.
*   **G3. Targets are enabled per repository.** A repository with no target
    enabled behaves exactly as it does in 0.4.x and gets nothing new on disk.
    An enabled target gets its agent directory, created if missing.
*   **G4. Installed agent files are owned like installed skills.** Beskar
    records each output path with its fingerprint, detects local drift,
    updates and removes the file, and lets two targets share one file when
    they read the same path.
*   **G5. Skill text stays shared unless Stage 0 proves it cannot be.** One
    target-neutral `SKILL.md` in `.agents/skills/` is the default.
    Per-target skill variants and directories are added only for targets
    where Stage 0 shows neutral dispatch text fails.
*   **G6. Older binaries refuse new state before writing.** A registry or
    remotes file that needs this version carries a `beskar 2` header. 0.4.x
    stops on it with its existing error, `line 1: expected 'beskar 1' as the
    first record`, and never overwrites anything. Binaries from this release
    on print that the file was written by a newer Beskar. A file is upgraded
    only when it first needs a new record, and a plain `skills/NAME` in the
    library still works for 0.4.x.
*   **G7. Adapted collections are ordinary remotes.** A adapted repository
    uses the existing collection layout plus `agents/`, `upstreams/` and
    `adaptation/`. Subscribers track it with `beskar remote add` and never run
    a model.
*   **G8. A `beskar-adapt` skill produces adapted collections.** Run in the
    user's own agent, it imports an upstream, rewrites skills into portable
    form, creates agent definitions only where a skill needs them, runs a
    council review with a finding ledger and a round cap, records who
    reviewed, and opens a pull request for approval.
*   **G9. Deterministic adaptation steps are Beskar commands, not model work.**
    Storing originals with their upstream commit, writing and checking
    provenance, and running the fixed output checks are std-only `beskar`
    commands the skill calls. A model never computes a fingerprint.
*   **G10. Sync reports dirty adapted items.** `beskar remote sync` reports
    "upstream moved" per upstream, using one `git ls-remote` call each, and
    "hand-edited" per item by comparing fingerprints with provenance. It never
    changes deployed content, and an unreachable upstream shows as "unknown"
    without blocking the sync.
*   **G11. Remote target definitions come last, with approval.** A remote may
    define a new target. It is pinned by digest, never activated by sync, and
    writes only to output roots the user approved. Protected paths
    (`.git/`, `.github/workflows/`, Beskar's own files) are always refused.

**Success criterion:** track the upstream EGM collection through a adapted
repository, run `beskar repo enable` with all six targets enabled, and get
skills whose reviewer dispatch reaches a working reviewer agent in each
target (through the fallback for Codex).

## Non-Goals

*   **Install-time path substitution.** Beskar never rewrites
    `${CLAUDE_PLUGIN_ROOT}` or similar while installing. Adaptation rewrites
    such paths once, in the adapted repository.
*   **Beskar starting coding agents.** No headless runners, command-line
    templates, model listing, model-to-vendor map or remembered adapting agent
    choice. Adaptation runs inside the user's agent (brief, decision 14).
*   **Adaptation inside the Beskar library.** The library never holds adaptation
    state. Adapted content arrives through a remote (brief, decision 13), so
    ADR 0005's sync rules stay unchanged.
*   **Automatic re-adaptation or patching.** A dirty item is reported, and the
    maintainer re-runs adaptation. Nothing replays or patches quietly.
*   **Editing shared harness config files.** Beskar does not edit part of
    `opencode.json`, `config.toml` or similar. ADR 0003's whole-file ownership
    stays.
*   **User-global directories.** Only files inside a repository are written.
*   **Runner definitions from remotes.** Remote target definitions describe
    where files go and how they are rendered, never a program to run.
*   **Plugin manifests and MCP server configuration.**
*   **Third-party crates anywhere in v1.** Agent files are only written,
    never parsed (D2), so no crate is needed. Brief decision 11 is dropped
    for v1. A future need goes through a new ADR (D3).
*   **Translating tool access between targets.** `readonly` and `readwrite`
    are the only portable access levels. Anything finer is a native override.
    A loss that would widen access fails closed.

## Open Questions

No questions are open. Resolved during drafting (2026-11-08):

*   **Brief's stale headless goals:** decisions 13 and 14 apply.
*   **Round-1 reversals:** confirmed by approving G7, G8 and Alternative A3.
*   **Council defaults:** round cap 2; out-of-scope findings are recorded,
    not applied; the adapting agent breaks single-vendor ties and lists them
    in the PR.
*   **Version mechanism:** `beskar 2` header (D1).
*   **Native overrides:** whole-file replacement with a prompt hash (D2).
*   **Crate boundary:** std-only `beskar-targets` crate (D3).
*   **Terminology:** "curated" keeps its existing meaning (the user's chosen
    skills, `beskar-core/CONTEXT.md`, ADR 0005). The new process is
    **adaptation**, done by an **adapting agent**, producing an **adapted
    repository**. The brief calls this process "curation".
*   **Naming:** adapted items keep upstream names. A clash between upstreams
    fails `adapt check` and is resolved by a recorded `rename`.
*   **Maintainer detection:** none. Sync shows everyone the same message.
*   **Existing `.claude/skills` symlinks:** a grouped, confirmed migration in
    Stage 4, limited to links that resolve to the repository's own skill
    destination (Part F).

## Critical User Journeys (CUJs)

### CUJ 1: Maintainer adapts EGM

*   **Given**: A maintainer has `beskar` and a coding agent with the
    `beskar-adapt` skill installed, `gh` on the path, and an empty Git
    repository `skills-adapted`.
*   **When**: In that repository they ask their agent to adapt
    `https://github.com/google/elephant-goldfish-model`.
*   **Then**: The skill stores the upstream byte-for-byte under
    `upstreams/egm/`, writes portable skills under `skills/` and seven agent
    definitions under `agents/`, runs `beskar adapt check` until it passes,
    runs at most two council rounds, writes `adaptation/egm.bsk`, commits on a
    branch and opens a pull request with the council table. Nothing is merged
    without the maintainer.

### CUJ 2: Subscriber deploys adapted EGM to two targets

*   **Given**: A teammate on a newer Beskar has run
    `beskar remote add team <skills-adapted URL>` and tracks the EGM skills.
*   **When**: They run `beskar remote sync team`, then
    `beskar repo enable . egm-review --target claude-code --target opencode`.
*   **Then**: The library gains the skills and the agents those skills name.
    The repository gets the skills in `.agents/skills/` and one rendered agent
    file per agent per target. `beskar repo status` shows the agent files as
    owned. Running the review skill in either tool reaches a reviewer agent.

### CUJ 3: Upstream moves

*   **Given**: CUJ 2 is complete, and EGM has new commits past the one
    recorded in `adaptation/egm.bsk`.
*   **When**: Any subscriber runs `beskar remote sync team`.
*   **Then**: Sync reports `upstream moved: egm (recorded abc1234)`. Every
    subscriber sees the same line: `re-adaptation pending for upstream egm;
    maintainers run beskar adapt status in the adapted repository`.
    Deployed skills and agents do not change. If the upstream is unreachable,
    sync reports `unknown` and finishes normally.

### CUJ 4: An old binary meets new state

*   **Given**: A machine still runs Beskar 0.4.x and shares the library through
    Git with a machine that has enabled targets.
*   **When**: The 0.4.x binary runs any command on the newer registry, or
    `remote sync` on the newer remotes file.
*   **Then**: It stops before writing anything, with an error naming the file.
    It never overwrites agent records or adapted skills.

## Technical Plan (Architecture)

### Data flow

```text
 upstream (EGM, Git)                     maintainer's machine
 ───────────────────                     ────────────────────
        │  beskar adapt upstream add      coding agent + beskar-adapt skill
        ▼                                   │ rewrite, council, check
 ┌───────────────────────────────┐          │
 │ adapted repo (Git, ordinary   │◀─────────┘ commit + pull request
 │ collection layout)            │
 │  skills/  agents/  profiles/  │
 │  metadata/  upstreams/        │
 │  adaptation/<upstream>.bsk      │
 └──────────────┬────────────────┘
                │ beskar remote sync  (+ ls-remote per upstream → dirty report)
                ▼
 ┌───────────────────────────────┐
 │ LIBRARY                       │
 │  skills/  agents/  profiles/  │
 │  metadata/                    │
 └──────────────┬────────────────┘
                │ beskar repo enable / sync   (plan → transaction)
                ▼
 ┌───────────────────────────────────────────────────────────┐
 │ repository                                                │
 │  .agents/skills/NAME/            shared skill text        │
 │  .claude/agents/NAME.md          rendered, per target     │
 │  .opencode/agent/NAME.md         (paths unverified, B9)   │
 │  ...                                                      │
 └───────────────────────────────────────────────────────────┘
```

Subscribers only ever run the two lower arrows. The model runs only in the top
right box, on the maintainer's machine.

### Part A: Agent definitions (Stage 1)

**Library layout.** A new top-level `agents/` directory sits beside `skills/`,
`profiles/` and `metadata/`. 0.4.x ignores unknown top-level directories, so
the library stays readable for it (store.rs:181-182 only scans the three known
ones).

```text
LIBRARY/agents/NAME/
  PROMPT.md            the agent's instructions, shared by all targets
  agent.bsk            the record below
  native/<target>.<ext> optional whole-file override for one target (D2)
```

`agent.bsk` (a new bsk file type, documented in FORMAT.md):

```text
beskar 1
description "Reviews a design for unnecessary scope."
skill egm-review
skill egm-review-loop
access readonly
tier deep
```

*   `description` (required, once): every target's frontmatter needs one.
*   `skill NAME` (one or more): the skills this agent serves. An agent is
    desired in a repository when any skill it names is desired. An agent that
    names no skill is an error, so agents cannot be deployed on their own.
*   `access readonly|readwrite` (required, once). Each target maps these to its
    own tool list. A target that cannot express `readonly` refuses to render a
    `readonly` agent instead of widening it.
*   `tier fast|balanced|deep` (optional, default `balanced`). Each target maps
    a tier to a model value, or omits the field.
*   `override TARGET PROMPT-HASH` (zero or more): one line per file in
    `native/`. It records the `tree::text_fingerprint` of `PROMPT.md` that
    the override was written against (see D2).
*   Agent names follow `format::name`, the same rule as skill names.

**Collections.** `collection::read` gains optional reading of `agents/`. Each
agent must pass the same checks as in the library, and each `skill` it names
must exist in the collection. Tracking a skill brings along the agents that
name it. Agents are synced with the same three-way comparison as skills
(`reconcile::classify` and `decide`), recorded in remotes.bsk as
`synced-agent NAME UNIT`.

**Targets.** A target is a built-in, std-only description in the new
`beskar-targets` crate (D3):

| Field | Meaning |
| --- | --- |
| `name` | `claude-code`, `cursor`, `opencode`, `copilot-cli`, `copilot-vscode`, `codex` |
| `agent_dir` | Repository-relative directory for rendered agents |
| `file_name` | Pattern, for example `{name}.md` or `{name}.agent.md` |
| `renderer` | One of a fixed set: `markdown-frontmatter`, `toml`, `persona-only` |
| `access_map` | `readonly` and `readwrite` to native tool fields; absent means refuse |
| `tier_map` | Tier to native model value; absent means omit the field |

Codex uses `persona-only`. It writes the prompt with no frontmatter to a
literal path that the adapted skill names in its Codex dispatch line (G2). All
directory values in the built-in table are provisional until Stage 0 records
them in `docs/HARNESSES.md`.

Rendering only writes and never parses. Frontmatter and TOML values are quoted
with the existing JSON string encoder (beskar-cli/src/json.rs:106-122), moved
into `beskar-targets`; the CLI's JSON output imports it from there. A JSON-quoted string is a valid YAML 1.2 and TOML basic string;
Stage 0 checks that each target accepts it.

**Registry.** A repository gains enabled targets and installed agent files,
recorded per output path:

```text
beskar 2
repo /home/v/src/app
destination .agents/skills
profile egm-review
installed egm-review <hash>
target claude-code
target opencode
agent-file .claude/agents/egm-zoom-out.md egm-zoom-out <hash> claude-code
agent-file .github/agents/egm-zoom-out.agent.md egm-zoom-out <hash> copilot-cli copilot-vscode
end
```

Each output path is recorded once, with the targets that want it. When two
targets render the same path to the same bytes (Copilot CLI and VS Code, if
Stage 0 confirms they share `.github/agents/`), the path is shared and removed
only when no remaining target wants it. Different bytes at one path is a plan
error. The `beskar 2` header (Decision D1) keeps 0.4.x from misreading this file.

**Planning.** `reconcile::plan` grows a second loop beside the skill loop:

1.  Desired agents are those that name a desired skill.
2.  For each enabled target and desired agent, render the bytes in memory and
    compute the output path. This is the *library* fingerprint
    (`tree::text_fingerprint`).
3.  Names are desired output paths plus the recorded `agent-file` paths. Each
    one goes through the existing `classify` and `decide`, so local drift,
    conflicts and the `--conflict` policy behave exactly as for skills.
4.  The plan also fingerprints the enabled target set, each desired
    `agent.bsk`, `PROMPT.md` and override, and the target table revision, so
    the stale-plan check in `apply` catches changes to any of them.

Unmanaged detection for agents covers only files whose name matches the
target's `file_name` pattern in its `agent_dir`. A user's own agent files with
other names are left alone and never reported.

`tree::safe_path` already refuses symlinked components. If `.claude/` itself
is a symlink, the plan fails with an error that names the link and says how to
replace it. Automated replacement covers only the Stage 4 skill-directory case
(Part F).

**Transactions.** Agent files are written with `Transaction::text`. It
currently stages inside the target's parent, which fails when
`.claude/agents/` does not exist yet. It is changed to stage under
`reconcile::staging_parent` and to create the missing directory with
`ensure_directory` in the same transaction.

**CLI.** `repo enable` and `repo sync` take `--target NAME` (repeatable) and
`--no-target NAME`. Targets enabled for a repository persist in the registry.
`repo status` and `--json` output list agent files with their state. New
`library agent list|show|remove`. `doctor` gains a check for agents that name
missing skills.

### Part B: Registry and remotes versioning (Stage 1)

Released 0.4.x binaries cannot be taught a new message. Whatever we write,
0.4.x prints one of its existing errors. G6's "message that names a newer
Beskar" applies only to binaries from this release onward. The real choice
is which existing 0.4.x failure the new files trigger.

**Decision D1 (decided: header `beskar 2`).** bsk's parser returns the
version from the header instead of hard-coding 1 (bsk/src/lib.rs:92), and
each core file type states which versions it accepts. This amends ADR 0001.

*   `registry.bsk` is written as `beskar 2` the first time any repository
    enables a target, and as `beskar 1` while none has. `remotes.bsk` is
    written as `beskar 2` the first time it holds a `synced-agent` record.
    Other files (config, profiles, metadata, journal) stay `beskar 1`.
*   0.4.x stops on a `beskar 2` file with
    `<path>:line 1: expected 'beskar 1' as the first record` before touching
    anything. That includes `remote sync --conflict replace`, so 0.4.x cannot
    overwrite synced agents.
*   This and later binaries, given a header above what they accept, print
    `<path> was written by a newer Beskar (format N); upgrade Beskar`.
*   No downgrade path: once upgraded, a file stays `beskar 2`. Disabling every
    target does not rewrite it to version 1. The release notes and
    COLLECTIONS.md say that every machine sharing a registry must upgrade
    together.
*   Trade-off accepted: a 0.4.x machine that shares an upgraded registry can
    run no command until it upgrades.

### Part C: Adapted collection repository (Stage 3, provisional)

A adapted repository is an ordinary collection, so `beskar remote add` works
on it unchanged, plus three directories that 0.4.x ignores:

```text
skills/NAME/                  adapted skills (portable SKILL.md)
agents/NAME/                  agent definitions (Part A layout)
profiles/  metadata/          as today
upstreams/<upstream>/         byte-exact snapshot of the upstream tree
adaptation/<upstream>.bsk       provenance, one file per upstream
adaptation/notes/<item>.md      maintainer intent the skill must keep
LICENSE, NOTICE               kept from each upstream (EGM is Apache-2.0)
```

**Adaptation unit.** The unit is one upstream. B2 showed that EGM's personas
and standards are shared across skills, so adapting one skill at a time would
duplicate agents. One provenance file per upstream also matches how upstreams
move: `ls-remote` reports per upstream, not per skill.

**Provenance format** (`adaptation/<upstream>.bsk`, documented in FORMAT.md and
COLLECTIONS.md):

```text
beskar 1
upstream https://github.com/google/elephant-goldfish-model
ref main
commit 4f2c...            commit the originals were taken from
originals <hash>          tree::fingerprint of upstreams/<upstream>/
prompt-revision 1         revision of the beskar-adapt skill's instructions
adapted-by claude-code claude-opus-5-5
reviewer fidelity opencode gpt-6-astra
reviewer portability opencode gemini-4-argon
reviewer minimal-change claude-code claude-opus-5-5 same-model
rounds 2
rename skill review egm-review   only when a clash forced a rename
output skill egm-review <hash>
output agent egm-zoom-out <hash>
approved-by "Vinzenz Feenstra" 2026-11-08
```

Provenance is written by `beskar adapt record`, never by the model.
`rename KIND UPSTREAM-NAME LOCAL-NAME` records a rename forced by a name clash
between upstreams. Adapted items otherwise keep their upstream names. The
adapting agent reads existing `rename` lines and reuses them on re-adaptation,
so a rename never changes once deployed.
Reviewer lines record what actually ran, including same-model fallbacks.

**Deterministic helpers (G9).** New `beskar adapt` commands. They work on a
adapted repository directory given with `--dir`, and never touch the library
or registry, so they need no library locks:

| Command | Effect |
| --- | --- |
| `adapt init` | Creates the layout above in an empty Git repository. |
| `adapt upstream add NAME URL [--ref R]` | Clones byte-faithfully (the `core.autocrlf=false` clone path from a8a9f26) into `upstreams/NAME/` and records url, ref, commit and originals fingerprint. |
| `adapt upstream update NAME` | Replaces the snapshot with the current upstream and reports which original files changed. |
| `adapt check` | Fixed checks, listed below. Exits 1 with a list of failures. |
| `adapt record NAME` | Writes or rewrites `adaptation/NAME.bsk` from the current tree plus reviewer and adapting agent facts passed as flags. |
| `adapt status` | The dirty report for this directory: the same code `remote sync` uses. |

`adapt check` runs these fixed checks:

*   No `${CLAUDE_PLUGIN_ROOT}` or other plugin-root variable in any adapted
    file.
*   Each agent named in a skill's dispatch text exists under `agents/`, and
    each agent names that skill.
*   No agent is `readwrite` where the upstream adapter declared read-only
    tools. A widening is an error until the maintainer records it in a notes
    file.
*   The layout passes `collection::read`.
*   No two upstreams contribute a skill or agent with the same local name.
    The failure names both upstreams and suggests a `<upstream>-<name>`
    rename.
*   LICENSE and NOTICE exist for each upstream.
*   No symlinks, no executable bits that were not in the original.

### Part D: The `beskar-adapt` skill (Stage 5, provisional)

Shipped like `beskar-completions`: in `skills/beskar-adapt/` and in release
archives, not embedded in the binary. It runs in the maintainer's agent and
tells the agent to:

1.  Run `beskar adapt upstream add` or `update`.
2.  Read the originals and any `adaptation/notes/`. Rewrite each skill into
    one neutral `SKILL.md` whose dispatch section has one line per target
    (EGM's `docs/harnesses.md` table is the model). Create an agent definition
    only where a skill dispatches to a named agent.
3.  Run `beskar adapt check` until it passes.
4.  Run the council: up to three reviewer agents, read-only, each with a
    fixed lens (fidelity to upstream, portability, minimal change), each
    writing a verdict (`approve` or `changes`) and findings (id, severity,
    file, in-scope flag, text) to a file. The adapting agent marks each finding
    `applied`, `rejected` with a reason, or `out-of-scope`. Stop when all
    approve, or a round brings no new in-scope Blocker or Major, or after
    round 2.
5.  Run `beskar adapt record`, commit on a branch `adapt/<upstream>-<date>`,
    and open a pull request with `gh`. The PR body contains the council table
    and every rejected or out-of-scope finding. Without `gh`, leave the branch
    and print the push command.

The skill never merges. Approval is the PR review.

### Part E: Dirty report in `remote sync` (Stage 3, provisional)

After a remote's normal sync, if its collection has `adaptation/*.bsk`:

1.  **Hand edits.** Compare each `output` fingerprint with the collection's
    current item fingerprint. A mismatch is `hand-edited`.
2.  **Upstream moved.** For each upstream, run `git ls-remote -- URL REF` once
    (new function in git.rs). A different commit is `upstream-moved`; a
    failure is `unknown`. The URL comes from a remote's file, so it is
    untrusted input. Before calling git, check it: only `https://`, `ssh://`,
    `user@host:path` and absolute `file://` URLs are allowed, and a URL
    starting with `-` is refused. Git runs with `-c protocol.allow=never`
    plus `always` for exactly those four, `GIT_PROTOCOL_FROM_USER=0` and
    `GIT_TERMINAL_PROMPT=0`, and the URL follows `--`. `ext::` and every
    other transport are blocked. Allowing `file://` means a hostile
    provenance file can make sync run `ls-remote` on a local path. The only
    output is a commit hash shown on that machine, and tests need local
    upstreams.
3.  **Report.** Human output adds one line per dirty upstream or item. JSON
    output adds `data.adaptation: [{upstream, state, recorded, current,
    items}]`. Exit status does not change. `--no-upstream-check` skips step 2.

Nothing in this path changes deployed content or remote state. Every reader gets the same wording. Beskar does not try to detect who
maintains the adapted repository.

### Part F: Per-target skill variants and local target definitions (Stage 4, only if Stage 0 requires)

If Stage 0 shows one neutral `SKILL.md` cannot dispatch in some target:

*   `skills/NAME/variants/<target>/SKILL.md` replaces the whole `SKILL.md` for
    that target only. The target's table entry gains a `skill_dir`, and the
    registry records `skill-file PATH NAME HASH TARGET...` the same way as
    agent files.
*   **Existing skill-directory symlinks.** When a target's `skill_dir` is a
    symlink that resolves to the repository's own skill destination (for
    example `.claude/skills -> ../.agents/skills`), the plan includes one
    grouped change: "replace symlink `.claude/skills` with a Beskar-managed
    directory". It runs inside the same transaction as the variant writes.
    The journal records `unlink LINK TARGET`, so rollback and recovery
    recreate the link exactly. This change is never approved by `--yes`.
    Interactive runs ask once. JSON and non-interactive runs require
    `--replace-link PATH` naming that exact link, and otherwise fail with an
    error that names the flag. Any other symlink (pointing elsewhere, or a
    symlinked parent such as `.claude/`) is refused, and the error explains
    the manual fix. On Windows, where recreating a symlink may need
    privileges, the migration is refused and the error explains the manual
    fix.
*   Planning refuses when a target reads both `.agents/skills/` and its own
    skill directory and both would hold the same name (duplicate discovery).

Local target definitions: `HOME/targets/NAME.bsk` with the same fields as the
built-in table, limited to the fixed renderers. A local definition can add a
target or shadow a built-in one; `target list` shows both and which is active.

### Part G: Remote target definitions (Stage 6, provisional)

A collection may ship `targets/NAME.bsk` in the same format as local
definitions. Sync never activates one. `beskar target approve REMOTE NAME
--digest SHA` copies it into `HOME/targets/` with its source and digest, and
asks the user to approve each output root it requests. Roots under `.git/`,
`.github/workflows/` or any Beskar file are refused. A definition with any
field outside the target schema (for example a command) is rejected whole.

### Rendering decisions (apply to Part A)

**Decision D2 (decided: an override replaces the whole rendered file).**
`agents/NAME/native/<target>.<ext>`, where `<ext>` is the extension from the
target's `file_name`, is copied byte-for-byte as that target's output. The
renderer is skipped for that target. No parser is needed anywhere.

*   The override's bytes are the library fingerprint for that output path, so
    drift and conflicts work exactly as for rendered files.
*   Stale overrides are detected rather than prevented. When `PROMPT.md`'s
    current fingerprint differs from the `override` line's recorded hash,
    `adapt check` fails and `beskar doctor` warns, naming the agent and
    target. The maintainer edits the override and updates the hash with
    `beskar library agent override-ack NAME TARGET`.
*   An override file with no matching `override` line, or the reverse, is an
    error when `agent.bsk` is read.
*   An override bypasses the `access_map` refusal, and Beskar cannot check its
    tool list without parsing it. So `adapt check` lists every override on a
    `readonly` agent as a manual access-review item. The `beskar-adapt` skill
    puts these at the top of the pull request, and `repo status` marks the
    rendered file `override`.

**Decision D3 (decided: a new std-only crate `beskar-targets`).**

*   Dependencies point one way: `beskar-cli → beskar-core → beskar-targets →
    bsk`. The CLI also depends on `beskar-targets` directly for the JSON
    string encoder and `target list`.
*   `beskar-targets` holds the built-in target table, the `persona-only`,
    `markdown-frontmatter` and `toml` renderers, the access and tier maps,
    and (Stage 4) reading local target definitions from bsk.
*   Its interface is pure functions: given an agent (`description`, `access`,
    `tier`, prompt bytes, optional override bytes) and a target, return
    `Result<(relative_path, bytes), RenderError>`. It does no filesystem
    writes and no I/O apart from reading definition files it is passed.
    Core calls it during planning and fingerprints the result, so core still
    owns plan bytes (ADR 0002).
*   It is std-only with `unsafe_code = forbid`, like the other crates. Brief
    decision 12 (separate crate) is kept; decision 11 (third-party format
    crates) is dropped for v1. If a crate is ever needed, it goes through a
    new ADR, and the dependency is inverted (core defines a trait, an outer
    crate implements it) so core stays std-only.
*   Rejected: depending on third-party crates in this crate (option c). Core
    would link them transitively, breaking the Non-Goal, or the CLI would
    hand bytes to core, breaking ADR 0002 (brief B4).

## Alternatives Considered

### Where adaptation happens

#### Alternative A1: Beskar runs headless adaptation in its own library

Beskar starts a headless coding agent chosen by the user, rewrites skills in
place in the library, and remembers the adapting agent choice (brief, round 2,
decision 6).

*   **Pros**: One tool does everything. A user never has to run a separate
    repository.
*   **Cons**: A adapted library copy never matches upstream, so ADR 0005 sync
    marks it locally changed and `--conflict replace` overwrites it unread
    (B1). Fixing that needs a second sync state machine. Beskar would also
    need runner definitions, headless flags, model listing, a vendor map and
    a headless security model (B3). Every subscriber pays to adapt.
*   **Reason for Rejection**: Rejected by author (decision 13): the publisher
    model removes B1 and makes one adaptation serve every subscriber.

#### Alternative A2: Beskar runs headless adaptation, but writes a publisher repository

Keep the publisher layout, but have a `beskar adapt run` command start the
headless adapting agent and council.

*   **Pros**: Repeatable, scriptable runs, and Beskar records exactly which
    process ran.
*   **Cons**: Keeps all the runner and isolation work from A1 (B3, command
    templates, process handling with timeouts), and its facts are unverified
    for every tool.
*   **Reason for Rejection**: Rejected by author (decision 14): adaptation runs
    as a skill inside the maintainer's own agent and sandbox. Beskar starts no
    coding agents.

#### Alternative A3: Require maintainers to publish in Beskar format

*   **Pros**: No model in the loop at all. Beskar only installs.
*   **Cons**: EGM and similar collections would stay unusable until their
    maintainers adopt Beskar's layout.
*   **Reason for Rejection**: Superseded by author in round 2: Beskar's
    tooling converts upstream content itself (the reversal was confirmed
    during drafting).

#### Alternative A4: Plugin-root path substitution at install time

Replace `${CLAUDE_PLUGIN_ROOT}` with a real path while copying a skill.

*   **Pros**: Upstream text works with no rewrite step.
*   **Cons**: Installed bytes then differ per repository, which breaks the
    library-to-install fingerprint comparison. Every new plugin variable
    needs new code.
*   **Reason for Rejection**: Rejected by author: it leads to a bad design and
    hacky code. Adaptation rewrites such paths once instead.

### Agent format and rendering

#### Alternative B1: Eight-capability neutral tool vocabulary

Describe tool access as a list such as `read, search, edit, shell, web` and
map it to each target.

*   **Pros**: Finer control than `readonly` and `readwrite`.
*   **Cons**: A mapped capability is not an equivalent one. `read, search`
    cannot express OpenCode's rule that denies grep on `.env`.
*   **Reason for Rejection**: Rejected after the round-1 panel. Only
    `readonly` and `readwrite` are portable; anything finer is a native
    override.

#### Alternative B2: Full data-only translation engine

*   **Pros**: New targets with no code at all.
*   **Cons**: It becomes a template language with its own bugs, and it still
    cannot express target-specific semantics.
*   **Reason for Rejection**: Rejected by the round-1 review and panel.

#### Alternative B3: Verbatim native files only

Every agent ships one hand-written file per target, and Beskar only places them.

*   **Pros**: No rendering code. Each file is exactly what the target expects.
*   **Cons**: One prompt is maintained up to six times and drifts.
*   **Reason for Rejection**: Rejected by author for v1 in favour of a shared
    prompt plus optional whole-file overrides. D2 keeps verbatim files
    available as an override.

#### Alternative B4: Overrides that replace only the frontmatter (D2 option b)

*   **Pros**: A prompt change reaches every target automatically. No parser.
*   **Cons**: Does not fit targets that store the prompt inside a field (such
    as TOML), or that need a different prompt body.
*   **Reason for Rejection**: Rejected by author in favour of whole-file
    replacement, with a recorded prompt hash to detect stale overrides.

#### Alternative B5: Field-by-field override merge (D2 option c)

*   **Pros**: Overrides stay small and change only what differs.
*   **Cons**: Needs YAML and TOML parsers that match each target's real parser.
*   **Reason for Rejection**: Rejected by author: it forces third-party crates
    or hand-written partial parsers.

#### Alternative B6: Silently dropping fields a target cannot express

*   **Pros**: Rendering never fails.
*   **Cons**: A dropped restriction silently widens what an agent can do.
*   **Reason for Rejection**: Rejected by author: warn and let the user
    decide. A loss that widens access fails closed.

### Code layout and dependencies

#### Alternative C1: Harness crate allowed third-party crates (D3 option c)

*   **Pros**: Format crates available for any future need. Matches brief
    decisions 11 and 12 as written.
*   **Cons**: Core plans rendered bytes, so it would either link those crates
    transitively (breaking std-only core) or receive bytes from the CLI
    (breaking ADR 0002, brief B4). No current renderer needs a crate.
*   **Reason for Rejection**: Rejected by author after the conflict with the
    Non-Goal and ADR 0002 was raised. A future need goes through a new ADR
    with the dependency inverted.

#### Alternative C2: Rendering as a module inside `beskar-core` (D3 option a)

*   **Pros**: No new crate, and the simplest wiring.
*   **Cons**: No seam if rendering ever needs to move out.
*   **Reason for Rejection**: Rejected by author in favour of a std-only
    `beskar-targets` crate.

#### Alternative C3: Compiled-in target table only

*   **Pros**: Simplest. No definition format to design or validate.
*   **Cons**: A new target needs a new release.
*   **Reason for Rejection**: Rejected by author: it drops the no-recompile
    goal. Local definitions (Stage 4) and remote ones (Stage 6) keep it.

#### Alternative C4: Built-in and local target definitions only

*   **Pros**: No trust model for remote definitions.
*   **Cons**: Teams cannot share a definition for an in-house tool.
*   **Reason for Rejection**: Rejected by author: remote target definitions
    stay, as the last stage, with approval of output roots. Remote definitions
    choosing their own output paths were rejected by author and panel.

### Registry and compatibility

#### Alternative D1: Keep `beskar 1`, add a `schema 2` record (D1 option b)

*   **Pros**: No change to bsk or ADR 0001.
*   **Cons**: 0.4.x prints `line 2: expected 'repo <path>'`, which gives no
    hint that an upgrade is needed. It adds a second version concept.
*   **Reason for Rejection**: Rejected by author in favour of the header.

#### Alternative D2: Let new records stop 0.4.x, with no version change (D1 option c)

*   **Pros**: No version machinery. Files without new records stay usable by
    both versions.
*   **Cons**: Nothing protects a future change to what an existing record
    means. The 0.4.x error is the least clear.
*   **Reason for Rejection**: Rejected by author in favour of the header.

#### Alternative D3: A sibling state file next to `registry.bsk`

*   **Pros**: 0.4.x keeps working on the registry untouched.
*   **Cons**: 0.4.x could remove a skill and leave its agent files orphaned,
    because it never reads the sibling file.
*   **Reason for Rejection**: Rejected by author.

#### Alternative D4: Editing shared harness config files

Write agent entries into `opencode.json` or `config.toml`, or own an included
file.

*   **Pros**: Reaches targets that only accept agents in config files.
*   **Cons**: Beskar would own part of a file, which ADR 0003's whole-file
    ownership and fingerprints cannot express.
*   **Reason for Rejection**: Rejected by the round-1 panel: ADR 0003 stays
    strict.

### Targets and skill text

#### Alternative E1: Per-target skill variants from the start

Every enabled target gets its own skill directory with its own `SKILL.md`.

*   **Pros**: Each target gets dispatch text written for it.
*   **Cons**: Duplicate discovery when a target reads both `.agents/skills/`
    and its own directory. Several copies of each skill to review. EGM
    already dispatches from one text with a lookup table.
*   **Reason for Rejection**: Rejected by author (G5): one neutral `SKILL.md`
    is the default; variants come only where Stage 0 shows they are needed.

#### Alternative E2: Copilot as one target

*   **Pros**: One fewer target to maintain.
*   **Cons**: Copilot CLI and Copilot in VS Code are configured and tested
    differently, and VS Code has no headless mode.
*   **Reason for Rejection**: Rejected by author: they are separate targets
    that may share an output path.

#### Alternative E3: Skill author owns dispatch; Beskar only places files

*   **Pros**: Beskar never touches skill text.
*   **Cons**: Upstream collections written for one plugin system stay
    non-portable.
*   **Reason for Rejection**: Rejected by author. Adaptation rewrites dispatch.

### Review and approval

#### Alternative F1: User diff approval only, no council

*   **Pros**: Far cheaper. The brief estimated 4–12 million tokens per
    collection for a three-reviewer, three-round council.
*   **Cons**: A large rewrite gets one human pass, and approval fatigue sets
    in.
*   **Reason for Rejection**: Rejected by author (G8): the council runs, with
    a round cap of 2 and a scope test, and the pull request is the human gate.

--------------------------------------------------------------------------------

## Non-Functional Requirements

*   **Security.**
    *   Rendered agent files load automatically at session start, so an
        access widening is the main risk. `readonly` never renders as
        anything wider, and a target that cannot express it refuses (Part A).
        Overrides on `readonly` agents are flagged for manual review (D2).
    *   Provenance URLs are untrusted. They go through the URL and protocol
        allowlist in Part E.
    *   The `beskar-adapt` skill runs with whatever sandbox the maintainer's
        agent has. Beskar adds no isolation of its own. The skill tells the
        agent to work only inside the adapted repository and never to run
        scripts from `upstreams/`.
*   **Performance.**
    *   Rendering is in-memory string building, done once per agent per
        target per plan.
    *   `remote sync` adds one `ls-remote` per upstream. A slow upstream
        delays sync, so `--no-upstream-check` skips the step. A timeout is a
        future improvement, because `git.rs::run` has none today.
    *   Council cost falls on the maintainer: at most 3 reviewers times 2
        rounds per adaptation.
*   **Rollout and rollback.**
    *   With no target enabled and no agents synced, every file stays
        `beskar 1` and 0.4.x keeps working.
    *   The first `--target` or `synced-agent` upgrades that file to
        `beskar 2`. There is no automatic downgrade. Rolling back to 0.4.x
        means restoring the file from before the upgrade. The release notes
        say so.
    *   Each stage ships in its own release. Stages 4 and 6 can be skipped.

--------------------------------------------------------------------------------

### Additional Resources

*   `concepts_brief.md` (round 2) and `.panel/synthesis.md` (round 1 panel).
*   ADRs 0001 to 0005 in `docs/adr/`.
*   EGM 0.1.1 `docs/harnesses.md` (delegation table), `skills/*/test_scenarios.yaml`.
*   t3dotgg/tstack council skill, as read on 2026-11-08.

### Future Improvements

*   **Timeouts for git calls.** Add a timeout to `git.rs::run`, so a hung
    `ls-remote` cannot stall sync.
*   **Per-item upstream precision.** Run a shallow fetch to tell whether the
    files an item came from changed, not only that the upstream commit moved.
*   **Three-way replay on re-adaptation.** Use `git merge-file` over old
    original, new original and adapted copy, so small upstream changes need
    no model.
*   **Remote collections shipping overrides for targets Beskar does not know
    yet.**

--------------------------------------------------------------------------------

## Detailed Implementation Plan

Stages ship in order. Each stage leaves `cargo test --workspace --offline`
green and is releasable alone.

### Stage 0: Conformance spike (no Rust code)

#### 1. `docs/HARNESSES.md` (Create)

*   **Type / Scope**: Reference documentation.
*   **Proposed Changes**: One section per target (Claude Code, Cursor,
    OpenCode, Copilot CLI, Copilot VS Code, Codex). Each records, with the
    vendor documentation URL and the date checked:
    *   every skill directory the target reads, and whether it reads
        `.agents/skills/` alongside its own;
    *   its agent directory, file name pattern and accepted frontmatter
        fields;
    *   whether JSON-quoted YAML values are accepted;
    *   how a skill dispatches to a named agent;
    *   how tool access maps to `readonly` and `readwrite`;
    *   how model tiers map to model values.

    It ends with the results of the hand-converted EGM loop (skill → agent →
    result) in each target, plus a manual checklist for Copilot VS Code. The
    values here become the built-in target table in Stages 1 and 2. If a
    target cannot dispatch from neutral text, that triggers Stage 4.

The hand-converted EGM itself lives in a scratch Git repository outside this
one. It is an Apache-2.0 derivative and is not committed here.

### Stage 1: Agent definitions, two targets, format version 2

#### 2. `Cargo.toml` (Modify)

*   **Type / Scope**: Workspace manifest.
*   **Proposed Changes**: Add `crates/beskar-targets` to `members`.

#### 3. `Cargo.lock` (Modify)

*   **Type / Scope**: Lock file.
*   **Proposed Changes**: Gains the local `beskar-targets` package. No
    registry packages are added. CI's `--offline --locked` builds keep
    passing.

#### 4. `crates/beskar-targets/Cargo.toml` (Create)

*   **Type / Scope**: Crate manifest.
*   **Proposed Changes**: `beskar-targets`, workspace version, edition and
    lints (`unsafe_code = forbid`). Depends only on `bsk` (path dependency).

#### 5. `crates/beskar-targets/src/lib.rs` (Create)

*   **Type / Scope**: Library root.
*   **Proposed Changes**: Public API (pseudocode):

    ```text
    enum Access { ReadOnly, ReadWrite }
    enum Tier { Fast, Balanced, Deep }
    enum Renderer { PersonaOnly, MarkdownFrontmatter, Toml }
    struct Target {
        name, agent_dir, file_name, extension, renderer,
        access_map: { ReadOnly -> Option<Vec<(key, value)>>, ReadWrite -> ... },
        tier_map:   { Tier -> Option<String> },
        revision: u32,
    }
    struct AgentInput<'a> { name, description, access, tier, prompt: &'a [u8],
                            override_bytes: Option<&'a [u8]> }
    fn builtin() -> &'static [Target]
    fn find(name) -> Option<&'static Target>
    fn render(target, agent) -> Result<Rendered { relative_path, bytes }, RenderError>
    enum RenderError { AccessNotExpressible { target, access }, InvalidName, ... }
    ```

    `render` uses the override bytes when they are present (D2). Otherwise
    it runs the target's renderer. `revision` changes whenever a built-in
    entry changes, and plans fingerprint it.

#### 6. `crates/beskar-targets/src/builtin.rs` (Create)

*   **Type / Scope**: Data.
*   **Proposed Changes**: The built-in table. Stage 1 fills `claude-code` and
    one of `opencode` or `copilot-cli`, whichever Stage 0 shows to be most
    different from Claude Code, with values copied from `docs/HARNESSES.md`.
*   Each entry carries a comment linking its HARNESSES.md section.

#### 7. `crates/beskar-targets/src/render.rs` (Create)

*   **Type / Scope**: Renderers.
*   **Proposed Changes**: Three renderers. Values are quoted with `quote::json`.
    *   `markdown_frontmatter`: a `---` block holding `name`, `description`
        and the mapped access and tier fields, in a fixed order, then a blank
        line and the prompt.
    *   `toml`: `key = "value"` lines, with the prompt as a TOML basic string.
    *   `persona_only`: the prompt bytes unchanged.

    Output always ends in exactly one `\n`, so the same input always gives
    the same bytes.

#### 8. `crates/beskar-targets/src/quote.rs` (Create)

*   **Type / Scope**: Utility.
*   **Proposed Changes**: Takes the JSON string encoder from
    `beskar-cli/src/json.rs:106-122` unchanged as `pub fn json(out: &mut
    String, value: &str)`. Unit tests check that its output is valid as a
    YAML 1.2 double-quoted scalar and a TOML basic string for control
    characters, quotes, backslashes and non-ASCII text, with hand-written
    expected strings and no parser.

#### 9. `crates/beskar-targets/CONTEXT.md` (Create)

*   **Type / Scope**: Domain doc.
*   **Proposed Changes**: Terms **Target**, **Rendered agent file**,
    **Renderer**, **Access level**, **Tier**, **Override**, each with an
    *Avoid* line (for example, avoid "harness" for a target).

#### 10. `CONTEXT-MAP.md` (Modify)

*   **Type / Scope**: Domain doc.
*   **Proposed Changes**: Add the `beskar-targets` line, and note that core
    calls it to render agent files.

#### 11. `crates/bsk/src/lib.rs` (Modify)

*   **Type / Scope**: Parser.
*   **Proposed Changes**:
    *   Replace the hard-coded check at line 92 with a version read from the
        header: `pub struct Document { pub version: u32, pub records:
        Vec<Record> }`, with `parse_document(text) -> Result<Document>` and
        `read_document(path)`.
    *   Keep `parse` and `read` as wrappers that accept only version 1, so
        callers that don't care stay unchanged.
    *   Add `parse_accepting(text, max: u32)`. A header above `max` fails with
        `line 1: written by a newer Beskar (format N); upgrade Beskar`. A
        malformed header keeps today's message.
    *   Unit tests cover versions 1 and 2, a version above max, and a missing
        header.

#### 12. `crates/bsk/CONTEXT.md` (Modify)

*   **Type / Scope**: Domain doc.
*   **Proposed Changes**: Define **Format version** (the header's N), and say
    that each file type chooses which versions it accepts.

#### 13. `docs/adr/0006-agent-definitions-and-targets.md` (Create)

*   **Type / Scope**: ADR.
*   **Proposed Changes**: Records agents as library items (`agents/NAME/`),
    agents following skills, the std-only `beskar-targets` crate and its
    dependency direction, whole-file overrides with a prompt hash (D2), the
    rule that rendering never parses, and the decision against third-party
    crates in v1.

#### 14. `docs/adr/0007-format-version-2-and-path-ownership.md` (Create)

*   **Type / Scope**: ADR.
*   **Proposed Changes**: Records D1: `beskar 2` for registry and remotes,
    upgrade on first need, no downgrade, the exact 0.4.x message. Also
    records path-keyed ownership with shared paths (`agent-file`). Amends
    0001 and 0003. Stage 4 adds a section to this ADR for the one permitted
    symlink operation (replacing a skill-directory link that resolves to the
    repository's own destination, journaled and reversible).

#### 15. `docs/adr/0001-versioned-record-format.md` and `docs/adr/0003-explicit-ownership-and-complete-fingerprints.md` (Modify)

*   **Type / Scope**: ADR.
*   **Proposed Changes**: Add an "Amended by ADR 0007" line at the top. The
    body stays unchanged.

#### 16. `crates/beskar-core/Cargo.toml` (Modify)

*   **Type / Scope**: Crate manifest.
*   **Proposed Changes**: Add a path dependency on `beskar-targets`.

#### 17. `crates/beskar-core/src/model.rs` (Modify)

*   **Type / Scope**: Domain model.
*   **Proposed Changes**:
    *   **`Repository`** gains:
        *   `targets: BTreeSet<String>`;
        *   `agent_files: BTreeMap<String /*relative path*/, AgentFile { agent,
            hash, targets: BTreeSet<String> }>`.
    *   **`Registry::decode`** uses `parse_accepting(.., 2)` and accepts
        `target NAME` and `agent-file PATH AGENT HASH TARGET...` only when
        the version is 2. On version 1 these records give the existing
        unknown-record error.
    *   **`Registry::encode`** writes `beskar 2` when any repository has a
        target or agent file, and `beskar 1` otherwise. Once a file has been
        read as version 2, it is never written back as 1 (a `version` field
        is carried through).
    *   **`Remote`** gains `synced_agents: BTreeMap<String, String>`.
        `RemoteBook` decode and encode handle `synced-agent NAME UNIT` with
        the same version rule.
    *   **New `AgentDefinition { description, skills: BTreeSet, access, tier,
        overrides: BTreeMap<target, prompt_hash> }`** with `decode` and
        `encode` for `agent.bsk`. Unknown records are rejected, at least one
        `skill` is required, `description` and `access` are required exactly
        once, and `tier` and each `override` target may appear at most once.

#### 18. `crates/beskar-core/src/store.rs` (Modify)

*   **Type / Scope**: Library store.
*   **Proposed Changes**:
    *   `agents_dir()`;
    *   `agent_names()`;
    *   `read_agent(name) -> Result<LoadedAgent { definition, prompt: Vec<u8>,
        overrides: BTreeMap<target, Vec<u8>> }>`, which checks that the
        `native/` files match the `override` lines one to one;
    *   `desired_agents(desired_skills) -> BTreeSet<String>`;
    *   `remove_agent(name)` inside a transaction.

    `library remove SKILL` reports agents that would be left naming no
    skill, and removes them in the same transaction.

#### 19. `crates/beskar-core/src/reconcile.rs` (Modify)

*   **Type / Scope**: Planning.
*   **Proposed Changes**:
    *   New `AgentFilePlan { path, agent, targets, action, state, library_hash,
        current_hash }` and `Plan.agent_files: Vec<AgentFilePlan>`.
    *   `plan()` adds the agent loop from Technical Plan Part A. Rendering
        errors (such as `AccessNotExpressible`) are plan errors that name the
        agent and target. Two targets producing different bytes for one path
        is a plan error.
    *   Unmanaged agent files: for each enabled target, the files in its
        `agent_dir` that match its file pattern and are not recorded. They go
        into a new `Plan.unmanaged_agents` and classify as `State::Unmanaged`
        when a desired path collides with one.
    *   Fingerprints add the enabled target set, each desired agent's
        `agent.bsk`, `PROMPT.md` and override bytes, and each target's
        `revision`. `apply()` re-plans and compares `agent_files` and these
        fingerprints, like skills today.

#### 20. `crates/beskar-core/src/transaction.rs` (Modify)

*   **Type / Scope**: Transactions.
*   **Proposed Changes**: `text()` and `prepare()` take a staging parent from
    `reconcile::staging_parent` instead of `target.parent()`. Missing parent
    directories are created with `ensure_directory` and journaled as `mkdir`,
    so rollback removes them. The journal stays `beskar 1`, because its
    records are unchanged.

#### 21. `crates/beskar-core/src/transaction_tests.rs` (Modify)

*   **Type / Scope**: Unit tests.
*   **Proposed Changes**:
    *   `text()` into a missing `.claude/agents/` commits, and rollback
        removes the directory it created.
    *   Recovery after a crash between `mkdir` and move.

#### 22. `crates/beskar-core/src/collection.rs` (Modify)

*   **Type / Scope**: Remote collection reader.
*   **Proposed Changes**:
    *   `Collection` gains `agents: BTreeMap<String, CollectionAgent { dir,
        definition, unit_hash }>`. `agents/` is optional. Each agent must
        name only skills that exist in the collection.
    *   `upstreams/`, `adaptation/` and `targets/` are ignored here: Stage 3
        and Stage 6 read them separately.

#### 23. `crates/beskar-core/src/remote.rs` (Modify)

*   **Type / Scope**: Remote sync.
*   **Proposed Changes**:
    *   The tracked agents are those naming any tracked skill. They go
        through the same `classify` and `decide_remote` as skills, with
        `synced-agent` as the baseline.
    *   An agent removed upstream, or no longer naming a tracked skill, is
        planned as a removal, with the same conflict rules as a removed skill.
    *   `RemotePlan` gains `agents: Vec<RemoteItemPlan>` with `Kind::Agent`.

#### 24. `crates/beskar-core/src/app.rs` (Modify)

*   **Type / Scope**: Operations layer.
*   **Proposed Changes**: New operations:
    *   `enable_targets(path, add, remove)`, used by `repo enable` and
        `repo sync`;
    *   `agent_list()`;
    *   `agent_show(name)`;
    *   `agent_remove(name)`;
    *   `agent_override_ack(name, target)`, which rewrites the `override`
        line to the current `PROMPT.md` hash;
    *   `target_list()`.

    Existing `status` and `doctor` include agent files. `doctor` adds three
    checks: agents naming missing skills, stale overrides, and orphaned
    `native/` files.

#### 25. `crates/beskar-core/src/diff.rs` (Modify)

*   **Type / Scope**: Diff.
*   **Proposed Changes**: Add `compare_file(expected: &[u8], path)`, so
    `repo diff` can show drift in a rendered agent file against freshly
    rendered bytes. It reuses `unified` and the same 1 MiB and `MAX_CELLS`
    limits.

#### 26. `crates/beskar-core/src/lib.rs` (Modify)

*   **Type / Scope**: Exports.
*   **Proposed Changes**: Export `AgentDefinition`, `AgentFilePlan` and
    `Kind::Agent`, and re-export `beskar_targets::{Target, Access, Tier}`.

#### 27. `crates/beskar-core/CONTEXT.md` (Modify)

*   **Type / Scope**: Domain doc.
*   **Proposed Changes**: Add **Agent definition**, **Enabled target** and
    **Agent file** (installed, owned per path). Update **Registry** to
    mention agent files. Add **Adaptation** (rewriting
    an upstream collection into an adapted repository) with *Avoid: curation*,
    since **curated** keeps its existing meaning of the user's chosen skills.

#### 28. `crates/beskar-cli/Cargo.toml` (Modify)

*   **Type / Scope**: Crate manifest.
*   **Proposed Changes**: Add a path dependency on `beskar-targets`.

#### 29. `crates/beskar-cli/src/json.rs` (Modify)

*   **Type / Scope**: JSON output.
*   **Proposed Changes**: Remove `write_string` and call
    `beskar_targets::quote::json`. Output bytes stay unchanged; the existing
    JSON tests prove it.

#### 30. `crates/beskar-cli/src/args.rs` (Modify)

*   **Type / Scope**: Argument parsing.
*   **Proposed Changes**: `--target NAME` and `--no-target NAME`, each
    repeatable, on `repo enable` and `repo sync`. Each name is checked
    against `beskar_targets::find`, and an unknown name is a usage error
    that lists the valid ones.

#### 31. `crates/beskar-cli/src/spec.rs` (Modify)

*   **Type / Scope**: Command table.
*   **Proposed Changes**: Add `library agent list`, `library agent show
    NAME`, `library agent remove NAME`, `library agent override-ack NAME
    TARGET` and `target list` to `COMMANDS`, with help text.

#### 32. `crates/beskar-cli/src/commands.rs` (Modify)

*   **Type / Scope**: Command dispatch.
*   **Proposed Changes**:
    *   Dispatch for the new commands.
    *   `repo enable` and `repo sync` pass target changes into core.
    *   Confirmation prompts list agent files beside skills.

#### 33. `crates/beskar-cli/src/output.rs` (Modify)

*   **Type / Scope**: Presentation.
*   **Proposed Changes**: Human output shows agent file lines as
    `agent  .claude/agents/egm-zoom-out.md  add  (claude-code)`, and marks
    overrides with `override`. JSON output adds `agent_files[]` and
    `targets[]` to repository status and plan data.

#### 34. `crates/beskar-cli/tests/workflows.rs` (Modify)

*   **Type / Scope**: Integration tests.
*   **Proposed Changes**: New cases.
    *   **Enable and render.** Enable two targets and check that the exact
        rendered bytes land at both paths.
    *   **Disable.** Disabling one target removes only its files.
    *   **Drift.** Local drift on an agent file is reported, and
        `--conflict keep` and `replace` behave as for skills.
    *   **Access.** A `readonly` agent on a target with no read-only mapping
        fails the plan.
    *   **Overrides.** An override is used verbatim. A stale override hash
        makes `doctor` warn.
    *   **Versions.** The registry stays `beskar 1` until the first
        `--target`, then becomes `beskar 2`. A `beskar 3` registry gives the
        "newer Beskar" error.
    *   **Remote sync.** Sync of a collection with `agents/` writes
        `synced-agent` and upgrades remotes.bsk.
    *   **Agent dependencies.** Removing the last skill an agent names
        removes the agent.
    *   **Symlinked agent directory.** A symlinked `.claude/` fails with an
        error naming the link and the manual fix.
    *   **Missing agent directory.** `.claude/agents/` missing beforehand is
        created.

#### 35. `docs/FORMAT.md` (Modify)

*   **Type / Scope**: Format reference.
*   **Proposed Changes**:
    *   New sections: **Format versions** (which file accepts which, the
        upgrade rule, both error messages), **Agent definition**
        (`agent.bsk`), and **Remotes**. The Remotes section documents the
        existing records plus `synced-agent`, filling the current gap.
    *   The **Registry** section gains `target` and `agent-file`.

#### 36. `docs/ARCHITECTURE.md` (Modify)

*   **Type / Scope**: Architecture doc.
*   **Proposed Changes**: Add the crate diagram `cli → core → targets → bsk`
    and the agent loop in planning. Replace "skills are opaque" with "skills
    are opaque; agent files are rendered from agent definitions".

#### 37. `docs/COLLECTIONS.md` (Modify)

*   **Type / Scope**: User doc.
*   **Proposed Changes**: Document `agents/` in the collection layout and
    how tracking a skill brings its agents. Add the upgrade note: every
    machine sharing a registry or remotes file must run this version.

#### 38. `README.md` (Modify)

*   **Type / Scope**: User doc.
*   **Proposed Changes**: Add a short "Agents and targets" section with
    `--target` examples and the list of targets in this release.

### Stage 2: Remaining targets

#### 39. `crates/beskar-targets/src/builtin.rs` (Modify)

*   **Type / Scope**: Data.
*   **Proposed Changes**: Add `cursor`, the remaining one of `opencode` and
    `copilot-cli`, `copilot-vscode` and `codex` (`persona-only`), one per
    commit, each from `docs/HARNESSES.md`. If Stage 0 confirms Copilot CLI
    and VS Code share `.github/agents/`, both entries use the same
    `agent_dir` and `file_name`, and the path is shared through
    `agent-file`'s target list.

#### 40. `crates/beskar-targets/src/render.rs` (Modify)

*   **Type / Scope**: Renderers.
*   **Proposed Changes**: Unit tests with golden bytes for each new target.

#### 41. `crates/beskar-cli/tests/workflows.rs` (Modify)

*   **Type / Scope**: Integration tests.
*   **Proposed Changes**:
    *   **Shared path.** Enable `copilot-cli` and `copilot-vscode`, check
        that one file is written and recorded with both targets, disable one
        and check the file stays, then disable both and check it is removed.
    *   **Codex.** Its persona file lands at the literal path named in
        `docs/HARNESSES.md`.

#### 42. `docs/HARNESSES.md`, `README.md` (Modify)

*   **Type / Scope**: Docs.
*   **Proposed Changes**: Mark each target as shipped, and add the Copilot VS
    Code manual checklist result.

### Stage 3: Adapted repositories and the dirty report (provisional)

#### 43. `crates/beskar-core/src/adaptation.rs` (Create)

*   **Type / Scope**: Pure adaptation logic (no processes, no network).
*   **Proposed Changes**:
    *   **Provenance model.**
        *   `Provenance { upstream_name, url, reference, commit,
            originals_hash, prompt_revision, adapting agent, reviewers, rounds,
            outputs: BTreeMap<(Kind, name), hash>, approved_by, approved_on }`
            with `decode` and `encode` for `adaptation/<upstream>.bsk` (format
            in Technical Plan Part C).
        *   `decode` rejects unknown records.
        *   `url` must pass `validate_upstream_url` (Part E rules).
    *   **Layout and checks.**
        *   `init_layout(dir)` plans the directories from Part C.
        *   `check(dir) -> Vec<CheckFailure>` runs the fixed checks from Part
            C plus the manual-review list for overrides.
    *   **Status and naming.**
        *   `status(dir, heads: &BTreeMap<upstream, HeadResult>) ->
            Vec<DirtyItem>` returns `hand-edited`, `upstream-moved`, `unknown`
            or `clean`.
        *   Naming: `check` fails when two upstreams yield the same local
            skill or agent name. `Provenance` carries `renames:
            Vec<(Kind, upstream_name, local_name)>`, and `record` takes
            `--rename KIND UPSTREAM-NAME LOCAL-NAME` (repeatable).

#### 44. `crates/beskar-core/src/collection.rs` (Modify)

*   **Type / Scope**: Collection reader.
*   **Proposed Changes**: `read_provenance(root) -> Result<Vec<Provenance>>`.
    It is called only by remote sync and the adapt commands, never by
    planning.

#### 45. `crates/beskar-core/src/remote.rs` (Modify)

*   **Type / Scope**: Remote sync.
*   **Proposed Changes**: The sync result carries the remote's provenance and
    the current output fingerprints. Core starts no processes, so the CLI
    does the `ls-remote` calls and passes the results back to
    `adaptation::status`.

#### 46. `crates/beskar-core/src/lib.rs` (Modify)

*   **Type / Scope**: Exports.
*   **Proposed Changes**: `pub mod adaptation`, plus exports of `Provenance` and
    `DirtyItem`.

#### 47. `crates/beskar-cli/src/git.rs` (Modify)

*   **Type / Scope**: Git process wrapper.
*   **Proposed Changes**:
    *   `ls_remote(url, reference) -> Result<String /*commit*/>`, with the
        URL and protocol rules from Part E. The environment sets
        `GIT_PROTOCOL_FROM_USER=0` and `GIT_TERMINAL_PROMPT=0`.
    *   `clone_snapshot(url, reference, dest)` reuses the existing
        byte-faithful clone, then removes `.git/` from the snapshot.

#### 48. `crates/beskar-cli/src/args.rs`, `spec.rs`, `commands.rs` (Modify)

*   **Type / Scope**: CLI.
*   **Proposed Changes**: Add `adapt init`, `adapt upstream add NAME URL
    [--ref R]`, `adapt upstream update NAME`, `adapt check`, `adapt record
    NAME --adapted-by HARNESS MODEL [--rename KIND FROM TO] --reviewer LENS HARNESS MODEL [same-model]
    ... --rounds N --approved-by NAME`, and `adapt status`. All take `--dir`
    (default `.`) and none opens the Beskar store. Also add `remote sync
    --no-upstream-check`.

#### 49. `crates/beskar-cli/src/output.rs` (Modify)

*   **Type / Scope**: Presentation.
*   **Proposed Changes**: The dirty report in human output, and
    `data.adaptation[]` in JSON, per Part E. Wording is the same for every reader:
    "re-adaptation pending for upstream X; maintainers run `beskar adapt
    status` in the adapted repository".

#### 50. `crates/beskar-cli/tests/workflows.rs` (Modify)

*   **Type / Scope**: Integration tests.
*   **Proposed Changes**: Local `file://` upstream repositories.
    *   **Init and record.** `adapt init` creates the layout. `upstream add`
        stores byte-exact files (CRLF fixture) and records the commit.
    *   **Fixed checks.** `check` fails on `${CLAUDE_PLUGIN_ROOT}`, on a
        dangling agent reference, and on a readonly-to-readwrite widening.
    *   **Upstream moved.** After an upstream commit, `remote sync` on a
        subscriber reports `upstream-moved` and deploys nothing new.
    *   **Hand edits.** A hand edit in the adapted repository reports
        `hand-edited`.
    *   **Unreachable upstream.** It reports `unknown`, and sync exits 0.
    *   **Blocked URLs.** An `ext::` URL and a URL starting with `-` in
        provenance are refused before git runs.

#### 51. `docs/adr/0008-adapted-collections.md` (Create)

*   **Type / Scope**: ADR.
*   **Proposed Changes**: Records the publisher model (decisions 13 and 14),
    one upstream per adaptation unit, provenance written only by `beskar
    adapt record`, the dirty report never changing deployment, and the
    `ls-remote` network call in sync with its URL rules. It notes that ADR
    0005's sync rules are unchanged.

#### 52. `docs/FORMAT.md`, `docs/COLLECTIONS.md`, `docs/ARCHITECTURE.md`, `README.md` (Modify)

*   **Type / Scope**: Docs.
*   **Proposed Changes**:
    *   **FORMAT.md** gains a **Provenance** section.
    *   **COLLECTIONS.md** gains "Adapted collections": the layout, the
        maintainer workflow, and what subscribers see.
    *   **ARCHITECTURE.md** describes `adaptation.rs` and the rule that the CLI
        runs `ls-remote`.
    *   **README.md** changes "only remote commands need the network" to
        include `adapt upstream` commands. It also says `remote sync`
        contacts each adapted collection's upstreams unless
        `--no-upstream-check` is given.

### Stage 4: Skill variants and local target definitions (only if Stage 0 requires)

#### 53. `crates/beskar-targets/src/definition.rs` (Create)

*   **Type / Scope**: Definition reader.
*   **Proposed Changes**: Decode `targets/NAME.bsk` (fields equal to `Target`,
    renderer limited to the fixed set, paths relative with no `..`) into a
    `Target`. Unknown records reject the whole file.

#### 54. `crates/beskar-core/src/store.rs`, `reconcile.rs`, `model.rs` (Modify)

*   **Type / Scope**: Core.
*   **Proposed Changes**:
    *   **Store.** Load `HOME/targets/*.bsk`. A local definition shadows a
        built-in one of the same name.
    *   **Variants.** `skills/NAME/variants/<target>/SKILL.md` produces a
        per-target skill copy in that target's `skill_dir`. It is recorded
        as `skill-file PATH NAME HASH TARGET...` in registry format 2.
    *   **Duplicate discovery.** A plan error, per Part F.
    *   **Symlink migration.** `reconcile.rs` detects the skill-directory
        symlink case from Part F and adds the grouped change. Any other
        symlink is a plan error with the manual fix in its message.

#### 55. `crates/beskar-cli/src/spec.rs`, `commands.rs`, `output.rs`, `tests/workflows.rs` (Modify)

*   **Type / Scope**: CLI and tests.
*   **Proposed Changes**: `target list` shows built-in and local definitions,
    and which one is active. Tests cover a shadowed target, a variant, a
    duplicate-discovery refusal, the `.claude/skills` link migration
    (interactive yes, `--yes` refused, `--json` without and with
    `--replace-link`), and a link pointing elsewhere being refused.

#### 56. `crates/beskar-core/src/transaction.rs` (Modify)

*   **Type / Scope**: Transactions.
*   **Proposed Changes**: `replace_link_with_directory(link, expected_target)`.
    It checks with `symlink_metadata` that `link` is still a symlink whose
    `read_link` equals `expected_target`, journals `unlink LINK TARGET`,
    removes the link and creates the directory. Rollback and `recover`
    remove the directory if it is empty of Beskar-written files, then
    recreate the symlink with the recorded target. Recovery refuses, as it
    does today, if the user changed the path in between. The journal stays
    `beskar 1`, because `unlink` is a new record only this version writes
    and only this version recovers. A 0.4.x binary that finds such a journal
    stops with its unknown-record error before touching anything.

#### 57. `crates/beskar-core/src/transaction_tests.rs` (Modify)

*   **Type / Scope**: Unit tests.
*   **Proposed Changes**:
    *   Commit replaces the link with a directory.
    *   Rollback after a failed variant write restores the identical link.
    *   Crash recovery after `unlink` restores it.
    *   A link retargeted by the user between plan and apply aborts.

#### 58. `crates/beskar-cli/src/args.rs` (Modify)

*   **Type / Scope**: Argument parsing.
*   **Proposed Changes**: `--replace-link PATH` (repeatable) on `repo enable`
    and `repo sync`, ignored with a warning when no such grouped change is
    planned.

#### 59. `docs/FORMAT.md`, `docs/HARNESSES.md` (Modify)

*   **Type / Scope**: Docs.
*   **Proposed Changes**: Document the target definition format and the
    variant layout.

### Stage 5: `beskar-adapt` skill (provisional)

#### 60. `skills/beskar-adapt/SKILL.md` (Create)

*   **Type / Scope**: Agent skill.
*   **Proposed Changes**: The steps in Technical Plan Part D. Hard rules:
    *   work only inside the adapted repository;
    *   never run anything from `upstreams/`;
    *   never merge;
    *   call `beskar adapt check` and `record` for every deterministic step;
    *   stop and ask the maintainer when `check` reports an access widening;
    *   record same-model reviewers honestly.

    The frontmatter description triggers on "adapt", "adapt" or "import
    upstream skills".

#### 61. `skills/beskar-adapt/references/dispatch.md` (Create)

*   **Type / Scope**: Skill reference.
*   **Proposed Changes**: The per-target dispatch lines a neutral `SKILL.md`
    must contain, copied from `docs/HARNESSES.md`, including the Codex
    persona-path line.

#### 62. `skills/beskar-adapt/references/council.md` (Create)

*   **Type / Scope**: Skill reference.
*   **Proposed Changes**:
    *   **Reviewers.** The three lenses, each reviewer's prompt, and the
        verdict file format (`approve` or `changes`; findings with id,
        severity, file, in-scope flag and text).
    *   **Ledger.** The adapting agent marks each finding `applied`, `rejected` with
        a reason, or `out-of-scope`.
    *   **Stopping and scope.** The stop rule (all approve, no new in-scope
        Blocker or Major, or round 2), and the scope test (portability,
        agent extraction, dispatch correctness, access level).

#### 63. `skills/beskar-adapt/references/pull-request.md` (Create)

*   **Type / Scope**: Skill reference.
*   **Proposed Changes**: The PR body template. Its sections, in order:
    1.  manual access-review items;
    2.  council table (lens, harness, model, rounds, verdict);
    3.  rejected and out-of-scope findings with reasons;
    4.  upstream commit;
    5.  the commands to reproduce `adapt check`.

    It ends with the fallback when `gh` is missing.

#### 64. `.github/workflows/release.yml` (Modify)

*   **Type / Scope**: Release packaging.
*   **Proposed Changes**: Add `cp -R skills/beskar-adapt "dist/$NAME/skills/"`
    next to `beskar-completions`.

#### 65. `README.md` (Modify)

*   **Type / Scope**: User doc.
*   **Proposed Changes**: List `beskar-adapt` next to `beskar-completions`,
    with one paragraph on the maintainer workflow.

### Stage 6: Remote target definitions (provisional)

#### 66. `crates/beskar-core/src/collection.rs`, `store.rs`, `model.rs` (Modify)

*   **Type / Scope**: Core.
*   **Proposed Changes**:
    *   **Reading.** `collection::read_targets(root)` reads `targets/*.bsk`
        through `beskar_targets::definition`.
    *   **Approval.** Approved remote targets are copied to
        `HOME/targets/<remote>--<name>.bsk` with `source REMOTE`, `digest
        SHA` and one `root PATH` line per approved output root. Sync never
        activates or updates them; it only reports "new definition available"
        or "definition changed".
    *   **Protected paths.** Roots under `.git/`, `.github/workflows/` or any
        Beskar file are refused.

#### 67. `crates/beskar-cli/src/spec.rs`, `commands.rs`, `output.rs`, `tests/workflows.rs` (Modify)

*   **Type / Scope**: CLI and tests.
*   **Proposed Changes**: `target approve REMOTE NAME --digest SHA` prompts
    for each requested root and never accepts `--yes`. `target remove NAME`.
    Tests cover a wrong digest, a protected root, a definition with an
    unknown record, and an upstream change not being activated.

#### 68. `docs/adr/0009-remote-target-definitions.md` (Create)

*   **Type / Scope**: ADR.
*   **Proposed Changes**: The trust model: pinned by digest, approved roots,
    protected paths, data only and never a command.

#### 69. `docs/FORMAT.md`, `docs/COLLECTIONS.md` (Modify)

*   **Type / Scope**: Docs.
*   **Proposed Changes**: `targets/` in collections and the approval flow.

### Verification Plan

*   **Compilation/Build**: `cargo build --workspace --release --offline` on
    1.89.0 and stable (CI matrix).
*   **Unit and integration tests**: `cargo test --workspace --offline`. The
    integration tests are in `crates/beskar-cli/tests/workflows.rs`.
*   **Static analysis**: `cargo +stable fmt --all --check`,
    `cargo +stable clippy --workspace --all-targets --offline -- -D warnings`,
    `RUSTDOCFLAGS=-D warnings cargo +stable doc --workspace --no-deps --offline`.
*   **Dependency check**: `cargo tree --workspace --edges normal` lists only
    `bsk`, `beskar-core`, `beskar-targets` and `beskar-cli`. A CI step
    fails if any other package appears.
*   **Compatibility**: a test fixture holds a registry and remotes file
    written by 0.4.0. The new binary reads them, and leaves them at
    `beskar 1` when no target is enabled. A manual check runs the 0.4.0
    release binary against a `beskar 2` registry and records the exact
    error in FORMAT.md.
*   **End to end (success criterion)**: after Stage 5, adapt upstream EGM
    with `beskar-adapt`, track the result as a remote, enable all six
    targets in a scratch repository, and run the HARNESSES.md loop checklist
    in each target. Record the results in `docs/HARNESSES.md`.

--------------------------------------------------------------------------------

*Drafted with EGM*

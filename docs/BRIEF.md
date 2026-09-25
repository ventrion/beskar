# Beskar — Better Skill Arrangement
## Agent Implementation Brief

### 1. Purpose

**Beskar** is a CLI-first tool for managing reusable agent skills across multiple local workspaces.

The fundamental problem Beskar solves is:

> An agent should have access only to the skills relevant to the workspace and task it is operating in, while the user maintains one curated global collection of skills.

Instead of installing every available skill into every repository, Beskar maintains a **global skill library**, lets the user organize skills into **profiles**, and materializes selected profiles into a repository's `.agents/skills/` directory.

Beskar also maintains a **registry** describing which repositories are managed and which profiles are active in each repository.

The intended model is:

```text
                   BESKAR

        ┌─────────────────────────┐
        │         Library         │
        │                         │
        │  skills/                │
        │  profiles/              │
        │                         │
        │  User-curated source    │
        │  of truth               │
        └────────────┬────────────┘
                     │
              profiles select
              groups of skills
                     │
                     ▼
        ┌─────────────────────────┐
        │        Profiles         │
        │                         │
        │ coding                  │
        │ research                │
        │ frontend                │
        │ minimal                 │
        └────────────┬────────────┘
                     │
                     │ applied to
                     ▼
 ┌─────────────────────────────────────────┐
 │                 Registry                │
 │                                         │
 │ repo-a → [coding]                       │
 │ repo-b → [research, frontend]           │
 │ repo-c → [minimal]                      │
 └───────────────┬─────────────────────────┘
                 │
                 │ materialize / update
                 ▼
 ┌─────────────────────────────────────────┐
 │              Repository                 │
 │                                         │
 │ .agents/skills/                         │
 │   skill-a/                              │
 │   skill-b/                              │
 │   skill-c/                              │
 │                                         │
 │ ← actual files available to agents      │
 └─────────────────────────────────────────┘
```

---

# 2. Core Concepts

Beskar should treat the following as distinct domain concepts.

### Library

The **Library** is the user's curated global collection of skills and profile definitions.

It is the canonical source from which repository installations are produced.

Conceptually:

```text
~/.beskar/library/

skills/
    playwright/
    python/
    code-review/
    research/
    pdf/
    ...

profiles/
    coding.yaml
    research.yaml
    frontend.yaml
    ...
```

The exact location should be configurable.

The library **must not itself be an agent skill directory**. Agents should not automatically discover or consume skills directly from it.

The user may choose to make the library a Git repository. Beskar should not require Git, however.

Version control is an external concern:

```text
Beskar manages library contents.
Git may manage library synchronization.
```

Therefore `push`, `pull`, branching, remotes, etc. do not need to be core Beskar abstractions.

---

### Skill

A **Skill** is the smallest reusable capability managed by Beskar.

A skill exists canonically inside the Library.

For example:

```text
skills/
    playwright/
        SKILL.md
        scripts/
        references/

    code-review/
        SKILL.md

    pdf/
        SKILL.md
        scripts/
```

Beskar should initially avoid imposing unnecessary requirements on the internal skill format.

It should primarily treat a skill as a managed directory, while optionally extracting metadata from known files such as `SKILL.md`.

Future metadata could include:

```yaml
name: playwright
description: Browser automation and testing
tags:
  - browser
  - testing
category: development
version: 1.2.0
dependencies: []
```

---

### Profile

A **Profile** is a named set of skills.

Profiles represent use cases rather than physical installations.

For example:

```yaml
name: coding

skills:
  - code-review
  - git
  - testing
  - documentation
```

Another profile might be:

```yaml
name: research

skills:
  - web-research
  - pdf
  - summarization
  - citations
```

Profiles allow users to think in terms of:

> "Give this repository my coding environment."

rather than:

> "Install skills A, B, C, D, E and F."

A repository may have **multiple profiles enabled simultaneously**.

The effective skill set is therefore the union of their skills.

```text
repo profiles:

coding
research

        ↓

effective skills:

code-review
git
testing
documentation
web-research
pdf
summarization
citations
```

Duplicate skills must collapse into one installation.

---

### Repository / Workspace

A **Repository** is a local workspace managed by Beskar.

It does not necessarily have to be a Git repository. "Repo" is convenient terminology, but the actual abstraction is a filesystem workspace.

Example:

```text
~/projects/my-app/
```

Beskar materializes active skills into:

```text
~/projects/my-app/.agents/skills/
```

These are the files visible to agents.

**Installed skills should initially be copies rather than symlinks.**

This has several advantages:

- repositories remain self-contained;
- agents don't need access to the global library;
- repository skills can temporarily diverge;
- repositories continue working if the library moves;
- future workflows can promote local modifications back into the library.

---

### Registry

The **Registry** is machine-local Beskar state.

It answers questions such as:

- Which repositories does Beskar know about?
- Which profiles are enabled in each repository?
- Which skills are currently installed?
- Where is a particular profile used?
- Where is a particular skill used?
- Which repositories need updating?

Conceptually:

```yaml
repos:

  /home/user/projects/api:
    profiles:
      - coding
      - backend

  /home/user/projects/docs:
    profiles:
      - writing
      - research

  /home/user/projects/site:
    profiles:
      - coding
      - frontend
```

The registry should **not live inside the Library** because it contains machine-specific filesystem paths.

This distinction is important:

```text
Library
    portable
    syncable
    user-curated
    potentially version controlled

Registry
    machine-local
    contains absolute/local paths
    tracks deployments
```

---

# 3. Initialization

A user should be able to bootstrap Beskar with:

```bash
beskar init
```

This should initialize the Beskar environment, including:

- configuration;
- registry;
- library location;
- required directory structure.

Library-specific initialization may still exist:

```bash
beskar library init
```

but normal users should generally only need `beskar init`.

A useful additional command is:

```bash
beskar doctor
```

which verifies the library, registry, repository paths, configuration, and installed state.

---

# 4. Library Operations

Library commands manage the canonical skill collection.

Examples:

```bash
beskar library list
beskar library add <path>
beskar library scan <path>
beskar library show <skill>
beskar library remove <skill>
```

`add` imports a particular skill into the library.

`scan` discovers multiple skills from an external directory and offers to ingest them.

For example:

```bash
beskar library scan ~/Downloads/agent-skills
```

might discover:

```text
Found 17 skills.

✓ playwright
✓ pdf
✓ code-review
✓ postgres
...

Import 17 skills? [Y/n]
```

Classification and categorization can later augment this process.

---

# 5. Profile Operations

Profiles should have their own first-class command namespace.

```bash
beskar profile create coding

beskar profile add coding playwright
beskar profile add coding code-review

beskar profile remove coding playwright

beskar profile show coding
beskar profile list

beskar profile delete coding
```

Profile definitions belong to the Library and therefore travel with it.

---

# 6. Repository Operations

Repositories represent deployment targets.

A repository can first be registered:

```bash
beskar repo add .
```

Profiles can then be enabled:

```bash
beskar repo enable coding
beskar repo enable research
```

or disabled:

```bash
beskar repo disable research
```

A convenience operation may toggle them:

```bash
beskar repo toggle coding
```

The important distinction is:

```text
enable/disable
        changes desired configuration

update
        reconciles filesystem state
```

For example:

```bash
beskar repo update
```

calculates the effective skills from enabled profiles and reconciles:

```text
Library + Profiles + Registry
              ↓
       desired skill set
              ↓
     .agents/skills/
```

`--all` can operate across every relevant item where appropriate.

---

# 7. Reconciliation

Reconciliation should be one of Beskar's central internal abstractions.

Suppose a repository currently contains:

```text
.agents/skills/

git
testing
pdf
```

but its active profiles now resolve to:

```text
git
testing
playwright
```

Beskar determines:

```text
unchanged:
  git
  testing

add:
  playwright

remove:
  pdf
```

and then brings the workspace into the desired state.

A dry-run mode would be valuable:

```bash
beskar repo update --dry-run
```

producing something like:

```text
Repository: ~/projects/foo

+ playwright
~ testing
- pdf

No files changed.
```

The reconciliation engine should be reusable by both single-repository and global operations.

---

# 8. Global Updates

One major value proposition of Beskar is centralized maintenance.

Imagine `code-review` exists in 14 repositories.

The user improves the canonical version:

```text
Library/code-review
```

They should then be able to propagate it.

For example:

```bash
beskar registry update --all
```

or eventually a shorter global command such as:

```bash
beskar update --all
```

Beskar consults the Registry, resolves each repository's active profiles, and reconciles every workspace.

Conceptually:

```text
             Library
                │
          skill changed
                │
                ▼
             Beskar
                │
       consult registry
        ┌───────┼───────┐
        ▼       ▼       ▼
      repo A  repo B  repo C
        │       │       │
      update  update   skip
```

Only repositories whose desired state requires the changed skill need modification.

---

# 9. Registry Inspection

The Registry should make the system observable.

Useful commands include:

```bash
beskar registry list
beskar registry status
beskar registry stats
```

Users should be able to answer questions such as:

```text
Where is profile "coding" used?

coding
  ~/projects/api
  ~/projects/frontend
  ~/projects/tooling
```

or:

```text
Where is skill "playwright" installed?

playwright
  frontend   [profile: coding]
  e2e-tests  [profile: testing]
  website    [profile: frontend]
```

Stats could show:

```text
Repositories       18
Profiles             7
Library skills      64
Installed skills   143
Unused skills       11
```

This information can eventually support pruning and library cleanup.

---

# 10. Local Modification and Promotion

Because repository skills are copied rather than symlinked, Beskar needs to recognize **drift**.

There are three possible states:

```text
Library version == installed version
    clean

Library version != installed version
    library changed

Installed version modified locally
    local drift
```

This is an important part of the design that should not be deferred indefinitely, because blindly running `update` could otherwise destroy user changes.

Beskar should therefore track fingerprints/hashes of installed content.

For example:

```text
library hash
     │
     ▼
installed hash ─── current workspace hash
```

If the workspace differs from the version Beskar originally installed, Beskar knows it was modified locally.

Eventually this enables:

```bash
beskar skill promote <skill>
```

or similar semantics:

```text
workspace modification
        ↓
review diff
        ↓
promote
        ↓
library
        ↓
update other repositories
```

The exact command name can be decided later.

---

# 11. Important Safety Principle

**Beskar should never silently destroy locally modified skills.**

Before overwriting or removing a modified installed skill, it should detect the conflict and require an explicit resolution.

For example:

```text
Conflict: code-review

The workspace copy has local modifications.

  [k] keep local
  [l] replace with library
  [p] promote to library
  [d] show diff
```

Non-interactive environments should have explicit conflict policies rather than guessing.

---

# 12. Classification

Classification is useful but should remain separate from the core architecture.

Beskar's basic functionality should work without AI.

Later, a classifier can inspect skills and propose:

- category;
- tags;
- description;
- related skills;
- possible duplicates;
- suggested profiles.

Conceptually:

```text
new skills
    │
    ▼
classifier
    │
    ├── development
    ├── research
    ├── documents
    └── browser
```

This could use a local/open-source model or another classifier. The classifier should be an optional enrichment mechanism rather than something required for library correctness.

---

# 13. Recommended Domain Model

Internally, agents implementing Beskar should think in approximately these entities:

```text
BeskarConfig
    library_path
    registry_path
    agent_skills_path
    settings

Library
    skills[]
    profiles[]

Skill
    id
    name
    path
    metadata
    fingerprint

Profile
    id
    name
    skills[]

Registry
    repositories[]

Repository
    path
    enabled_profiles[]
    installed_skills[]
    last_sync

InstalledSkill
    skill_id
    source_fingerprint
    installed_fingerprint
    status
```

Do not duplicate information unnecessarily. In particular, profiles should reference skill identities rather than copying skill definitions.

---

# 14. State Model

There are really **three layers of state**:

```text
             DESIRED GLOBAL STATE

                  Library
                     +
                  Profiles
                     │
                     ▼

             DEPLOYMENT STATE

                  Registry
        repo → enabled profiles
                     │
                     ▼

              MATERIALIZED STATE

            repository filesystem
             .agents/skills/
```

This distinction should guide the architecture.

The filesystem is not the configuration.

The Registry is not the source of skill content.

The Library does not determine where skills are deployed.

Each layer has one responsibility.

---

# 15. Suggested CLI Shape

A coherent initial command tree would be:

```text
beskar
├── init
├── doctor
│
├── library
│   ├── init
│   ├── add
│   ├── scan
│   ├── list
│   ├── show
│   └── remove
│
├── profile
│   ├── create
│   ├── delete
│   ├── list
│   ├── show
│   ├── add
│   └── remove
│
├── repo
│   ├── add
│   ├── remove
│   ├── list
│   ├── status
│   ├── enable
│   ├── disable
│   ├── toggle
│   └── update
│
└── registry
    ├── list
    ├── status
    ├── stats
    ├── update
    └── prune
```

There is room later for convenience aliases such as:

```bash
beskar status
beskar update
beskar update --all
```

without changing the underlying domain model.

---

# 16. Example End-to-End Workflow

A new user might run:

```bash
# Initialize Beskar
beskar init

# Import existing skills
beskar library scan ~/my-skills

# Create a reusable profile
beskar profile create coding
beskar profile add coding git
beskar profile add coding code-review
beskar profile add coding testing

# Enter a project
cd ~/projects/beskar

# Register it
beskar repo add .

# Activate coding skills
beskar repo enable coding

# Materialize them
beskar repo update
```

The result:

```text
~/projects/beskar/

.agents/
└── skills/
    ├── git/
    ├── code-review/
    └── testing/
```

Later the user improves `code-review` in the Library and runs:

```bash
beskar registry update --all
```

Every registered repository using a profile containing `code-review` can then be reconciled with the new canonical version.

---

# 17. Design Principles

Agents implementing Beskar should preserve these principles:

**Local-first.** Beskar should work entirely offline and with ordinary filesystem operations.

**Library as source of truth.** Canonical skills belong in one curated location.

**Profiles describe intent.** Users select use cases rather than manually managing dozens of skill directories.

**Repositories are deployments.** `.agents/skills` contains materialized copies intended for agents.

**Registry provides visibility.** Users should always be able to determine what is installed where and why.

**No Git dependency.** A Git-backed Library should work extremely well, but Git is not Beskar's storage abstraction.

**No silent data loss.** Local changes must be detected before reconciliation overwrites them.

**Deterministic reconciliation.** Given the same Library, profiles, and repository configuration, Beskar should calculate the same desired `.agents/skills` state.

**CLI first, UI later.** Domain logic should not depend on CLI presentation so a TUI, GUI, or other integrations can eventually sit on top of the same core.

---

## The core mental model

The entire project can ultimately be reduced to:

```text
LIBRARY
"What skills do I own?"
        │
        ▼
PROFILE
"Which skills belong together?"
        │
        ▼
REGISTRY
"Where should those profiles be active?"
        │
        ▼
REPOSITORY
"Materialize exactly those skills here."
        │
        ▼
.agents/skills
"What the agent actually sees."
```

That separation should be treated as the architectural foundation of Beskar.

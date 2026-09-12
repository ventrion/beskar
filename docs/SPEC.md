# Beskar — Better Skill Arrangement

**Status:** Draft specification v0.2
**Implementation language:** Rust
**Primary executable:** `beskar`
**Desktop executable:** `beskar-gui`
**Product phrase:** “This is the way.”
**Purpose:** Manage, curate, version, compose, distribute, install, and update Agent Skills across multiple workspaces.

---

# 1. Product definition

Beskar is a local-first skill library and installation manager for Agent Skills.

Beskar maintains one user-level Git repository containing a canonical collection of skills and named skill profiles.

A **profile** is a reusable named capability set consisting of skills.

A **workspace installation** may have **one or more profiles attached simultaneously**.

Beskar computes the effective installed skill set as the union of all attached profiles and reconciles that desired state into the workspace.

Beskar remembers those installations, detects drift between installed copies and the skill library, tracks which profile or profiles require each skill, and safely updates one or all registered installations.

Beskar exposes this functionality through:

* a CLI;
* an interactive terminal UI;
* a desktop GUI.

All interfaces MUST invoke the same Rust domain/core APIs.

Business logic MUST NOT be separately implemented in the CLI, TUI, and GUI.

The CLI is the normative behavioral interface.

---

# 2. Product goals

Beskar MUST support the following lifecycle:

1. Establish or clone a user-level skill repository.
2. Ingest Agent Skills into that repository.
3. Organize skills into human-readable buckets.
4. Attach curation metadata such as tags and display rank.
5. Create and maintain named skill profiles.
6. Attach one or more profiles to a workspace target.
7. Install the union of all attached profiles.
8. Record that installation in a machine-local registry.
9. Explain why each installed skill exists by identifying the profiles requiring it.
10. Detect whether installed skills are:

* current;
* outdated;
* locally modified;
* missing;
* unmanaged;
* orphaned;
* newly required;
* no longer required.

11. Add and remove profiles independently without disrupting skills still required by other profiles.
12. Update an individual installation.
13. Update all registered installations.
14. Fetch changes from the skill repository's Git remote.
15. Push a local branch containing skill-library changes.
16. Work offline for all operations except explicit remote operations.
17. Support Linux, macOS, and Windows.
18. Be usable both by humans and automation/coding agents.
19. Provide deterministic dry-run plans for every meaningful mutation.
20. Preserve user-created files that Beskar does not own.

---

# 3. Non-goals for v1

The following are explicitly outside v1:

* executing skills;
* interpreting or enforcing skill runtime permissions;
* providing an agent runtime;
* providing a hosted skill marketplace;
* semantic dependency resolution between skills;
* profile inheritance;
* profiles including other profiles;
* per-profile versions of the same skill within one installation;
* automatic Git merge-conflict resolution;
* automatic rebasing;
* automatic force-pushing;
* automatic pull-request creation;
* background daemon operation;
* cloud synchronization outside Git;
* automatic package dependency installation;
* self-updating the Beskar executable;
* executing skill scripts during validation or installation.

These MAY be added in future versions without invalidating the v1 installation model.

---

# 4. Normative language

The terms **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are normative.

When this specification leaves a behavior unspecified, the implementation SHOULD choose the safer and less destructive behavior.

---

# 5. Terminology

## Skill

An Agent Skills-compatible directory containing `SKILL.md`.

The canonical identity of a skill is the `name` field in `SKILL.md`.

The directory leaf name MUST exactly equal the skill name.

Skill names MUST be globally unique within one Beskar Library revision.

---

## Library

The user-level Git repository managed by Beskar.

The Library is the canonical source of:

* skills;
* profiles;
* curation metadata.

---

## Remote

A Git remote belonging to the Library.

The default remote is normally:

```text
origin
```

Beskar does not introduce a custom synchronization protocol.

---

## Bucket

A directory hierarchy underneath `skills/` used only for human organization.

Example:

```text
skills/engineering/process/code-review/
```

The bucket in this example is:

```text
engineering/process
```

Buckets MUST NOT form part of skill identity.

Moving a skill between buckets MUST NOT change its identity.

---

## Profile

A named, ordered set of skill names.

Profiles are reusable capability sets.

Examples:

```text
dev-core
rust-development
github
documentation
frontend
writing
```

Profiles represent desired capability membership.

---

## Profile attachment

The relationship between a Profile and an Installation.

An Installation MUST support multiple Profile attachments simultaneously.

---

## Installation

A machine-local registration representing one managed target.

An Installation is uniquely associated with:

* one Library;
* one workspace;
* one target directory;
* one source ref;
* one or more attached Profiles.

The Installation reconciles the union of those Profiles into that target.

---

## Effective skill set

The unique union of every Skill required by every Profile attached to an Installation.

For attached profiles:

```text
P1, P2, ... Pn
```

the desired skill set is:

```text
desired_skills(I) =
    union(skills(P1), skills(P2), ..., skills(Pn))
```

---

## Skill membership

The set of attached Profiles requiring a particular Skill.

Example:

```text
testing
  required by:
    dev-core
    rust-development
    ci
```

---

## Workspace

The directory supplied to commands such as:

```text
beskar add dev-core ~/src/project
```

The Workspace will commonly be a Git repository but MAY be any existing directory.

Beskar MUST NOT silently replace the supplied path with an automatically detected Git root.

---

## Target

The workspace-relative directory into which managed skills are copied.

Default:

```text
.agents/skills
```

---

## Adapter

A convenience configuration defining a conventional Target for an agent environment.

Adapters MUST NOT modify skill contents.

---

## Source ref

The Git branch, tag, or commit from which an Installation resolves all attached Profiles and Skills.

An Installation has exactly one Source ref.

All Profiles attached to that Installation MUST resolve against that same Source ref.

---

## Stamp

Beskar-owned metadata inside each installed Skill recording the precise source revision and managed files.

---

## Registry

Machine-local state describing all registered Installations.

The Registry MUST NOT normally be committed to consumer repositories.

---

## Managed file

A file listed in a Beskar Stamp.

---

## Extra file

A file inside an installed skill directory that is not listed in the Stamp.

Extra files are not owned by Beskar.

---

## Drift

A difference between installed state and expected Library state.

---

## Reconciliation

The process of comparing:

```text
last applied state
+
current installed state
+
current attached profiles
+
current Library state
```

and producing a deterministic operation plan that converges the target toward desired state.

---

# 6. Core design model

One Installation owns exactly one:

```text
(workspace, target)
```

pair.

An Installation contains one or more attached Profiles.

For example:

```text
Workspace:
  ~/src/beskar

Target:
  .agents/skills

Source ref:
  main

Profiles:
  dev-core
  rust-development
  github
  documentation
```

Those Profiles may overlap.

Example:

```text
dev-core:
  git-workflow
  testing
  code-review

rust-development:
  rust
  cargo
  testing

github:
  git-workflow
  github-pr

documentation:
  technical-writing
  documentation-review
```

Effective set:

```text
git-workflow
testing
code-review
rust
cargo
github-pr
technical-writing
documentation-review
```

There is exactly one physical installation of:

```text
git-workflow
```

and exactly one physical installation of:

```text
testing
```

even though multiple Profiles require them.

---

# 7. Multi-profile invariants

The following are fundamental Beskar invariants.

## 7.1 Profiles are composable

Profiles MUST be capable of simultaneous attachment.

Users SHOULD be encouraged to create small capability-oriented Profiles instead of every possible combination as monolithic Profiles.

---

## 7.2 One skill copy per target

A Skill MUST appear at most once physically within one Installation Target.

Profile overlap does not produce duplicate files.

---

## 7.3 Membership is many-to-many

A Skill may belong to many Profiles.

A Profile may contain many Skills.

Beskar MUST retain enough state to explain this relationship.

---

## 7.4 Removal depends on membership

A Skill MUST NOT be physically retired merely because it was removed from one attached Profile.

A Skill becomes eligible for retirement only when no attached Profile requires it.

---

## 7.5 One source ref per installation

Every attached Profile within one Installation resolves against the same Source ref.

Beskar v1 MUST NOT support:

```text
dev-core            @ main
rust-development    @ experimental
github              @ v2.0
```

inside one Target.

---

## 7.6 One reconciliation plan per installation

Beskar MUST NOT update attached Profiles independently.

It MUST:

1. resolve all attached Profiles;
2. compute their union;
3. compute one desired-state plan;
4. reconcile the Target once.

This avoids overlapping Profiles fighting over shared Skills.

---

# 8. Design invariants inherited from the earlier skill-manager

## 8.1 Committed state is authoritative

Install and update operations MUST source Profile definitions and Skill contents from committed Git objects.

They MUST NOT install arbitrary uncommitted Library working-tree bytes.

A dirty Library MAY coexist with read operations.

Beskar MUST clearly state that uncommitted Library changes are not part of the resolved source revision.

---

## 8.2 Skill names are globally unique

Two Skills with the same canonical name make the Library invalid regardless of bucket placement.

---

## 8.3 Buckets do not affect identity

Re-bucketing does not affect Profiles or existing Installations.

---

## 8.4 Profiles describe desired state

Attached Profiles are not merely installation history.

Changes to attached Profiles alter the desired state of the Target.

---

## 8.5 Local modifications are protected

Beskar MUST NOT overwrite modified managed files without explicit consent.

---

## 8.6 Extra files are preserved

Extra files MUST NOT cause a Skill to be classified as Modified.

Normal update and profile detach operations MUST preserve Extra files.

---

## 8.7 Stamps are written last

The new Stamp MUST only be written after all intended file updates/removals for that Skill complete successfully.

---

## 8.8 Network activity is explicit

`beskar update` MUST NOT implicitly contact a remote.

Network access occurs only for commands such as:

```text
fetch
push
init --remote
```

or another explicitly networked command.

---

## 8.9 Git conflicts are never silently resolved

Beskar MAY perform strict fast-forward operations.

Beskar MUST NOT automatically:

* merge;
* rebase;
* resolve conflicts;
* force-push.

---

# 9. Library filesystem format

Conceptual Library:

```text
/
├── beskar.toml
├── catalog.toml
├── skills/
│   ├── engineering/
│   │   └── process/
│   │       └── code-review/
│   │           ├── SKILL.md
│   │           └── ...
│   ├── languages/
│   │   └── rust/
│   │       └── rust-development/
│   │           └── SKILL.md
│   └── writing/
│       └── editing/
│           └── unslop/
│               └── SKILL.md
└── profiles/
    ├── dev-core.toml
    ├── rust-development.toml
    ├── github.toml
    └── documentation.toml
```

Only committed Git content is part of a Library revision.

---

# 10. `beskar.toml`

Every Library MUST contain:

```text
beskar.toml
```

Minimum schema:

```toml
schema = 1
library_id = "550e8400-e29b-41d4-a716-446655440000"

skills_dir = "skills"
profiles_dir = "profiles"
catalog_file = "catalog.toml"

default_ref = "main"
```

`library_id` MUST:

* be generated once;
* be globally unique;
* remain unchanged across clones.

It distinguishes unrelated Libraries containing identically named Skills.

Unsupported schema versions MUST fail closed.

---

# 11. Agent Skill validation

Beskar MUST validate the supported Agent Skills format rather than defining an incompatible Skill format.

At minimum validate:

* `SKILL.md` exists directly within the Skill root;
* YAML frontmatter parses;
* `name` exists;
* `description` exists;
* `name` follows supported Agent Skills naming constraints;
* directory leaf exactly matches `name`;
* duplicate skill names do not exist.

Unknown valid Skill fields MUST be preserved.

Beskar MUST NOT rewrite `SKILL.md` merely for formatting normalization.

Beskar MUST NOT execute bundled Skill scripts during validation.

---

# 12. Filesystem safety

Beskar v1 MUST manage only:

* regular files;
* regular directories.

During Library ingest, Beskar MUST reject:

* symlinks;
* junctions;
* reparse-point links;
* sockets;
* devices;
* named pipes;
* other filesystem-special objects.

Path traversal through:

```text
..
```

MUST be rejected.

Beskar MUST use process argument arrays rather than shell-generated command strings.

---

# 13. Bucket rules

Bucket paths are Library-relative directories beneath `skills/`.

Accepted v1 bucket segments SHOULD consist of portable lowercase ASCII characters:

```text
a-z
0-9
.
_
-
```

Segments MUST NOT:

* equal `.` or `..`;
* contain path separators;
* use absolute paths;
* use Windows reserved device names.

Example:

```text
engineering/process
```

---

# 14. Catalog metadata

`catalog.toml` stores optional Beskar-specific curation metadata.

Example:

```toml
schema = 1

[skills.code-review]
tags = ["engineering", "git", "review"]
rank = 100

[skills.unslop]
tags = ["writing", "editing"]
rank = 200
```

Supported v1 metadata:

* `tags: string[]`
* `rank: integer`
* `notes: string`, optional

The filesystem remains authoritative for bucket placement.

Catalog metadata MUST NOT be copied into workspace installations.

---

# 15. Profile format

Each Profile is one TOML file under:

```text
profiles/
```

Example:

```toml
schema = 1
id = "98f1513d-94fa-4ace-907e-544c66233653"

name = "rust-development"
description = "Rust development capability set"

skills = [
  "rust",
  "cargo",
  "testing",
]
```

`id` is immutable.

`name` is human-facing and MUST be unique within one Library revision.

Registry attachments MUST internally refer to Profile ID.

This enables Profile rename without breaking Installations.

Duplicate Skill entries in a Profile are invalid.

Nested Profiles are unsupported in v1.

---

# 16. Profile ordering

A Profile is an ordered list.

The order is meaningful for:

* editing;
* presentation;
* exported views.

It MUST NOT imply runtime or execution precedence.

---

# 17. Installation profile ordering

Attached Profiles also have deterministic order.

By default, Profiles are ordered by the time they were attached.

The effective presentation order of Skills is an **ordered union**:

1. process Profiles in attachment order;
2. process each Profile's Skills in Profile order;
3. emit a Skill only on its first occurrence.

Example:

```text
dev-core:
  git
  testing
  review

rust:
  rust
  testing
  cargo
```

Effective presentation order:

```text
git
testing
review
rust
cargo
```

Membership remains:

```text
testing:
  dev-core
  rust
```

The ordering affects display only.

---

# 18. Source refs

Every Installation MUST store exactly one Source ref.

Examples:

```text
main
develop
feature/new-skills
v1.4.0
8d5b31a...
```

When creating a new Installation without explicit `--ref`, Beskar MUST use:

```text
default_ref
```

from `beskar.toml`.

Beskar MUST NOT silently use whichever Library branch happens to be checked out.

---

# 19. Source ref behavior

Branch refs follow branch movement after explicit synchronization.

Tags and commit hashes are effectively pinned.

During one operation:

* all Profiles;
* all Skills;
* all Library metadata

MUST be resolved from the exact same resolved Library commit.

This gives the operation a consistent snapshot.

---

# 20. Adding a Profile

Normative syntax:

```text
beskar add <profile-name> <workspace>
```

Example:

```text
beskar add dev-core ~/src/project
```

Options:

```text
--target <relative-path>
--adapter <adapter-name>
--ref <git-ref>
--force
--replace-unmanaged
--dry-run
--json
```

---

# 21. Meaning of `beskar add`

`beskar add` means:

> Ensure this Profile is attached to this Installation and reconcile the resulting effective skill set.

If no Installation exists for:

```text
(workspace, target)
```

Beskar creates one.

If an Installation already exists, Beskar attaches the Profile to the existing Installation.

`beskar add` MUST NOT require a new Target merely because another Profile is already installed.

---

# 22. Idempotent add

If the Profile is already attached:

```text
beskar add rust .
```

MUST be idempotent.

Beskar SHOULD:

* validate the attachment;
* reconcile desired state;
* otherwise produce a no-op.

It MUST NOT create duplicate Profile attachments.

---

# 23. Adding to an existing Installation with a different ref

Suppose an Installation currently uses:

```text
main
```

and the user runs:

```text
beskar add rust . --ref experimental
```

Beskar MUST refuse because one Installation cannot resolve Profiles against multiple refs.

It MUST tell the user that the Installation currently uses:

```text
main
```

and that changing the Installation's ref is a separate operation.

Recommended command:

```text
beskar ref set <workspace> <ref>
```

or:

```text
beskar installation ref <workspace> <ref>
```

The final CLI grammar MUST choose one canonical spelling.

---

# 24. Target adapters

Initial adapters:

## `agents`

```text
.agents/skills
```

## `claude`

```text
.claude/skills
```

## `custom`

Requires explicit `--target`.

Adapters are convenience policy.

They MUST NOT alter Skill contents or update semantics.

---

# 25. Registry model

Registry entries represent Installations rather than Profile installations.

Conceptual schema:

```json
{
  "schema": 1,
  "id": "installation-uuid",

  "library_id": "library-uuid",

  "workspace": "/absolute/path/to/project",
  "target": ".agents/skills",
  "adapter": "agents",

  "source_ref": "main",

  "profiles": [
    {
      "id": "profile-uuid-1",
      "name": "dev-core",
      "attached_at": "RFC3339"
    },
    {
      "id": "profile-uuid-2",
      "name": "rust-development",
      "attached_at": "RFC3339"
    }
  ],

  "last_applied": {
    "source_commit": "sha",

    "profile_commits": {
      "profile-uuid-1": "sha",
      "profile-uuid-2": "sha"
    },

    "skill_membership": {
      "git-workflow": [
        "profile-uuid-1"
      ],
      "testing": [
        "profile-uuid-1",
        "profile-uuid-2"
      ],
      "rust": [
        "profile-uuid-2"
      ]
    }
  },

  "installed_at": "RFC3339",
  "updated_at": "RFC3339"
}
```

---

# 26. Registry uniqueness

One Registry entry is uniquely keyed by:

```text
(workspace, target)
```

Exactly one Installation owns that pair.

That Installation may contain any number of attached Profiles.

Two independent Beskar Installations MUST NOT simultaneously own the same Target.

---

# 27. Last-applied membership

The Registry MUST retain the Skill-to-Profile membership map from the last successful reconciliation.

This snapshot answers:

```text
Why was this Skill installed?
```

and provides safe comparison when Profiles change.

Example:

```text
testing:
  previously required by:
    dev-core
    rust-development
```

The Registry snapshot is historical state.

The current Library Profiles remain authoritative for new desired state.

---

# 28. Profile names in Registry

Registry attachments MUST store:

* immutable Profile ID;
* last known Profile name.

The ID is authoritative.

The name is diagnostic/display metadata.

This allows:

```text
dev
```

to be renamed to:

```text
dev-core
```

without creating a new attachment.

---

# 29. Registry write safety

Registry mutation MUST use:

1. exclusive process/file lock;
2. temporary file;
3. write;
4. flush/sync as appropriate;
5. atomic rename where supported.

Partially written Registry files MUST NOT replace valid Registry state.

---

# 30. Registry repair metadata

For Git workspaces the Registry SHOULD additionally retain informational metadata:

* detected Git repository root;
* sanitized origin URL;
* repository identity information;
* HEAD at registration time.

This MAY assist repair after directory moves.

Credential-bearing Git URLs MUST be sanitized.

---

# 31. Installed Skill Stamp

Every managed Skill contains:

```text
.beskar.json
```

Example:

```json
{
  "schema": 1,
  "manager": "beskar",

  "installation_id": "installation-uuid",
  "library_id": "library-uuid",

  "skill": "testing",
  "source_ref": "main",

  "library_commit": "abc123...",
  "skill_commit": "def456...",

  "files": {
    "SKILL.md": {
      "sha256": "...",
      "executable": false
    },
    "scripts/check.sh": {
      "sha256": "...",
      "executable": true
    }
  }
}
```

---

# 32. Profiles are deliberately absent from Skill Stamps

A Stamp MUST NOT list Profile ownership.

Reason:

* the Stamp identifies **where the installed bytes came from**;
* the Registry identifies **why the Skill is installed**.

These are separate concerns.

Profile attachment changes SHOULD NOT require rewriting an otherwise unchanged Skill Stamp.

---

# 33. Hashing

Hashes MUST use SHA-256 over exact file bytes.

The Stamp itself is excluded from the file manifest.

Beskar MUST copy committed Git blob bytes exactly.

Beskar MUST NOT perform automatic line-ending conversion.

---

# 34. Executable metadata

Executable state SHOULD be preserved on POSIX.

The Stamp SHOULD store executable state independently of platform.

On Windows, executable-mode drift MAY be ignored while retaining canonical metadata.

---

# 35. Per-Skill source revision

`skill_commit` records the most recent Git commit at or before the resolved Source ref that touched the Skill directory.

`library_commit` records the exact snapshot used for the reconciliation.

These MAY differ.

---

# 36. Effective desired-state calculation

For every Installation, Beskar computes:

```text
Profile ID -> Profile definition
```

then:

```text
Skill name -> ordered set of requiring Profile IDs
```

Example:

```text
git-workflow:
  dev-core
  github

testing:
  dev-core
  rust-development

rust:
  rust-development
```

This membership map is a primary reconciliation artifact.

---

# 37. Duplicate Profile membership

The same Skill appearing in many Profiles is normal.

It MUST NOT produce:

* duplicate installation;
* warning;
* validation error.

It SHOULD be visible in status and UI.

---

# 38. Drift states

Beskar MUST support at least the following states.

## Current

Tracked files match the Stamp and the selected Library revision of the Skill has not changed.

---

## Outdated

Tracked files match the Stamp but a newer resolved Skill revision exists.

---

## Modified

At least one Stamp-tracked file:

* differs;
* is missing;
* has incompatible executable metadata where relevant.

---

## Extra

An additional untracked file exists inside the Skill directory.

Informational only.

---

## Gap

An attached Profile references a Skill absent from the resolved Library.

---

## Unstamped

The expected Skill directory exists but has no valid Beskar Stamp.

---

## Foreign

The directory contains a Stamp associated with:

* another Library;
* another Installation;
* another Skill identity;
* unsupported manager/schema.

---

## Profile-added

A Skill appears in current desired membership but not in the last-applied membership.

---

## Profile-removed

A Skill existed in last-applied membership but has no membership in current desired state.

---

## Membership-changed

The Skill remains required but its requiring Profile set changed.

Example:

```text
before:
  testing <- dev-core, rust

after:
  testing <- rust
```

No Skill file operation may be necessary.

---

## Orphaned-managed

A valid Beskar-stamped Skill exists inside a registered Target but belongs to neither:

* last-applied membership;
* current desired membership.

---

## Missing-target

The registered Target directory does not exist.

---

## Missing-workspace

The registered Workspace does not exist.

---

## Missing-ref

The Source ref can no longer be resolved.

---

## Missing-profile

An attached Profile ID cannot be found in the resolved Library revision.

---

# 39. Missing Profile safety

A missing attached Profile is a protected state.

Suppose the Registry contains:

```text
dev-core
rust-development
```

and `rust-development` cannot be found after synchronization.

Beskar MUST NOT interpret that as an empty Profile.

It MUST NOT automatically retire Skills previously owned by that Profile.

Instead the Installation becomes partially non-reconcilable:

```text
rust-development    missing-profile
```

Its last-applied membership MUST remain available for protection.

---

# 40. Explicitly detaching a missing Profile

The user MUST be able to explicitly detach a Profile that no longer exists in the Library.

For example:

```text
beskar remove rust-development .
```

Beskar may identify it by:

* current Profile name;
* last-known Registry name;
* Profile ID.

This explicit detach allows Beskar to calculate which previously owned Skills may now be retired.

---

# 41. Status

Syntax:

```text
beskar status [<workspace>]
beskar status --all
```

Status MUST:

* perform no writes;
* perform no network access;
* resolve all attached Profiles from local refs;
* compute current membership;
* compare against last-applied membership;
* inspect every managed Skill;
* identify extra files;
* identify broken registrations;
* identify orphaned managed Skills;
* identify missing Profiles;
* identify membership changes.

---

# 42. Status human output

Example:

```text
~/src/project
.agents/skills @ main

Profiles
  dev-core               8 skills
  rust-development       5 skills
  github                  3 skills

Effective set
  13 unique skills

✓ code-review          current       dev-core
✓ git-workflow         current       dev-core, github
↑ rust                 outdated      rust-development
! testing              modified      dev-core, rust-development
✓ github-pr            current       github

13 skills
3 profiles
1 outdated
1 modified
```

---

# 43. Explaining a Skill

Beskar SHOULD provide:

```text
beskar why <skill> [<workspace>]
```

Example:

```text
beskar why testing .
```

Output:

```text
testing is installed in .agents/skills because it is required by:

  dev-core
  rust-development

Source ref:
  main

State:
  current
```

This is especially useful with composed Profiles.

---

# 44. Update

Syntax:

```text
beskar update <workspace>
beskar update --all
```

Options:

```text
--force
--replace-unmanaged
--best-effort
--dry-run
--json
```

Update MUST use only locally available Library refs.

It MUST NOT fetch automatically.

---

# 45. Update planning

Before writing to one Installation, Beskar MUST:

1. resolve the Source ref to an exact commit;
2. resolve every attached Profile ID;
3. detect missing attached Profiles;
4. load every valid Profile;
5. build desired Skill membership;
6. compute the effective Skill union;
7. resolve each Skill from the same Library commit;
8. identify missing Skills;
9. inspect current Stamps;
10. inspect managed hashes;
11. identify Extra files;
12. compare current membership against last-applied membership;
13. classify additions;
14. classify removals;
15. classify membership-only changes;
16. identify destructive operations;
17. build a complete plan;
18. perform safety checks;
19. ask for confirmation where required;
20. execute.

No target writes occur before planning completes.

---

# 46. Example multi-profile reconciliation

Previous applied Profiles:

```text
dev:
  A
  B

rust:
  B
  C
```

Last-applied membership:

```text
A <- dev
B <- dev, rust
C <- rust
```

New Library definitions:

```text
dev:
  A
  D

rust:
  B
  E
```

New desired membership:

```text
A <- dev
B <- rust
D <- dev
E <- rust
```

Update plan:

```text
A    retain/update
B    retain/update; membership changed
C    retire; no Profile requires it
D    install
E    install
```

---

# 47. Modified-copy behavior

Managed local modifications MUST never be silently discarded.

Without `--force`, if reconciliation would overwrite or remove modified managed content, Beskar MUST block destructive execution.

The implementation SHOULD report every blocker in one planning pass.

Interactive `--force` MUST display the exact managed files whose local versions will be discarded.

In noninteractive operation, supplying `--force` constitutes explicit consent.

Extra files MUST remain preserved.

---

# 48. Best-effort updates

For large registries:

```text
beskar update --all --best-effort
```

MAY continue safe Installations while skipping blocked Installations.

Each Installation remains internally reconciled as one unit.

Beskar MUST NOT perform best-effort independently per Profile.

Overall exit status MUST indicate partial success.

---

# 49. Profile-added Skills

If a newly attached or modified Profile introduces a new Skill, Beskar installs it.

If an unmanaged same-name directory exists, reconciliation MUST refuse unless:

```text
--replace-unmanaged
```

is explicitly supplied.

`--force` MUST NOT automatically imply replacement of unmanaged content.

---

# 50. Profile-removed Skills

A Skill is Profile-removed only when current desired membership becomes empty.

Example:

Before:

```text
testing <- dev, rust
```

After removing `dev`:

```text
testing <- rust
```

`testing` MUST remain installed.

Only its membership changes.

If after removing `rust`:

```text
testing <- nobody
```

the Skill becomes eligible for retirement.

---

# 51. Skill retirement

For a clean managed Skill no longer required by any attached Profile:

1. remove Stamp-tracked files;
2. remove the Stamp;
3. preserve Extra files;
4. remove now-empty directories.

If Extras remain, the directory becomes unmanaged and remains on disk.

If tracked files were locally modified, retirement requires `--force`.

---

# 52. Detaching a Profile

Normative command:

```text
beskar remove <profile> <workspace>
```

Example:

```text
beskar remove dev-core .
```

This means:

> Detach the Profile from this Installation and reconcile the remaining attached Profiles.

---

# 53. Detach example

Current Profiles:

```text
dev:
  testing
  git
  review

rust:
  rust
  testing
```

Effective membership:

```text
testing <- dev, rust
git     <- dev
review  <- dev
rust    <- rust
```

Command:

```text
beskar remove dev .
```

Plan:

```text
testing    keep      still required by rust
git        retire    no longer required
review     retire    no longer required
rust       keep      required by rust
```

---

# 54. Detaching the final Profile

If `beskar remove` would detach the last Profile, Beskar SHOULD allow it but treat the resulting Installation as empty.

After safe reconciliation, Beskar SHOULD ask whether the empty Installation registration should also be removed.

For noninteractive deterministic behavior:

* detaching the last Profile leaves an empty Installation registered;
* `beskar unregister` removes the Installation record.

This avoids one command having two hidden meanings.

---

# 55. Unregistering an Installation

Provide:

```text
beskar unregister <workspace>
```

Options:

```text
--target <path>
--keep-files
--force
```

Default behavior:

* safely retire all Beskar-managed Skills;
* preserve Extra files;
* remove the Registry entry.

With:

```text
--keep-files
```

Beskar removes the Registry entry without touching target contents.

---

# 56. Changing an Installation source ref

Provide a first-class operation:

```text
beskar ref set <workspace> <ref>
```

Optional:

```text
--target <path>
--dry-run
```

This changes the Source ref for the entire Installation.

It MUST immediately compute the reconciliation implications before applying.

All attached Profiles move together.

---

# 57. Why per-Profile refs are forbidden in v1

Suppose:

```text
dev@main
```

expects:

```text
testing@abc123
```

while:

```text
rust@experimental
```

expects:

```text
testing@def456
```

One Target contains only one:

```text
testing/
```

directory.

Supporting this requires package-manager-style version conflict resolution.

Beskar v1 deliberately avoids that complexity.

Users requiring two incompatible refs SHOULD use separate Targets.

---

# 58. Library-deleted Skills

If an attached Profile still names a Skill absent from the resolved Library revision:

```text
state = gap
```

Beskar MUST NOT:

* invent the Skill;
* search the Internet;
* silently remove it from the Profile.

The Installation cannot fully converge.

Other safe actions MAY occur depending on plan policy, but the command MUST return non-zero.

---

# 59. File replacement algorithm

For an existing managed Skill update:

1. write incoming tracked files;
2. restore executable metadata where supported;
3. remove old Stamp-tracked files absent from the incoming manifest;
4. preserve Extra files;
5. write the new Stamp last.

Beskar MUST NOT recursively replace a managed Skill directory during normal update.

---

# 60. Add transaction semantics

When attaching a new Profile, Beskar MUST preflight the resulting entire Installation state.

If reconciliation encounters:

* missing required Skill;
* ambiguous Skill;
* invalid Skill;
* protected local modification;
* unmanaged collision;

Beskar MUST report all relevant blockers before writes.

A failed Profile attachment MUST NOT be persisted into the Registry unless reconciliation successfully completes.

---

# 61. Registry commit point

Registry desired state MUST correspond to successfully applied target state.

For Profile attachment/detachment:

1. build new desired state;
2. reconcile filesystem;
3. persist Registry attachment/membership changes last.

If the process crashes before Registry persistence, existing Stamps and subsequent status MUST permit diagnosis/recovery.

---

# 62. Fetch

Normative command:

```text
beskar fetch
```

Options:

```text
--remote <name>
--prune
--dry-run
--json
```

Default remote:

```text
origin
```

The operation MUST:

1. fetch remote refs;
2. fetch tags;
3. prune deleted remote tracking refs by default;
4. determine Library refs actively relevant to:

   * `default_ref`;
   * registered Installations;
   * currently checked-out branch;
5. safely fast-forward relevant local branches when possible.

Beskar MUST NOT merge or rebase.

---

# 63. Fetch branch rules

## Equal

No action.

## Local behind remote

Strict fast-forward permitted.

## Local ahead

Leave unchanged.

## Diverged

Leave unchanged and report divergence.

## Checked-out branch with incompatible dirty worktree/index

Fetch remote objects but do not change the local checked-out branch.

## Non-checked-out branch

May update only through strict fast-forward.

---

# 64. Multiple Installation refs and fetch

Different Installations MAY use different Source refs.

Example:

```text
project-A:
  main

project-B:
  stable

project-C:
  feature/new-skills
```

This is valid because they are separate Installations.

`beskar fetch` SHOULD consider all locally registered source branch refs relevant.

---

# 65. Push

Syntax:

```text
beskar push [<branch>]
```

Options:

```text
--remote <name>
--set-upstream
--allow-dirty
--dry-run
--json
```

Default branch is current Library branch.

Beskar MUST:

1. ensure branch exists;
2. determine remote tracking relationship;
3. determine remote state;
4. refuse non-fast-forward push;
5. push branch;
6. establish upstream when appropriate.

Beskar v1 MUST NOT perform unconditional force pushes.

---

# 66. Dirty Library and push

By default:

```text
beskar push
```

MUST refuse if the Library has uncommitted changes.

Users intentionally pushing only already-created commits may use:

```text
--allow-dirty
```

Beskar MUST NOT silently create a general-purpose commit during push.

---

# 67. Git authentication

Network Git operations SHOULD use the user's installed Git implementation.

Authentication is delegated to normal Git mechanisms:

* SSH agent;
* credential helper;
* Git Credential Manager;
* operating-system credential store;
* configured Git prompts.

Beskar MUST NOT persist:

* passwords;
* access tokens;
* private keys.

URLs containing credentials MUST be redacted.

---

# 68. Git backend abstraction

Domain code MUST depend on an abstraction conceptually equivalent to:

```rust
trait GitBackend {
    fn resolve_ref(...);
    fn tree(...);
    fn blob(...);
    fn last_commit_touching(...);
    fn status(...);
    fn fetch(...);
    fn push(...);
    fn commit_paths(...);
    fn move_path(...);
}
```

v1 SHOULD implement:

```text
SystemGitBackend
```

This isolates system-Git usage and permits a future pure-Rust backend.

---

# 69. Library mutation commands

At minimum:

```text
beskar ingest
beskar skill list
beskar skill show
beskar skill move
beskar skill rename
beskar skill remove
beskar skill tag
beskar skill rank

beskar profile list
beskar profile show
beskar profile create
beskar profile delete
beskar profile rename
beskar profile add
beskar profile remove
beskar profile move
beskar profile validate
```

Mutating Library commands MUST produce Git commits containing only operation-owned paths.

---

# 70. Ingest

Syntax:

```text
beskar ingest <skill-path> --bucket <bucket>
```

Options:

```text
--replace
--recursive
--dry-run
--json
```

Single-Skill ingest MUST:

* find `SKILL.md` directly in source;
* validate before copying;
* reject special filesystem objects;
* remove `.beskar.json`;
* strip recognized legacy installation Stamp files;
* preserve regular content;
* preserve executable metadata where supported.

---

# 71. Batch ingest

With:

```text
--recursive
```

Beskar MAY discover several Skill roots.

Before writing it MUST validate the complete incoming batch.

Duplicate incoming names are fatal.

Existing Library collisions require explicit resolution.

One batch ingest SHOULD result in one Git commit.

---

# 72. Git behavior of Library mutations

Before creating a Beskar-owned commit:

* inspect the Git index;
* refuse if unrelated staged files exist;
* leave unrelated unstaged files untouched;
* stage exactly operation-owned paths.

Example commit messages:

```text
beskar: ingest code-review
beskar: update code-review
beskar: move code-review
beskar: rename old-name to new-name
beskar: update profile dev-core
beskar: remove skill foo
```

Library mutations MUST NOT automatically push.

---

# 73. Skill move

Syntax:

```text
beskar skill move <skill> <bucket>
```

Because buckets do not affect identity:

* `SKILL.md` need not change;
* Profile membership does not change;
* Installation membership does not change.

---

# 74. Skill rename

Syntax:

```text
beskar skill rename <old-name> <new-name>
```

This is an identity migration.

It MUST atomically update:

* Skill directory leaf;
* `name:` in `SKILL.md`;
* Catalog metadata key;
* every Profile on the active Library branch referring to the old Skill.

The complete result MUST validate before commit.

Regex-only YAML mutation SHOULD NOT be used where it could corrupt formatting/content.

---

# 75. Skill removal

Syntax:

```text
beskar skill remove <skill>
```

If any Profile on the active branch references the Skill, default removal MUST refuse and list those Profiles.

Explicit:

```text
--cascade
```

MAY remove it from all such Profiles in the same Library commit.

Beskar cannot know remote machines' Registry state.

The command MUST therefore warn that synchronizing this change may alter external Installations.

---

# 76. Profile commands

Required:

```text
beskar profile list
beskar profile show <profile>
beskar profile create <profile>
beskar profile delete <profile>
beskar profile rename <old> <new>
beskar profile add <profile> <skill>...
beskar profile remove <profile> <skill>...
beskar profile move <profile> <skill> --before <skill>
beskar profile move <profile> <skill> --after <skill>
beskar profile validate [<profile>]
```

Profile mutations MUST create Git commits.

Profile rename MUST retain Profile UUID.

---

# 77. Deleting a Profile from the Library

Deleting a Profile is distinct from detaching it from an Installation.

Command:

```text
beskar profile delete <profile>
```

affects the Library.

Any Installation still attached to the deleted Profile will later classify it as:

```text
missing-profile
```

and protect its previously attributed Skills until explicit detach or Profile restoration.

---

# 78. Installation Profile management commands

`add` and `remove` are the primary shorthand.

The richer namespace SHOULD also exist:

```text
beskar installation profiles <workspace>
beskar installation attach <profile> <workspace>
beskar installation detach <profile> <workspace>
```

The exact aliases may be finalized before implementation.

Normative semantics remain:

```text
add    = attach + reconcile
remove = detach + reconcile
```

---

# 79. Profile attachment ordering commands

Optional v1 convenience:

```text
beskar installation profile-order <workspace>
```

and:

```text
beskar installation profile-order <workspace> \
  dev-core rust-development github
```

Changing attachment order affects presentation ordering only.

It MUST NOT modify physical Skill contents where effective membership is unchanged.

---

# 80. Listing and discovery

Provide:

```text
beskar skill list
```

Useful filters:

```text
--bucket
--tag
--profile
--query
```

Sort modes SHOULD include:

```text
name
bucket
rank
recent
```

Lexical search is sufficient for v1.

---

# 81. Library status

Provide:

```text
beskar library status
```

Show:

* Library path;
* Library ID;
* active branch;
* HEAD;
* dirty state;
* staged files;
* configured remote;
* default ref;
* local ahead/behind information from fetched refs;
* Source refs currently used by registered Installations.

No network access.

---

# 82. Library branch helpers

Beskar SHOULD provide:

```text
beskar library branch
beskar library branch create <name>
beskar library switch <name>
```

These are convenience wrappers only.

Users remain free to use ordinary Git commands in the Library.

---

# 83. Doctor

Provide:

```text
beskar doctor
```

Inspect:

* global configuration;
* Library existence;
* Git validity;
* `beskar.toml`;
* schema versions;
* duplicate Skills;
* invalid `SKILL.md`;
* duplicate Profiles;
* missing Profile Skills;
* Git executable;
* remote configuration;
* Registry parse validity;
* duplicate Installation targets;
* missing workspaces;
* missing targets;
* missing attached Profiles;
* invalid Source refs;
* stale locks where safely detectable.

Doctor MUST NOT perform destructive repair automatically.

---

# 84. Registry maintenance

Provide:

```text
beskar registry list
beskar registry show <id>
beskar registry prune
beskar registry repair
beskar registry move <id> <new-path>
```

`registry show` SHOULD expose:

* attached Profiles;
* attachment order;
* last-applied Skill membership;
* Source ref;
* last reconciled commit.

---

# 85. Global storage locations

Beskar MUST follow platform conventions.

Conceptually:

## Linux

Config:

```text
$XDG_CONFIG_HOME/beskar/
```

State:

```text
$XDG_STATE_HOME/beskar/
```

Data:

```text
$XDG_DATA_HOME/beskar/
```

## macOS

Use appropriate per-user Application Support/configuration locations.

## Windows

Use standard per-user AppData locations.

A mature Rust directory abstraction SHOULD be used.

---

# 86. Environment overrides

Support:

```text
BESKAR_HOME
BESKAR_LIBRARY
```

`BESKAR_HOME` overrides Beskar's platform directories for portable/test operation.

`BESKAR_LIBRARY` overrides the active Library path.

Environment values take precedence over persisted configuration.

---

# 87. First-run setup

Provide:

```text
beskar init
```

Supported flows:

## New Library

```text
beskar init
```

Creates:

* Git repository;
* `beskar.toml`;
* `catalog.toml`;
* `skills/`;
* `profiles/`;
* initial commit.

## Clone existing Library

```text
beskar init --remote <git-url>
```

Clone and validate.

## Adopt existing local Library

```text
beskar init --library <path>
```

Register an already valid Beskar Library.

Legacy `skill-manager` Homes require explicit migration.

---

# 88. Concurrency

Use advisory/process locking for:

* Registry writes;
* Library-mutating operations;
* each Installation Target during mutation.

Read-only operations SHOULD avoid exclusive locking where unnecessary.

Errors SHOULD identify:

* locked resource;
* owner process where discoverable;
* recovery guidance.

Beskar MUST NOT blindly remove locks.

---

# 89. Operation planning model

Every mutating domain operation MUST separate:

1. planning;
2. safety evaluation;
3. confirmation;
4. execution;
5. Registry/state finalization.

Operation plans MUST be serializable.

Conceptual action types include:

```text
AttachProfile
DetachProfile
InstallSkill
UpdateSkill
ChangeSkillMembership
RetireSkill
PreserveExtra
OverwriteModified
ReplaceUnmanaged
UpdateRegistry
CommitLibraryPaths
FastForwardBranch
PushBranch
```

---

# 90. Membership changes as first-class plan actions

Suppose:

```text
testing:
  before = [dev, rust]
  after  = [rust]
```

No filesystem update is required if content is otherwise current.

The plan SHOULD nevertheless expose:

```text
ChangeSkillMembership
```

so:

* dry-run;
* TUI;
* GUI;
* audit output

can explain the effect of Profile changes.

---

# 91. Dry-run

Meaningful mutating commands SHOULD support:

```text
--dry-run
```

Dry-run MUST execute all possible planning and validation without state mutation.

Example:

```text
beskar remove dev . --dry-run
```

could show:

```text
Detach profile:
  dev

Skills:
  testing    keep      rust still requires it
  git        retire
  review     retire
  rust       keep

No changes written.
```

---

# 92. CLI output contract

Human output goes to stdout.

Warnings/progress MAY use stderr.

`--json` MUST emit structured machine-readable output without interleaving prose on stdout.

JSON state identifiers are public API.

---

# 93. JSON representation of membership

A Skill result SHOULD include:

```json
{
  "name": "testing",
  "state": "current",
  "required_by": [
    {
      "profile_id": "uuid-1",
      "profile_name": "dev-core"
    },
    {
      "profile_id": "uuid-2",
      "profile_name": "rust-development"
    }
  ]
}
```

This enables agent automation without parsing terminal text.

---

# 94. Exit codes

Recommended v1:

```text
0  success
1  general operation failure
2  CLI usage error
3  action required / protected drift / divergence
4  partial success
5  invalid Beskar configuration or Library format
6  Git/network/authentication failure
7  resource locked / concurrent operation
8  unrecoverable filesystem/state error
```

Structured error codes SHOULD additionally appear in JSON mode.

---

# 95. TUI

Launch:

```text
beskar tui
```

Recommended implementation:

* Ratatui;
* Crossterm.

Primary screens:

* Dashboard;
* Skills;
* Profiles;
* Installations;
* Git;
* Activity.

---

# 96. TUI Dashboard

Display:

* Library;
* active branch;
* default ref;
* Git state;
* Profile count;
* Skill count;
* Installation count;
* total Profile attachments;
* outdated Installations;
* modified Installations;
* broken/missing Profiles.

---

# 97. TUI Skills

Recommended three-pane model:

1. bucket tree;
2. searchable Skill list;
3. metadata / `SKILL.md` preview.

Actions:

* ingest;
* move;
* rename;
* remove;
* tag;
* rank;
* add Skill to Profile.

---

# 98. TUI Profiles

Profile screen SHOULD allow:

* Profile creation;
* rename;
* deletion;
* Skill addition/removal;
* Skill ordering;
* showing which Installations currently attach that Profile locally.

---

# 99. TUI Installations

Each Installation view SHOULD display:

```text
Workspace
Target
Source ref

Attached Profiles
  dev-core
  rust-development
  github

Effective Skills
  ...
```

It SHOULD provide:

* attach Profile;
* detach Profile;
* reorder Profile presentation;
* change Source ref;
* preview reconciliation;
* update;
* unregister.

---

# 100. TUI skill-membership view

Selecting a Skill in an Installation SHOULD clearly show:

```text
testing

Required by:
  dev-core
  rust-development

Source:
  library/main
  skill commit abc123

State:
  current
```

This is important enough to be a first-class UI concept.

---

# 101. TUI Git

Show:

* Library branches;
* current branch;
* tracking branch;
* ahead/behind;
* fetch action;
* push action.

A merge-conflict editor is not required.

---

# 102. Desktop GUI

The desktop GUI SHOULD be a separate Rust application:

```text
beskar-gui
```

Recommended stack:

* `eframe`;
* `egui`;
* shared `beskar-core`;
* shared `beskar-git`.

---

# 103. GUI information architecture

Primary pages:

* Dashboard;
* Skills;
* Profiles;
* Installations;
* Git;
* Settings;
* Activity.

The Installations page MUST make multiple simultaneously attached Profiles visible and editable.

---

# 104. GUI attachment workflow

The GUI SHOULD allow users to:

1. open an Installation;
2. select **Add Profile**;
3. choose one or several Profiles;
4. preview effective membership changes;
5. inspect Skills that will be:

   * installed;
   * retained;
   * retired;
   * membership-only changed;
6. apply.

Detach uses the equivalent inverse workflow.

---

# 105. CLI/TUI/GUI interoperability

All interfaces operate on exactly the same:

* Library;
* Registry;
* Stamps;
* operation planner.

UI-specific state MUST NOT alter domain semantics.

---

# 106. Rust workspace architecture

Recommended:

```text
crates/
  beskar-core/
  beskar-git/
  beskar-cli/
  beskar-tui/
  beskar-gui/
  beskar-test-support/
```

---

# 107. `beskar-core`

Owns:

* domain models;
* Skill validation;
* Library scanning;
* Profiles;
* Profile attachment model;
* effective membership calculation;
* Catalog;
* Registry;
* Stamps;
* hashing;
* drift classification;
* reconciliation planning;
* operation execution interfaces;
* config;
* locking abstractions.

No CLI, terminal-rendering, or GUI logic.

---

# 108. Recommended core domain types

Conceptually:

```rust
struct LibraryId(Uuid);
struct InstallationId(Uuid);
struct ProfileId(Uuid);
struct SkillName(String);

struct Profile {
    id: ProfileId,
    name: String,
    description: Option<String>,
    skills: Vec<SkillName>,
}

struct ProfileAttachment {
    profile_id: ProfileId,
    last_known_name: String,
    attached_at: Timestamp,
}

struct Installation {
    id: InstallationId,
    library_id: LibraryId,
    workspace: PathBuf,
    target: RelativePath,
    adapter: Adapter,
    source_ref: GitRef,
    profiles: Vec<ProfileAttachment>,
    last_applied: LastAppliedState,
}

struct SkillMembership {
    skill: SkillName,
    required_by: Vec<ProfileId>,
}
```

Exact Rust syntax may vary.

The conceptual distinctions MUST remain.

---

# 109. Reconciliation types

Conceptually:

```rust
struct DesiredInstallationState {
    resolved_commit: CommitId,
    profiles: Vec<ResolvedProfile>,
    skills: BTreeMap<SkillName, DesiredSkill>,
}

struct DesiredSkill {
    source: ResolvedSkill,
    required_by: Vec<ProfileId>,
}

struct ReconciliationPlan {
    installation_id: InstallationId,
    profile_changes: Vec<ProfileChange>,
    skill_actions: Vec<SkillAction>,
    blockers: Vec<Blocker>,
}
```

This planner is the central abstraction consumed by CLI, TUI, and GUI.

---

# 110. `beskar-git`

Owns:

* `GitBackend`;
* system-Git implementation;
* branch relation calculations;
* committed tree/blob access;
* Git process execution;
* Git authentication integration.

---

# 111. `beskar-cli`

Owns:

* Clap command grammar;
* argument validation;
* human rendering;
* JSON rendering;
* exit-code mapping.

---

# 112. `beskar-tui`

Owns only:

* TUI application state;
* navigation;
* rendering;
* dialogs;
* invocation of core plans/actions.

---

# 113. `beskar-gui`

Owns only:

* graphical application state;
* windows/views;
* dialogs;
* graphical rendering;
* invocation of core plans/actions.

---

# 114. Recommended Rust libraries

Recommended categories:

* `clap`
* `serde`
* `serde_json`
* `toml`
* suitable YAML parser
* `sha2`
* `uuid`
* `time` or `chrono`
* `thiserror`
* `tracing`
* `directories`
* `tempfile`
* cross-platform locking library
* `walkdir`
* `ratatui`
* `crossterm`
* `eframe`
* `egui`

Exact versions belong in `Cargo.lock`.

---

# 115. Error model

Core errors MUST be typed.

Distinguish at least:

* configuration;
* schema;
* validation;
* path safety;
* Library;
* Profile;
* Profile attachment;
* Registry;
* drift conflict;
* lock;
* Git;
* remote authentication;
* filesystem I/O;
* unsupported state.

UI code MUST NOT classify errors by parsing text.

---

# 116. Logging

Use structured tracing.

Normal CLI output SHOULD remain concise.

Verbose diagnostic logging SHOULD be enabled through:

```text
-v
-vv
```

and/or:

```text
RUST_LOG
```

Credentials MUST be redacted.

---

# 117. Caching

Correctness MUST NOT depend on cache state.

Beskar MAY cache:

* parsed Skill metadata;
* Library Skill index;
* Profiles;
* membership calculation;
* content hashes.

Caches MUST be keyed by exact resolved Library commit.

Deleting caches MUST be harmless.

---

# 118. Security boundaries

Skills are untrusted content.

Beskar MUST NOT:

* execute Skill scripts during ingest;
* execute Skill scripts during validation;
* execute Skill scripts during installation;
* execute Skill scripts during update;
* interpret instructions inside `SKILL.md`;
* follow Skill-controlled symlinks;
* interpolate Skill text into shells.

Git hooks configured by the user's own Git environment MAY run during normal Git operations and SHOULD be documented separately.

---

# 119. Cross-platform path behavior

Serialized Skill-relative paths MUST use:

```text
/
```

Conversion to native path separators occurs only at filesystem boundaries.

Path comparison MUST NOT blindly assume case sensitivity.

Windows prefixes and macOS filesystem behavior require dedicated tests.

---

# 120. Migration from `skill-manager`

Provide:

```text
beskar migrate skm
```

Options:

```text
--home <path>
--registry <path>
--dry-run
```

Migration goals:

* preserve Skill Git history;
* preserve bucket layout;
* convert Profiles;
* convert Registry entries;
* convert Stamps where safe.

---

# 121. Legacy single-Profile Installation migration

The earlier `skill-manager` Registry represents one Profile per Installation.

Each legacy entry SHOULD migrate naturally into one Beskar Installation containing one Profile attachment.

Example legacy:

```text
workspace X
target .agents/skills
profile dev-default
```

becomes:

```text
Installation X/.agents/skills

Profiles:
  dev-default
```

---

# 122. Merging legacy registrations

If migration discovers multiple compatible legacy registrations resolving to the exact same:

```text
(workspace, target)
```

Beskar SHOULD merge them into one Installation with several attached Profiles only if all of these match:

* Library identity/source;
* target;
* compatible source revision/ref semantics.

If there is ambiguity, migration MUST stop and request explicit resolution rather than guess.

---

# 123. Profile migration

Every legacy Profile becomes a Beskar Profile with:

* generated UUID;
* same name;
* same Skill order where recoverable.

Profiles referencing missing Skills SHOULD still be migrated but marked invalid.

---

# 124. Legacy Stamp migration

Beskar SHOULD recognize legacy:

```text
.skm.json
```

during explicit migration.

If source identity and hashes can be confidently mapped, Beskar MAY convert it to:

```text
.beskar.json
```

without recopying the Skill.

Otherwise the Installation is marked for reinstall/reconciliation.

---

# 125. Testing strategy

The project MUST have comprehensive automated tests.

## Unit tests

Cover:

* paths;
* Skill names;
* frontmatter;
* Profile parsing;
* Profile ID stability;
* attachment ordering;
* effective union computation;
* membership calculation;
* Registry serialization;
* Stamp hashing;
* drift classification;
* reconciliation planning.

## Git integration tests

Use real temporary Git repositories.

Cover:

* commits;
* branches;
* tags;
* fetch;
* fast-forward;
* ahead;
* divergence;
* pushes;
* source ref resolution.

## Filesystem tests

Cover:

* Extra files;
* modified files;
* deleted tracked files;
* nested files;
* executable bits;
* unmanaged collisions;
* interrupted states.

## Migration tests

Use legacy fixture repositories.

## CI

Test:

* Linux;
* Windows;
* macOS.

---

# 126. Required multi-profile test matrix

At minimum:

```text
attach first profile -> installation created

attach second profile -> union installed

attach same profile again -> idempotent

two profiles share skill -> one physical copy

shared skill -> membership records both profiles

detach one of two profiles sharing skill -> shared skill remains

detach last owner of skill -> skill retired

detach profile with unique and shared skills -> only unique skills retire

profile adds skill -> skill installed

profile removes skill but another profile still owns it -> skill remains

profile removes skill from final owning profile -> skill retires

profile rename -> attachment follows UUID

profile deleted from Library -> missing-profile, no automatic retirement

explicit detach missing profile -> last-applied membership safely reconciled

adding profile with different --ref -> refuse

changing Installation ref -> all Profiles move together

Profile ordering change -> no unnecessary filesystem rewrite

membership-only change -> Registry updates, Stamp unchanged
```

---

# 127. Required drift/update test matrix

Also test:

```text
current -> no change

clean outdated -> updated

modified current -> blocked

modified outdated -> blocked

modified outdated + --force -> overwritten

extra file + current -> preserved

extra file + outdated -> updated, extra preserved

upstream deletes tracked file -> tracked file removed

unmanaged name collision -> blocked

unmanaged collision + --replace-unmanaged -> replaced

foreign Stamp -> blocked

Library ID mismatch -> blocked

missing target -> broken

missing Source ref -> blocked

branch fast-forward -> newer state available

diverged branch -> fetch reports divergence
```

---

# 128. Recovery behavior

If reconciliation is interrupted:

* fully completed Skills MAY have new Stamps;
* incomplete Skills MUST retain old or absent Stamps;
* Registry finalization may still reflect previous successful state;
* subsequent `status` MUST expose the discrepancy.

Rerunning reconciliation SHOULD be the normal recovery mechanism.

A separate hidden transaction log is not mandatory for v1.

---

# 129. Schema evolution

Schema versions MUST exist independently for:

* Library config;
* Catalog;
* Profiles;
* Registry;
* Stamps;
* JSON command output.

Older Beskar versions encountering unsupported newer schemas MUST fail closed.

Lossless local-state migrations MAY be automatic.

Committed Library-format migrations SHOULD be explicit Git commits.

---

# 130. JSON/API stability

Machine-readable output is a public API.

Top-level form SHOULD include:

```json
{
  "schema": 1,
  "command": "update",
  "ok": true
}
```

Stable identifiers include:

```text
current
outdated
modified
gap
unstamped
foreign
profile_added
profile_removed
membership_changed
missing_profile
missing_ref
missing_target
```

Human prose is not stable API.

---

# 131. Primary UX model

Normal synchronization flow:

```text
beskar fetch
beskar status --all
beskar update --all
```

Profile composition:

```text
beskar add dev-core .
beskar add rust-development .
beskar add github .
```

Result:

```text
Profiles:
  dev-core
  rust-development
  github
```

Detach:

```text
beskar remove github .
```

Beskar reconciles only Skills no longer needed.

---

# 132. Library editing flow

Example:

```text
git switch -c improve-rust-skills

beskar ingest ./cargo-workflow \
  --bucket languages/rust

beskar profile add rust-development cargo-workflow

beskar push improve-rust-skills
```

No automatic push occurs during ingest/Profile editing.

---

# 133. Desired TUI/GUI workflow

A non-CLI user should be able to:

1. see Library synchronization state;
2. browse Skills;
3. inspect metadata;
4. ingest Skills;
5. organize Skills;
6. create Profiles;
7. compose small reusable Profiles;
8. open a Workspace Installation;
9. attach several Profiles simultaneously;
10. see their effective union;
11. see which Profiles require each Skill;
12. preview attaching/detaching a Profile;
13. inspect drift;
14. update one or all Installations;
15. fetch;
16. push branches.

---

# 134. Branding

“Beskar” is a project/product name.

Expansion:

**Better Skill Arrangement**

Technical documentation SHOULD prominently use this expansion.

Because the name intentionally references Star Wars/Mandalorian terminology, public commercial branding, artwork, logos, slogans, and store distribution SHOULD receive appropriate trademark/IP review.

Beskar MUST NOT ship copied franchise artwork.

---

# 135. Locked architectural decisions

The following decisions are intentionally fixed by this specification.

A coding agent MUST NOT reinvent them during implementation.

1. Skills use the Agent Skills format.
2. Skill name is global identity within a Library revision.
3. Buckets are organizational only.
4. Profiles have immutable UUIDs.
5. Profiles are ordered Skill sets.
6. Profiles are independently composable.
7. One Installation may have multiple simultaneously attached Profiles.
8. One `(workspace, target)` corresponds to one Installation.
9. Attached Profiles form an ordered set.
10. The effective Skill set is the unique union of attached Profiles.
11. Each Skill records membership in all requiring Profiles.
12. There is one physical Skill copy per Target.
13. Detaching one Profile MUST NOT remove a Skill still required by another.
14. A Skill retires only when its desired membership becomes empty.
15. Profile membership lives in Registry state, not in Skill Stamps.
16. All attached Profiles share one Library.
17. All attached Profiles share one Source ref.
18. Per-Profile refs in one Installation are forbidden in v1.
19. One reconciliation plan is produced per Installation, never per Profile.
20. Missing attached Profiles are protected states.
21. Missing Profiles MUST NOT cause automatic Skill retirement.
22. Profiles are resolved by immutable ID.
23. Profile rename therefore does not break attachments.
24. Installs use committed Git state.
25. Updates never implicitly fetch.
26. Fetch may fast-forward but never merge/rebase.
27. Push never force-pushes in v1.
28. Managed modifications are protected.
29. Extra files are preserved.
30. Replacing unmanaged content requires separate explicit consent.
31. Stamps are written last.
32. Registry state is machine-local.
33. Library mutation creates Git commits.
34. Library changes are never automatically pushed.
35. Git network operations use an abstract backend.
36. v1's recommended backend delegates networking/authentication to system Git.
37. CLI, TUI, and GUI share one domain implementation.
38. Planning is separated from confirmation and execution.
39. Dry-run uses the same planner as real execution.
40. JSON output is a supported automation API.

---

# 136. Suggested implementation phases

## Phase 1 — Core data model

Implement:

* config;
* IDs;
* Library discovery;
* Git committed-object reading;
* Skill parsing;
* Profile parsing;
* Profile attachment types;
* effective membership calculation;
* Catalog;
* Registry;
* Stamp parsing;
* status classifier.

The multi-Profile model MUST be implemented from the beginning.

Do not build a single-Profile implementation and retrofit composition later.

---

## Phase 2 — Reconciliation engine

Implement:

* desired-state builder;
* membership comparison;
* reconciliation planner;
* Skill installation;
* Skill update;
* Skill retirement;
* attachment;
* detach;
* force semantics;
* unmanaged replacement policy;
* dry-run;
* JSON.

---

## Phase 3 — Registry lifecycle

Implement:

* `add`;
* `remove`;
* `unregister`;
* `why`;
* `status`;
* `update`;
* `update --all`;
* Source ref change;
* Registry repair.

---

## Phase 4 — Library editing

Implement:

* ingest;
* move;
* rename;
* remove;
* Profile operations;
* Catalog operations;
* scoped Git commits.

---

## Phase 5 — Remote Git

Implement:

* fetch;
* branch relationships;
* strict fast-forward;
* push;
* authentication/error handling.

---

## Phase 6 — Migration

Implement:

* skill-manager Home conversion;
* legacy Profile conversion;
* Registry conversion;
* Stamp conversion.

---

## Phase 7 — TUI

Build Ratatui interface entirely on the existing core planning APIs.

---

## Phase 8 — Desktop GUI

Build egui/eframe interface against the exact same APIs.

---

# 137. Definition of Beskar v1 complete

Beskar v1 is complete only when a clean machine can successfully perform this scenario:

1. clone a Beskar Library;
2. validate it;
3. list its Skills;
4. create several Profiles;
5. ingest Skills;
6. assign overlapping Skills to several Profiles;
7. commit the changes;
8. push a branch;
9. create a Workspace Installation with `dev-core`;
10. attach `rust-development` to the same Target;
11. attach `github` to the same Target;
12. confirm overlapping Skills exist physically only once;
13. confirm Beskar can explain all requiring Profiles for each shared Skill;
14. change one Profile so that a shared Skill is removed from it;
15. update and confirm the Skill remains because another Profile requires it;
16. detach the final Profile requiring that Skill;
17. confirm the Skill is safely retired;
18. preserve Extra local files during that retirement;
19. change a Library Skill;
20. fetch/fast-forward on another machine;
21. detect Installations as outdated;
22. update all with `beskar update --all`;
23. refuse to overwrite locally modified managed files;
24. show exact destructive changes;
25. update after explicit force;
26. detect a deleted attached Profile as `missing-profile`;
27. preserve that Profile's previously attributed Skills;
28. explicitly detach the missing Profile;
29. reconcile safely;
30. survive a moved or deleted registered Workspace;
31. expose all Profile composition and membership state in the TUI;
32. expose the same behavior in the desktop GUI.

That behavior is the normative Beskar v1 product contract.

---

# 138. Core mental model

The simplest correct description of Beskar is:

```text
Library
  contains Skills
  contains Profiles

Profile
  names Skills

Installation
  attaches Profiles
  selects one Library ref

Desired state
  = union of all attached Profiles

Registry
  remembers attachments
  remembers last applied membership

Stamp
  remembers installed Skill bytes

Reconciliation
  makes the Target converge toward desired state
  without destroying data Beskar does not own
```

Or, more compactly:

```text
Profiles answer:
  "Why should this skill be here?"

The Library answers:
  "What should this skill contain?"

The Stamp answers:
  "What did Beskar put here?"

The Registry answers:
  "What is this installation supposed to be?"

The reconciler answers:
  "What needs to change now?"
```

This separation is foundational to Beskar.


# Skill management

The user's curated skills are copied into the workspaces that need them.
Beskar tracks what it copied, so it can tell library changes apart from
local edits.

## Language

### Desired state

**Library**:
The user's portable, canonical collection of skills and profiles. It is never an agent skill directory itself.
_Avoid_: store, repository, catalog

**Skill**:
A directory in the library's `skills/`, identified by its directory name (its id). It usually contains a `SKILL.md`.
_Avoid_: plugin, package

**Profile**:
A named set of skill ids stored in the library, describing a use case.
_Avoid_: bundle, preset, group

**Effective skills**:
The union of the skills of every profile enabled in a workspace, with duplicates collapsed.

### Deployment state

**Registry**:
Machine-local record of which workspaces Beskar manages, their enabled profiles, and what was installed there.
_Avoid_: database, state file

**Repository**:
A registered workspace directory. It does not have to be a Git repository.
_Avoid_: project, target

**Skills directory**:
Where a repository's skills are materialized, `.agents/skills` by default.

**Installed skill**:
A skill Beskar copied into a skills directory, recorded with the fingerprint it had.

### Reconciliation

**Fingerprint**:
A SHA-256 over a skill directory's paths, contents, executable bits and symlink targets.
_Avoid_: hash, checksum, version

**Plan**:
Every skill's state in one repository, computed from the library, recorded and workspace fingerprints without touching files.

**Drift**:
A workspace copy whose fingerprint differs from the recorded one, meaning it was edited locally.
_Avoid_: dirty, changed

**Conflict**:
A state where going ahead would lose local edits, or where a directory Beskar didn't create is in the way. It needs a **resolution**: keep, replace or promote.

**Promote**:
Copy a workspace's version of a skill into the library, making it canonical.
_Avoid_: push, upstream, publish

**Untracked**:
A directory in the skills directory that Beskar did not install. Beskar never modifies it.

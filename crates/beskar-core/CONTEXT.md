# Skill management

Beskar's core: one curated library of agent skills, profiles that group them, and reconciliation that gives every managed workspace copies of exactly the skills its profiles name.

## Language

### Sources

**Library**:
The user's portable, curated collection of skills and profiles; every installation is copied from it.
_Avoid_: store, catalog, repository (for the library)

**Skill**:
A directory of files that gives an agent a capability, usually with a `SKILL.md`.
_Avoid_: plugin, tool, extension

**Skill name**:
A skill's identity: its directory name, made of lowercase letters, digits and hyphens.
_Avoid_: slug, skill id (in user-facing text)

**Profile**:
A named set of skills for a use case, such as coding or research.
_Avoid_: bundle, preset, group

### Deployment

**Workspace**:
A directory whose skills Beskar manages. The command line calls it a repository (`beskar repo`), but it need not be a Git repository.
_Avoid_: project, checkout

**Registry**:
The machine-local record of workspaces, their enabled profiles and their installations.
_Avoid_: state file, database, index

**Enabled profile**:
A profile a workspace should receive the skills of.
_Avoid_: active profile, applied profile

**Wanted skill**:
A skill that at least one enabled profile of a workspace includes.
_Avoid_: active skill, desired skill

**Skills directory**:
The directory inside a workspace that holds its skills, `.agents/skills` by default; what agents read.
_Avoid_: agent directory

**Installation**:
A copy of a library skill that Beskar put in a workspace and recorded in the registry.
_Avoid_: deployment, link

**Unmanaged skill**:
A skill directory in a skills directory that Beskar did not install and no enabled profile wants; Beskar leaves it alone.
_Avoid_: foreign skill, local skill

**Release**:
To stop managing a skill and leave its directory where it is: an unwanted copy that holds files that are not part of the skill, or an orphaned copy the person keeps.
_Avoid_: forget (which drops the record of a copy that is already gone), abandon

### Drift

**Fingerprint**:
A digest of a skill's files, their paths and executable bits; equal fingerprints mean equal skills.
_Avoid_: hash, checksum, version

**Recorded base**:
The fingerprint of the library version an installation is based on, kept in the registry.
_Avoid_: installed hash, source fingerprint

**Local change**:
A difference between an installation and its recorded base, made in the workspace.
_Avoid_: drift (on its own), modification, dirty copy

**Library change**:
A difference between the library version of a skill and an installation's recorded base.
_Avoid_: upstream change

**Kept version**:
A library version the person chose not to take for one workspace, keeping the local copy; Beskar asks again only when the library moves past it. The recorded base stays.
_Avoid_: skipped version, ignored version

**Conflict**:
A skill whose update would lose a local change: changed in both places (diverged), a directory Beskar did not install in the way (untracked), or changed and no longer wanted (orphaned).
_Avoid_: merge conflict, clash

**Blocked skill**:
A skill whose update or removal would delete something that is not part of it, such as the copy's own `.git` or a file the user's ignore patterns name; it waits until the user clears the way.
_Avoid_: locked skill, stuck skill

**Resolution**:
The decision that settles one conflict: keep, replace or promote.
_Avoid_: strategy

**Conflict policy**:
How an update settles its conflicts: ask, keep, replace or abort.
_Avoid_: mode

**Promote**:
To copy an installation into the library, making it the version every workspace gets.
_Avoid_: push, upload, publish

**Restore**:
To replace an installation with the library version, discarding local changes.
_Avoid_: reset, revert

### Reconciliation

**Reconciliation**:
Bringing a workspace's skills directory in line with its wanted skills without losing local changes.
_Avoid_: sync, deploy

**Plan**:
The steps a reconciliation would take in one workspace, computed before anything changes.
_Avoid_: diff, changeset

**Update**:
Carrying out a plan, settling its conflicts by resolution.
_Avoid_: apply, sync

### Operations

**Operation**:
One thing a person can ask Beskar to do, a method of the core that returns a report of what it found or did. Front ends parse, ask and render; the rules live here.
_Avoid_: use case (in code), command (the command line's word for it)

**Transaction**:
One change to the library, the registry or a workspace, made while holding the lock, on a registry read fresh, and saved only if it changed.
_Avoid_: session, batch

**Preview**:
What an operation would discard (local changes, a skill, a profile), shown to the person before the operation acts; the operation refuses if things changed since.
_Avoid_: confirmation (that is the person's answer)

**Leftover**:
A temporary entry a run left behind because it was interrupted halfway; the next transaction recovers it.
_Avoid_: junk, orphan

**Work directory**:
The `.beskar` directory inside a skills directory where copies are assembled, moved aside and deleted, one level below where agents look for skills.
_Avoid_: temp dir, staging area

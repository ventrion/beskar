# Core

Beskar keeps one curated collection of agent skills and installs the right ones into each repository. This context is the model behind that: what a skill, a profile and a repository are, and how a repository is brought in line with what it should have.

## Language

### What the user owns

**Library**:
The user's curated global collection of skills and profiles, and the source of truth for both. It is never a place agents read from.
_Avoid_: store, catalog, collection

**Skill**:
The smallest reusable capability, held as a directory in the library and known by its directory name.
_Avoid_: plugin, package, extension

**Profile**:
A named set of skills that stands for a use case, such as coding or research. It lists skills by name and never copies them.
_Avoid_: bundle, preset, group, template

### Where skills go

**Repository**:
A local directory Beskar installs skills into. It does not have to be a Git repository.
_Avoid_: workspace, project, checkout

**Skills directory**:
The directory inside a repository that agents read, `.agents/skills` by default.
_Avoid_: agent directory, install path

**Enabled profile**:
A profile a repository is meant to have. Enabling changes what the repository should have and touches no files.
_Avoid_: active profile, applied profile

**Wanted skills**:
The union of the skills of a repository's enabled profiles, with each skill counted once.
_Avoid_: effective skills, desired set

**Installed skill**:
A copy of a library skill that Beskar placed in a repository's skills directory and recorded in the registry.
_Avoid_: deployed skill, materialized skill, synced skill

**Unmanaged skill**:
A directory in a skills directory that Beskar did not install. Beskar never removes or overwrites one without a decision.
_Avoid_: foreign skill, external skill

**Registry**:
The machine-local record of which repositories exist, which profiles each has enabled, and which skills Beskar installed in each. It holds absolute paths, so it stays out of the library.
_Avoid_: database, state file, manifest

### Keeping them in line

**Fingerprint**:
A content hash of a skill directory. Comparing fingerprints is how Beskar tells one version of a skill from another.
_Avoid_: checksum, digest, version

**Local drift**:
An installed skill whose files differ from what Beskar installed. Drift alone does not stop an update, and an update leaves a drifted skill as it is.
_Avoid_: dirty, modified copy, divergence

**Library change**:
The library's version of a skill differs from the one recorded at install time.
_Avoid_: upstream change, new version

**Reconciliation**:
Comparing the wanted skills with what a skills directory holds and making the second match the first.
_Avoid_: sync, deploy, apply

**Plan**:
What reconciliation would do, worked out without changing anything. A dry run shows a plan.
_Avoid_: diff, preview, changeset

**Update**:
Carrying out a plan for one repository, or for every repository.
_Avoid_: sync, refresh, install

**Conflict**:
A step of a plan that would destroy local work: overwriting a drifted skill the library also changed, removing a drifted skill nobody wants any more, or replacing an unmanaged directory. A conflict always needs a decision.
_Avoid_: clash, collision

**Resolution**:
How a conflict is settled: keep the local copy, replace it with the library's version (or delete it), or promote it.
_Avoid_: choice, strategy

**Conflict policy**:
The rule that settles conflicts without asking: ask, abort, keep or replace. On a terminal `ask` prompts. Anywhere else the default is `abort`, so nothing is guessed.
_Avoid_: merge strategy

**Promotion**:
Copying a repository's locally modified skill into the library so it becomes the canonical version, and every other repository can pick it up.
_Avoid_: push, upstreaming, publishing

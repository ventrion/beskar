# Beskar core

Manages one curated library of agent skills and installs into each repository only the skills that repository needs.

## Language

**Library**:
The user's curated global collection of skills and profiles. The source of truth for everything Beskar installs. It holds no machine-specific state.
_Avoid_: catalog, store, repository (a repository receives skills)

**Skill**:
The smallest reusable capability: a directory in the library, identified by its **skill id**, which is the directory's name.
_Avoid_: plugin, package, tool

**Profile**:
A named set of skills that describes a use case, kept as one file in the library.
_Avoid_: bundle, preset, group

**Repository**:
A local folder managed by Beskar. It does not have to be a Git repository.
_Avoid_: project, workspace (as a noun for the folder)

**Skills folder**:
The folder inside a repository where skills are installed, `.agents/skills` unless the settings say otherwise.
_Avoid_: target, install directory

**Enabled profile**:
A profile switched on for a repository. Enabling records what is wanted and changes no files.
_Avoid_: active profile, attached profile

**Effective skills**:
The union of the skills of a repository's enabled profiles. A skill in several profiles is installed once.
_Avoid_: resolved set, desired list

**Installed skill**:
A copy of a library skill that Beskar placed in a repository's skills folder and recorded in the registry. What is in the folder now is its **workspace copy**.
_Avoid_: deployment, materialization

**Registry**:
The machine-local record of repositories, their enabled profiles and their installed skills. It holds absolute paths, so it lives outside the library.
_Avoid_: database, manifest, lockfile

**Home**:
The folder that holds Beskar's own files: settings, registry and, by default, the library.
_Avoid_: config directory, data directory

**Fingerprint**:
A digest of a skill directory's file names, contents and executable bits. Equal fingerprints mean equal skills.
_Avoid_: hash, checksum, version

**Noise**:
Files that tools leave inside a skill folder and that are not part of the skill: `__pycache__`, `*.pyc` and `.DS_Store`. No fingerprint sees them.
_Avoid_: junk, ignored files

**Recorded fingerprint**:
The fingerprint stored when Beskar installed a skill. Installs are exact copies, so it is both the library's version at that moment and the workspace copy's.
_Avoid_: installed hash, source hash

**Reconciliation**:
Bringing a repository's skills folder in line with its effective skills. The **plan** lists what it would do; **update** carries it out.
_Avoid_: sync, deploy, apply (the plan is applied by an update)

**Clean**, **library changed**, **local drift**, **diverged**:
The states of an installed skill. Clean: workspace copy and library agree. Library changed: the library moved on and the copy is untouched. Local drift: the copy was edited. Diverged: both changed.
_Avoid_: dirty, stale, modified

**Conflict**:
A skill whose workspace copy holds content Beskar does not recognise, so overwriting or removing it needs a decision. Either the copy was edited, or the folder was never installed by Beskar and differs from the library.
_Avoid_: collision, merge conflict

**Conflict policy**:
The rule that decides conflicts when nobody is asked: `ask`, `fail`, `keep` or `replace`.
_Avoid_: strategy, mode

**Promotion**:
Copying a workspace copy into the library as the skill's new canonical version.
_Avoid_: push, publish, upload

**Unmanaged folder**:
A folder in the skills folder that Beskar did not install and no profile asks for. Beskar never touches it.
_Avoid_: foreign skill, orphan

**Adoption**:
Starting to track a folder that Beskar did not install but that is identical to the library's version. No files change.
_Avoid_: import (importing adds to the library)

**Import**:
Copying a skill into the library from outside it.
_Avoid_: ingest, install

# Beskar core

The user's curated skills and their installation into local repositories.

## Language

**Library**:
The portable canonical collection of skills and profiles.
_Avoid_: registry, workspace

**Skill**:
An identified directory of reusable agent instructions and supporting files.
_Avoid_: plugin, package

**Profile**:
A named set of skills expressing a use case.
_Avoid_: installation, bundle

**Skill metadata**:
Beskar's library record about one skill, kept outside the skill directory.
_Avoid_: manifest, frontmatter

**Dependency**:
A library skill that another skill's metadata requires. It is desired wherever a skill that requires it is desired.
_Avoid_: prerequisite, profile entry

**Repository**:
A local workspace where enabled profiles determine which skills should be installed. Git is optional.
_Avoid_: library

**Registry**:
The machine-local record of repositories, enabled profiles, and installed fingerprints.
_Avoid_: library, source of content

**Installed skill**:
A copied library skill whose ownership and last installed fingerprint Beskar records.
_Avoid_: symlink, canonical skill

**Local drift**:
A change or deletion in an installed copy relative to its recorded fingerprint.
_Avoid_: library update

**Divergence**:
Local drift accompanied by a change to the canonical library skill.
_Avoid_: clean copy

**Unmanaged skill**:
A workspace copy whose ownership Beskar has not recorded.
_Avoid_: implicitly adopted skill

**Reconciliation**:
Bringing installed skills into agreement with enabled profiles while preserving unresolved local work.
_Avoid_: Git synchronization

**Promotion**:
Making a tracked workspace copy the canonical library skill.
_Avoid_: network push

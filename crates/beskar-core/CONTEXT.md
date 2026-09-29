# Core: glossary

The domain model. No terminal, no prompts; everything returns data.

## State layers

- **Library** — the user's curated, portable collection: `skills/` and `profiles/` under one root with a `library.slate` marker. The source of truth for skill content. Never itself an agent skills directory.
- **Registry** — machine-local deployment state: which repositories exist, which profiles each enables, what was installed and when. Lives outside the library because it holds absolute paths.
- **Workspace** (materialised state) — a repository's `.agents/skills/` directory: the copies agents actually read.

## Entities

- **Skill** — a directory, identified by its directory name. `SKILL.md` front matter may supply a display name and description but is optional.
- **Profile** — a named list of skill names in `profiles/<name>.slate`. Describes intent ("coding"), not an installation. Profiles reference skills by name; they never copy skill content.
- **Repository** (repo) — a registered workspace root. Any directory; Git is irrelevant. Holds *enabled profiles*, *installed records* and a *synced* timestamp.
- **Installed record** — for one skill in one repository, the fingerprint of the content Beskar last wrote there.
- **Effective skills** / **resolved** — the union of the skills in a repository's enabled profiles, with the profiles that asked for each. Duplicates collapse.
- **Home** — `$BESKAR_HOME` or `~/.beskar`; where `config.slate` and (by default) the library and registry live.
- **Conflict policy** — what `update` does when it meets local modifications: `ask`, `keep`, `replace`, `fail`.

## Fingerprints

- **Fingerprint** — SHA-256 over a directory tree (sorted relative paths, executable bit, size, bytes). Copies have equal fingerprints.
- **L / R / W** — library, recorded, workspace fingerprints of one skill. `W != R` means a local modification. `R != L` means the library moved on.

## Reconciliation

- **Plan** — the full comparison of desired and actual state for one repository: one *plan item* per skill that is desired, recorded or present. Computing a plan changes nothing; `repo status` is a printed plan.
- **Action** — what a plan item calls for: `Unchanged`, `Add`, `Update`, `Remove`, `Adopt`, `Forget`, `Modified`, `MissingInLibrary`, `Unmanaged`, `Conflict`.
- **Adopt** — a workspace copy that already equals the library but has no record; Beskar records it instead of copying.
- **Forget** — a record whose directory is gone and whose skill is no longer wanted; drop the record.
- **Modified** — edited locally while the library did not change. Kept and reported; not a conflict because nothing wants to overwrite it.
- **Unmanaged** — a directory in the skills dir that Beskar never installed and no profile wants. Never touched, always listed.
- **Conflict** — a local modification that an update or removal would destroy: `Diverged` (both sides changed), `Untracked` (present without a record and unlike the library), `RemoveModified` (unwanted but edited).
- **Resolution** — the answer to a conflict: `KeepLocal`, `UseLibrary`, `Promote`, `Abort`. All conflicts are resolved before anything is applied.
- **Promote** — copy the workspace version into the library, making it canonical. Other repositories then see an `Update`.
- **Drift** — the umbrella term for `Modified` and the conflicts: workspace differs from what Beskar installed.

Avoid: "sync" as a verb for reconciliation (it suggests two-way merging), "link"/"symlink" (skills are copied), "install" for the library side (skills are *imported* into the library and *installed* into repositories).

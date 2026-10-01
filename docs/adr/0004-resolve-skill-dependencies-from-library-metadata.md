# Resolve skill dependencies from library metadata at plan time

Some skills stop working without other skills. Beskar records these requirements in `LIBRARY/metadata/NAME.bsk` and adds the required skills when it calculates a workspace's desired set. The skill directory stays untouched. A record inside it would change the skill's fingerprint, report every installed copy as library-changed, and copy Beskar's bookkeeping into workspaces where agents read it. Profiles stay untouched too. If `profile add` wrote dependencies into the profile, they would stay there after the requirement went away, and Beskar could no longer tell a skill the user chose from one that only came along. The registry stores no reason for an installation; the library is the only source of desired state.

## Considered options

- Frontmatter in `SKILL.md`. Rejected for the fingerprint and copy reasons above, and because Beskar treats skills as opaque directories.
- Expanding dependencies when a profile is edited. Rejected for the removal reason above.
- One `dependencies.bsk` file for the whole library. It works, but every edit touches the same file, which makes Git conflicts more likely, and removing a skill must edit a shared file.
- Dependency records in the registry. Rejected because the registry is machine-local and requirements must travel with the library.

## Consequences

Cycles are allowed. The result is a set and one transaction installs every copy, so order does not matter. There are no version constraints, because the library holds one version of each skill. A plan fingerprints every metadata file it reads, including absent ones, so a requirement added after a preview blocks the apply.

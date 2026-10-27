# Pull remote collections through Beskar-owned clones

Users want curated skills from Git repositories — their own collections on other machines, or an organization's shared set — without giving up the locally maintained library. Beskar records each source as a remote in `HOME/remotes.bsk`, clones it under `HOME/remotes/NAME/`, and imports selected skills and profiles into the library. The library stays the single source for deployments; workspaces never read clones.

A collection is library-shaped: a `skills` directory, and optional `profiles` and `metadata` directories. That symmetry means a user's own library can be published as a collection by pushing its content, and an organization can review skills as ordinary files. A `--path` option tracks one folder inside a larger repository.

## Considered options

- Making the library itself a Git clone. Works today and needs no code, but it couples personal synchronization to one repository and offers nothing for subscribing to a third party's subset. Chosen instead: remotes layer on top of a local library; users who want whole-library sync can still point Git at the library themselves.
- Reading upstream state through fingerprints without cloning. Rejected: comparing content requires the bytes, and re-downloading per skill is slower and rate-limited than one local clone that `beskar remote where` also exposes for contributing back.
- Syncing straight into workspaces, skipping the library. Rejected: the library is the only source of desired state, and profiles, requirements, and update flows all read it.
- Git bindings in `beskar-core`. Rejected: the core crate is process-free by design. `beskar-cli` owns every `git` invocation; the core sees only filesystem paths, so it stays testable offline and dependency-free. `doctor` reports a missing `git` as a problem only when remotes exist.

## Consequences

Sync reuses the workspace decision table with recast roles: baseline is the fingerprint the remote last synced, current is the library copy, desired is the collection. The states and `--conflict keep|replace|abort` policies mean the same thing one level up. A tracked unit is the skill directory plus its metadata file, compared as one composite fingerprint, so an upstream requirements change syncs with the skill and a local metadata addition is drift. Ownership is never inferred from equal bytes: tracking a name the library already owns is refused, and a profile colliding with a local one is a conflict, not an overwrite. Upstream removals honor the library's guards — profile references, requirements, and enabled workspaces block them. Import is one transaction per remote that also rewrites its record; the clones are cache and can be deleted and re-cloned, but they hold no state that `remotes.bsk` cannot rebuild. Sync is pull-only; contributing back is a manual `git` workflow from the clone path, by decision, to keep auth and merge concerns out of v1.

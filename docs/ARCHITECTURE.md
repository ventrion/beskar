# Architecture

Beskar separates the file format, domain operations, and terminal interface into three crates. Dependencies point toward the format crate. All three build offline without third-party dependencies.

```text
beskar-cli ──> beskar-core ──> bsk
```

| Crate | Owns | Does not own |
| --- | --- | --- |
| `bsk` | Record syntax, escaping, source line locations, lossless singleton edits | Skill identities, filesystem layout, reconciliation |
| `beskar-core` | Library, profiles, registry, locks, fingerprints, plans, transactions, diffs | Standard streams, prompts, JSON, command-line arguments |
| `beskar-cli` | Command specification, validation, help, prompts, text and JSON rendering | Deployment writes, registry updates, drift decisions |

`Beskar` is the core's session interface. It acquires locks and exposes operations such as `prepare_imports`, `select_profiles`, `plan`, `apply`, and `promote`. Storage and transaction modules are private. A caller receives a plan it can inspect and resolve through explicit conflict choices; it cannot replace its paths, fingerprints, or next registry state. There are no filesystem abstraction traits because only the local filesystem is implemented.

## State and reconciliation

The library and profiles define desired content. The registry records enabled profiles and the last installed fingerprints. Repository files are observed materialized state. None of these substitutes for another.

A pure decision table classifies the recorded, current, and desired fingerprints. Conflict policy is applied separately. Missing tracked copies count as drift, and unrelated destinations remain unmanaged under every policy. Keeping a copy retains the old baseline, so a later run still sees its drift. `synced` does not advance for a kept conflict or an unchanged update.

Planning reads state and changes nothing. Applying revalidates the configuration, repository record, profiles, sources, and observed copies before staging. It also checks read observations at commit, including skills that need no replacement. This prevents a stale preview from silently refreshing the baseline of a newly edited copy.

The CLI can resolve individual conflicts before calling `apply`. It collects every decision before making a filesystem change. Promotion is a separate transaction, so aborting an update cannot leave an earlier prompt's promotion applied.

## Filesystem changes and recovery

One transaction includes every selected repository and the registry. The journal records each staging root before Beskar creates it on the destination filesystem. Recovery of this phase only removes recorded roots, including incomplete copies. Once replacements have been staged and fingerprinted, Beskar atomically replaces the staging journal with the apply journal. The apply journal records missing destination directories and every replacement before targets change. Originals move to backups, are checked again after moving, and remain available until all targets have been installed.

Ordinary failures roll back partial work. `doctor --recover` rolls back an interrupted partial transaction or completes cleanup when every target already has its expected new content. It refuses to discard targets or backups changed after interruption. Configuration changes use the same transaction mechanism; recovery can locate a config file in its backup and acquire the old and new state locks before restoring it.

Kernel locks cover the home, library, registry and registered repositories. Lock files remain in place and are never unlinked, which avoids two processes locking different inodes under the same name. Locks release on process exit. Sessions fail promptly when another process holds a required lock; interactive conflict resolution keeps those locks until the session closes. This trades concurrent throughput for predictable updates.

Locks coordinate Beskar processes. Editors do not participate in them. Fingerprint checks narrow editor races and preserve detected edits, but this is not a filesystem snapshot or a defense against a malicious process changing paths continuously.

## Format and content choices

The versioned record grammar uses quoting only when values need it. Paths containing whitespace, quotes, backslashes, `#`, or line breaks round-trip without relying on shell expansion. Config and profile edits preserve comments and existing line endings. The registry is generated bookkeeping and is rewritten deterministically.

Skills are opaque directories. Copies and fingerprints include every regular file and empty directory, including `.git` metadata and generated files. This avoids the ambiguity of ignoring content during drift detection and then deleting it during replacement. Symlinks and special files are rejected. No Git repository, classifier, or metadata schema is required.

## Verification

CLI tests run the binary in isolated homes. Core tests exercise transaction failure points and the complete small fingerprint state space. Tests cover rollback, interrupted recovery, stale plans, shared resources across homes, unchanged and kept copies, executable bits, binary content, Unicode paths, unmanaged destinations, and configuration changes. The bounded diff and JSON encoder have their own tests.

The source comparison is in [CONSOLIDATION.md](CONSOLIDATION.md). Format details are in [FORMAT.md](FORMAT.md).

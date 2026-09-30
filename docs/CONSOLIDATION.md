# Consolidation review

The nine open source PRs were inspected at the revisions below. Each original test suite was run with `cargo test --workspace --offline`; all passed. Counts include doctests and reflect the checked-out code, which sometimes differs from the PR description. Test counts were evidence of coverage, not a ranking criterion.

| PR | Reviewed revision | Passing tests | Useful design | Decision |
| --- | --- | ---: | --- | --- |
| [#2](https://github.com/ventrion/beskar/pull/2) | `32be5376f62d` | 83 | Broad command coverage and a pure planner separated from application. | Keep the complete brief workflow; move prompting and presentation out of reconciliation. |
| [#3](https://github.com/ventrion/beskar/pull/3) | `52488188a1a1` | 26 | Strict ownership, JSON output, staged replacements and recovery records. | Keep the explicit ownership rule and machine-readable output; use the more complete transaction engine from #10. |
| [#4](https://github.com/ventrion/beskar/pull/4) | `434a099d3947` | 6 | A compact implementation with verified move-aside replacements. | Recheck originals after moving them into backup; retain the compact format and filesystem approach. |
| [#5](https://github.com/ventrion/beskar/pull/5) | `728529543882` | 75 | Three-crate separation and a bounded unified-diff algorithm. | Adopt the crate responsibilities and diff algorithm; fix final-newline reporting and handle binary/large files explicitly. |
| [#6](https://github.com/ventrion/beskar/pull/6) | `28b673c1a9d5` | 37 | First-class configuration commands and separated domain/presentation code. | Expose configuration through the CLI and core; use journaled changes and stronger ownership checks. |
| [#7](https://github.com/ventrion/beskar/pull/7) | `9ff0999bff77` | 311 | Lossless editing and detailed reconciliation states. | Preserve comments and line endings and distinguish local drift from divergence in reports. |
| [#8](https://github.com/ventrion/beskar/pull/8) | `062233c6d96b` | 204 | Kernel locks and careful treatment of local and ignored content. | Use kernel locks; fingerprint all content instead of adding an ignore/carry system with a second ownership model. |
| [#9](https://github.com/ventrion/beskar/pull/9) | `5e2fe68564c2` | 665 | The broadest CLI, one command specification for help/validation, JSON and exact prompt answers. | Adapt the JSON encoder and command-table approach; use exact prompt choices and explicit unlocks. Keep promotion outside update prompts so abort remains batch-wide. |
| [#10](https://github.com/ventrion/beskar/pull/10) | `e4a7139ab284` | 31 | Whole-batch journaled transactions, automatic recovery, promotion and recorded deployment paths. | Use its transaction/storage implementation as the foundation, then put private storage behind the core session and immutable plan interfaces. |

## Selection

The result combines the transaction safety of #10 with the separation and CLI design of #5 through #9. It retains one grammar and one reconciliation engine. Merging all branch histories or keeping multiple parsers would create competing state models and leave callers responsible for choosing among them.

The core session owns domain operations and returns structured results. Its private storage modules manage locks, copies, fingerprints, registry writes, and recovery. Public plans expose observations and explicit conflict choices, with no writable paths or baselines. The CLI owns terminal interaction and JSON serialization.

## Additional corrections

The consolidation adds checks beyond the selected foundation:

- Revalidate unchanged copies, profiles, sources, configuration, and registry state between planning and applying.
- Recheck originals after moving them into backup, and retain detected concurrent edits with the recovery journal.
- Journal temporary roots before staging starts so recovery can remove interrupted copies without changing targets.
- Coordinate separate homes that share a library, registry, or workspace using persistent kernel lock files.
- Reject nested workspace registrations in either order before writing the registry, keeping subsequent commands usable.
- Unlock explicitly when closing a session, including when another thread briefly inherits descriptors while launching a subprocess.
- Recover a configuration file from its transaction backup before opening state, and acquire workspace locks before recovery.
- Preserve the old baseline and sync timestamp when keeping drift; leave a no-op update's registry unchanged.
- Use a single JSON result document for successes, validation errors, and conflict reports.
- Normalize command aliases before dispatching commands or either help form.
- Preserve final-newline changes in text diffs and existing comments and line endings in config/profile edits.

## Deliberate limits

The CLI does not add Git synchronization, AI classification, a TUI, or symlink support. It treats skill contents as opaque regular files and directories, including files that some candidates ignored. It does not implicitly adopt unmanaged copies. These choices keep the ownership and recovery rules consistent with the brief.

The candidate formats are unreleased alternatives, not formats that this PR promises to migrate. The selected grammar and persistence contract are documented in [FORMAT.md](FORMAT.md) and [ARCHITECTURE.md](ARCHITECTURE.md).

## Consolidated validation

The consolidated workspace passes 59 tests on both Rust 1.89.0 and the installed stable toolchain. Clippy with warnings denied, formatting, rustdoc with warnings denied, and the offline release builds pass. A release-binary smoke test parsed 45 JSON responses and exercised terminal conflict choices, abort without changes, explicit replacement, and lock release after SIGKILL. A separate SIGKILL test interrupted a 5,001-file import during staging, then verified recovery, library listing, and a successful retry. GitHub CI repeats the tests and release build on Rust 1.89.0 and stable, with lint and documentation checks on stable. Validation was performed on Linux.

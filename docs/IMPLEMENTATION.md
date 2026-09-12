# Beskar implementation notes

Companion to the normative spec (`docs/SPEC.md`) and the coverage report
(`docs/COVERAGE.md`). This file records how the v1 implementation maps to the
spec's suggested phases (§136), which gap-closure passes ran after the main
build-out, design decisions worth keeping, and the honest list of remaining
limitations.

State: **v1 complete and green** — `cargo build --workspace`,
`cargo clippy --workspace --all-targets` (zero warnings),
`cargo test --workspace` (489 tests across 24 suites) all pass; CI workflow in
`.github/workflows/ci.yml`.

## Phase-by-phase summary (spec §136)

| Phase | Scope | Head of implementation |
| --- | --- | --- |
| 1 — Core data model | config, IDs, Library discovery, committed-object reading via a `GitBackend` trait, Skill/Profile parsing, attachments, effective membership, Catalog, Registry, Stamp, status classifier. The multi-Profile model was implemented from the start, never retrofitted. | `beskar-core` (`library`, `profile`, `skill`, `registry`, `catalog`, `stamp`, `status`), `beskar-git` |
| 2 — Reconciliation engine | desired-state builder, planner (one plan per Installation), install/update/retire, attach/detach, force and unmanaged-replacement semantics, dry-run, JSON. | `beskar-core::reconcile` |
| 3 — Registry lifecycle | `add`/`remove`/`unregister`/`why`/`status`/`update [--all]`/`ref set`, registry persistence last, locks. | `beskar-core::lifecycle`, `beskar-cli` |
| 4 — Library editing | ingest, move, rename, remove, Profile and Catalog operations, scoped Git commits; `doctor`. | `beskar-core::editing`, `beskar-core::doctor` |
| 5 — Remote Git | fetch (strict fast-forward, never merges/rebases), branch relationships, push (never force, refuses dirty), explicit network only. | `beskar-core::remote`, `beskar-git::SystemGitBackend` |
| 6 — Migration | legacy skill-manager Home → Beskar Library (in place, Git history preserved), legacy Profiles/Registry/Stamps; ambiguity fails closed before any write. | `beskar-core::migrate`, `beskar migrate skm` |
| 7 — TUI | Ratatui UI built entirely on the core planning APIs: pure reducer (`reduce(app, event) -> Vec<Effect>`), one service bridge with the plan → confirm → execute flow. No domain logic in the TUI. | `beskar-tui` |
| 8 — Desktop GUI | eframe/egui controller + services over the exact same core APIs; fails closed without a display. | `beskar-gui` |

### Hardening and gap-closure passes (post-Phase-8)

1. **Final verification & hardening** — `init`/`--remote` clone/`--library`
   adopt (§87), `registry move`, §126/§127 behavioral-matrix completion, first
   end-to-end §137 product-contract test.
2. **Registry commands (§84) + installation namespace (§78–§79)** — new
   `registry_service::RegistryService` (works without an active Library):
   `registry list/show/prune/repair` plus `installation
   profiles/attach/detach/profile-order`. `attach` and `detach` are literally
   the same `Lifecycle::add`/`Lifecycle::remove` calls as `add`/`remove`
   (§78: attach = add, detach = remove). Prefix resolution is
   case/hyphen-insensitive with deterministic ambiguity reporting; `prune` is
   existence-check-only (never touches files, dry-run is zero-write);
   `repair` is conservative — it refreshes §28/§30 last-known names and
   workspace metadata, and reports anything it cannot prove (moved workspace,
   unresolvable ref, §39-protected missing profiles) as `needs_manual_action`
   with exact-command guidance. It never deletes records or attachments and
   never guesses moves.
3. **TUI/GUI coverage deepening** — the §133/§137.31–32 product contract is
   now demonstrated through tests on both UIs: drift gallery rendering,
   §39 missing-profile protection and explicit detach, §100 shared-skill
   ownership popups, §56 ref-set plans, §47 all-blockers-before-force, and a
   full browse → compose → preview → apply → drift → update flow through the
   pure reducer fed by real fulfilled effects; the GUI controller mirrors
   every row against real hermetic core state. Two GUI contract drifts found
   and fixed (§39-detach preview was a no-op; update-all no-op summaries were
   misclassified by the toast heuristic).
4. **Portability + CI** — fixed three real Windows bugs where native paths
   were stringified without separator conversion (new fail-closed
   `paths::to_slash_path` is now the single native → serialized conversion);
   `Registry::insert` refuses case-insensitively-equal `(workspace, target)`
   near-duplicates (case-insensitive-filesystem hazard); added
   `.github/workflows/ci.yml` (fmt, clippy `-D warnings`, test matrix over
   ubuntu/macos/windows, isolated GUI-Wayland feature check); beskar-gui
   gained a non-default `wayland` feature.

## Design notes worth keeping

- **Dependency substitutions**
  - `serde-saphyr` instead of the deprecated `serde_yaml` for SKILL.md
    frontmatter — panic-free, untrusted-input friendly (skills are untrusted,
    §118; they are parsed, never executed).
  - `time` instead of `chrono` for timestamps/serialization.
  - `fs4` is declared as a workspace dependency key but unused: Rust's std
    `File::try_lock` (advisory flock) covers §29/§88, so
    `beskar_core::lock` uses std directly with a short retry window before
    failing closed with typed guidance. `fs4` remains the drop-in fallback if
    std semantics ever prove insufficient.
  - `eframe` with the `glow` renderer and `default-features = false` so the
    GUI never needs link-time system libraries on Linux (X11 feature default;
    Wayland opt-in behind the `wayland` feature).
  - `ratatui` pinned with `crossterm_0_29` so exactly one crossterm version
    exists in the tree.
- **Git access**: one `GitBackend` trait (`beskar-git`); v1 ships only
  `SystemGitBackend`, which shells out to system `git` via subprocess argument
  arrays (no shell interpolation, no string concatenation of user data).
  Authentication is delegated to the user's Git setup (§135.35–36). Committed
  state is authoritative: reads go through exact committed objects, never the
  working tree (§8.1).
- **Mutations everywhere are** plan → all blockers in one pass → lock →
  execute → registry persisted last (§89/§61). Dry-run runs the same planner
  (§135.39). The TUI's `PendingAction`/`PlannedChange` and the GUI's staged
  plans are thin confirm-flows over that one engine (§135.37–38).
- **JSON is the automation API** (§130): stable snake_case identifiers,
  `{schema, command, ok}` envelopes, `--json` means only JSON on stdout.
  Registry/skill files skip empty fields, so absent keys mean "empty", not
  "corrupt".
- **Typed errors** with stable `code()` ids (§115) shared by CLI exit codes
  (§94), TUI toasts/dialogs, and GUI error surfacing.
- **Tests are hermetic**: `tempfile` roots, `BESKAR_HOME`/`BESKAR_LIBRARY`
  child-env overrides, local path remotes, no network (§125); `beskar-test-support`
  provides `TestRepo`/`TempRoot`/skm fixtures. Executable-bit assertions are
  `cfg(unix)`-gated; stamps store executable state platform-independently (§34).

## Remaining known limitations (v1)

Honest list; nothing here blocks the §137 contract, but each is a deliberate
scope cut or an unverified claim:

1. **Non-Linux platforms are untested locally.** The CI matrix runs the full
   suite on ubuntu/macos/windows, but no Windows/macOS machine was exercised
   during development. The Windows path-separator bugs were found by audit and
   fixed statically (`paths::to_slash_path`), and case-sensitivity hazards are
   covered by pure tests — but the platform-specific code paths have not been
   run on real macOS/Windows hardware.
2. **GUI Wayland is compile-checked only.** The `wayland` feature builds
   (`cargo check -p beskar-gui --features wayland`, CI job included) but was
   never visually exercised; the default build is X11/glow.
3. **TUI and GUI use different glyph tables** for §38 drift states
   (terminal-safe vs. Unicode). §130 pins identifiers, not glyphs; unifying
   the two tables is future visual-consistency work only.
4. **`registry repair` is deliberately conservative.** It never rewrites
   history it cannot prove: a moved workspace must be confirmed with
   `registry move`, an unresolvable source ref must be resolved manually with
   `beskar ref set`, and §39-protected missing profiles are never silently
   detached. This is a safety choice (§4), not a missing feature.
5. **One Git backend.** `GitBackend` is abstract but v1 ships only
   `SystemGitBackend` (§135.36); there is no built-in transport or credential
   handling — networking/auth is exactly what the user's system Git does.
6. **Fetch is fast-forward only; push never forces** (§135.26–27). Diverged
   library branches always stop for manual resolution; that is the design, and
   there is no escape hatch.
7. **`update` never fetches** (§135.25): convergence uses only locally
   available refs; keeping installations current is `fetch` + `update --all`.
8. **Headless GUI fails closed.** `beskar-gui` requires a graphical session
   (X11 or Wayland display) and exits with a typed error otherwise; there is
   no remote/web fallback by design.
9. **Case-insensitive filesystems:** duplicate targets differing only by
   ASCII case are refused at insertion and workspaces are canonicalized at
   registration; lookups stay exact. Exotic collisions (Unicode
   normalization, e.g. NFC vs NFD directory names on macOS) are not detected.
10. **Migration covers the skill-manager layout of §120–§124 only**, and any
    ambiguous legacy state (differing refs, unknown profiles, ownership
    conflicts) stops the migration before any write instead of best-effort
    guessing.
11. **The `fs4` dependency key is currently unused** (std flock is used — see
    design notes). It is kept as a documented fallback; removing it is a
    one-line cleanup if it stays unused.

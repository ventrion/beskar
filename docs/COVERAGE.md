# Beskar v1 coverage report — spec §137

§137 defines Beskar v1 complete as a 32-step scenario ("a clean machine can
successfully perform this scenario"); that behavior is the normative v1
product contract. This report maps every step to its status and the test or
command that proves it. Conventions:

- **done** — behavior is implemented and asserted by an automated test.
- Test names are `name (file)`. CLI test files live under
  `crates/beskar-cli/tests/`, core integration tests under
  `crates/beskar-core/tests/`, TUI service tests under
  `crates/beskar-tui/tests/`, and `…[src]` denotes in-crate unit tests
  (`beskar-core/src/init.rs`, `beskar-tui/src/reduce.rs`/`view.rs`/`app.rs`,
  `beskar-gui/src/`).

**Overall: 32/32 done.** The full scenario additionally runs end-to-end in one
test through the real CLI binary:
`v1_product_contract_end_to_end (beskar-cli/tests/contract.rs)`; the same
contract runs through the TUI reducer + service bridge in
`spec_133_workflow_end_to_end_through_reducer_and_service
(beskar-tui/tests/service.rs)` and through the GUI controller in
`spec_133_workflow_through_the_controller (beskar-gui)`.

| # | §137 step | Status | Proof |
| --- | --- | --- | --- |
| 1 | clone a Beskar Library | done | `cloned_library_keeps_identity_and_validates (init.rs [src])`; also `cloning_refuses_a_non_library_remote_instead_of_adopting_it_blindly (init.rs [src])` |
| 2 | validate it | done | `adoption_validates_without_writing (init.rs [src])`; discovery validation underpins every suite; `healthy_library_reports_no_errors (tests/doctor.rs)` |
| 3 | list its Skills | done | `skill_list_filters_and_sorts (beskar-core/tests/editing.rs)`; `skill_list_filters_and_json_shape (tests/library_editing.rs)` |
| 4 | create several Profiles | done | `profile_operations_cover_the_full_surface (beskar-core/tests/editing.rs)`; `profile_operations_end_to_end (tests/library_editing.rs)` |
| 5 | ingest Skills | done | `ingest_single_skill_copies_content_and_commits_scoped_paths (beskar-core/tests/editing.rs)`; `ingest_json_contract_and_commit (tests/library_editing.rs)` |
| 6 | assign overlapping Skills to several Profiles | done | `add_second_profile_unions_membership_with_one_physical_copy (beskar-core/tests/lifecycle.rs)`; `second_profile_unions_membership_with_one_copy (tests/lifecycle.rs)` |
| 7 | commit the changes | done | `ingest_json_contract_and_commit (tests/library_editing.rs)` (scoped commits); `mutations_refuse_unrelated_staged_files_and_leave_unstaged_alone (beskar-core/tests/editing.rs)` |
| 8 | push a branch | done | `push_publishes_commits_and_establishes_upstream (beskar-core/tests/remote.rs)`; non-FF refusal: `push_refuses_non_fast_forward_targets_with_action_required_exit (tests/remote.rs)` |
| 9 | create a Workspace Installation with `dev-core` | done | `add_creates_installation_installs_files_and_persists_registry_last (beskar-core/tests/lifecycle.rs)`; `add_status_why_remove_unregister_happy_path (tests/lifecycle.rs)` |
| 10 | attach `rust-development` to the same Target | done | `attaching_second_profile_installs_union_once_and_records_membership (beskar-core/tests/reconciliation.rs)`; `installation_attach_and_detach_behave_like_add_and_remove (tests/registry.rs)` |
| 11 | attach `github` to the same Target | done | same attachment machinery; N-attachment state asserted in `refresh_shows_multiple_simultaneous_attachments (beskar-gui [src])` and `list_sorts_installations_deterministically (beskar-core/tests/registry_service.rs)` |
| 12 | overlapping Skills exist physically only once | done | "one physical copy per target" assertion in `attaching_second_profile_installs_union_once_and_records_membership (reconciliation.rs)`; `add_second_profile_unions_membership_with_one_physical_copy (lifecycle.rs)` |
| 13 | explain all requiring Profiles for each shared Skill | done | `why_reports_requiring_profiles_state_and_ref (beskar-core/tests/lifecycle.rs)` + CLI `why` in `add_status_why_remove_unregister_happy_path`; `membership_view_reports_owners_source_and_state (beskar-tui/tests/service.rs)`; `the_membership_view_matches_the_100_shape (beskar-tui [src view])`; `membership_view_explains_a_shared_skill (beskar-gui [src])` |
| 14 | change one Profile so a shared Skill is removed from it | done | desired-state change incl. membership-only transitions: `spec46_reconciliation_example (reconciliation.rs)` (dev(A,B), rust(B,C) → dev(A,D), rust(B,E): B membership-changed, C retired, no unnecessary rewrite) |
| 15 | update; the Skill remains because another Profile requires it | done | "testing is kept: only its membership changes" in `spec46_reconciliation_example (reconciliation.rs)`; update applies new library state: `update_picks_up_new_library_state_without_fetching (beskar-core/tests/lifecycle.rs)` |
| 16 | detach the final Profile requiring that Skill | done | `detaching_the_final_owner_retires_the_skill_but_preserves_extras (beskar-core/tests/lifecycle.rs)`; `detaching_the_final_profile_retires_everything_managed (reconciliation.rs)` |
| 17 | the Skill is safely retired | done | same tests; TUI `detaching_the_final_owner_retires_and_preserves_extras (beskar-tui/tests/service.rs)`; GUI `detaching_the_final_owner_retires_and_preserves_extras (beskar-gui [src])`; `detach_final_owner_retires_and_shared_survives (beskar-gui [src])` |
| 18 | Extra local files preserved during retirement | done | `retirement_preserves_extras_and_leaves_unmanaged_dir (reconciliation.rs)`; `extra_files_survive_current_and_outdated_updates (reconciliation.rs)`; `detaching_the_final_owner_retires_the_skill_but_preserves_extras (lifecycle.rs)` |
| 19 | change a Library Skill | done | committed skill edits + drift in `update_picks_up_new_library_state_without_fetching (lifecycle.rs)`; skill ops: `skill_move_end_to_end`, `skill_rename_end_to_end_updates_all_references (tests/library_editing.rs)` |
| 20 | fetch/fast-forward on another machine | done | two-repo fixture (authoring → bare "remote" → library clone): `fetch_fast_forwards_behind_checked_out_and_non_checked_out_branches (beskar-core/tests/remote.rs)`; CLI `fetch_fast_forwards_and_reports_the_movement (tests/remote.rs)` |
| 21 | detect Installations as outdated | done | `DriftState::Outdated` asserted in `update_picks_up_new_library_state_without_fetching (lifecycle.rs)`; `clean_outdated_skill_updates_in_place (reconciliation.rs)`; glyphs/counts: `drifted_skills_render_their_state_glyphs` + `glyphs_cover_every_drift_state (beskar-tui [src])`, `dashboard_counts_drift_and_missing_profiles_from_the_snapshot (beskar-tui [src])`, `drift_states_surface_glyphs_and_dashboard_counts (beskar-gui [src])` |
| 22 | update all with `beskar update --all` | done | `update_all_strict_refuses_everything_and_best_effort_is_partial (beskar-core/tests/lifecycle.rs)` + CLI of same name; TUI `update_and_update_all_flow_through_plan_then_execute (beskar-tui/tests/service.rs)`; GUI `update_and_update_all_flow_through_plan_preview_and_execute (beskar-gui [src])` |
| 23 | refuse to overwrite locally modified managed files | done | `modified_managed_files_block_updates_and_force_overwrites (reconciliation.rs)`; `update_blocks_on_modified_content_until_forced (lifecycle.rs)`; CLI `update_converges_and_refuses_modified_until_forced (tests/lifecycle.rs)` |
| 24 | show exact destructive changes | done | `blocked_update_json_includes_typed_blockers (tests/lifecycle.rs)`; `blocked_update_lists_all_blockers_with_exact_paths`-flow: `blocked_plan_lists_every_blocker_with_exact_paths (beskar-tui [src reduce])`; `blocked_update_lists_all_blockers_and_force_applies (beskar-tui/tests/service.rs)`; `blocked_update_shows_all_blockers_then_force_completes (beskar-gui [src])` |
| 25 | update after explicit force | done | force halves of the same tests (`modified_managed_files_block_updates_and_force_overwrites`, `update_converges_and_refuses_modified_until_forced`, `blocked_update_lists_all_blockers_and_force_applies`, `blocked_update_shows_all_blockers_then_force_completes`); waiver gating: `blocked_plan_offers_force_replan_for_waivable_blockers_only (beskar-tui [src reduce])` |
| 26 | deleted attached Profile detected as `missing-profile` | done | `deleted_profile_is_protected_not_treated_as_empty (reconciliation.rs)`; `deleted_profile_is_protected_until_explicitly_detached (lifecycle.rs)`; CLI `deleted_profile_is_protected_then_explicitly_detached (tests/lifecycle.rs)` |
| 27 | preserve that Profile's previously attributed Skills | done | protection halves of the same tests; `missing_profile_protection_renders (beskar-tui [src view])`; `missing_profile_is_protected_and_explicitly_detachable (beskar-gui [src])` |
| 28 | explicitly detach the missing Profile | done | `explicit_detach_of_missing_profile_retires_only_its_unique_skills (reconciliation.rs)`; TUI `missing_profile_is_protected_then_explicitly_detached (beskar-tui/tests/service.rs)` + `detach_picker_offers_the_missing_profile_by_last_known_name (beskar-tui [src app])`; GUI `missing_profile_is_protected_and_explicitly_detachable` |
| 29 | reconcile safely afterwards | done | `assert_converged` in the explicit-detach tests; `orphaned_managed_skill_is_retired_but_extras_survive (reconciliation.rs)`; `registry_repair_protects_missing_profiles_via_cli (tests/registry.rs)` keeps §39 states out of automated repair |
| 30 | survive a moved or deleted registered Workspace | done | `registry_move_repairs_a_moved_workspace (lifecycle.rs)`; `registry_repair_missing_workspace_exits_3_with_move_guidance (tests/registry.rs)`; `prune_removes_missing_workspace_or_target_and_keeps_healthy (beskar-core/tests/registry_service.rs)`; `status_all_reports_broken_registrations_without_crashing (tests/lifecycle.rs)`; `unregister_keep_files_removes_only_the_record_even_for_deleted_workspaces (lifecycle.rs)` |
| 31 | expose Profile composition and membership state in the TUI | done | `spec_133_workflow_end_to_end_through_reducer_and_service (beskar-tui/tests/service.rs)`; composition: `attach_second_profile_previews_install_and_membership_only_then_applies`, `detaching_one_owner_keeps_the_shared_skill`, `ref_set_previews_implications_then_applies`, `reorder_updates_only_attachment_order (all service.rs)`; `membership_view_reports_owners_source_and_state (service.rs)`; rendering: `the_membership_view_matches_the_100_shape`, `skill_state_glyphs_appear_in_the_installation_detail`, `missing_profile_protection_renders (beskar-tui [src view])`; `screens_cycle_in_spec_order (beskar-tui [src app])` |
| 32 | expose the same behavior in the desktop GUI | done | `spec_133_workflow_through_the_controller (beskar-gui [src])`; composition: `refresh_shows_multiple_simultaneous_attachments`, `the_104_workflow_previews_then_applies_a_second_profile`, `idempotent_reattach_plans_a_no_op_toast_not_a_dialog`, `detaching_one_of_two_owners_keeps_the_shared_skill`, `detaching_the_final_owner_retires_and_preserves_extras`, `missing_profile_is_protected_and_explicitly_detachable`, `ref_set_previews_implications_before_apply`, `reorder_is_presentation_only`, `blocked_update_shows_all_blockers_then_force_completes`, `update_and_update_all_flow_through_plan_preview_and_execute`, `membership_view_explains_a_shared_skill`, `drift_states_surface_glyphs_and_dashboard_counts` (all beskar-gui [src]) |

## Honesty notes

- **"Another machine" (step 20) is simulated hermetically**: the remote is a
  local bare repository and the "second machine" is a local clone (spec §125
  forbids network access in tests). The Git operations exercised are the real
  ones `SystemGitBackend` performs.
- **Steps 31–32 are proven by automated tests against real core state**, not
  by visual inspection. TUI and GUI intentionally use different glyph tables
  for drift states (terminal-safe vs. Unicode); §130 pins identifiers, not
  glyphs (see `docs/IMPLEMENTATION.md`).
- **§84-adjacent behavior inside step 30 is deliberately conservative**:
  `registry repair` reports a moved workspace with `beskar registry move`
  guidance (exit 3) instead of guessing paths; `prune`/`move` perform the
  actual recovery. This is the §4-safer choice, not a gap.
- **Platform caveat**: all proofs above ran on Linux. macOS/Windows run the
  same suite in CI but were not exercised on real hardware during development
  (see `docs/IMPLEMENTATION.md`, limitation 1).

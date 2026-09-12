//! The controller glue: runs core service calls and feeds the results into
//! the pure [`GuiState`](crate::state::GuiState).
//!
//! No rendering, no egui types — the view calls these when the user clicks.
//! Every mutation follows the §89/§135.38-39 discipline: plan (dry-run) →
//! confirm → execute; failures surface as typed-error dialogs (§115).

use beskar_core::ids::InstallationId;
use beskar_core::reconcile::ReconcileOptions;

use crate::action::GuiAction;
use crate::services::{self, Services};
use crate::state::{ConfirmApply, GuiState};

/// Refreshes the snapshot from core read APIs (§41: read-only, no network).
pub fn refresh(state: &mut GuiState, services: Option<&Services>) {
    let Some(services) = services else {
        // No library discovered: keep showing the discovery error.
        return;
    };
    state.set_busy(Some("Refreshing…".to_owned()));
    match services::gather(services) {
        Ok(snapshot) => state.loaded(snapshot),
        Err(err) => state.load_failed(format!("error ({}): {}", err.code(), err)),
    }
    state.set_busy(None);
}

/// Plans an action as a dry run and opens the confirm dialog (§89 steps
/// 2-3). No-op plans become toasts, never dialogs (§22, §91).
pub fn plan_action(state: &mut GuiState, services: &Services, action: GuiAction) {
    let title = action.title();
    state.set_busy(Some(title.clone()));
    match services::plan(services, &action, ReconcileOptions::default()) {
        Ok(planned) => state.planned(action, planned, ReconcileOptions::default()),
        Err(err) => state.operation_failed(&title, &err),
    }
    state.set_busy(None);
}

/// Executes a confirmed action for real (§89 steps 4-5).
pub fn execute_action(
    state: &mut GuiState,
    services: &Services,
    action: GuiAction,
    options: ReconcileOptions,
) {
    let title = action.title();
    state.set_busy(Some(title.clone()));
    match services::execute(services, &action, options) {
        Ok(executed) => state.executed(executed),
        Err(err) => state.operation_failed(&title, &err),
    }
    state.set_busy(None);
    // Mutations changed the Registry/target/library: refresh the snapshot
    // so no page keeps showing stale state (§41).
    refresh(state, Some(services));
}

/// Computes the §104 workflow preview for the current staged selection
/// (read-only; §104 step 4).
pub fn preview_workflow(state: &mut GuiState, services: &Services) {
    let Some(row) = state.selected_installation() else {
        return;
    };
    let Some(workflow) = &state.installations.workflow else {
        return;
    };
    if workflow.selected.is_empty() {
        state.toast = Some("Select at least one profile first".to_owned());
        return;
    }
    let mode = workflow.mode;
    let selection = workflow.selected.clone();
    let workspace = row.installation.workspace.clone();
    let target = row.installation.target.clone();
    let title = format!(
        "Preview {} {}",
        mode.verb().to_lowercase(),
        selection.join(", ")
    );
    state.set_busy(Some(title.clone()));
    match services::attachment_preview(services, &workspace, Some(&target), mode, &selection) {
        Ok(preview) => state.preview_loaded(preview),
        Err(err) => {
            state.preview_failed(format!("error ({}): {}", err.code(), err));
            state.operation_failed(&title, &err);
        }
    }
    state.set_busy(None);
}

/// Loads the skill preview for the Skills page (§97). The attempt is
/// recorded so the view never retries a failing load every frame; the
/// typed error is surfaced once as a dialog (§115).
pub fn load_skill_preview(state: &mut GuiState, services: &Services, skill: &str) {
    state.skills.preview_attempted = Some(skill.to_owned());
    state.set_busy(Some(format!("Loading {skill}…")));
    match services::load_skill(services, skill) {
        Ok(view) => state.skill_loaded(view),
        Err(err) => state.operation_failed(&format!("Show {skill}"), &err),
    }
    state.set_busy(None);
}

/// Loads the §100 membership view for one skill in one installation.
pub fn load_membership(state: &mut GuiState, services: &Services, id: InstallationId, skill: &str) {
    // The §35 skill commit resolves against the skill's Library path, when
    // the snapshot knows it; informational only.
    let path = state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.skill(skill))
        .map(|listing| listing.path.clone());
    state.set_busy(Some(format!("Inspecting {skill}…")));
    match services::load_membership(services, id, skill, path.as_deref()) {
        Ok(view) => {
            state.installations.membership = Some(view);
            state.dialog = Some(crate::state::Dialog::Membership(Box::new(
                state.installations.membership.clone().expect("just set"),
            )));
        }
        Err(err) => state.operation_failed(&format!("Why {skill}"), &err),
    }
    state.set_busy(None);
}

/// Runs the read-only profile validation (§76) and shows the results.
pub fn validate_profiles(state: &mut GuiState, services: &Services, profile: Option<&str>) {
    state.set_busy(Some("Validating profiles…".to_owned()));
    match services::validate_profiles(services, profile) {
        Ok(validations) => {
            let failures = validations.iter().filter(|v| !v.valid).count();
            state.toast = Some(if failures == 0 {
                format!("{} profile(s) valid", validations.len())
            } else {
                format!("{failures} profile(s) invalid — see the Profiles page")
            });
            state.profiles.validations = Some(validations);
        }
        Err(err) => state.operation_failed("Validate profiles", &err),
    }
    state.set_busy(None);
}

/// Routes a confirmed dialog choice: Run executes, Replan re-plans with
/// consent options (§47/§49), Cancel does nothing (the dialog is already
/// closed by the time this is called).
pub fn handle_confirm(state: &mut GuiState, services: &Services, apply: ConfirmApply) {
    match apply {
        ConfirmApply::Run { action, options } => {
            execute_action(state, services, *action, options);
        }
        ConfirmApply::Replan { action, options } => {
            // Carries explicit consent options into the planner (§47
            // interactive `--force`, §49 `--replace-unmanaged`); the new
            // plan is confirmed again before anything executes.
            let title = action.title();
            state.set_busy(Some(title.clone()));
            match services::plan(services, &action, options) {
                Ok(planned) => state.planned(*action, planned, options),
                Err(err) => state.operation_failed(&title, &err),
            }
            state.set_busy(None);
        }
        ConfirmApply::Cancel => {}
    }
}

#[cfg(test)]
mod tests {
    //! Controller-level contract tests (§133, §137.32): the exact code
    //! path the GUI event loop uses — `controller::*` over `Services` and
    //! the pure `GuiState` — against real hermetic core state (temp
    //! Git libraries, local paths only, no display, no network; §86, §125).

    use std::path::{Path, PathBuf};

    use beskar_core::config::PlatformDirs;
    use beskar_core::reconcile::ReconcileOptions;
    use beskar_test_support::TempRoot;
    use beskar_test_support::fs::write_file;
    use beskar_test_support::git::TestRepo;

    use crate::action::GuiAction;
    use crate::preview::PreviewMode;
    use crate::state::{Dialog, GuiState};

    use super::*;

    const DEV_ID: &str = "98f1513d-94fa-4ace-907e-544c66233653";
    const RUST_ID: &str = "0b0e1c3a-9d24-4c5a-8a30-2b7f0a519102";

    struct Env {
        #[allow(dead_code)]
        home: TempRoot,
        repo: TestRepo,
        #[allow(dead_code)]
        ws_root: TempRoot,
        workspace: PathBuf,
    }

    fn profile_toml(id: &str, name: &str, skills: &[&str]) -> String {
        let list: Vec<String> = skills.iter().map(|s| format!("  \"{s}\",")).collect();
        format!(
            "schema = 1\nid = \"{id}\"\nname = \"{name}\"\ndescription = \"{name} set\"\n\nskills = [\n{}\n]\n",
            list.join("\n")
        )
    }

    fn write_skill_tree(repo: &Path, bucket: &str, name: &str, body: &str) {
        write_file(
            repo,
            &format!("skills/{bucket}/{name}/SKILL.md"),
            &format!("---\nname: {name}\ndescription: The {name} skill.\n---\n\n{body}\n"),
        );
        write_file(
            repo,
            &format!("skills/{bucket}/{name}/docs/guide.md"),
            &format!("# {name} guide\n"),
        );
    }

    impl Env {
        fn new() -> Self {
            let repo = TestRepo::new();
            let root = repo.path();
            write_file(
                root,
                "beskar.toml",
                "schema = 1\nlibrary_id = \"550e8400-e29b-41d4-a716-446655440000\"\ndefault_ref = \"main\"\n",
            );
            for (bucket, name) in [
                ("engineering", "git-workflow"),
                ("quality", "testing"),
                ("engineering/process", "code-review"),
                ("languages", "rust"),
            ] {
                write_skill_tree(root, bucket, name, "body");
            }
            write_file(
                root,
                "profiles/dev-core.toml",
                &profile_toml(
                    DEV_ID,
                    "dev-core",
                    &["git-workflow", "testing", "code-review"],
                ),
            );
            write_file(
                root,
                "profiles/rust-development.toml",
                &profile_toml(RUST_ID, "rust-development", &["rust", "testing"]),
            );
            repo.commit_all("beskar: seed library");
            let ws_root = TempRoot::new();
            let workspace = ws_root.child("workspace");
            Self {
                home: TempRoot::new(),
                repo,
                ws_root,
                workspace,
            }
        }

        fn services(&self) -> Services {
            let dirs = PlatformDirs::resolve(Some(self.home.path()));
            let library =
                beskar_core::library::Library::open_at(self.repo.path()).expect("valid library");
            Services::new(dirs, library)
        }

        fn registry(&self) -> beskar_core::registry::Registry {
            beskar_core::registry::RegistryStore::new(
                self.home.path().join("data").join("registry.json"),
            )
            .load()
            .expect("registry loads")
        }

        fn target(&self) -> PathBuf {
            self.workspace.join(".agents").join("skills")
        }

        /// Attaches one profile for real (setup helper).
        fn attach(&self, services: &Services, profile: &str) {
            services
                .lifecycle()
                .add(beskar_core::lifecycle::AddRequest {
                    workspace: &self.workspace,
                    profile,
                    target: None,
                    adapter: None,
                    source_ref: None,
                    options: ReconcileOptions::default(),
                    dry_run: false,
                })
                .expect("attach succeeds");
        }
    }

    fn file_sig(path: &Path) -> (Vec<u8>, std::time::SystemTime) {
        let metadata = std::fs::metadata(path).expect("file exists");
        (
            std::fs::read(path).expect("read"),
            metadata.modified().expect("mtime"),
        )
    }

    /// Applies the open confirm dialog through the controller (§89 steps
    /// 3-5): the choice runs/replans, then the state settles.
    fn confirm_applied(state: &mut GuiState, services: &Services) {
        // Plan flows keep their dialog in `state.dialog`; the staged
        // Â§104 workflow builds its dialog on demand, exactly like the view
        // does on the Apply click (Â§104 step 6).
        if matches!(state.dialog, Some(Dialog::Confirm(_))) {
            let apply = state.confirm_choice(0).expect("a confirm choice");
            handle_confirm(state, services, apply);
        } else {
            let confirm = state.workflow_confirm().expect("the apply dialog");
            assert_eq!(confirm.choices.len(), 1, "staged applies run directly");
            handle_confirm(state, services, confirm.choices[0].apply.clone());
        }
    }

    /// The staged §104 workflow for one selection: open, select, preview.
    /// Selects the first (only) installation the way a click on the
    /// Installations list would.
    fn select_first(state: &mut GuiState) {
        let id = state.snapshot.as_ref().expect("snapshot").installations[0]
            .installation
            .id;
        state.select_installation(id);
    }

    fn stage_workflow(state: &mut GuiState, services: &Services, mode: PreviewMode, pick: &str) {
        select_first(state);
        state.begin_workflow(mode);
        state.toggle_workflow_profile(pick);
        preview_workflow(state, services);
        assert!(
            state
                .installations
                .workflow
                .as_ref()
                .expect("workflow")
                .preview
                .is_some(),
            "the preview computed"
        );
    }

    #[test]
    fn refresh_shows_multiple_simultaneous_attachments() {
        // §103/§7/§137.32: one installation with several simultaneously
        // attached profiles is visible on the Installations page.
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        env.attach(&services, "rust-development");

        let mut state = GuiState::new();
        refresh(&mut state, Some(&services));

        let snapshot = state.snapshot.as_ref().expect("snapshot");
        let row = &snapshot.installations[0];
        assert_eq!(row.installation.profiles.len(), 2);
        assert_eq!(row.installation.profiles[0].name, "dev-core");
        assert_eq!(row.installation.profiles[1].name, "rust-development");
        let metrics = snapshot.dashboard();
        assert_eq!(metrics.attachments, 2, "§103: both attachments visible");
        assert_eq!(metrics.installations, 1);
        select_first(&mut state);
        assert!(state.selected_installation().is_some());
    }

    #[test]
    fn the_104_workflow_previews_then_applies_a_second_profile() {
        // §104/§133.12: attach the overlapping second profile — the
        // preview shows install/retain/membership-only, and applying
        // never rewrites the shared skill's files (§90).
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        let mut state = GuiState::new();
        refresh(&mut state, Some(&services));

        let shared = env.target().join("testing/SKILL.md");
        let before = file_sig(&shared);

        stage_workflow(
            &mut state,
            &services,
            PreviewMode::Attach,
            "rust-development",
        );
        let preview = state
            .installations
            .workflow
            .as_ref()
            .expect("workflow")
            .preview
            .clone()
            .expect("preview");
        let installed: Vec<String> = preview
            .rows_of(crate::preview::MembershipChange::Install)
            .map(|row| row.skill.clone())
            .collect();
        assert_eq!(installed, vec!["rust"], "the new skill installs");
        let changed: Vec<&crate::preview::MembershipRow> = preview
            .rows_of(crate::preview::MembershipChange::MembershipOnly)
            .collect();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].skill, "testing");
        assert_eq!(changed[0].after, vec!["dev-core", "rust-development"]);
        assert!(!preview.is_no_op());

        // §104 step 6: Apply runs the staged attach through the controller.
        let confirm = state.workflow_confirm().expect("the apply dialog");
        confirm_applied(&mut state, &services);

        assert!(env.target().join("rust/SKILL.md").is_file());
        assert_eq!(
            before,
            file_sig(&shared),
            "§90: the shared skill's files are not rewritten"
        );
        let registry = env.registry();
        assert_eq!(registry.installations[0].profiles.len(), 2, "§7");
        assert!(
            state
                .toast
                .as_deref()
                .is_some_and(|t| t.contains("applied")),
            "the outcome surfaces: {:?}",
            state.toast
        );
        assert_eq!(state.activity.entries[0].title, "Attach rust-development");
        let _ = confirm;
    }

    #[test]
    fn idempotent_reattach_plans_a_no_op_toast_not_a_dialog() {
        // §22/§137.10: attaching an already-attached profile is a no-op —
        // a toast, never a dialog or a duplicate attachment.
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "rust-development");
        let mut state = GuiState::new();
        refresh(&mut state, Some(&services));

        plan_action(
            &mut state,
            &services,
            GuiAction::AttachProfile {
                workspace: env.workspace.clone(),
                target: None,
                profile: "rust-development".to_owned(),
            },
        );
        assert!(state.dialog.is_none(), "no dialog for a no-op plan");
        assert!(
            state
                .toast
                .as_deref()
                .is_some_and(|t| t.contains("nothing to do")),
            "{:?}",
            state.toast
        );
        assert_eq!(env.registry().installations[0].profiles.len(), 1);
    }

    #[test]
    fn detaching_one_of_two_owners_keeps_the_shared_skill() {
        // §50/§137.14-15: after detaching dev-core the shared `testing`
        // stays — its membership changes, the dev-only skills retire.
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        env.attach(&services, "rust-development");
        let mut state = GuiState::new();
        refresh(&mut state, Some(&services));
        let shared = env.target().join("testing/SKILL.md");
        let before = file_sig(&shared);

        stage_workflow(&mut state, &services, PreviewMode::Detach, "dev-core");
        let preview = state
            .installations
            .workflow
            .as_ref()
            .expect("workflow")
            .preview
            .clone()
            .expect("preview");
        let retired: Vec<String> = preview
            .rows_of(crate::preview::MembershipChange::Retire)
            .map(|row| row.skill.clone())
            .collect();
        assert_eq!(retired, vec!["code-review", "git-workflow"]);
        assert!(
            preview
                .rows_of(crate::preview::MembershipChange::MembershipOnly)
                .any(|row| row.skill == "testing"),
            "the shared skill previews as a membership-only change"
        );
        assert!(
            preview
                .rows_of(crate::preview::MembershipChange::Retain)
                .any(|row| row.skill == "rust"),
            "rust-development's exclusive skill is retained"
        );

        confirm_applied(&mut state, &services);
        assert!(shared.is_file(), "§50: the shared skill remains installed");
        assert_eq!(before.0, file_sig(&shared).0);
        assert!(!env.target().join("git-workflow").exists());
        let registry = env.registry();
        assert_eq!(registry.installations[0].profiles.len(), 1);
        assert_eq!(
            registry.installations[0].profiles[0].name,
            "rust-development"
        );
    }

    #[test]
    fn detaching_the_final_owner_retires_and_preserves_extras() {
        // §51/§137.16-18: the last owner's detach retires the skill while
        // extra files survive.
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "rust-development");
        write_file(
            &env.workspace,
            ".agents/skills/testing/local-notes.md",
            "user data",
        );
        let mut state = GuiState::new();
        refresh(&mut state, Some(&services));

        stage_workflow(
            &mut state,
            &services,
            PreviewMode::Detach,
            "rust-development",
        );
        let preview = state
            .installations
            .workflow
            .as_ref()
            .expect("workflow")
            .preview
            .clone()
            .expect("preview");
        let retired: Vec<String> = preview
            .rows_of(crate::preview::MembershipChange::Retire)
            .map(|row| row.skill.clone())
            .collect();
        assert_eq!(retired, vec!["rust", "testing"]);

        confirm_applied(&mut state, &services);
        assert!(!env.target().join("testing/SKILL.md").exists());
        assert!(
            env.target().join("testing/local-notes.md").is_file(),
            "§51: extras survive retirement"
        );
        assert!(env.registry().installations[0].profiles.is_empty(), "§54");
    }

    #[test]
    fn missing_profile_is_protected_and_explicitly_detachable() {
        // §39/§40/§137.26-29: a vanished profile is a protected state on
        // the Installations page, and the staged workflow still previews
        // and applies its explicit detach (by last-known name).
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        env.attach(&services, "rust-development");
        std::fs::remove_file(env.repo.path().join("profiles/rust-development.toml"))
            .expect("remove profile");
        env.repo.commit_all("beskar: drop rust-development");

        let mut state = GuiState::new();
        refresh(&mut state, Some(&services));
        let snapshot = state.snapshot.as_ref().expect("snapshot");
        let metrics = snapshot.dashboard();
        assert_eq!(metrics.missing_profiles, 1, "§39 visible on the dashboard");
        assert_eq!(metrics.broken, 1);
        let status = snapshot.installations[0].status.as_ref().expect("status");
        assert!(status.profiles[1].profile.is_none());
        let rust = beskar_core::ids::SkillName::parse("rust").expect("valid");
        assert!(
            status.skills[&rust].protected_by_missing_profile,
            "§39: the skill stays protected"
        );
        assert!(env.target().join("rust/SKILL.md").is_file());

        // §40: explicitly detach the missing profile through the workflow.
        stage_workflow(
            &mut state,
            &services,
            PreviewMode::Detach,
            "rust-development",
        );
        let preview = state
            .installations
            .workflow
            .as_ref()
            .expect("workflow")
            .preview
            .clone()
            .expect("preview");
        assert!(
            !preview.is_no_op(),
            "the detach of a §39 profile must not be a no-op"
        );
        let retired: Vec<String> = preview
            .rows_of(crate::preview::MembershipChange::Retire)
            .map(|row| row.skill.clone())
            .collect();
        assert_eq!(retired, vec!["rust"], "the released skills may retire");

        confirm_applied(&mut state, &services);
        let registry = env.registry();
        assert_eq!(registry.installations[0].profiles.len(), 1);
        assert!(
            !env.target().join("rust").exists(),
            "the released skill retired"
        );
        assert!(env.target().join("testing/SKILL.md").is_file());
    }

    #[test]
    fn ref_set_previews_implications_before_apply() {
        // §56/§137.20: the ref change plans first, showing the
        // reconciliation implications; applying moves the whole
        // installation.
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        // A `next` branch carries a newer revision of `testing`.
        beskar_test_support::git::git_ok(env.repo.path(), &["branch", "next"]);
        beskar_test_support::git::git_ok(env.repo.path(), &["checkout", "next"]);
        write_skill_tree(
            env.repo.path(),
            "quality",
            "testing",
            "Updated testing body.",
        );
        env.repo.commit_all("beskar: update testing on next");
        beskar_test_support::git::git_ok(env.repo.path(), &["checkout", "main"]);
        let mut state = GuiState::new();
        refresh(&mut state, Some(&services));

        plan_action(
            &mut state,
            &services,
            GuiAction::RefSet {
                workspace: env.workspace.clone(),
                target: None,
                new_ref: "next".to_owned(),
            },
        );
        // The plan previewed the implication BEFORE anything applied
        // (§56), and the dry run left the registry untouched.
        let Dialog::Confirm(confirm) = state.dialog.as_ref().expect("dialog") else {
            panic!("expected the ref-set confirm dialog");
        };
        let body = confirm.lines.join("\n");
        assert!(body.contains("update testing"), "{body}");
        assert_eq!(env.registry().installations[0].source_ref, "main");

        confirm_applied(&mut state, &services);
        // Applying moves the whole installation at once (§56).
        assert_eq!(env.registry().installations[0].source_ref, "next");
        let updated = std::fs::read_to_string(env.target().join("testing/SKILL.md")).expect("read");
        assert!(updated.contains("Updated testing body."), "applied");
    }

    #[test]
    fn reorder_is_presentation_only() {
        // §79/§137.9: attachment reorder changes presentation order only —
        // never skill bytes or mtimes.
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        env.attach(&services, "rust-development");
        let mut state = GuiState::new();
        refresh(&mut state, Some(&services));
        let shared = env.target().join("testing/SKILL.md");
        let before = file_sig(&shared);

        plan_action(
            &mut state,
            &services,
            GuiAction::ReorderProfiles {
                workspace: env.workspace.clone(),
                target: None,
                order: vec![
                    beskar_core::ids::ProfileId::parse(RUST_ID).expect("id"),
                    beskar_core::ids::ProfileId::parse(DEV_ID).expect("id"),
                ],
            },
        );
        let Dialog::Confirm(confirm) = state.dialog.as_ref().expect("dialog") else {
            panic!("expected the reorder confirm dialog");
        };
        let body = confirm.lines.join("\n");
        assert!(body.contains("presentation only"), "{body}");
        assert!(body.contains("rust-development, dev-core"), "{body}");
        assert_eq!(env.registry().installations[0].profiles[0].name, "dev-core");

        confirm_applied(&mut state, &services);
        let registry = env.registry();
        assert_eq!(
            registry.installations[0].profiles[0].name,
            "rust-development"
        );
        assert_eq!(before, file_sig(&shared), "§79: no fs effects");
    }

    #[test]
    fn blocked_update_shows_all_blockers_then_force_completes() {
        // §47/§137.23-25: one planning pass lists EVERY blocker with its
        // exact managed paths; force is an explicit re-plan; extras stay.
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        write_file(
            &env.workspace,
            ".agents/skills/git-workflow/SKILL.md",
            "---\nname: git-workflow\ndescription: locally changed\n---\n",
        );
        write_file(
            &env.workspace,
            ".agents/skills/testing/SKILL.md",
            "---\nname: testing\ndescription: locally changed\n---\n",
        );
        write_file(&env.workspace, ".agents/skills/testing/keep.md", "extra");
        let mut state = GuiState::new();
        refresh(&mut state, Some(&services));

        plan_action(
            &mut state,
            &services,
            GuiAction::UpdateInstallation {
                workspace: env.workspace.clone(),
                target: None,
            },
        );
        let Dialog::Confirm(confirm) = state.dialog.as_ref().expect("dialog") else {
            panic!("expected the blocked-plan dialog");
        };
        let body = confirm.lines.join("\n");
        for needle in ["BLOCKED", "git-workflow", "testing", "SKILL.md", "§47"] {
            assert!(body.contains(needle), "{needle} missing from:\n{body}");
        }
        assert!(confirm.choices.len() >= 2, "force + cancel");
        assert!(!confirm.lines.is_empty());

        // Explicit consent re-plans (§47); the fresh plan then applies.
        confirm_applied(&mut state, &services);
        confirm_applied(&mut state, &services);

        for skill in ["git-workflow", "testing"] {
            let content =
                std::fs::read_to_string(env.target().join(skill).join("SKILL.md")).expect("read");
            assert!(
                !content.contains("locally changed"),
                "{skill} restored to the library copy after consent"
            );
        }
        assert!(
            env.target().join("testing/keep.md").is_file(),
            "§47: extras are never discarded"
        );
    }

    #[test]
    fn update_and_update_all_flow_through_plan_preview_and_execute() {
        // §44/§137.21-22: update and update --all both plan first, then
        // execute on confirmation.
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        write_skill_tree(
            env.repo.path(),
            "quality",
            "testing",
            "Updated testing body.",
        );
        env.repo.commit_all("beskar: update testing");
        let mut state = GuiState::new();
        refresh(&mut state, Some(&services));
        assert_eq!(
            state
                .snapshot
                .as_ref()
                .expect("snapshot")
                .dashboard()
                .outdated,
            1,
            "the drift is detected"
        );

        plan_action(
            &mut state,
            &services,
            GuiAction::UpdateInstallation {
                workspace: env.workspace.clone(),
                target: None,
            },
        );
        {
            let Dialog::Confirm(confirm) = state.dialog.as_ref().expect("dialog") else {
                panic!("expected the update plan dialog");
            };
            assert!(
                confirm
                    .lines
                    .iter()
                    .any(|line| line.contains("update testing")),
                "{:?}",
                confirm.lines
            );
        }
        confirm_applied(&mut state, &services);
        let updated = std::fs::read_to_string(env.target().join("testing/SKILL.md")).expect("read");
        assert!(updated.contains("Updated testing body."));
        assert_eq!(
            state
                .snapshot
                .as_ref()
                .expect("snapshot")
                .dashboard()
                .outdated,
            0,
            "the refresh after execution cleared the drift"
        );

        // update --all previews a per-installation summary while there is
        // work (new drift), and is a toast when everything is current.
        write_skill_tree(env.repo.path(), "quality", "testing", "Second update body.");
        env.repo.commit_all("beskar: update testing again");
        plan_action(&mut state, &services, GuiAction::UpdateAll);
        let Dialog::Confirm(confirm) = state.dialog.as_ref().expect("dialog") else {
            panic!("expected the update-all summary");
        };
        assert!(
            confirm
                .lines
                .iter()
                .any(|line| line.contains("1 installation(s)")),
            "{:?}",
            confirm.lines
        );
        assert!(
            confirm.lines.iter().any(|line| line.contains("(planned)")),
            "{:?}",
            confirm.lines
        );
        confirm_applied(&mut state, &services);
        let updated = std::fs::read_to_string(env.target().join("testing/SKILL.md")).expect("read");
        assert!(updated.contains("Second update body."));

        // §22: a fully-current update --all is a toast, never a dialog.
        plan_action(&mut state, &services, GuiAction::UpdateAll);
        assert!(state.dialog.is_none());
        assert!(
            state
                .toast
                .as_deref()
                .is_some_and(|t| t.contains("nothing to do")),
            "{:?}",
            state.toast
        );
    }

    #[test]
    fn spec_133_workflow_through_the_controller() {
        // §133/§137.32 end to end at the GUI level: browse → compose →
        // install several profiles → inspect the union/why → drift →
        // update — every mutation through plan → confirm → execute.
        let env = Env::new();
        let services = env.services();
        let mut state = GuiState::new();

        // Browse (§133.1-4).
        state.goto(crate::state::Page::Skills);
        refresh(&mut state, Some(&services));
        {
            let snapshot = state.snapshot.as_ref().expect("snapshot");
            assert_eq!(snapshot.skills.len(), 4);
            assert_eq!(snapshot.profiles.len(), 2);
            assert_eq!(snapshot.library.branch.as_deref(), Some("main"));
        }

        // Compose (§133.5-7): create a profile and add a skill to it.
        state.goto(crate::state::Page::Profiles);
        plan_action(
            &mut state,
            &services,
            GuiAction::ProfileCreate {
                name: "github".to_owned(),
                description: Some("GitHub workflows".to_owned()),
            },
        );
        confirm_applied(&mut state, &services);
        plan_action(
            &mut state,
            &services,
            GuiAction::ProfileAddSkills {
                profile: "github".to_owned(),
                skills: vec!["testing".to_owned()],
            },
        );
        confirm_applied(&mut state, &services);
        assert_eq!(
            state.snapshot.as_ref().expect("snapshot").profiles.len(),
            3,
            "the composed profile exists"
        );

        // Install (§133.8-10): first profile via the new-installation path,
        // second profile staged through the §104 workflow.
        state.goto(crate::state::Page::Installations);
        plan_action(
            &mut state,
            &services,
            GuiAction::AttachProfile {
                workspace: env.workspace.clone(),
                target: None,
                profile: "dev-core".to_owned(),
            },
        );
        {
            let Dialog::Confirm(confirm) = state.dialog.as_ref().expect("dialog") else {
                panic!("expected the install plan");
            };
            let body = confirm.lines.join("\n");
            assert!(body.contains("install git-workflow"), "{body}");
            assert!(body.contains("install testing"), "{body}");
        }
        confirm_applied(&mut state, &services);
        assert!(env.target().join("git-workflow/SKILL.md").is_file());

        stage_workflow(
            &mut state,
            &services,
            PreviewMode::Attach,
            "rust-development",
        );
        confirm_applied(&mut state, &services);
        assert_eq!(
            state.snapshot.as_ref().expect("snapshot").installations[0]
                .installation
                .profiles
                .len(),
            2,
            "§103: both attachments visible"
        );

        // Why (§133.11/§100): the shared skill lists ALL requiring profiles.
        let installation_id = state.snapshot.as_ref().expect("snapshot").installations[0]
            .installation
            .id;
        load_membership(&mut state, &services, installation_id, "testing");
        let membership = state.installations.membership.as_ref().expect("membership");
        assert_eq!(membership.required_by.len(), 2, "§37: both owners");

        // Drift + update (§133.13-14): the Library moves, the page shows
        // the drift, the update plan previews, the execute clears it.
        write_skill_tree(
            env.repo.path(),
            "quality",
            "testing",
            "Updated testing body.",
        );
        env.repo.commit_all("beskar: update testing");
        refresh(&mut state, Some(&services));
        assert_eq!(
            state
                .snapshot
                .as_ref()
                .expect("snapshot")
                .dashboard()
                .outdated,
            1
        );
        plan_action(
            &mut state,
            &services,
            GuiAction::UpdateInstallation {
                workspace: env.workspace.clone(),
                target: None,
            },
        );
        confirm_applied(&mut state, &services);
        let snapshot = state.snapshot.as_ref().expect("snapshot");
        assert_eq!(snapshot.dashboard().outdated, 0);
        let registry = env.registry();
        assert_eq!(registry.installations.len(), 1);
        assert_eq!(registry.installations[0].profiles.len(), 2);
    }

    #[test]
    fn typed_failures_surface_through_the_controller() {
        // §115: a failing mutation shows its stable code, never prose to
        // parse, and the UI keeps working.
        let env = Env::new();
        let services = env.services();
        let mut state = GuiState::new();
        refresh(&mut state, Some(&services));

        plan_action(
            &mut state,
            &services,
            GuiAction::AttachProfile {
                workspace: env.workspace.clone(),
                target: None,
                profile: "no-such-profile".to_owned(),
            },
        );
        let Dialog::Message(message) = state.dialog.as_ref().expect("dialog") else {
            panic!("expected the error dialog");
        };
        assert!(message.failed);
        assert!(
            message.lines[0].starts_with("error ("),
            "{:?}",
            message.lines[0]
        );
        assert!(state.activity.entries[0].failed);
    }
}

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

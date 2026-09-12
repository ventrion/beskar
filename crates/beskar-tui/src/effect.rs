//! Effects: what the app asks the runtime to do (spec §112 "invocation of
//! core plans/actions").
//!
//! The reducer never performs I/O itself; it returns [`Effect`]s, the
//! terminal runtime fulfills each one against beskar-core/beskar-git, and
//! the result comes back as a [`crate::event::Event`]. This keeps every
//! state transition pure and unit-testable without a terminal.

use beskar_core::reconcile::ReconcileOptions;

use crate::app::PendingAction;

/// A command for the runtime (all fulfilled via the shared core services,
/// §105).
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Re-gather the full data snapshot from core read APIs (§41: read-only,
    /// no network).
    Refresh,
    /// Load one skill's detail + `SKILL.md` preview (§97 pane 3).
    LoadSkill { skill: String },
    /// Load the §100 membership view of one skill in one installation.
    Membership {
        installation: beskar_core::ids::InstallationId,
        skill: String,
        /// Library-relative skill path from the snapshot (§119).
        path: Option<String>,
    },
    /// Plan a mutation as a dry run (§91, §135.39: same planner, no writes).
    Plan {
        action: Box<PendingAction>,
        options: ReconcileOptions,
    },
    /// Execute a confirmed mutation (§89 step 4).
    Execute {
        action: Box<PendingAction>,
        options: ReconcileOptions,
    },
    /// Run the read-only profile validation (§76 `profile validate`).
    Validate { profile: Option<String> },
}

impl Effect {
    /// A short label shown while the effect runs.
    pub fn label(&self) -> String {
        match self {
            Effect::Refresh => "refreshing…".to_owned(),
            Effect::LoadSkill { skill } => format!("loading {skill}…"),
            Effect::Membership { skill, .. } => format!("resolving {skill}…"),
            Effect::Plan { action, .. } => format!("planning {}…", action.title()),
            Effect::Execute { action, .. } => format!("applying {}…", action.title()),
            Effect::Validate { .. } => "validating profiles…".to_owned(),
        }
    }
}

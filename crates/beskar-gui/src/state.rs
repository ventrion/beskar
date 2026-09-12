//! Pure GUI application state (spec §103-§104, §113).
//!
//! No I/O, no rendering, no egui types: everything here is presentation
//! state over snapshots of core data plus the transition methods the view
//! and controller call. Every transition is unit-testable without opening a
//! window. UI state never alters domain semantics (§105): mutations are
//! only ever *described* here as [`GuiAction`] values, which the controller
//! fulfills through the same core services the CLI and TUI use.
//!
//! Mutation flow (§89, §135.38-39): an action is planned (dry-run through
//! the real planner), the REAL serializable plan is shown in a confirm
//! dialog, then executed. Blocked plans are displayed with all blockers;
//! only ModifiedContent/UnmanagedCollision blockers offer explicit-consent
//! force re-plans (§47, §49). The §104 attach/detach workflow additionally
//! gates Apply behind a computed membership preview.

use std::path::PathBuf;

use beskar_core::drift::{DriftState, MembershipDrift};
use beskar_core::editing::{
    BranchDisplay, LibraryPlan, LibraryStatusReport, ProfileValidation, SkillDetail, SkillListing,
};
use beskar_core::ids::{InstallationId, ProfileId};
use beskar_core::plan::{Blocker, BlockerKind, PlanAction, ReconciliationPlan};
use beskar_core::reconcile::ReconcileOptions;
use beskar_core::registry::Installation;
use beskar_core::remote::{BranchSyncState, FetchOutcome, PushOutcome, PushState};
use beskar_core::status::InstallationStatus;

use crate::action::GuiAction;
use crate::preview::{MembershipPreview, PreviewMode};

// ---- pages (§103) -----------------------------------------------------------

/// The primary GUI pages (§103). Settings is GUI-specific: read-only
/// environment/config diagnostics (§85, §86) — it must never alter domain
/// semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Dashboard,
    Skills,
    Profiles,
    Installations,
    Git,
    Settings,
    Activity,
}

impl Page {
    /// All pages in §103 order.
    pub const ALL: [Page; 7] = [
        Page::Dashboard,
        Page::Skills,
        Page::Profiles,
        Page::Installations,
        Page::Git,
        Page::Settings,
        Page::Activity,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Dashboard => "Dashboard",
            Page::Skills => "Skills",
            Page::Profiles => "Profiles",
            Page::Installations => "Installations",
            Page::Git => "Git",
            Page::Settings => "Settings",
            Page::Activity => "Activity",
        }
    }
}

// ---- snapshot ---------------------------------------------------------------

/// One registered installation plus its read-only status (§41), when the
/// status could be computed at all (broken registrations surface the error).
#[derive(Debug, Clone)]
pub struct InstallationRow {
    pub installation: Installation,
    pub status: Option<InstallationStatus>,
    pub error: Option<String>,
}

impl InstallationRow {
    /// Display label: `target — workspace` (§99).
    pub fn label(&self) -> String {
        format!(
            "{} — {}",
            self.installation.target,
            self.installation.workspace.display()
        )
    }
}

/// A full data snapshot for rendering, gathered from core read APIs (§41:
/// read-only, no network).
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub library: LibraryStatusReport,
    pub skills: Vec<SkillListing>,
    pub profiles: Vec<beskar_core::profile::Profile>,
    pub installations: Vec<InstallationRow>,
    pub branches: Vec<BranchDisplay>,
}

impl Snapshot {
    pub fn skill(&self, name: &str) -> Option<&SkillListing> {
        self.skills.iter().find(|s| s.name.as_str() == name)
    }

    pub fn installation(&self, id: InstallationId) -> Option<&InstallationRow> {
        self.installations.iter().find(|r| r.installation.id == id)
    }

    pub fn profile(&self, name: &str) -> Option<&beskar_core::profile::Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// Unique buckets beneath `skills/` in name order (§13: organization
    /// only).
    pub fn buckets(&self) -> Vec<String> {
        let mut buckets: Vec<String> = self
            .skills
            .iter()
            .map(|s| s.bucket.clone())
            .filter(|b| !b.is_empty())
            .collect();
        buckets.sort();
        buckets.dedup();
        buckets
    }

    /// The dashboard metrics (§96, mirrored in the GUI per §105).
    pub fn dashboard(&self) -> DashboardMetrics {
        let attachments = self
            .installations
            .iter()
            .map(|row| row.installation.profiles.len())
            .sum();
        let mut metrics = DashboardMetrics {
            skills: self.skills.len(),
            profiles: self.profiles.len(),
            installations: self.installations.len(),
            attachments,
            outdated: 0,
            modified: 0,
            broken: 0,
            missing_profiles: 0,
        };
        for row in &self.installations {
            if let Some(status) = &row.status {
                if status.count(DriftState::Outdated) > 0 {
                    metrics.outdated += 1;
                }
                if status.count(DriftState::Modified) > 0 {
                    metrics.modified += 1;
                }
                metrics.missing_profiles += status
                    .profiles
                    .iter()
                    .filter(|p| p.profile.is_none())
                    .count();
            }
            let broken = row.error.is_some()
                || row
                    .status
                    .as_ref()
                    .is_some_and(|s| s.installation_state.is_some())
                || row
                    .status
                    .as_ref()
                    .is_some_and(|s| s.profiles.iter().any(|p| p.profile.is_none()));
            if broken {
                metrics.broken += 1;
            }
        }
        metrics
    }
}

/// Dashboard metrics (§96).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DashboardMetrics {
    pub skills: usize,
    pub profiles: usize,
    pub installations: usize,
    /// Total profile attachments across all installations (§96).
    pub attachments: usize,
    /// Installations with at least one outdated skill (§96).
    pub outdated: usize,
    /// Installations with at least one modified skill (§96).
    pub modified: usize,
    /// Installations that are broken: failed status, missing
    /// workspace/target/ref, or a missing attached profile (§96).
    pub broken: usize,
    /// Missing attached profiles across all installations (§39).
    pub missing_profiles: usize,
}

/// The Skills-page preview payload: skill detail plus the `SKILL.md` bytes
/// as they exist in the Library working tree (the editing surface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillView {
    pub name: String,
    pub detail: SkillDetail,
    pub skill_md: String,
}

/// The §100 membership view for one skill in one installation (first-class
/// in the GUI as well, §133.11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipView {
    pub installation: InstallationId,
    pub skill: String,
    /// Current desired requiring profiles (ID, display name) (§93).
    pub required_by: Vec<(ProfileId, String)>,
    /// Requiring profiles at last successful apply (§27).
    pub last_required_by: Vec<(ProfileId, String)>,
    pub source_ref: String,
    /// The exact Library commit the status resolved against (§19).
    pub library_commit: Option<String>,
    /// The skill's most recent commit at or before the source ref (§35).
    pub skill_commit: Option<String>,
    pub state: Option<DriftState>,
    pub membership_drift: Option<MembershipDrift>,
}

// ---- plans, confirmations, outcomes (§89) -----------------------------------

/// What a planned mutation looked like — always the REAL core plan
/// (§135.38-39: planning is separated from confirmation and execution;
/// dry-run uses the same planner).
#[derive(Debug, Clone, PartialEq)]
pub enum Planned {
    /// A reconciliation plan over one installation (§89, §109).
    Install {
        title: String,
        plan: ReconciliationPlan,
    },
    /// A scoped Library-editing plan (§72, §89).
    Library {
        title: String,
        plan: LibraryPlan,
        notes: Vec<String>,
    },
    /// A textual preview for registry-only/branch-level operations that
    /// have no reconciliation plan (§79 reorder, branch helpers, update-all
    /// summary).
    Summary { title: String, lines: Vec<String> },
    /// The fetch plan from a dry run (§62, §91: no network, no writes).
    Fetch { outcome: FetchOutcome },
    /// The push plan from a dry run (§65, §91).
    Push { outcome: PushOutcome },
}

impl Planned {
    pub fn title(&self) -> &str {
        match self {
            Planned::Install { title, .. }
            | Planned::Library { title, .. }
            | Planned::Summary { title, .. } => title,
            Planned::Fetch { .. } => "Fetch from remote",
            Planned::Push { .. } => "Push branch",
        }
    }
}

/// What confirming one choice in a [`ConfirmDialog`] does.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmApply {
    /// Execute the mutation for real (§89 step 4).
    Run {
        action: Box<GuiAction>,
        options: ReconcileOptions,
    },
    /// Re-plan with explicit consent options (§47 interactive `--force`:
    /// the exact managed files to be discarded are displayed first).
    Replan {
        action: Box<GuiAction>,
        options: ReconcileOptions,
    },
    Cancel,
}

/// One selectable choice in a [`ConfirmDialog`].
#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmChoice {
    pub label: String,
    pub apply: ConfirmApply,
}

/// The plan-confirmation dialog: shows the real core plan (§89 step 3).
#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    pub title: String,
    pub lines: Vec<String>,
    pub choices: Vec<ConfirmChoice>,
}

/// Why a staged multi-attachment run stopped early (§47, §60): the REAL
/// plan of the profile that blocked, plus the continuation action covering
/// the profiles not yet applied. Earlier profiles remain applied — each is
/// a complete §89 transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockedStop {
    pub plan: ReconciliationPlan,
    pub continuation: GuiAction,
}

/// The summary of an executed mutation, appended to the Activity log (§95).
#[derive(Debug, Clone, PartialEq)]
pub struct Executed {
    pub title: String,
    /// Whether any state changed (false for no-ops and blocked runs).
    pub applied: bool,
    pub lines: Vec<String>,
    pub warnings: Vec<String>,
    /// Set when a staged multi-attachment run stopped on a blocked plan.
    pub stop: Option<BlockedStop>,
}

// ---- dialogs -----------------------------------------------------------------

/// One labeled text field in an [`InputDialog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputField {
    pub label: String,
    pub buffer: String,
}

impl InputField {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            buffer: String::new(),
        }
    }

    fn value(&self) -> String {
        self.buffer.trim().to_owned()
    }
}

/// Which pending action an [`InputDialog`] builds on submit.
#[derive(Debug, Clone, PartialEq)]
pub enum InputKind {
    Ingest,
    SkillMove {
        skill: String,
    },
    SkillRename {
        skill: String,
    },
    SkillTag {
        skill: String,
    },
    SkillRank {
        skill: String,
    },
    ProfileCreate,
    ProfileRename {
        old: String,
    },
    RefSet {
        workspace: PathBuf,
        target: Option<String>,
    },
    CreateBranch,
    /// First install from the Installations page (§21): attach one profile
    /// to a possibly-new `(workspace, target)` pair.
    NewInstallation,
}

/// A text-input dialog.
#[derive(Debug, Clone)]
pub struct InputDialog {
    pub title: String,
    pub fields: Vec<InputField>,
    pub kind: InputKind,
}

/// Which pending action a [`PickDialog`] builds on choose.
#[derive(Debug, Clone, PartialEq)]
pub enum PickKind {
    /// Add a skill to a Library profile (§76).
    AddToProfile { skill: String },
    /// Add one Library skill to the selected profile (§76).
    ProfileAddSkill { profile: String },
    /// Skill removal; `values` are `"false"`/`"true"` cascade flags (§75).
    SkillRemove { skill: String },
    /// Unregister mode; `values` are `"false"`/`"true"` keep-files flags
    /// (§55).
    Unregister {
        workspace: PathBuf,
        target: Option<String>,
    },
}

/// A single-choice dialog. Labels display; `values` are machine inputs.
#[derive(Debug, Clone)]
pub struct PickDialog {
    pub title: String,
    pub options: Vec<String>,
    pub values: Vec<String>,
    pub kind: PickKind,
}

/// A read-only message dialog (errors display their stable §115 code, so
/// the UI never needs prose parsing to classify).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageDialog {
    pub title: String,
    pub lines: Vec<String>,
    pub failed: bool,
}

/// Every modal overlay (§113: dialogs).
#[derive(Debug, Clone)]
pub enum Dialog {
    Confirm(ConfirmDialog),
    Input(InputDialog),
    Pick(PickDialog),
    Message(MessageDialog),
    /// The §100 first-class membership view.
    Membership(Box<MembershipView>),
}

// ---- panels + the §104 workflow ----------------------------------------------

/// Skills page state (§97 equivalent, §133.2-5).
#[derive(Debug, Clone, Default)]
pub struct SkillsPanel {
    /// Lexical query over names/descriptions/tags (§97 "searchable").
    pub query: String,
    /// Selected bucket filter; `None` = all buckets.
    pub bucket: Option<String>,
    /// Selected skill by canonical name.
    pub skill: Option<String>,
    /// Loaded preview for the selected skill.
    pub preview: Option<SkillView>,
    /// The skill the last preview load was attempted for — prevents the
    /// view from retrying a failed load every frame (the error dialog is
    /// shown once).
    pub preview_attempted: Option<String>,
}

/// Profiles page state (§98).
#[derive(Debug, Clone, Default)]
pub struct ProfilesPanel {
    /// Selected profile by name.
    pub profile: Option<String>,
    /// Highlighted skill index within the selected profile's ordered list.
    pub skill_index: usize,
    /// The last `profile validate` results (§76), kept until the next run.
    pub validations: Option<Vec<ProfileValidation>>,
}

/// The staged §104 attach/detach workflow on the Installations page:
/// select profiles → preview effective membership changes → inspect →
/// apply (§104 steps 2-6).
#[derive(Debug, Clone)]
pub struct Workflow {
    pub mode: PreviewMode,
    /// Selected profile names (attach) or attachments (detach), in
    /// selection order.
    pub selected: Vec<String>,
    /// The computed preview; `None` until Preview runs for the current
    /// selection (a stale preview is cleared on every selection change).
    pub preview: Option<MembershipPreview>,
    pub preview_error: Option<String>,
}

impl Workflow {
    fn new(mode: PreviewMode) -> Self {
        Self {
            mode,
            selected: Vec::new(),
            preview: None,
            preview_error: None,
        }
    }

    fn toggle(&mut self, name: &str) {
        if let Some(position) = self.selected.iter().position(|s| s == name) {
            self.selected.remove(position);
        } else {
            self.selected.push(name.to_owned());
        }
        // A stale preview must never survive a selection change (§4).
        self.preview = None;
        self.preview_error = None;
    }

    pub fn selected(&self, name: &str) -> bool {
        self.selected.iter().any(|s| s == name)
    }
}

/// Installations page state (§99, §104).
#[derive(Debug, Clone, Default)]
pub struct InstallationsPanel {
    /// Selected installation by ID.
    pub installation: Option<InstallationId>,
    /// The staged §104 workflow, when open.
    pub workflow: Option<Workflow>,
    /// The §100 membership view for the last inspected skill.
    pub membership: Option<MembershipView>,
}

/// Git page state (§101).
#[derive(Debug, Clone, Default)]
pub struct GitPanel {
    /// Selected branch by name.
    pub branch: Option<String>,
    /// Remote input (default `origin`, §62/§65).
    pub remote: String,
    /// Push input: branch (blank = current branch, §65).
    pub push_branch: String,
    pub set_upstream: bool,
    pub allow_dirty: bool,
}

// ---- activity log (§95) -------------------------------------------------------

/// One Activity entry (newest first in [`ActivityLog`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityEntry {
    pub time: String,
    pub title: String,
    pub lines: Vec<String>,
    /// Whether the entry reports a failure (§115 typed errors).
    pub failed: bool,
}

/// The Activity log (§95). Bounded; newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityLog {
    pub entries: Vec<ActivityEntry>,
    pub limit: usize,
}

impl Default for ActivityLog {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            limit: 200,
        }
    }
}

impl ActivityLog {
    pub fn push(&mut self, entry: ActivityEntry) {
        self.entries.insert(0, entry);
        self.entries.truncate(self.limit);
    }
}

// ---- the application state ----------------------------------------------------

/// The whole GUI application state. Pure data: the view renders it; the
/// controller fulfills the actions it accumulates.
#[derive(Debug, Clone)]
pub struct GuiState {
    pub page: Page,
    /// The latest data snapshot; `None` until the first refresh completes.
    pub snapshot: Option<Snapshot>,
    /// Why the last refresh (or library discovery) failed, rendered from
    /// the typed error (code + message; §115).
    pub load_error: Option<String>,
    /// Label of the operation currently running (core calls are
    /// synchronous in v1).
    pub busy: Option<String>,
    /// One-line status message shown in the footer until replaced.
    pub toast: Option<String>,
    pub skills: SkillsPanel,
    pub profiles: ProfilesPanel,
    pub installations: InstallationsPanel,
    pub git: GitPanel,
    pub activity: ActivityLog,
    pub dialog: Option<Dialog>,
}

impl Default for GuiState {
    fn default() -> Self {
        Self::new()
    }
}

impl GuiState {
    pub fn new() -> Self {
        Self {
            page: Page::Dashboard,
            snapshot: None,
            load_error: None,
            busy: None,
            toast: None,
            skills: SkillsPanel::default(),
            profiles: ProfilesPanel::default(),
            installations: InstallationsPanel::default(),
            git: GitPanel {
                remote: "origin".to_owned(),
                ..GitPanel::default()
            },
            activity: ActivityLog::default(),
            dialog: None,
        }
    }

    // ---- navigation + refresh -------------------------------------------

    pub fn goto(&mut self, page: Page) {
        self.page = page;
    }

    /// Feeds a fresh snapshot in, repairing selections that no longer
    /// resolve (dangling names/ids are dropped, never re-pointed).
    pub fn loaded(&mut self, snapshot: Snapshot) {
        if let Some(skill) = &self.skills.skill
            && snapshot.skill(skill).is_none()
        {
            self.skills.skill = None;
            self.skills.preview = None;
            self.skills.preview_attempted = None;
        }
        if let Some(profile) = &self.profiles.profile
            && snapshot.profile(profile).is_none()
        {
            self.profiles.profile = None;
            self.profiles.skill_index = 0;
        }
        if let Some(id) = self.installations.installation
            && snapshot.installation(id).is_none()
        {
            self.installations.installation = None;
            self.installations.workflow = None;
            self.installations.membership = None;
        }
        if let Some(branch) = &self.git.branch
            && !snapshot.branches.iter().any(|b| &b.name == branch)
        {
            self.git.branch = None;
        }
        // Prefill the push input with the current branch (§65: the default
        // push target); never clobbers a value the user typed.
        if self.git.push_branch.is_empty()
            && let Some(branch) = &snapshot.library.branch
        {
            self.git.push_branch = branch.clone();
        }
        self.repair_workflow();
        self.load_error = None;
        self.snapshot = Some(snapshot);
    }

    /// Records a refresh failure; the last snapshot stays visible under the
    /// error banner (read-only diagnostics are never lost silently).
    pub fn load_failed(&mut self, message: String) {
        self.load_error = Some(message);
    }

    /// Drops workflow selections that no longer resolve (§4: never keep a
    /// stale selection).
    fn repair_workflow(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            self.installations.workflow = None;
            return;
        };
        let Some(id) = self.installations.installation else {
            self.installations.workflow = None;
            return;
        };
        let Some(row) = snapshot.installation(id) else {
            self.installations.workflow = None;
            return;
        };
        let Some(workflow) = &mut self.installations.workflow else {
            return;
        };
        workflow.selected.retain(|name| match workflow.mode {
            PreviewMode::Attach => snapshot.profile(name).is_some(),
            // Detach selections are attachment names (last-known, §28).
            PreviewMode::Detach => row
                .installation
                .profiles
                .iter()
                .any(|attachment| attachment.name == *name),
        });
        if workflow.selected.is_empty() {
            workflow.preview = None;
            workflow.preview_error = None;
        }
    }

    pub fn set_busy(&mut self, label: Option<String>) {
        self.busy = label;
    }

    // ---- selection --------------------------------------------------------

    pub fn selected_installation(&self) -> Option<&InstallationRow> {
        let id = self.installations.installation?;
        self.snapshot.as_ref()?.installation(id)
    }

    pub fn selected_profile(&self) -> Option<&beskar_core::profile::Profile> {
        let name = self.profiles.profile.as_deref()?;
        self.snapshot.as_ref()?.profile(name)
    }

    /// The skills passing the active bucket filter and lexical query
    /// (presentation filtering over the snapshot; §97 lexical search).
    pub fn filtered_skills(&self) -> Vec<&SkillListing> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let query = self.skills.query.to_lowercase();
        snapshot
            .skills
            .iter()
            .filter(|skill| match &self.skills.bucket {
                Some(bucket) => skill.bucket.starts_with(bucket.as_str()),
                None => true,
            })
            .filter(|skill| {
                query.is_empty()
                    || skill.name.as_str().to_lowercase().contains(&query)
                    || skill.description.to_lowercase().contains(&query)
                    || skill
                        .tags
                        .iter()
                        .any(|tag| tag.to_lowercase().contains(&query))
            })
            .collect()
    }

    pub fn select_skill(&mut self, name: &str) {
        if self.skills.skill.as_deref() != Some(name) {
            self.skills.skill = Some(name.to_owned());
            self.skills.preview = None;
            self.skills.preview_attempted = None;
        }
    }

    pub fn skill_loaded(&mut self, view: SkillView) {
        if self.skills.skill.as_deref() == Some(view.name.as_str()) {
            self.skills.preview = Some(view);
        }
    }

    pub fn select_profile(&mut self, name: &str) {
        self.profiles.profile = Some(name.to_owned());
        self.profiles.skill_index = 0;
    }

    pub fn select_installation(&mut self, id: InstallationId) {
        if self.installations.installation != Some(id) {
            self.installations.installation = Some(id);
            self.installations.workflow = None;
            self.installations.membership = None;
        }
    }

    pub fn select_branch(&mut self, name: &str) {
        self.git.branch = Some(name.to_owned());
    }

    // ---- the §104 workflow ------------------------------------------------

    /// Opens the staged attach (or detach) workflow for the selected
    /// installation (§104 steps 1-2).
    pub fn begin_workflow(&mut self, mode: PreviewMode) {
        if self.installations.installation.is_some() {
            self.installations.workflow = Some(Workflow::new(mode));
        }
    }

    pub fn close_workflow(&mut self) {
        self.installations.workflow = None;
    }

    /// Toggles one profile in the staged selection, invalidating any
    /// preview (§104 step 3; a stale preview is never shown).
    pub fn toggle_workflow_profile(&mut self, name: &str) {
        if let Some(workflow) = &mut self.installations.workflow {
            workflow.toggle(name);
        }
    }

    /// Feeds a computed preview in (§104 step 4).
    pub fn preview_loaded(&mut self, preview: MembershipPreview) {
        if let Some(workflow) = &mut self.installations.workflow {
            workflow.preview = Some(preview);
            workflow.preview_error = None;
        }
    }

    pub fn preview_failed(&mut self, message: String) {
        if let Some(workflow) = &mut self.installations.workflow {
            workflow.preview_error = Some(message);
        }
    }

    /// Builds the action a confirmed §104 apply executes, after enforcing
    /// the §104 ordering: a non-empty selection and a fresh preview must
    /// exist before Apply is reachable (§104 steps 3-6).
    pub fn workflow_apply_action(&self) -> Option<GuiAction> {
        let row = self.selected_installation()?;
        let workflow = self.installations.workflow.as_ref()?;
        if workflow.selected.is_empty() || workflow.preview.is_none() {
            return None;
        }
        let action = match workflow.mode {
            PreviewMode::Attach => GuiAction::AttachProfiles {
                workspace: row.installation.workspace.clone(),
                target: Some(row.installation.target.clone()),
                profiles: workflow.selected.clone(),
            },
            PreviewMode::Detach => GuiAction::DetachProfiles {
                workspace: row.installation.workspace.clone(),
                target: Some(row.installation.target.clone()),
                profiles: workflow.selected.clone(),
            },
        };
        Some(action)
    }

    /// The confirm-dialog body for a staged apply: the §104 preview plus
    /// transaction semantics notes (§54, §60).
    pub fn workflow_confirm(&self) -> Option<ConfirmDialog> {
        let workflow = self.installations.workflow.as_ref()?;
        let preview = workflow.preview.as_ref()?;
        if workflow.selected.is_empty() {
            return None;
        }
        let mut lines = preview.lines();
        if preview.is_no_op() {
            return None;
        }
        match workflow.mode {
            PreviewMode::Attach => {
                if workflow.selected.len() > 1 {
                    lines.push(String::new());
                    lines.push(
                        "Applied as sequential additions; each one plans, \
                         reconciles, and persists as a complete transaction \
                         (§89, §60)."
                            .to_owned(),
                    );
                }
            }
            PreviewMode::Detach => {
                if workflow.selected.len() == self.attached_profile_count() {
                    lines.push(String::new());
                    lines.push(
                        "Detaching every profile leaves the installation \
                         registered but empty (§54); use Unregister to \
                         remove the record."
                            .to_owned(),
                    );
                }
                if workflow.selected.len() > 1 {
                    lines.push(
                        "Applied as sequential detachments; each one plans, \
                         reconciles, and persists as a complete transaction \
                         (§89, §52)."
                            .to_owned(),
                    );
                }
            }
        }
        Some(ConfirmDialog {
            title: format!("{} {}", workflow.mode.verb(), workflow.selected.join(", ")),
            lines,
            choices: vec![ConfirmChoice {
                label: "Apply".to_owned(),
                apply: ConfirmApply::Run {
                    action: Box::new(self.workflow_apply_action()?),
                    options: ReconcileOptions::default(),
                },
            }],
        })
    }

    fn attached_profile_count(&self) -> usize {
        self.selected_installation()
            .map(|row| row.installation.profiles.len())
            .unwrap_or(0)
    }

    // ---- planning + confirmation (§89 steps 2-3) ---------------------------

    /// Feeds a computed dry-run plan in and opens the confirm dialog
    /// (§89 step 3). No-op plans never reach a dialog (§22, §91).
    pub fn planned(&mut self, action: GuiAction, planned: Planned, options: ReconcileOptions) {
        let title = planned.title().to_owned();
        match planned {
            Planned::Install { plan, .. } => self.open_plan_confirm(&title, plan, action, options),
            Planned::Library { plan, notes, .. } => {
                self.open_library_confirm(&title, plan, notes, action)
            }
            Planned::Summary { lines, .. } => {
                // Only the service's explicit no-op line suppresses the
                // dialog — never prose matching beyond it (the update-all
                // summary says "all-or-nothing mode", §48).
                if lines.iter().any(|line| line.contains("nothing to do")) {
                    self.toast = Some(format!("{title}: nothing to do"));
                    return;
                }
                self.open_generic_confirm(&title, lines, action, options);
            }
            Planned::Fetch { outcome } => {
                self.open_generic_confirm(
                    "Fetch from remote",
                    fetch_lines(&outcome),
                    action,
                    options,
                );
            }
            Planned::Push { outcome } => {
                self.open_generic_confirm("Push branch", push_lines(&outcome), action, options);
            }
        }
    }

    fn open_generic_confirm(
        &mut self,
        title: &str,
        lines: Vec<String>,
        action: GuiAction,
        options: ReconcileOptions,
    ) {
        self.dialog = Some(Dialog::Confirm(ConfirmDialog {
            title: title.to_owned(),
            lines,
            choices: vec![
                ConfirmChoice {
                    label: "Apply".to_owned(),
                    apply: ConfirmApply::Run {
                        action: Box::new(action),
                        options,
                    },
                },
                ConfirmChoice {
                    label: "Cancel".to_owned(),
                    apply: ConfirmApply::Cancel,
                },
            ],
        }));
    }

    /// The confirm dialog for one installation plan (§89 step 3, §47, §49):
    /// normal plans offer Apply; blocked plans offer explicit-consent force
    /// re-plans only when every blocker is ModifiedContent and/or
    /// UnmanagedCollision; no-ops are toasts.
    pub fn open_plan_confirm(
        &mut self,
        title: &str,
        plan: ReconciliationPlan,
        action: GuiAction,
        options: ReconcileOptions,
    ) {
        let lines = plan_lines(&plan);
        if plan.is_no_op() {
            self.toast = Some(format!("{title}: nothing to do — already up to date"));
            return;
        }
        let mut lines = lines;
        if plan.is_blocked() {
            lines.push(String::new());
            lines.push(
                "Blocked: nothing will be written without explicit consent \
                 (§47, §49)."
                    .to_owned(),
            );
            if waivable_options(&plan).is_none() {
                lines.push(
                    "These blockers cannot be waived by consent; resolve \
                     them first (see `beskar doctor`)."
                        .to_owned(),
                );
            }
            let choices = waiver_choices(&plan, &action, options);
            self.dialog = Some(Dialog::Confirm(ConfirmDialog {
                title: title.to_owned(),
                lines,
                choices,
            }));
            return;
        }
        lines.push(String::new());
        self.open_generic_confirm(title, lines, action, options);
    }

    /// The confirm dialog for a Library-editing plan (§72, §75).
    pub fn open_library_confirm(
        &mut self,
        title: &str,
        plan: LibraryPlan,
        notes: Vec<String>,
        action: GuiAction,
    ) {
        if plan.is_no_op() {
            self.toast = Some(format!("{title}: nothing to do"));
            return;
        }
        let mut lines = Vec::new();
        for path in plan.owned_paths() {
            lines.push(format!("  {path}"));
        }
        for note in &notes {
            lines.push(String::new());
            lines.push(note.clone());
        }
        self.open_generic_confirm(title, lines, action, ReconcileOptions::default());
    }

    /// Pops the confirm dialog and returns the chosen apply, if the dialog
    /// is a confirm dialog with that choice index.
    pub fn confirm_choice(&mut self, index: usize) -> Option<ConfirmApply> {
        match self.dialog.take() {
            Some(Dialog::Confirm(confirm)) => {
                let apply = confirm
                    .choices
                    .get(index)
                    .map(|choice| choice.apply.clone());
                if apply.is_none() {
                    self.dialog = Some(Dialog::Confirm(confirm));
                }
                apply
            }
            other => {
                self.dialog = other;
                None
            }
        }
    }

    /// Feeds an execution outcome in: logs it, and when a staged run
    /// stopped on a blocked plan, re-opens the §47 consent flow for the
    /// continuation (§47: report every blocker, require explicit consent).
    pub fn executed(&mut self, executed: Executed) {
        let failed = executed.stop.is_some();
        let mut lines = executed.lines.clone();
        for warning in &executed.warnings {
            lines.push(format!("warning: {warning}"));
        }
        self.activity.push(ActivityEntry {
            time: timestamp(),
            title: executed.title.clone(),
            lines,
            failed,
        });
        self.toast = Some(if failed {
            format!(
                "{}: blocked — nothing applied beyond that point",
                executed.title
            )
        } else if executed.applied {
            format!("{}: applied", executed.title)
        } else {
            format!("{}: done", executed.title)
        });
        if let Some(stop) = executed.stop {
            let continuation = stop.continuation;
            let title = continuation.title();
            self.open_plan_confirm(&title, stop.plan, continuation, ReconcileOptions::default());
        }
    }

    /// Records a typed failure: stable code + message in the Activity log
    /// and a message dialog (§115: the UI classifies by type/code, never by
    /// parsing prose).
    pub fn operation_failed(&mut self, title: &str, err: &beskar_core::Error) {
        self.activity.push(ActivityEntry {
            time: timestamp(),
            title: title.to_owned(),
            lines: vec![format!("error ({}): {}", err.code(), err)],
            failed: true,
        });
        self.dialog = Some(Dialog::Message(MessageDialog {
            title: title.to_owned(),
            lines: vec![
                format!("error ({}): {}", err.code(), err),
                String::new(),
                "See the Activity page and `beskar doctor` for diagnostics.".to_owned(),
            ],
            failed: true,
        }));
    }

    // ---- input + pick dialogs ----------------------------------------------

    fn open_input(&mut self, title: &str, fields: Vec<InputField>, kind: InputKind) {
        self.dialog = Some(Dialog::Input(InputDialog {
            title: title.to_owned(),
            fields,
            kind,
        }));
    }

    pub fn begin_ingest(&mut self) {
        self.open_input(
            "Ingest skill (§70)",
            vec![InputField::new("Source path"), InputField::new("Bucket")],
            InputKind::Ingest,
        );
    }

    pub fn begin_skill_move(&mut self, skill: &str) {
        self.open_input(
            &format!("Move {skill} (§73)"),
            vec![InputField::new("New bucket")],
            InputKind::SkillMove {
                skill: skill.to_owned(),
            },
        );
    }

    pub fn begin_skill_rename(&mut self, skill: &str) {
        self.open_input(
            &format!("Rename {skill} (§74)"),
            vec![InputField::new("New name")],
            InputKind::SkillRename {
                skill: skill.to_owned(),
            },
        );
    }

    pub fn begin_skill_tag(&mut self, skill: &str) {
        self.open_input(
            &format!("Tag {skill} (§14)"),
            vec![
                InputField::new("Add tags (comma-separated)"),
                InputField::new("Remove tags (comma-separated)"),
            ],
            InputKind::SkillTag {
                skill: skill.to_owned(),
            },
        );
    }

    pub fn begin_skill_rank(&mut self, skill: &str) {
        self.open_input(
            &format!("Rank {skill} (blank clears, §14)"),
            vec![InputField::new("Rank (integer)")],
            InputKind::SkillRank {
                skill: skill.to_owned(),
            },
        );
    }

    pub fn begin_profile_create(&mut self) {
        self.open_input(
            "Create profile (§76)",
            vec![
                InputField::new("Name"),
                InputField::new("Description (optional)"),
            ],
            InputKind::ProfileCreate,
        );
    }

    pub fn begin_profile_rename(&mut self, old: &str) {
        self.open_input(
            &format!("Rename profile {old} (keeps the immutable ID, §76)"),
            vec![InputField::new("New name")],
            InputKind::ProfileRename {
                old: old.to_owned(),
            },
        );
    }

    pub fn begin_ref_set(&mut self) {
        let Some(row) = self.selected_installation() else {
            return;
        };
        self.open_input(
            "Change source ref (§56: all profiles move together)",
            vec![InputField::new("New source ref")],
            InputKind::RefSet {
                workspace: row.installation.workspace.clone(),
                target: Some(row.installation.target.clone()),
            },
        );
    }

    pub fn begin_create_branch(&mut self) {
        self.open_input(
            "Create branch (§82: at HEAD, then switch)",
            vec![InputField::new("Branch name")],
            InputKind::CreateBranch,
        );
    }

    pub fn begin_new_installation(&mut self) {
        self.open_input(
            "New installation (§21)",
            vec![
                InputField::new("Workspace path"),
                InputField::new("Target (blank = .agents/skills)"),
                InputField::new("Profile"),
            ],
            InputKind::NewInstallation,
        );
    }

    /// Builds the action an input-dialog submit produces; `None` when a
    /// required field is missing (a toast explains, the dialog stays open).
    pub fn submit_input(&mut self) -> Option<GuiAction> {
        let Some(Dialog::Input(dialog)) = self.dialog.take() else {
            return None;
        };
        let values: Vec<String> = dialog.fields.iter().map(InputField::value).collect();
        // Required-field matrix per dialog kind. Keeps the dialog open and
        // explains what is missing (§4: never guess an unspecified path).
        let required = |kind: &InputKind, index: usize| match kind {
            InputKind::Ingest => index <= 1,
            InputKind::SkillMove { .. }
            | InputKind::SkillRename { .. }
            | InputKind::ProfileCreate
            | InputKind::ProfileRename { .. }
            | InputKind::RefSet { .. }
            | InputKind::CreateBranch => index == 0,
            InputKind::NewInstallation => index == 0 || index == 2,
            // Tags may be empty (no-op); a blank rank clears the rank (§14).
            InputKind::SkillTag { .. } | InputKind::SkillRank { .. } => false,
        };
        if let Some(index) =
            (0..values.len()).find(|&i| values[i].is_empty() && required(&dialog.kind, i))
        {
            self.toast = Some(format!("{} is required", dialog.fields[index].label));
            self.dialog = Some(Dialog::Input(dialog));
            return None;
        }
        let action = match dialog.kind {
            InputKind::Ingest => GuiAction::Ingest {
                source: PathBuf::from(&values[0]),
                bucket: values[1].clone(),
            },
            InputKind::SkillMove { skill } => GuiAction::SkillMove {
                skill,
                bucket: values[0].clone(),
            },
            InputKind::SkillRename { skill } => GuiAction::SkillRename {
                old: skill,
                new: values[0].clone(),
            },
            InputKind::SkillTag { skill } => GuiAction::SkillTag {
                skill,
                add: comma_separated(&values[0]),
                remove: comma_separated(&values[1]),
            },
            InputKind::SkillRank { skill } => GuiAction::SkillRank {
                skill,
                rank: values[0].parse::<i64>().ok(),
            },
            InputKind::ProfileCreate => GuiAction::ProfileCreate {
                name: values[0].clone(),
                description: (!values[1].is_empty()).then(|| values[1].clone()),
            },
            InputKind::ProfileRename { old } => GuiAction::ProfileRename {
                old,
                new: values[0].clone(),
            },
            InputKind::RefSet { workspace, target } => GuiAction::RefSet {
                workspace,
                target,
                new_ref: values[0].clone(),
            },
            InputKind::CreateBranch => GuiAction::CreateBranch {
                name: values[0].clone(),
            },
            InputKind::NewInstallation => GuiAction::AttachProfile {
                workspace: PathBuf::from(&values[0]),
                target: (!values[1].is_empty()).then(|| values[1].clone()),
                profile: values[2].clone(),
            },
        };
        Some(action)
    }

    /// Builds the action a pick-dialog choice produces, closing the dialog.
    pub fn choose_pick(&mut self, index: usize) -> Option<GuiAction> {
        let Some(Dialog::Pick(dialog)) = self.dialog.take() else {
            return None;
        };
        let Some(value) = dialog.values.get(index) else {
            self.dialog = Some(Dialog::Pick(dialog));
            return None;
        };
        let action = match dialog.kind {
            PickKind::AddToProfile { skill } => GuiAction::ProfileAddSkills {
                profile: value.clone(),
                skills: vec![skill],
            },
            PickKind::ProfileAddSkill { profile } => GuiAction::ProfileAddSkills {
                profile,
                skills: vec![value.clone()],
            },
            PickKind::SkillRemove { skill } => GuiAction::SkillRemove {
                skill,
                cascade: value == "true",
            },
            PickKind::Unregister { workspace, target } => GuiAction::Unregister {
                workspace,
                target,
                keep_files: value == "true",
            },
        };
        Some(action)
    }

    /// Opens the add-to-profile pick for one skill (§76, §97).
    pub fn begin_add_to_profile(&mut self, skill: &str) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let options: Vec<String> = snapshot.profiles.iter().map(|p| p.name.clone()).collect();
        if options.is_empty() {
            self.toast = Some("No profiles exist yet; create one first".to_owned());
            return;
        }
        self.dialog = Some(Dialog::Pick(PickDialog {
            title: format!("Add {skill} to profile (§76)"),
            options: options.clone(),
            values: options,
            kind: PickKind::AddToProfile {
                skill: skill.to_owned(),
            },
        }));
    }

    /// Opens the add-skill pick for one profile (§76).
    pub fn begin_profile_add_skill(&mut self, profile: &str) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let mut options: Vec<String> = snapshot
            .skills
            .iter()
            .map(|skill| skill.name.to_string())
            .collect();
        options.sort();
        if options.is_empty() {
            self.toast = Some("No skills exist yet; ingest one first".to_owned());
            return;
        }
        self.dialog = Some(Dialog::Pick(PickDialog {
            title: format!("Add skill to {profile} (§76)"),
            options: options.clone(),
            values: options,
            kind: PickKind::ProfileAddSkill {
                profile: profile.to_owned(),
            },
        }));
    }

    /// Opens the skill-removal mode pick (§75: refuse by default, explicit
    /// cascade).
    pub fn begin_skill_remove(&mut self, skill: &str) {
        self.dialog = Some(Dialog::Pick(PickDialog {
            title: format!("Remove {skill} (§75)"),
            options: vec![
                "Keep referencing profiles (refuse if referenced)".to_owned(),
                "Cascade: remove from all referencing profiles".to_owned(),
            ],
            values: vec!["false".to_owned(), "true".to_owned()],
            kind: PickKind::SkillRemove {
                skill: skill.to_owned(),
            },
        }));
    }

    /// Opens the unregister-mode pick (§55).
    pub fn begin_unregister(&mut self) {
        let Some(row) = self.selected_installation() else {
            return;
        };
        self.dialog = Some(Dialog::Pick(PickDialog {
            title: "Unregister installation (§55)".to_owned(),
            options: vec![
                "Retire managed skills, keep extra files, remove the record".to_owned(),
                "Keep all files, remove only the record".to_owned(),
            ],
            values: vec!["false".to_owned(), "true".to_owned()],
            kind: PickKind::Unregister {
                workspace: row.installation.workspace.clone(),
                target: Some(row.installation.target.clone()),
            },
        }));
    }

    pub fn close_dialog(&mut self) {
        self.dialog = None;
    }
}

// ---- consent helpers (§47, §49) -------------------------------------------------

/// The explicit-consent options a blocked plan MAY be re-planned with:
/// `force` for ModifiedContent, `replace_unmanaged` for UnmanagedCollision
/// (§135.30: replacing unmanaged content requires separate consent from
/// force). Any other blocker kind makes the plan unwaivable.
pub fn waivable_options(plan: &ReconciliationPlan) -> Option<ReconcileOptions> {
    let mut options = ReconcileOptions::default();
    for blocker in &plan.blockers {
        match blocker.kind {
            BlockerKind::ModifiedContent => options.force = true,
            BlockerKind::UnmanagedCollision => options.replace_unmanaged = true,
            _ => return None,
        }
    }
    if plan.blockers.is_empty() {
        return None;
    }
    Some(options)
}

fn waiver_choices(
    plan: &ReconciliationPlan,
    action: &GuiAction,
    current: ReconcileOptions,
) -> Vec<ConfirmChoice> {
    let mut choices = Vec::new();
    let has_modified = plan
        .blockers
        .iter()
        .any(|b| b.kind == BlockerKind::ModifiedContent);
    let has_unmanaged = plan
        .blockers
        .iter()
        .any(|b| b.kind == BlockerKind::UnmanagedCollision);
    if has_modified {
        let mut options = current;
        options.force = true;
        choices.push(ConfirmChoice {
            label: "Force: discard the modified content listed above (§47)".to_owned(),
            apply: ConfirmApply::Replan {
                action: Box::new(action.clone()),
                options,
            },
        });
    }
    if has_unmanaged {
        let mut options = current;
        options.replace_unmanaged = true;
        choices.push(ConfirmChoice {
            label: "Replace the unmanaged directories listed above (§49)".to_owned(),
            apply: ConfirmApply::Replan {
                action: Box::new(action.clone()),
                options,
            },
        });
    }
    choices.push(ConfirmChoice {
        label: "Cancel".to_owned(),
        apply: ConfirmApply::Cancel,
    });
    choices
}

// ---- shared line builders ---------------------------------------------------

/// Current UTC wall-clock time as `HH:MM:SS` for Activity entries.
pub fn timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!("{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second())
}

/// Short display form of a commit hash (owned, for format strings that
/// need a `String`).
pub fn short_display(commit: &str) -> String {
    short(commit).to_owned()
}

/// Short display form of a commit hash.
pub fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

/// The stable §130-style identifier of a fetch/push state.
pub fn sync_state_id(state: BranchSyncState) -> &'static str {
    match state {
        BranchSyncState::Current => "current",
        BranchSyncState::FastForwarded => "fast_forwarded",
        BranchSyncState::Planned => "planned",
        BranchSyncState::Ahead => "ahead",
        BranchSyncState::Diverged => "diverged",
        BranchSyncState::DirtyCheckedOut => "dirty_checked_out",
        BranchSyncState::Unpublished => "unpublished",
        BranchSyncState::Unknown => "unknown",
    }
}

/// The stable §130-style identifier of a push state.
pub fn push_state_id(state: PushState) -> &'static str {
    match state {
        PushState::Current => "current",
        PushState::Pushed => "pushed",
        PushState::Planned => "planned",
        PushState::Unverified => "unverified",
    }
}

/// The §130-style glyph for a drift state (mirrors the CLI/TUI renderings;
/// the UI classifies nothing itself — it renders typed core data, §115).
pub fn drift_glyph(state: DriftState) -> &'static str {
    match state {
        DriftState::Current => "✓",
        DriftState::Outdated => "↑",
        DriftState::Modified => "!",
        DriftState::Extra => "+",
        DriftState::Gap => "?",
        DriftState::Unstamped => "○",
        DriftState::Foreign => "✗",
        DriftState::ProfileAdded => "⊕",
        DriftState::ProfileRemoved => "⊖",
        DriftState::MembershipChanged => "≈",
        DriftState::OrphanedManaged => "∅",
        DriftState::MissingTarget => "⊘",
        DriftState::MissingWorkspace => "⊘",
        DriftState::MissingRef => "⊘",
        DriftState::MissingProfile => "⚑",
    }
}

fn action_label(action: PlanAction) -> &'static str {
    match action {
        PlanAction::AttachProfile => "attach profile",
        PlanAction::DetachProfile => "detach profile",
        PlanAction::InstallSkill => "install",
        PlanAction::UpdateSkill => "update",
        PlanAction::ChangeSkillMembership => "membership change",
        PlanAction::RetireSkill => "retire",
        PlanAction::PreserveExtra => "preserve extra",
        PlanAction::OverwriteModified => "overwrite modified",
        PlanAction::ReplaceUnmanaged => "replace unmanaged",
        PlanAction::UpdateRegistry => "update registry",
        PlanAction::CommitLibraryPaths => "commit library paths",
        PlanAction::FastForwardBranch => "fast-forward branch",
        PlanAction::PushBranch => "push branch",
    }
}

fn blocker_label(blocker: &Blocker) -> String {
    let kind = match blocker.kind {
        BlockerKind::ModifiedContent => "modified content",
        BlockerKind::UnmanagedCollision => "unmanaged collision",
        BlockerKind::ForeignStamp => "foreign stamp",
        BlockerKind::LibraryMismatch => "library mismatch",
        BlockerKind::MissingRef => "missing ref",
        BlockerKind::MissingProfile => "missing profile",
        BlockerKind::MissingWorkspace => "missing workspace",
        BlockerKind::MissingSkill => "missing skill",
        BlockerKind::InvalidSkill => "invalid skill",
        BlockerKind::Diverged => "diverged",
    };
    match (&blocker.skill, &blocker.profile) {
        (Some(skill), Some(profile)) => format!("{kind} [{skill}, {profile}]"),
        (Some(skill), None) => format!("{kind} [{skill}]"),
        (None, Some(profile)) => format!("{kind} [{profile}]"),
        (None, None) => kind.to_owned(),
    }
}

/// Summarizes a reconciliation plan as plain lines (§89, §91 output shape;
/// also the confirm-dialog body, §47: the exact destructive file lists are
/// fully inspectable).
pub fn plan_lines(plan: &ReconciliationPlan) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(commit) = &plan.resolved_commit {
        lines.push(format!("resolved library commit: {}", short(commit)));
    }
    for change in &plan.profile_changes {
        let verb = match change.action {
            PlanAction::AttachProfile => "attach",
            PlanAction::DetachProfile => "detach",
            _ => "change",
        };
        lines.push(format!("  {verb} profile {}", change.profile_name));
    }
    for action in &plan.skill_actions {
        let name = action.skill.as_str();
        match action.action {
            PlanAction::InstallSkill => lines.push(format!("  install {name}")),
            PlanAction::UpdateSkill => lines.push(format!("  update {name}")),
            PlanAction::RetireSkill => lines.push(format!("  retire {name}")),
            PlanAction::ChangeSkillMembership => {
                lines.push(format!("  membership change: {name}"));
            }
            PlanAction::OverwriteModified => lines.push(format!(
                "  overwrite modified files of {name}: {}",
                action.paths.join(", ")
            )),
            PlanAction::ReplaceUnmanaged => {
                lines.push(format!("  replace unmanaged directory {name}"));
            }
            PlanAction::PreserveExtra => lines.push(format!(
                "  preserve extra files in {name}: {}",
                action.paths.join(", ")
            )),
            other => lines.push(format!("  {}: {name}", action_label(other))),
        }
    }
    for action in &plan.state_actions {
        lines.push(format!("  {}", action_label(*action)));
    }
    for blocker in &plan.blockers {
        let mut line = format!("  BLOCKED {}", blocker_label(blocker));
        if !blocker.paths.is_empty() {
            line.push_str(&format!(": {}", blocker.paths.join(", ")));
        }
        lines.push(line);
    }
    if plan.is_no_op() {
        lines.push("  nothing to do — already up to date".to_owned());
    }
    lines
}

/// Summarizes a fetch outcome/plan as plain lines (§62, §91).
pub fn fetch_lines(outcome: &FetchOutcome) -> Vec<String> {
    let mut lines = vec![format!(
        "remote {} ({}){}",
        outcome.remote,
        outcome.remote_url,
        if outcome.dry_run {
            " — dry run, no network"
        } else {
            ""
        }
    )];
    for branch in &outcome.branches {
        let mut line = format!("  {}: {}", branch.branch, sync_state_id(branch.state));
        if let (Some(ahead), Some(behind)) = (branch.ahead, branch.behind) {
            line.push_str(&format!(" (ahead {ahead}, behind {behind})"));
        } else if let Some(behind) = branch.behind {
            line.push_str(&format!(" (behind {behind})"));
        } else if let Some(ahead) = branch.ahead {
            line.push_str(&format!(" (ahead {ahead})"));
        }
        if let Some(note) = &branch.note {
            line.push_str(&format!(" — {note}"));
        }
        lines.push(line);
    }
    for action in &outcome.plan.actions {
        lines.push(format!(
            "  fast-forward {} → {}",
            action.branch,
            short(action.to.as_deref().unwrap_or("?"))
        ));
    }
    lines
}

/// Summarizes a push outcome/plan as plain lines (§65, §91).
pub fn push_lines(outcome: &PushOutcome) -> Vec<String> {
    let mut lines = vec![format!(
        "{} → {} ({}): {}{}",
        outcome.branch,
        outcome.remote,
        outcome.remote_url,
        push_state_id(outcome.state),
        if outcome.dry_run {
            " — dry run, no network"
        } else {
            ""
        }
    )];
    if let Some(ahead) = outcome.ahead {
        lines.push(format!("  {ahead} local commit(s) not on the remote"));
    }
    if outcome.created_remote_branch {
        lines.push("  the remote branch does not exist yet and will be created".to_owned());
    }
    match (
        outcome.upstream_before.as_deref(),
        outcome.upstream_after.as_deref(),
    ) {
        (before, Some(after)) if before != Some(after) => {
            lines.push(format!("  upstream will be set to {after}"));
        }
        (Some(before), _) => lines.push(format!("  upstream: {before}")),
        (None, Some(after)) => lines.push(format!("  upstream: {after}")),
        (None, None) => {}
    }
    lines
}

/// `value[, value…]` → Vec<String>, tolerating whitespace (used by tag and
/// skill-list inputs).
pub fn comma_separated(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use beskar_core::editing::BranchDisplay;

    fn snapshot() -> Snapshot {
        Snapshot {
            library: LibraryStatusReport {
                path: PathBuf::from("/lib"),
                library_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
                branch: Some("main".to_owned()),
                head: Some("abc1234".to_owned()),
                dirty: false,
                staged: Vec::new(),
                unstaged: Vec::new(),
                untracked: Vec::new(),
                remote: None,
                default_ref: "main".to_owned(),
                upstream: None,
                ahead: None,
                behind: None,
                installations: Vec::new(),
            },
            skills: Vec::new(),
            profiles: Vec::new(),
            installations: Vec::new(),
            branches: vec![BranchDisplay {
                name: "main".to_owned(),
                current: true,
                upstream: None,
                ahead: None,
                behind: None,
            }],
        }
    }

    fn listing(name: &str, bucket: &str) -> SkillListing {
        SkillListing {
            name: beskar_core::ids::SkillName::parse(name).expect("valid name"),
            path: format!("skills/{bucket}/{name}"),
            bucket: bucket.to_owned(),
            description: format!("the {name} skill"),
            tags: vec!["tag".to_owned()],
            rank: None,
            notes: None,
            profiles: Vec::new(),
            last_commit: None,
        }
    }

    #[test]
    fn pages_cover_the_spec_order() {
        assert_eq!(Page::ALL.len(), 7);
        assert_eq!(Page::ALL[0], Page::Dashboard);
        assert_eq!(Page::ALL[3], Page::Installations);
        assert_eq!(Page::ALL[5], Page::Settings);
        assert_eq!(Page::ALL[6], Page::Activity);
    }

    #[test]
    fn loaded_snapshot_repairs_dangling_selections() {
        let mut state = GuiState::new();
        let mut snap = snapshot();
        snap.skills.push(listing("testing", "quality"));
        state.loaded(snap);
        state.select_skill("testing");
        assert!(state.skills.preview.is_none());

        // A refresh that no longer contains the skill drops the selection
        // and any preview (never a stale selection).
        let mut snap = snapshot();
        snap.skills.push(listing("rust", "languages"));
        state.loaded(snap);
        assert!(state.skills.skill.is_none());
        assert!(state.skills.preview.is_none());

        // Branch selections repair the same way.
        state.select_branch("main");
        let mut snap = snapshot();
        snap.branches.clear();
        state.loaded(snap);
        assert!(state.git.branch.is_none());
    }

    #[test]
    fn skill_filter_matches_name_description_and_tag() {
        let mut state = GuiState::new();
        let mut snap = snapshot();
        snap.skills.push(listing("code-review", "engineering"));
        snap.skills.push(listing("rust", "languages"));
        state.loaded(snap);
        assert_eq!(state.filtered_skills().len(), 2);

        state.skills.query = "review".into();
        assert_eq!(state.filtered_skills().len(), 1);
        state.skills.query = "tag".into();
        assert_eq!(state.filtered_skills().len(), 2);
        state.skills.query = "zzz".into();
        assert!(state.filtered_skills().is_empty());

        state.skills.query = String::new();
        state.skills.bucket = Some("languages".into());
        let filtered = state.filtered_skills();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name.as_str(), "rust");
    }

    #[test]
    fn dashboard_metrics_count_attachments_and_breakage() {
        let mut state = GuiState::new();
        let mut snap = snapshot();
        snap.skills.push(listing("a", "x"));
        snap.skills.push(listing("b", "y"));
        snap.profiles
            .push(beskar_core::profile::Profile::new("dev-core", Vec::new()));
        state.loaded(snap);
        let metrics = state.snapshot.as_ref().expect("snapshot").dashboard();
        assert_eq!(metrics.skills, 2);
        assert_eq!(metrics.profiles, 1);
        assert_eq!(metrics.installations, 0);
        assert_eq!(metrics.attachments, 0);
    }

    #[test]
    fn workflow_gates_apply_behind_selection_and_preview() {
        let mut state = GuiState::new();
        state.begin_workflow(PreviewMode::Attach);
        assert!(
            state.installations.workflow.is_none(),
            "no installation selected"
        );

        // No selection: apply is unreachable even with a preview.
        assert!(state.workflow_apply_action().is_none());
        assert!(state.workflow_confirm().is_none());
    }

    #[test]
    fn toggling_selection_invalidates_the_preview() {
        let mut workflow = Workflow::new(PreviewMode::Attach);
        workflow.toggle("dev-core");
        assert!(workflow.selected("dev-core"));
        assert!(!workflow.selected("other"));
        workflow.toggle("dev-core");
        assert!(workflow.selected.is_empty());

        workflow.preview = Some(MembershipPreview {
            resolved_commit: "abc".to_owned(),
            rows: Vec::new(),
            gaps: Vec::new(),
            missing_profiles: Vec::new(),
        });
        workflow.toggle("rust-development");
        assert!(workflow.preview.is_none(), "stale preview must be cleared");
        assert!(workflow.preview_error.is_none());
    }

    #[test]
    fn comma_separated_splits_and_trims() {
        assert_eq!(comma_separated("a, b ,,c"), vec!["a", "b", "c"]);
        assert!(comma_separated(" , ").is_empty());
    }

    #[test]
    fn drift_states_surface_glyphs_and_dashboard_counts() {
        // §38: every per-skill drift state renders a glyph, and a synthetic
        // InstallationStatus drives the dashboard counts (§96) the
        // Installations page displays (§103, §133.13).
        use beskar_core::drift::DriftState;
        use beskar_core::ids::{InstallationId as Iid, LibraryId as Lid, SkillName as Sid};
        use beskar_core::status::SkillStatus;
        let gallery: [(DriftState, &str, &str); 8] = [
            (DriftState::Current, "current-skill", "✓"),
            (DriftState::Outdated, "outdated-skill", "↑"),
            (DriftState::Modified, "modified-skill", "!"),
            (DriftState::Extra, "extra-skill", "+"),
            (DriftState::Gap, "gap-skill", "?"),
            (DriftState::Unstamped, "unstamped-skill", "○"),
            (DriftState::Foreign, "foreign-skill", "✗"),
            (DriftState::OrphanedManaged, "orphaned-skill", "∅"),
        ];
        for (state, _, glyph) in gallery {
            assert_eq!(drift_glyph(state), glyph, "{state:?} glyph");
        }

        let installation_id = Iid::parse("11111111-2222-3333-4444-555555555555").expect("id");
        let mut status = InstallationStatus {
            installation_id,
            library_id: Lid::parse("550e8400-e29b-41d4-a716-446655440000").expect("id"),
            workspace: PathBuf::from("/ws"),
            target: ".agents/skills".to_owned(),
            source_ref: "main".to_owned(),
            installation_state: None,
            resolved_commit: Some("abc1234".to_owned()),
            profiles: Vec::new(),
            skills: gallery
                .iter()
                .map(|(state, name, _)| {
                    (
                        Sid::parse(name).expect("valid name"),
                        SkillStatus {
                            state: *state,
                            membership_drift: beskar_core::drift::MembershipDrift::Unchanged,
                            required_by: Vec::new(),
                            last_required_by: Vec::new(),
                            extra_files: Vec::new(),
                            protected_by_missing_profile: false,
                        },
                    )
                })
                .collect(),
            unmanaged: Vec::new(),
            unsafe_paths: Vec::new(),
        };
        let mut snap = snapshot();
        snap.installations.push(InstallationRow {
            installation: Installation {
                id: installation_id,
                library_id: status.library_id,
                workspace: PathBuf::from("/ws"),
                target: ".agents/skills".to_owned(),
                adapter: beskar_core::registry::Adapter::Agents,
                source_ref: "main".to_owned(),
                profiles: Vec::new(),
                last_applied: Default::default(),
                workspace_info: None,
                installed_at: time::OffsetDateTime::from_unix_timestamp(0).expect("time"),
                updated_at: time::OffsetDateTime::from_unix_timestamp(0).expect("time"),
            },
            status: Some(status.clone()),
            error: None,
        });
        let mut state = GuiState::new();
        state.loaded(snap);
        let metrics = state.snapshot.as_ref().expect("snapshot").dashboard();
        assert_eq!(metrics.outdated, 1);
        assert_eq!(metrics.modified, 1);
        assert_eq!(metrics.installations, 1);

        // Installation-level breakage surfaces through the §130 id.
        status.installation_state = Some(DriftState::MissingWorkspace);
        assert_eq!(status.installation_state_id(), Some("missing_workspace"));
    }

    #[test]
    fn plan_lines_show_actions_and_blockers() {
        let plan = ReconciliationPlan {
            installation_id: beskar_core::ids::InstallationId::parse(
                "11111111-2222-3333-4444-555555555555",
            )
            .expect("valid id"),
            resolved_commit: Some("abcdef1234567890".to_owned()),
            profile_changes: Vec::new(),
            skill_actions: vec![
                beskar_core::plan::SkillAction {
                    action: PlanAction::InstallSkill,
                    skill: beskar_core::ids::SkillName::parse("testing").expect("valid"),
                    resulting_state: DriftState::Current,
                    paths: Vec::new(),
                },
                beskar_core::plan::SkillAction {
                    action: PlanAction::OverwriteModified,
                    skill: beskar_core::ids::SkillName::parse("modified").expect("valid"),
                    resulting_state: DriftState::Current,
                    paths: vec!["SKILL.md".to_owned()],
                },
            ],
            state_actions: vec![PlanAction::UpdateRegistry],
            blockers: vec![beskar_core::plan::Blocker {
                kind: BlockerKind::ModifiedContent,
                skill: Some(beskar_core::ids::SkillName::parse("modified").expect("valid")),
                profile: None,
                paths: vec!["SKILL.md".to_owned()],
            }],
        };
        let lines = plan_lines(&plan).join("\n");
        assert!(lines.contains("abcdef1"), "{lines}");
        assert!(lines.contains("install testing"), "{lines}");
        assert!(
            lines.contains("overwrite modified files of modified: SKILL.md"),
            "{lines}"
        );
        assert!(
            lines.contains("BLOCKED modified content [modified]: SKILL.md"),
            "{lines}"
        );
    }

    #[test]
    fn waiver_choices_require_uniform_blocker_kinds() {
        let installation_id =
            beskar_core::ids::InstallationId::parse("11111111-2222-3333-4444-555555555555")
                .expect("valid id");
        let blocker = |kind: BlockerKind| beskar_core::plan::Blocker {
            kind,
            skill: None,
            profile: None,
            paths: Vec::new(),
        };
        let plan = ReconciliationPlan {
            installation_id,
            resolved_commit: None,
            profile_changes: Vec::new(),
            skill_actions: Vec::new(),
            state_actions: Vec::new(),
            blockers: vec![blocker(BlockerKind::ModifiedContent)],
        };
        assert!(waivable_options(&plan).is_some_and(|o| o.force && !o.replace_unmanaged));

        let mixed = ReconciliationPlan {
            blockers: vec![
                blocker(BlockerKind::ModifiedContent),
                blocker(BlockerKind::UnmanagedCollision),
            ],
            ..plan.clone()
        };
        let options = waivable_options(&mixed).expect("waivable mix");
        assert!(options.force && options.replace_unmanaged);

        let foreign = ReconciliationPlan {
            blockers: vec![blocker(BlockerKind::ForeignStamp)],
            ..plan.clone()
        };
        assert!(
            waivable_options(&foreign).is_none(),
            "foreign stamps are never waived"
        );

        let empty = ReconciliationPlan {
            blockers: Vec::new(),
            ..plan
        };
        assert!(waivable_options(&empty).is_none());

        let choices = waiver_choices(&mixed, &GuiAction::UpdateAll, ReconcileOptions::default());
        assert_eq!(choices.len(), 3, "force + replace + cancel");
        assert!(choices[0].label.contains("§47"));
        assert!(choices[1].label.contains("§49"));
    }

    #[test]
    fn planned_no_op_becomes_a_toast_not_a_dialog() {
        let mut state = GuiState::new();
        let installation_id =
            beskar_core::ids::InstallationId::parse("11111111-2222-3333-4444-555555555555")
                .expect("valid id");
        let plan = ReconciliationPlan {
            installation_id,
            resolved_commit: None,
            profile_changes: Vec::new(),
            skill_actions: Vec::new(),
            state_actions: Vec::new(),
            blockers: Vec::new(),
        };
        state.planned(
            GuiAction::UpdateAll,
            Planned::Install {
                title: "Update installation".to_owned(),
                plan,
            },
            ReconcileOptions::default(),
        );
        assert!(state.dialog.is_none());
        assert!(
            state
                .toast
                .as_deref()
                .is_some_and(|t| t.contains("nothing to do"))
        );
    }

    #[test]
    fn planned_blocked_opens_consent_not_apply() {
        let mut state = GuiState::new();
        let installation_id =
            beskar_core::ids::InstallationId::parse("11111111-2222-3333-4444-555555555555")
                .expect("valid id");
        let plan = ReconciliationPlan {
            installation_id,
            resolved_commit: None,
            profile_changes: Vec::new(),
            skill_actions: Vec::new(),
            state_actions: Vec::new(),
            blockers: vec![beskar_core::plan::Blocker {
                kind: BlockerKind::ModifiedContent,
                skill: Some(beskar_core::ids::SkillName::parse("x").expect("valid")),
                profile: None,
                paths: vec!["SKILL.md".to_owned()],
            }],
        };
        let action = GuiAction::UpdateInstallation {
            workspace: PathBuf::from("/ws"),
            target: None,
        };
        state.planned(
            action.clone(),
            Planned::Install {
                title: "Update installation".to_owned(),
                plan,
            },
            ReconcileOptions::default(),
        );
        let Some(Dialog::Confirm(confirm)) = &state.dialog else {
            panic!("expected a confirm dialog");
        };
        assert!(
            confirm
                .choices
                .iter()
                .all(|c| !matches!(c.apply, ConfirmApply::Run { .. }))
        );
        assert!(confirm.choices.len() == 2, "force + cancel");
        assert!(confirm.lines.iter().any(|l| l.contains("BLOCKED")));

        // The force choice is a REPLAN with consent options; the dialog
        // closes on choice.
        let apply = state.confirm_choice(0).expect("choice");
        let ConfirmApply::Replan { options, .. } = apply else {
            panic!("expected a replan");
        };
        assert!(options.force);
        assert!(state.dialog.is_none());
    }

    #[test]
    fn executed_stops_surface_the_consent_flow_for_the_continuation() {
        let mut state = GuiState::new();
        let installation_id =
            beskar_core::ids::InstallationId::parse("11111111-2222-3333-4444-555555555555")
                .expect("valid id");
        let plan = ReconciliationPlan {
            installation_id,
            resolved_commit: None,
            profile_changes: Vec::new(),
            skill_actions: Vec::new(),
            state_actions: Vec::new(),
            blockers: vec![beskar_core::plan::Blocker {
                kind: BlockerKind::UnmanagedCollision,
                skill: Some(beskar_core::ids::SkillName::parse("y").expect("valid")),
                profile: None,
                paths: Vec::new(),
            }],
        };
        state.executed(Executed {
            title: "Attach ci".to_owned(),
            applied: false,
            lines: vec!["blocked at ci".to_owned()],
            warnings: Vec::new(),
            stop: Some(BlockedStop {
                plan,
                continuation: GuiAction::AttachProfiles {
                    workspace: PathBuf::from("/ws"),
                    target: Some(".agents/skills".to_owned()),
                    profiles: vec!["remaining".to_owned()],
                },
            }),
        });

        // The activity log records the stop and the dialog offers the §49
        // consent path for the remaining profiles.
        assert!(state.activity.entries[0].failed);
        let Some(Dialog::Confirm(confirm)) = &state.dialog else {
            panic!("expected a consent dialog");
        };
        assert!(confirm.choices[0].label.contains("§49"));
        let apply = state.confirm_choice(0).expect("choice");
        let ConfirmApply::Replan { action, options } = apply else {
            panic!("expected a replan");
        };
        assert!(options.replace_unmanaged);
        assert_eq!(
            *action,
            GuiAction::AttachProfiles {
                workspace: PathBuf::from("/ws"),
                target: Some(".agents/skills".to_owned()),
                profiles: vec!["remaining".to_owned()],
            }
        );
    }

    #[test]
    fn submit_input_builds_actions_or_keeps_the_dialog_open() {
        let mut state = GuiState::new();
        state.begin_ingest();
        assert!(
            state.submit_input().is_none(),
            "empty fields keep the dialog"
        );
        assert!(
            state
                .toast
                .as_deref()
                .is_some_and(|t| t.contains("required"))
        );
        assert!(matches!(state.dialog, Some(Dialog::Input(_))));

        let Some(Dialog::Input(dialog)) = &mut state.dialog else {
            panic!("dialog survived");
        };
        dialog.fields[0].buffer = "/tmp/skill".into();
        dialog.fields[1].buffer = "languages/rust".into();
        let action = state.submit_input().expect("action");
        assert_eq!(
            action,
            GuiAction::Ingest {
                source: PathBuf::from("/tmp/skill"),
                bucket: "languages/rust".into(),
            }
        );
        assert!(state.dialog.is_none());

        // New installation: blank target means the adapter default (§24).
        state.begin_new_installation();
        let Some(Dialog::Input(dialog)) = &mut state.dialog else {
            panic!("dialog");
        };
        dialog.fields[0].buffer = "/ws".into();
        dialog.fields[2].buffer = "dev-core".into();
        let action = state.submit_input().expect("action");
        assert_eq!(
            action,
            GuiAction::AttachProfile {
                workspace: PathBuf::from("/ws"),
                target: None,
                profile: "dev-core".into(),
            }
        );

        // Rank: blank clears (§14 rank is optional).
        state.begin_skill_rank("testing");
        let Some(Dialog::Input(dialog)) = &mut state.dialog else {
            panic!("dialog");
        };
        dialog.fields[0].buffer = "".into();
        let action = state.submit_input().expect("action");
        assert_eq!(
            action,
            GuiAction::SkillRank {
                skill: "testing".into(),
                rank: None,
            }
        );

        // Tag: comma lists split on both sides.
        state.begin_skill_tag("testing");
        let Some(Dialog::Input(dialog)) = &mut state.dialog else {
            panic!("dialog");
        };
        dialog.fields[0].buffer = "a, b".into();
        dialog.fields[1].buffer = "c".into();
        let action = state.submit_input().expect("action");
        assert_eq!(
            action,
            GuiAction::SkillTag {
                skill: "testing".into(),
                add: vec!["a".into(), "b".into()],
                remove: vec!["c".into()],
            }
        );
    }

    #[test]
    fn pick_choices_carry_their_machine_values() {
        let mut state = GuiState::new();
        state.begin_skill_remove("testing");
        let action = state.choose_pick(1).expect("action");
        assert_eq!(
            action,
            GuiAction::SkillRemove {
                skill: "testing".into(),
                cascade: true,
            }
        );
        assert!(state.dialog.is_none());
    }

    #[test]
    fn activity_log_is_bounded_and_newest_first() {
        let mut state = GuiState::new();
        for i in 0..5 {
            state.activity.push(ActivityEntry {
                time: "00:00:00".to_owned(),
                title: format!("entry {i}"),
                lines: Vec::new(),
                failed: false,
            });
        }
        assert_eq!(state.activity.entries.len(), 5);
        assert_eq!(state.activity.entries[0].title, "entry 4");

        state.activity.limit = 3;
        state.activity.push(ActivityEntry {
            time: "00:00:00".to_owned(),
            title: "latest".to_owned(),
            lines: Vec::new(),
            failed: false,
        });
        assert_eq!(state.activity.entries.len(), 3);
        assert_eq!(state.activity.entries[0].title, "latest");
    }

    #[test]
    fn operation_failed_surfaces_the_stable_code() {
        let mut state = GuiState::new();
        let err = beskar_core::Error::validation("something failed");
        state.operation_failed("Do a thing", &err);
        assert!(state.activity.entries[0].failed);
        let Some(Dialog::Message(message)) = &state.dialog else {
            panic!("expected a message dialog");
        };
        assert!(message.failed);
        assert!(
            message.lines[0].contains("validation"),
            "{}",
            message.lines[0]
        );
    }
}

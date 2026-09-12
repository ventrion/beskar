//! TUI application state (spec §95-§101, §112).
//!
//! Pure data: no I/O, no terminal, no rendering. The reducer in [`crate::reduce`]
//! transitions this state; the view in [`crate::view`] draws it; the runtime in
//! [`crate::terminal`] fulfills [`crate::effect::Effect`]s against the core
//! services and feeds the results back as [`crate::event::Event`]s.
//!
//! Everything here is presentation state over snapshots of core data — UI
//! state never alters domain semantics (§105).

use std::path::PathBuf;

use beskar_core::drift::{DriftState, MembershipDrift};
use beskar_core::editing::{
    BranchDisplay, LibraryPlan, LibraryStatusReport, SkillDetail, SkillListing,
};
use beskar_core::ids::{InstallationId, ProfileId};
use beskar_core::plan::ReconciliationPlan;
use beskar_core::reconcile::ReconcileOptions;
use beskar_core::registry::Installation;
use beskar_core::remote::{FetchOutcome, PushOutcome};
use beskar_core::status::InstallationStatus;

/// Primary screens (spec §95).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Skills,
    Profiles,
    Installations,
    Git,
    Activity,
}

impl Screen {
    /// All screens in §95 order.
    pub const ALL: [Screen; 6] = [
        Screen::Dashboard,
        Screen::Skills,
        Screen::Profiles,
        Screen::Installations,
        Screen::Git,
        Screen::Activity,
    ];

    /// The screen's display name.
    pub fn title(self) -> &'static str {
        match self {
            Screen::Dashboard => "Dashboard",
            Screen::Skills => "Skills",
            Screen::Profiles => "Profiles",
            Screen::Installations => "Installations",
            Screen::Git => "Git",
            Screen::Activity => "Activity",
        }
    }

    /// Next screen in cyclic order (Tab).
    pub fn next(self) -> Screen {
        let index = Screen::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Screen::ALL[(index + 1) % Screen::ALL.len()]
    }

    /// Previous screen in cyclic order (Shift+Tab).
    pub fn previous(self) -> Screen {
        let index = Screen::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Screen::ALL[(index + Screen::ALL.len() - 1) % Screen::ALL.len()]
    }
}

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

/// A full data snapshot for rendering, gathered from core read APIs.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub library: LibraryStatusReport,
    pub skills: Vec<SkillListing>,
    pub profiles: Vec<beskar_core::profile::Profile>,
    pub installations: Vec<InstallationRow>,
    pub branches: Vec<BranchDisplay>,
}

impl Snapshot {
    /// One skill listing by canonical name.
    pub fn skill(&self, name: &str) -> Option<&SkillListing> {
        self.skills.iter().find(|s| s.name.as_str() == name)
    }

    /// One installation row by ID.
    pub fn installation(&self, id: InstallationId) -> Option<&InstallationRow> {
        self.installations.iter().find(|r| r.installation.id == id)
    }

    /// One profile by name.
    pub fn profile(&self, name: &str) -> Option<&beskar_core::profile::Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// Unique buckets in skill order, as (bucket, depth) rows for the
    /// §97 tree pane. The skills directory prefix is stripped.
    pub fn buckets(&self) -> Vec<(String, usize)> {
        let mut buckets: Vec<String> = self
            .skills
            .iter()
            .map(|s| s.bucket.clone())
            .filter(|b| !b.is_empty())
            .collect();
        buckets.sort();
        buckets.dedup();
        let mut rows = Vec::new();
        for bucket in buckets {
            let depth = bucket.split('/').count();
            rows.push((bucket, depth));
        }
        rows
    }

    /// The §96 dashboard metrics.
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

/// Dashboard metrics (spec §96).
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
    /// Number of missing attached profiles across all installations (§39).
    pub missing_profiles: usize,
}

/// The §97 preview payload: skill detail plus the `SKILL.md` bytes as they
/// exist in the Library working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillView {
    pub name: String,
    pub detail: SkillDetail,
    pub skill_md: String,
}

/// The §100 skill-membership view for one installation.
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

/// What a planned mutation looked like — always the REAL core plan
/// (§135.38-39: planning is separated from confirmation and execution;
/// dry-run uses the same planner).
#[derive(Debug, Clone, PartialEq)]
pub enum PlannedChange {
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

impl PlannedChange {
    /// The dialog title for this change.
    pub fn title(&self) -> &str {
        match self {
            PlannedChange::Install { title, .. }
            | PlannedChange::Library { title, .. }
            | PlannedChange::Summary { title, .. } => title,
            PlannedChange::Fetch { .. } => "Fetch from remote",
            PlannedChange::Push { .. } => "Push branch",
        }
    }
}

/// The summary of an executed mutation, appended to the Activity log (§95).
#[derive(Debug, Clone, PartialEq)]
pub struct ActionOutcome {
    pub title: String,
    /// Whether state changed (false when the plan was a no-op or blocked
    /// races surfaced).
    pub applied: bool,
    pub lines: Vec<String>,
    pub warnings: Vec<String>,
}

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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActivityLog {
    pub entries: Vec<ActivityEntry>,
    pub scroll: usize,
    pub limit: usize,
}

impl ActivityLog {
    pub fn push(&mut self, entry: ActivityEntry) {
        self.entries.insert(0, entry);
        self.entries.truncate(self.limit);
        self.scroll = 0;
    }
}

/// Which pane of the Skills screen has focus (§97 three-pane model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillsFocus {
    #[default]
    Buckets,
    List,
    Preview,
}

/// Skills pane state (§97): bucket tree + searchable list + preview.
#[derive(Debug, Clone, Default)]
pub struct SkillsPane {
    pub focus: SkillsFocus,
    /// Lexical query over names/descriptions/tags (§97, §80).
    pub query: String,
    /// Whether keystrokes currently edit the query.
    pub querying: bool,
    /// Selected bucket filter; `None` = all buckets.
    pub bucket: Option<String>,
    /// Selected skill by canonical name.
    pub skill: Option<String>,
    /// Loaded preview for the selected skill.
    pub preview: Option<SkillView>,
    pub preview_scroll: u16,
}

/// Which pane of the Profiles screen has focus (§98).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProfilesFocus {
    #[default]
    List,
    Detail,
}

/// Profiles pane state (§98).
#[derive(Debug, Clone, Default)]
pub struct ProfilesPane {
    pub focus: ProfilesFocus,
    /// Selected profile by name.
    pub profile: Option<String>,
    /// Highlighted skill index within the selected profile's ordered list.
    pub skill_index: usize,
}

/// Which pane of the Installations screen has focus (§99).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstallationsFocus {
    #[default]
    List,
    Detail,
}

/// Installations pane state (§99).
#[derive(Debug, Clone, Default)]
pub struct InstallationsPane {
    pub focus: InstallationsFocus,
    /// Selected installation by ID.
    pub installation: Option<InstallationId>,
    /// Detail cursor over a combined row space: attached profiles first,
    /// then effective skills (§100). `None` = first row.
    pub cursor: Option<usize>,
}

/// Git pane state (§101).
#[derive(Debug, Clone, Default)]
pub struct GitPane {
    /// Selected branch by name.
    pub branch: Option<String>,
}

/// A pending mutation, first planned (dry-run), then confirmed, then
/// executed for real (§89, §135.38-39).
#[derive(Debug, Clone, PartialEq)]
pub enum PendingAction {
    // Installations (§99)
    AttachProfile {
        workspace: PathBuf,
        target: Option<String>,
        profile: String,
    },
    DetachProfile {
        workspace: PathBuf,
        target: Option<String>,
        profile: String,
    },
    UpdateInstallation {
        workspace: PathBuf,
        target: Option<String>,
    },
    UpdateAll,
    RefSet {
        workspace: PathBuf,
        target: Option<String>,
        new_ref: String,
    },
    Unregister {
        workspace: PathBuf,
        target: Option<String>,
        keep_files: bool,
    },
    ReorderProfiles {
        workspace: PathBuf,
        target: Option<String>,
        order: Vec<ProfileId>,
    },

    // Library editing (§97)
    Ingest {
        source: PathBuf,
        bucket: String,
    },
    SkillMove {
        skill: String,
        bucket: String,
    },
    SkillRename {
        old: String,
        new: String,
    },
    SkillRemove {
        skill: String,
        cascade: bool,
    },
    SkillTag {
        skill: String,
        add: Vec<String>,
        remove: Vec<String>,
    },
    SkillRank {
        skill: String,
        rank: Option<i64>,
    },

    // Profiles (§98)
    ProfileCreate {
        name: String,
        description: Option<String>,
    },
    ProfileDelete {
        name: String,
    },
    ProfileRename {
        old: String,
        new: String,
    },
    ProfileAddSkills {
        profile: String,
        skills: Vec<String>,
    },
    ProfileRemoveSkills {
        profile: String,
        skills: Vec<String>,
    },
    ProfileMoveSkill {
        profile: String,
        skill: String,
        pivot: beskar_core::editing::SkillPivot,
    },

    // Git (§101)
    Fetch {
        remote: String,
    },
    Push {
        branch: Option<String>,
        set_upstream: bool,
        allow_dirty: bool,
    },
    SwitchBranch {
        name: String,
    },
    CreateBranch {
        name: String,
    },
}

impl PendingAction {
    /// The title shown in confirm dialogs and the Activity log.
    pub fn title(&self) -> String {
        match self {
            PendingAction::AttachProfile { profile, .. } => format!("Attach profile {profile}"),
            PendingAction::DetachProfile { profile, .. } => format!("Detach profile {profile}"),
            PendingAction::UpdateInstallation { .. } => "Update installation".to_owned(),
            PendingAction::UpdateAll => "Update all installations".to_owned(),
            PendingAction::RefSet { new_ref, .. } => format!("Change source ref to {new_ref}"),
            PendingAction::Unregister { keep_files, .. } => {
                if *keep_files {
                    "Unregister installation (keep files)".to_owned()
                } else {
                    "Unregister installation (retire managed skills)".to_owned()
                }
            }
            PendingAction::ReorderProfiles { .. } => "Reorder attached profiles".to_owned(),
            PendingAction::Ingest { source, bucket } => {
                format!("Ingest {} into {bucket}", source.display())
            }
            PendingAction::SkillMove { skill, bucket } => format!("Move {skill} to {bucket}"),
            PendingAction::SkillRename { old, new } => format!("Rename {old} to {new}"),
            PendingAction::SkillRemove { skill, cascade } => {
                if *cascade {
                    format!("Remove {skill} (cascade from profiles)")
                } else {
                    format!("Remove {skill}")
                }
            }
            PendingAction::SkillTag { skill, .. } => format!("Tag {skill}"),
            PendingAction::SkillRank { skill, .. } => format!("Rank {skill}"),
            PendingAction::ProfileCreate { name, .. } => format!("Create profile {name}"),
            PendingAction::ProfileDelete { name } => format!("Delete profile {name}"),
            PendingAction::ProfileRename { old, new } => format!("Rename profile {old} to {new}"),
            PendingAction::ProfileAddSkills { profile, skills } => {
                format!("Add {} to {profile}", skills.join(", "))
            }
            PendingAction::ProfileRemoveSkills { profile, skills } => {
                format!("Remove {} from {profile}", skills.join(", "))
            }
            PendingAction::ProfileMoveSkill { profile, skill, .. } => {
                format!("Reorder {skill} in {profile}")
            }
            PendingAction::Fetch { remote } => format!("Fetch from {remote}"),
            PendingAction::Push { branch, .. } => match branch {
                Some(branch) => format!("Push {branch}"),
                None => "Push current branch".to_owned(),
            },
            PendingAction::SwitchBranch { name } => format!("Switch to branch {name}"),
            PendingAction::CreateBranch { name } => format!("Create branch {name}"),
        }
    }
}

/// What confirming one choice in a [`ConfirmDialog`] does.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmApply {
    /// Execute the mutation for real (§89 step 4).
    Run {
        action: Box<PendingAction>,
        options: ReconcileOptions,
    },
    /// Re-plan with explicit consent options (§47 interactive `--force`:
    /// the exact managed files to be discarded are displayed first).
    Replan {
        action: Box<PendingAction>,
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

/// The plan-confirmation dialog: shows the real core plan (§89 step 3,
/// §91, §47).
#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    pub title: String,
    pub change: PlannedChange,
    /// Additional context lines (e.g. §75 external-installation warning).
    pub summary: Vec<String>,
    pub choices: Vec<ConfirmChoice>,
    pub selected: usize,
    /// Scroll offset into the plan body (PageUp/PageDown; §47 requires the
    /// exact destructive file lists to be fully inspectable).
    pub scroll: u16,
}

/// One labeled text field in an [`InputDialog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputField {
    pub label: String,
    pub buffer: String,
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
    AttachNew {
        profile: String,
    },
    RefSet {
        workspace: PathBuf,
        target: Option<String>,
    },
    CreateBranch,
}

/// A text-input dialog.
#[derive(Debug, Clone)]
pub struct InputDialog {
    pub title: String,
    pub fields: Vec<InputField>,
    pub focused: usize,
    pub kind: InputKind,
}

/// Which pending action a [`PickDialog`] builds on choose.
#[derive(Debug, Clone, PartialEq)]
pub enum PickKind {
    AttachProfile {
        workspace: PathBuf,
        target: Option<String>,
    },
    DetachProfile {
        workspace: PathBuf,
        target: Option<String>,
    },
    AddToProfile {
        skill: String,
    },
    ProfileAddSkill {
        profile: String,
    },
    /// Skill removal; `values` are `"false"`/`"true"` cascade flags (§75).
    RemoveSkill {
        skill: String,
    },
    /// Unregister mode; `values` are `"false"`/`"true"` keep-files flags
    /// (§55). The chosen mode then goes through the normal plan → confirm
    /// pipeline.
    Unregister {
        workspace: PathBuf,
        target: Option<String>,
    },
    /// First install: the picked profile still needs a workspace path.
    AttachNew,
}

/// A single-choice dialog.
#[derive(Debug, Clone)]
pub struct PickDialog {
    pub title: String,
    /// Display labels, parallel to `values`.
    pub options: Vec<String>,
    /// Machine values (profile names, skill names).
    pub values: Vec<String>,
    pub selected: usize,
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

/// Every modal overlay (§112: dialogs).
#[derive(Debug, Clone)]
pub enum Dialog {
    Confirm(ConfirmDialog),
    Input(InputDialog),
    Pick(PickDialog),
    Message(MessageDialog),
    /// The §100 first-class membership view.
    Membership(Box<MembershipView>),
}

/// The whole TUI application state.
#[derive(Debug, Clone)]
pub struct App {
    pub screen: Screen,
    pub quit: bool,
    /// The latest data snapshot; `None` until the first refresh completes.
    pub snapshot: Option<Snapshot>,
    /// Why the last refresh failed (rendered from the typed error).
    pub load_error: Option<String>,
    /// Label of the effect currently being fulfilled.
    pub busy: Option<String>,
    /// One-line status message shown in the footer until replaced.
    pub toast: Option<String>,
    pub skills: SkillsPane,
    pub profiles: ProfilesPane,
    pub installations: InstallationsPane,
    pub git: GitPane,
    pub activity: ActivityLog,
    pub dialog: Option<Dialog>,
    /// The pending action + options an in-flight plan belongs to.
    pub planning: Option<(PendingAction, ReconcileOptions)>,
}

impl App {
    /// Fresh state on the Dashboard (§96).
    pub fn new() -> Self {
        Self {
            screen: Screen::Dashboard,
            quit: false,
            snapshot: None,
            load_error: None,
            busy: None,
            toast: None,
            skills: SkillsPane::default(),
            profiles: ProfilesPane::default(),
            installations: InstallationsPane::default(),
            git: GitPane::default(),
            activity: ActivityLog {
                entries: Vec::new(),
                scroll: 0,
                limit: 200,
            },
            dialog: None,
            planning: None,
        }
    }

    /// The skills passing the active bucket filter and lexical query
    /// (presentation filtering over the snapshot; §97 "searchable").
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

    /// The profiles attaching the given profile locally (§98).
    pub fn attaching_installations(&self, profile_id: &ProfileId) -> Vec<(PathBuf, String)> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        snapshot
            .installations
            .iter()
            .filter(|row| {
                row.installation
                    .profiles
                    .iter()
                    .any(|a| a.id == *profile_id)
            })
            .map(|row| {
                (
                    row.installation.workspace.clone(),
                    row.installation.target.clone(),
                )
            })
            .collect()
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Steps a selection index by `delta`, clamping to the list bounds.
pub fn step_index(current: Option<usize>, len: usize, delta: i64) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let current = current.unwrap_or(0).min(len - 1);
    let next = current as i64 + delta;
    Some(next.clamp(0, len as i64 - 1) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens_cycle_in_spec_order() {
        assert_eq!(Screen::Dashboard.next(), Screen::Skills);
        assert_eq!(Screen::Activity.next(), Screen::Dashboard);
        assert_eq!(Screen::Dashboard.previous(), Screen::Activity);
        assert_eq!(Screen::Skills.previous(), Screen::Dashboard);
    }

    #[test]
    fn step_index_clamps_and_handles_empty() {
        assert_eq!(step_index(None, 0, 1), None);
        assert_eq!(step_index(None, 3, 1), Some(1));
        assert_eq!(step_index(Some(2), 3, 1), Some(2));
        assert_eq!(step_index(Some(0), 3, -5), Some(0));
        assert_eq!(step_index(Some(2), 3, -1), Some(1));
    }

    #[test]
    fn activity_log_is_bounded_and_newest_first() {
        let mut app = App::new();
        app.activity.limit = 3;
        for i in 0..5 {
            app.activity.push(ActivityEntry {
                time: format!("{i}"),
                title: format!("entry {i}"),
                lines: vec![],
                failed: false,
            });
        }
        assert_eq!(app.activity.entries.len(), 3);
        assert_eq!(app.activity.entries[0].title, "entry 4");
    }
}

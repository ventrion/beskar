//! Reconciliation planning types (spec §89, §90, §109).
//!
//! Every mutating domain operation separates planning, safety evaluation,
//! confirmation, execution, and Registry finalization. Plans MUST be
//! serializable; dry-run uses the exact same planner as real execution
//! (§135.38-39). One plan is produced per Installation, never per Profile
//! (§135.19).

use serde::{Deserialize, Serialize};

use crate::drift::DriftState;
use crate::ids::{InstallationId, SkillName};

/// Operation plan action types (spec §89).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanAction {
    AttachProfile,
    DetachProfile,
    InstallSkill,
    UpdateSkill,
    ChangeSkillMembership,
    RetireSkill,
    PreserveExtra,
    OverwriteModified,
    ReplaceUnmanaged,
    UpdateRegistry,
    CommitLibraryPaths,
    FastForwardBranch,
    PushBranch,
}

/// Why a planned operation is blocked; blockers are typed, never prose
/// (spec §115; UI code must not classify by parsing text).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerKind {
    /// Local modifications would be overwritten or removed (§47).
    ModifiedContent,
    /// An unmanaged same-name directory exists (§49).
    UnmanagedCollision,
    /// A foreign stamp claims the directory (§38 "Foreign").
    ForeignStamp,
    /// The Library identity mismatches the installation's (§38).
    LibraryMismatch,
    /// The source ref cannot be resolved (§38 "Missing-ref").
    MissingRef,
    /// An attached profile is absent from the resolved Library (§39).
    MissingProfile,
    /// The registered workspace directory does not exist (§38
    /// "Missing-workspace"); Beskar never creates workspaces itself (§4).
    MissingWorkspace,
    /// A required skill is absent from the resolved Library (§58).
    MissingSkill,
    /// An ambiguous or invalid skill was encountered (§60).
    InvalidSkill,
    /// Diverged state requiring explicit user action (§63).
    Diverged,
}

/// One safety blocker discovered during planning (spec §45, §47, §60).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocker {
    pub kind: BlockerKind,
    /// Skill the blocker applies to, when scoped to one skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<SkillName>,
    /// Profile the blocker applies to, when scoped to one attachment (§39).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<crate::ids::ProfileId>,
    /// Stable machine identifiers of affected paths, `/`-separated (§119).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

/// A planned filesystem/state change for one skill (spec §45, §89).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillAction {
    pub action: PlanAction,
    pub skill: SkillName,
    /// Expected resulting state once the action applies.
    pub resulting_state: DriftState,
    /// Contextual paths, `/`-separated (§119): files whose local versions a
    /// forced operation discards (§47), extras preserved (§8.6), or the
    /// unmanaged directory replaced (§49). Empty when not applicable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

/// A planned attachment change (spec §89).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileChange {
    pub action: PlanAction,
    pub profile_id: crate::ids::ProfileId,
    /// Last known profile name for display (§28).
    pub profile_name: String,
}

/// The complete deterministic plan for one Installation
/// (spec §45, §109). No target writes occur before planning completes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationPlan {
    pub installation_id: InstallationId,
    /// The exact Library commit the plan resolved against, when the source
    /// ref resolved (§19); `None` for plans blocked before resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_commit: Option<String>,
    pub profile_changes: Vec<ProfileChange>,
    pub skill_actions: Vec<SkillAction>,
    /// Installation-level state actions that are neither profile changes nor
    /// skill operations — e.g. [`PlanAction::UpdateRegistry`] (§89). Later
    /// phases add `CommitLibraryPaths`, `FastForwardBranch`, `PushBranch`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state_actions: Vec<PlanAction>,
    /// Non-empty plans are not executable without explicit consent (§47).
    pub blockers: Vec<Blocker>,
}

impl ReconciliationPlan {
    /// A plan is executable only when no safety blockers were found (§47).
    pub fn is_blocked(&self) -> bool {
        !self.blockers.is_empty()
    }

    /// Whether the plan would change nothing (dry-run "no-op", §22, §91).
    pub fn is_no_op(&self) -> bool {
        self.profile_changes.is_empty() && self.skill_actions.is_empty() && !self.is_blocked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_actions_use_stable_serialization() {
        let json = serde_json::to_value(PlanAction::ChangeSkillMembership).expect("serialize");
        assert_eq!(json, serde_json::json!("change_skill_membership"));
    }

    #[test]
    fn empty_plan_is_a_no_op_and_not_blocked() {
        let plan = ReconciliationPlan {
            installation_id: InstallationId::generate(),
            resolved_commit: None,
            profile_changes: vec![],
            skill_actions: vec![],
            state_actions: vec![],
            blockers: vec![],
        };
        assert!(plan.is_no_op());
        assert!(!plan.is_blocked());
    }

    #[test]
    fn plan_roundtrips_through_json() {
        let plan = ReconciliationPlan {
            installation_id: InstallationId::generate(),
            resolved_commit: Some("abc123".to_owned()),
            profile_changes: vec![ProfileChange {
                action: PlanAction::DetachProfile,
                profile_id: crate::ids::ProfileId::generate(),
                profile_name: "dev-core".to_owned(),
            }],
            skill_actions: vec![SkillAction {
                action: PlanAction::RetireSkill,
                skill: SkillName::parse("git").expect("valid"),
                resulting_state: DriftState::ProfileRemoved,
                paths: vec![],
            }],
            state_actions: vec![PlanAction::UpdateRegistry],
            blockers: vec![Blocker {
                kind: BlockerKind::ModifiedContent,
                skill: Some(SkillName::parse("testing").expect("valid")),
                profile: None,
                paths: vec!["testing/SKILL.md".to_owned()],
            }],
        };
        let json = serde_json::to_string(&plan).expect("serialize");
        assert_eq!(
            serde_json::from_str::<ReconciliationPlan>(&json).expect("parse"),
            plan
        );
    }

    #[test]
    fn new_fields_roundtrip_with_serde_defaults() {
        // JSON without `resolved_commit`/`state_actions` (and blockers
        // without `profile`) still deserializes; the new fields are stable
        // API going forward.
        let old = "{\"installation_id\":\"550e8400-e29b-41d4-a716-446655440000\",\
            \"profile_changes\":[],\"skill_actions\":[],\"blockers\":[]}";
        let plan: ReconciliationPlan = serde_json::from_str(old).expect("parse legacy");
        assert_eq!(plan.resolved_commit, None);
        assert!(plan.state_actions.is_empty());
        assert!(plan.skill_actions.is_empty());
        assert!(plan.blockers.is_empty());
    }
}

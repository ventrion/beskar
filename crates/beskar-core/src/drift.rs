//! Drift classification (spec §38-§40, §130).
//!
//! Drift is any difference between installed state and expected Library
//! state. The serde identifiers below are the stable machine-readable API
//! (§130); human prose is not stable.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::ids::ProfileId;
use crate::membership::MembershipMap;

/// Classification of one skill's (or installation's) state relative to the
/// expected Library state (spec §38).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftState {
    /// Tracked files match the Stamp and the selected Library revision of the
    /// skill has not changed.
    Current,
    /// Tracked files match the Stamp but a newer resolved skill revision
    /// exists.
    Outdated,
    /// At least one Stamp-tracked file differs, is missing, or has
    /// incompatible executable metadata (§38 "Modified").
    Modified,
    /// An additional untracked file exists inside the skill directory;
    /// informational only (§38 "Extra", §8.6).
    Extra,
    /// An attached Profile references a skill absent from the resolved
    /// Library (§38 "Gap", §58).
    Gap,
    /// The expected skill directory exists but has no valid Beskar stamp.
    Unstamped,
    /// The directory contains a stamp bound to another Library,
    /// Installation, skill identity, or an unsupported manager/schema.
    Foreign,
    /// Required by current desired membership but absent from last-applied
    /// membership (§38 "Profile-added").
    ProfileAdded,
    /// Present in last-applied membership but no longer required (§38
    /// "Profile-removed", §50).
    ProfileRemoved,
    /// Still required, but its requiring Profile set changed (§38
    /// "Membership-changed", §90).
    MembershipChanged,
    /// A valid Beskar-stamped skill in a registered Target owned by neither
    /// last-applied nor current desired membership.
    OrphanedManaged,
    /// The registered Target directory does not exist.
    MissingTarget,
    /// The registered Workspace does not exist.
    MissingWorkspace,
    /// The Source ref can no longer be resolved.
    MissingRef,
    /// An attached Profile ID cannot be found in the resolved Library
    /// revision; a protected state (§39, §135.20-21).
    MissingProfile,
}

impl DriftState {
    /// The stable machine-readable identifier (spec §130).
    pub fn id(self) -> &'static str {
        match self {
            DriftState::Current => "current",
            DriftState::Outdated => "outdated",
            DriftState::Modified => "modified",
            DriftState::Extra => "extra",
            DriftState::Gap => "gap",
            DriftState::Unstamped => "unstamped",
            DriftState::Foreign => "foreign",
            DriftState::ProfileAdded => "profile_added",
            DriftState::ProfileRemoved => "profile_removed",
            DriftState::MembershipChanged => "membership_changed",
            DriftState::OrphanedManaged => "orphaned_managed",
            DriftState::MissingTarget => "missing_target",
            DriftState::MissingWorkspace => "missing_workspace",
            DriftState::MissingRef => "missing_ref",
            DriftState::MissingProfile => "missing_profile",
        }
    }
}

/// How a skill's membership changed between last apply and current desired
/// state (spec §38 "Profile-added"/"Profile-removed"/"Membership-changed").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipDrift {
    /// Required by exactly the same Profile set as at last apply.
    Unchanged,
    /// Required now but not at last apply (§38 "Profile-added").
    Added,
    /// Required at last apply but no longer required (§38 "Profile-removed",
    /// §50) — eligible for retirement.
    Removed,
    /// Still required, but its requiring Profile set changed (§38
    /// "Membership-changed"); no file operation may be necessary (§90).
    Changed,
}

/// Compares desired membership against last-applied membership (spec §38,
/// §27). Owner sets are compared as sets; order is presentation only.
pub fn compare_membership(
    desired: &MembershipMap,
    last_applied: &MembershipMap,
) -> BTreeMap<crate::ids::SkillName, MembershipDrift> {
    let mut result = BTreeMap::new();
    let keys: BTreeSet<&crate::ids::SkillName> =
        desired.keys().chain(last_applied.keys()).collect();
    for skill in keys {
        let desired_ids = desired.get(skill);
        let last_ids = last_applied.get(skill);
        let drift = match (desired_ids, last_ids) {
            (Some(now), Some(before)) => {
                let now: BTreeSet<_> = now.iter().collect();
                let before: BTreeSet<_> = before.iter().collect();
                if now == before {
                    MembershipDrift::Unchanged
                } else {
                    MembershipDrift::Changed
                }
            }
            (Some(_), None) => MembershipDrift::Added,
            (None, Some(_)) => MembershipDrift::Removed,
            (None, None) => continue,
        };
        result.insert(skill.clone(), drift);
    }
    result
}

/// Whether a skill that vanished from desired membership may retire, or is
/// held by missing-profile protection (spec §39, §40, §50).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalReview {
    /// No surviving owner: retirement is eligible (§50).
    Eligible,
    /// At least one unresolvable (missing) profile still owns the skill;
    /// it MUST NOT be retired automatically (§39, §135.20-21).
    Protected,
}

/// Reviews retirement eligibility for one last-applied skill absent from
/// desired membership (spec §39, §40).
///
/// `last_owners` are the Profile IDs recorded at last apply. Explicitly
/// detached profiles (`detached`, §40) are treated as removed intentionally;
/// missing (unresolvable) profiles keep their skills protected.
pub fn review_removal(
    last_owners: &[ProfileId],
    missing_profiles: &[ProfileId],
    detached: &[ProfileId],
) -> RemovalReview {
    let surviving = last_owners.iter().filter(|id| !detached.contains(id));
    if surviving.clone().any(|id| missing_profiles.contains(id)) {
        RemovalReview::Protected
    } else {
        RemovalReview::Eligible
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::build_membership;

    #[test]
    fn serde_names_match_stable_identifiers() {
        // §130: these identifiers are public API.
        let cases = [
            (DriftState::Current, "current"),
            (DriftState::Outdated, "outdated"),
            (DriftState::Modified, "modified"),
            (DriftState::Extra, "extra"),
            (DriftState::Gap, "gap"),
            (DriftState::Unstamped, "unstamped"),
            (DriftState::Foreign, "foreign"),
            (DriftState::ProfileAdded, "profile_added"),
            (DriftState::ProfileRemoved, "profile_removed"),
            (DriftState::MembershipChanged, "membership_changed"),
            (DriftState::OrphanedManaged, "orphaned_managed"),
            (DriftState::MissingTarget, "missing_target"),
            (DriftState::MissingWorkspace, "missing_workspace"),
            (DriftState::MissingRef, "missing_ref"),
            (DriftState::MissingProfile, "missing_profile"),
        ];
        for (state, name) in cases {
            assert_eq!(state.id(), name);
            let json = serde_json::to_value(state).expect("serialize");
            assert_eq!(json, serde_json::json!(name));
            let back: DriftState = serde_json::from_value(json).expect("deserialize");
            assert_eq!(back, state);
        }
    }

    fn pid(n: u128) -> ProfileId {
        ProfileId::from(uuid::Uuid::from_u128(n))
    }

    fn name(s: &str) -> crate::ids::SkillName {
        crate::ids::SkillName::parse(s).expect("valid")
    }

    #[test]
    fn membership_drift_classification_covers_spec38_states() {
        let desired = [
            (name("added"), vec![pid(1)]),
            (name("changed"), vec![pid(2)]),
            (name("same"), vec![pid(1), pid(2)]),
        ];
        let last = [
            (name("changed"), vec![pid(1), pid(2)]),
            (name("same"), vec![pid(2), pid(1)]), // same set, different order
            (name("removed"), vec![pid(1)]),
        ];
        let drift = compare_membership(&build_membership(desired), &build_membership(last));
        assert_eq!(drift[&name("added")], MembershipDrift::Added);
        assert_eq!(drift[&name("removed")], MembershipDrift::Removed);
        assert_eq!(drift[&name("changed")], MembershipDrift::Changed);
        // §50: dropping one of two owners is a membership change, not removal.
        assert_eq!(drift[&name("same")], MembershipDrift::Unchanged);
    }

    #[test]
    fn removal_review_protects_skills_owned_by_missing_profiles() {
        let missing = [pid(9)];
        let detached = [];
        // rust-development vanished from the Library; its skill is protected.
        assert_eq!(
            review_removal(&[pid(9)], &missing, &detached),
            RemovalReview::Protected
        );
        // Mixed ownership with a surviving profile still counts as protected:
        // the missing owner's contribution cannot be evaluated (§39).
        assert_eq!(
            review_removal(&[pid(1), pid(9)], &missing, &detached),
            RemovalReview::Protected
        );
        assert_eq!(
            review_removal(&[pid(1)], &missing, &detached),
            RemovalReview::Eligible
        );
    }

    #[test]
    fn explicit_detach_makes_skills_retirable() {
        // §40: explicitly detaching a missing profile allows retirement.
        assert_eq!(
            review_removal(&[pid(9)], &[pid(9)], &[pid(9)]),
            RemovalReview::Eligible
        );
    }
}

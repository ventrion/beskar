//! Drift classification (spec §38, §130).
//!
//! Drift is any difference between installed state and expected Library
//! state. The serde identifiers below are the stable machine-readable API
//! (§130); human prose is not stable.

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}

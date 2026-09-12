//! Effective skill membership across attached profiles (spec §36, §108).
//!
//! Membership is many-to-many (§7.3): each skill records *all* requiring
//! Profile IDs. This map is a primary reconciliation artifact and the source
//! of `beskar why` (§43) and the JSON membership representation (§93).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{ProfileId, SkillName};

/// One skill's membership: the set of attached Profiles requiring it
/// (spec §36, §108).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillMembership {
    pub skill: SkillName,
    /// Profile IDs requiring this skill; deterministic order.
    pub required_by: Vec<ProfileId>,
}

/// The full effective membership map for one desired state
/// (spec §36): skill name -> ordered requiring Profile IDs.
pub type MembershipMap = BTreeMap<SkillName, Vec<ProfileId>>;

/// Computes the membership map for `desired_skills` where each entry maps a
/// skill to its requiring Profile IDs. Deterministic: skills are ordered by
/// name; Profile IDs are sorted and de-duplicated (§36).
pub fn build_membership(
    desired: impl IntoIterator<Item = (SkillName, Vec<ProfileId>)>,
) -> MembershipMap {
    let mut map = MembershipMap::new();
    for (skill, profiles) in desired {
        let mut ids = profiles;
        ids.sort_unstable();
        ids.dedup();
        map.insert(skill, ids);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(n: u32) -> ProfileId {
        // Deterministic IDs keep assertions readable; UUID is only a wrapper.
        ProfileId::from(uuid::Uuid::from_u128(n as u128))
    }

    #[test]
    fn membership_records_all_requiring_profiles() {
        // testing <- dev-core, rust (spec §36 example)
        let map = build_membership([
            (
                SkillName::parse("testing").expect("valid"),
                vec![pid(1), pid(2)],
            ),
            (SkillName::parse("rust").expect("valid"), vec![pid(2)]),
        ]);
        assert_eq!(
            map[&SkillName::parse("testing").expect("valid")],
            vec![pid(1), pid(2)]
        );
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn membership_is_sorted_and_deduplicated() {
        let map = build_membership([(
            SkillName::parse("testing").expect("valid"),
            vec![pid(9), pid(2), pid(9)],
        )]);
        assert_eq!(
            map[&SkillName::parse("testing").expect("valid")],
            vec![pid(2), pid(9)]
        );
    }
}

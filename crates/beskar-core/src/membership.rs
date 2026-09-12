//! Effective skill membership across attached profiles (spec §17, §36, §108).
//!
//! Membership is many-to-many (§7.3): each skill records *all* requiring
//! Profile IDs. This map is a primary reconciliation artifact and the source
//! of `beskar why` (§43) and the JSON membership representation (§93).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{ProfileId, SkillName};
use crate::profile::Profile;
use crate::registry::ProfileAttachment;

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

/// Normalizes one skill's owner list: de-duplicated, first-occurrence order
/// preserved (spec §36; §37 — overlap is normal, never an error).
pub fn build_membership(
    desired: impl IntoIterator<Item = (SkillName, Vec<ProfileId>)>,
) -> MembershipMap {
    let mut map = MembershipMap::new();
    for (skill, profiles) in desired {
        let mut ids: Vec<ProfileId> = Vec::new();
        for id in profiles {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        map.insert(skill, ids);
    }
    map
}

/// The effective desired state of one Installation (spec §17, §36).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveState {
    /// Ordered-union presentation order (§17): profiles in attachment order,
    /// each profile's skills in profile order, first occurrence wins. Display
    /// only — never runtime precedence.
    pub order: Vec<SkillName>,
    /// skill -> requiring Profile IDs (attachment order, deduplicated).
    pub membership: MembershipMap,
    /// Attached Profile IDs not found among `profiles`; a protected state
    /// (§39) — their skills are never auto-retired.
    pub missing_profiles: Vec<ProfileId>,
}

/// Computes the effective desired state for an Installation's attachments
/// against the Profile definitions resolvable in one Library revision
/// (spec §36-§37, §17).
///
/// Attached profiles absent from `profiles` are reported in
/// [`EffectiveState::missing_profiles`] and contribute nothing to the
/// desired set — the classifier (§39) keeps their last-applied skills
/// protected instead of interpreting them as empty profiles.
pub fn effective_state(
    attachments: &[ProfileAttachment],
    profiles: &BTreeMap<ProfileId, Profile>,
) -> EffectiveState {
    let mut order: Vec<SkillName> = Vec::new();
    let mut membership = MembershipMap::new();
    let mut missing_profiles: Vec<ProfileId> = Vec::new();

    for attachment in attachments {
        let Some(profile) = profiles.get(&attachment.id) else {
            if !missing_profiles.contains(&attachment.id) {
                missing_profiles.push(attachment.id);
            }
            continue;
        };
        for skill in &profile.skills {
            if !order.contains(skill) {
                order.push(skill.clone());
            }
            let owners = membership.entry(skill.clone()).or_default();
            if !owners.contains(&attachment.id) {
                owners.push(attachment.id);
            }
        }
    }

    EffectiveState {
        order,
        membership,
        missing_profiles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ProfileId;
    use time::OffsetDateTime;

    fn pid(n: u128) -> ProfileId {
        // Deterministic IDs keep assertions readable; UUID is only a wrapper.
        ProfileId::from(uuid::Uuid::from_u128(n))
    }

    fn attachment(id: ProfileId, name: &str) -> ProfileAttachment {
        ProfileAttachment {
            id,
            name: name.to_owned(),
            attached_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn profile(id: ProfileId, name: &str, skills: &[&str]) -> Profile {
        Profile {
            schema: crate::profile::SCHEMA,
            id,
            name: name.to_owned(),
            description: None,
            skills: skills
                .iter()
                .map(|s| SkillName::parse(s).expect("valid"))
                .collect(),
        }
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
    fn membership_is_deduplicated_preserving_first_occurrence_order() {
        let map = build_membership([(
            SkillName::parse("testing").expect("valid"),
            vec![pid(9), pid(2), pid(9)],
        )]);
        assert_eq!(
            map[&SkillName::parse("testing").expect("valid")],
            vec![pid(9), pid(2)]
        );
    }

    #[test]
    fn ordered_union_matches_spec_example() {
        // Spec §17:
        //   dev-core: git, testing, review
        //   rust: rust, testing, cargo
        // Effective presentation order: git, testing, review, rust, cargo.
        let attachments = [attachment(pid(1), "dev-core"), attachment(pid(2), "rust")];
        let profiles = BTreeMap::from([
            (
                pid(1),
                profile(pid(1), "dev-core", &["git", "testing", "review"]),
            ),
            (
                pid(2),
                profile(pid(2), "rust", &["rust", "testing", "cargo"]),
            ),
        ]);
        let effective = effective_state(&attachments, &profiles);
        let names: Vec<&str> = effective.order.iter().map(SkillName::as_str).collect();
        assert_eq!(names, ["git", "testing", "review", "rust", "cargo"]);

        // Membership remains many-to-many: testing <- dev-core, rust.
        let testing = &effective.membership[&SkillName::parse("testing").expect("valid")];
        assert_eq!(testing, &[pid(1), pid(2)]);
    }

    #[test]
    fn duplicate_skill_across_profiles_installs_once() {
        // §7.2, §37: overlap is normal — one desired entry, both owners.
        let attachments = [attachment(pid(1), "a"), attachment(pid(2), "b")];
        let profiles = BTreeMap::from([
            (pid(1), profile(pid(1), "a", &["shared", "only-a"])),
            (pid(2), profile(pid(2), "b", &["shared"])),
        ]);
        let effective = effective_state(&attachments, &profiles);
        assert_eq!(effective.membership.len(), 2);
        assert_eq!(
            effective.membership[&SkillName::parse("shared").expect("valid")],
            vec![pid(1), pid(2)]
        );
    }

    #[test]
    fn unresolvable_profiles_are_reported_and_contribute_nothing() {
        // §39: a missing profile is NOT interpreted as an empty profile.
        let attachments = [attachment(pid(1), "dev"), attachment(pid(9), "vanished")];
        let profiles = BTreeMap::from([(pid(1), profile(pid(1), "dev", &["git"]))]);
        let effective = effective_state(&attachments, &profiles);
        assert_eq!(effective.missing_profiles, vec![pid(9)]);
        assert_eq!(
            effective.order,
            vec![SkillName::parse("git").expect("valid")]
        );
    }

    #[test]
    fn profile_order_in_attachment_list_drives_membership_order() {
        // §17: attachment order (not ID order) is the presentation order.
        let attachments = [attachment(pid(2), "second"), attachment(pid(1), "first")];
        let profiles = BTreeMap::from([
            (pid(1), profile(pid(1), "first", &["alpha"])),
            (pid(2), profile(pid(2), "second", &["beta", "alpha"])),
        ]);
        let effective = effective_state(&attachments, &profiles);
        let names: Vec<&str> = effective.order.iter().map(SkillName::as_str).collect();
        assert_eq!(names, ["beta", "alpha"]);
        assert_eq!(
            effective.membership[&SkillName::parse("alpha").expect("valid")],
            vec![pid(2), pid(1)]
        );
    }
}

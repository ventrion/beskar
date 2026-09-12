//! The §104 attachment-workflow preview: a pure membership-diff classifier.
//!
//! Given the desired membership before and after a staged attach/detach
//! selection, classifies every skill as installed / retained / retired /
//! membership-only changed — exactly the four inspection groups §104
//! requires — plus the proposed state's gaps (§58) and missing attached
//! profiles (§39). Pure data in, pure data out: the services layer gathers
//! the two [`DesiredInstallationState`] snapshots from the resolved Library
//! commit; this module never touches the filesystem or Git.

use std::collections::BTreeMap;

use beskar_core::ids::ProfileId;
use beskar_core::reconcile::DesiredInstallationState;

use beskar_core::membership::MembershipMap;

/// Which direction a staged §104 workflow moves in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewMode {
    Attach,
    Detach,
}

impl PreviewMode {
    /// The workflow's verb, for titles ("Attach profiles", "Detach profiles").
    pub fn verb(self) -> &'static str {
        match self {
            PreviewMode::Attach => "Attach",
            PreviewMode::Detach => "Detach",
        }
    }
}

/// What happens to one skill when the staged selection applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipChange {
    /// In the proposed effective set but not the current one (§49).
    Install,
    /// In both effective sets, required by the same profiles in the same
    /// order (§50: a skill stays installed while any profile requires it).
    Retain,
    /// In the current effective set but no longer required (§50-§51).
    Retire,
    /// Required before and after, but the requiring profile set changed
    /// (§90: membership changes are first-class plan output).
    MembershipOnly,
}

/// One skill row of the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipRow {
    pub skill: String,
    pub change: MembershipChange,
    /// Requiring profiles before the change (display names, §93).
    pub before: Vec<String>,
    /// Requiring profiles after the change (display names, §93).
    pub after: Vec<String>,
}

/// The full §104 preview for one staged selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipPreview {
    /// The exact Library commit both sides resolved against (§19).
    pub resolved_commit: String,
    /// Every skill of either effective set, sorted by name.
    pub rows: Vec<MembershipRow>,
    /// Skills the proposed state requires but the revision lacks:
    /// name → requiring display names (§38 Gap, §58).
    pub gaps: Vec<(String, Vec<String>)>,
    /// Attachments absent from the resolved revision in the proposed state:
    /// protected missing profiles, never empty profiles (§39).
    pub missing_profiles: Vec<String>,
}

impl MembershipPreview {
    /// Rows of one classification, in name order.
    pub fn rows_of(&self, change: MembershipChange) -> impl Iterator<Item = &MembershipRow> {
        self.rows.iter().filter(move |row| row.change == change)
    }

    /// Whether applying the selection changes nothing at all (§22: an
    /// idempotent attach is a no-op).
    pub fn is_no_op(&self) -> bool {
        self.rows
            .iter()
            .all(|row| row.change == MembershipChange::Retain)
            && self.gaps.is_empty()
            && self.missing_profiles.is_empty()
    }

    /// The preview as human-readable lines for confirm dialogs and the
    /// Activity log (prose is never parsed by machines, §115).
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.missing_profiles.is_empty() {
            lines.push(format!(
                "protected missing profiles (§39): {}",
                self.missing_profiles.join(", ")
            ));
        }
        for (skill, owners) in &self.gaps {
            lines.push(format!("gap: {skill} required by {}", owners.join(", ")));
        }
        for row in &self.rows {
            let owners = |names: &[String]| {
                if names.is_empty() {
                    "nobody".to_owned()
                } else {
                    names.join(", ")
                }
            };
            match row.change {
                MembershipChange::Install => {
                    lines.push(format!(
                        "install {} (required by {})",
                        row.skill,
                        owners(&row.after)
                    ));
                }
                MembershipChange::Retire => {
                    lines.push(format!(
                        "retire {} (was required by {})",
                        row.skill,
                        owners(&row.before)
                    ));
                }
                MembershipChange::MembershipOnly => {
                    lines.push(format!(
                        "membership change: {} ({} → {})",
                        row.skill,
                        owners(&row.before),
                        owners(&row.after)
                    ));
                }
                MembershipChange::Retain => {
                    lines.push(format!(
                        "retain {} (required by {})",
                        row.skill,
                        owners(&row.after)
                    ));
                }
            }
        }
        if self.is_no_op() {
            lines.push("nothing to do — the effective set is unchanged (§22)".to_owned());
        }
        lines
    }
}

/// Classifies the diff between two desired memberships (§104 step 4-5).
/// Both maps come from [`DesiredInstallationState::membership`] over the
/// SAME resolved commit; `proposed` additionally contributes gaps and
/// missing profiles. Ordering of `required_by` is meaningful (§17), so a
/// pure reorder classifies as membership-only.
pub fn membership_preview(
    names: &BTreeMap<ProfileId, String>,
    before: &MembershipMap,
    after: &MembershipMap,
    proposed: &DesiredInstallationState,
) -> MembershipPreview {
    let display = |ids: &[ProfileId]| -> Vec<String> {
        ids.iter()
            .map(|id| names.get(id).cloned().unwrap_or_else(|| id.to_string()))
            .collect()
    };

    let empty: Vec<ProfileId> = Vec::new();
    let mut keys: Vec<&beskar_core::ids::SkillName> = before.keys().collect();
    for key in after.keys() {
        if !before.contains_key(key) {
            keys.push(key);
        }
    }
    keys.sort();

    let mut rows = Vec::new();
    for key in keys {
        let before_owners = before.get(key);
        let after_owners = after.get(key);
        let change = match (before_owners, after_owners) {
            (None, Some(_)) => MembershipChange::Install,
            (Some(_), None) => MembershipChange::Retire,
            (Some(before_ids), Some(after_ids)) => {
                if before_ids == after_ids {
                    MembershipChange::Retain
                } else {
                    MembershipChange::MembershipOnly
                }
            }
            (None, None) => continue,
        };
        rows.push(MembershipRow {
            skill: key.to_string(),
            change,
            before: display(before_owners.unwrap_or(&empty)),
            after: display(after_owners.unwrap_or(&empty)),
        });
    }

    let gaps = proposed
        .gaps
        .iter()
        .map(|(name, ids)| (name.to_string(), display(ids)))
        .collect();
    let missing_profiles = proposed
        .missing_profiles
        .iter()
        .map(|attachment| {
            names
                .get(&attachment.id)
                .cloned()
                .unwrap_or_else(|| attachment.name.clone())
        })
        .collect();

    MembershipPreview {
        resolved_commit: proposed.resolved_commit.clone(),
        rows,
        gaps,
        missing_profiles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beskar_core::ids::SkillName;

    fn id(raw: &str) -> ProfileId {
        ProfileId::parse(raw).expect("valid profile id")
    }

    const DEV: &str = "98f1513d-94fa-4ace-907e-544c66233653";
    const RUST: &str = "0b0e1c3a-9d24-4c5a-8a30-2b7f0a519102";
    const CI: &str = "2f3c6a1e-5e7b-4f0a-9c1d-3a4b5c6d7e8f";

    fn map(entries: &[(&str, Vec<ProfileId>)]) -> MembershipMap {
        entries
            .iter()
            .map(|(name, ids)| (SkillName::parse(name).expect("valid name"), ids.clone()))
            .collect()
    }

    fn proposed(commit: &str) -> DesiredInstallationState {
        // Only the fields the classifier reads are populated: gaps,
        // missing_profiles, resolved_commit.
        DesiredInstallationState {
            resolved_commit: commit.to_owned(),
            profiles: BTreeMap::new(),
            missing_profiles: Vec::new(),
            skills: BTreeMap::new(),
            gaps: BTreeMap::new(),
        }
    }

    #[test]
    fn attach_installs_and_retains() {
        let dev = id(DEV);
        let rust = id(RUST);
        let names = BTreeMap::from([
            (dev, "dev-core".to_owned()),
            (rust, "rust-development".to_owned()),
        ]);
        let before = map(&[("testing", vec![dev])]);
        let after = map(&[("testing", vec![dev]), ("rust", vec![rust])]);
        let preview = membership_preview(&names, &before, &after, &proposed("abc123"));

        assert_eq!(preview.rows_of(MembershipChange::Install).count(), 1);
        assert_eq!(preview.rows_of(MembershipChange::Retain).count(), 1);
        assert_eq!(preview.rows_of(MembershipChange::Retire).count(), 0);
        assert!(!preview.is_no_op());
        let install = preview
            .rows_of(MembershipChange::Install)
            .next()
            .expect("install row");
        assert_eq!(install.skill, "rust");
        assert_eq!(install.after, vec!["rust-development".to_owned()]);
    }

    #[test]
    fn detach_final_owner_retires_and_shared_survives() {
        let dev = id(DEV);
        let rust = id(RUST);
        let names = BTreeMap::from([
            (dev, "dev-core".to_owned()),
            (rust, "rust-development".to_owned()),
        ]);
        // §53: detaching dev retires dev-only skills; shared testing stays.
        let before = map(&[
            ("testing", vec![dev, rust]),
            ("git", vec![dev]),
            ("review", vec![dev]),
            ("rust", vec![rust]),
        ]);
        let after = map(&[("testing", vec![rust]), ("rust", vec![rust])]);
        let preview = membership_preview(&names, &before, &after, &proposed("abc123"));

        let retired: Vec<&str> = preview
            .rows_of(MembershipChange::Retire)
            .map(|row| row.skill.as_str())
            .collect();
        assert_eq!(retired, vec!["git", "review"]);
        // rust stays required by exactly rust-development (§7.4).
        assert_eq!(preview.rows_of(MembershipChange::Retain).count(), 1);
        assert_eq!(preview.rows_of(MembershipChange::MembershipOnly).count(), 1);
        let changed = preview
            .rows_of(MembershipChange::MembershipOnly)
            .next()
            .expect("membership row");
        assert_eq!(changed.before, vec!["dev-core", "rust-development"]);
        assert_eq!(changed.after, vec!["rust-development"]);
    }

    #[test]
    fn reordering_owners_is_membership_only() {
        let dev = id(DEV);
        let rust = id(RUST);
        let names = BTreeMap::from([
            (dev, "dev-core".to_owned()),
            (rust, "rust-development".to_owned()),
        ]);
        let before = map(&[("testing", vec![dev, rust])]);
        let after = map(&[("testing", vec![rust, dev])]);
        let preview = membership_preview(&names, &before, &after, &proposed("abc123"));
        assert_eq!(preview.rows_of(MembershipChange::MembershipOnly).count(), 1);
    }

    #[test]
    fn identical_sets_are_a_no_op() {
        let dev = id(DEV);
        let names = BTreeMap::from([(dev, "dev-core".to_owned())]);
        let before = map(&[("testing", vec![dev])]);
        let after = before.clone();
        let preview = membership_preview(&names, &before, &after, &proposed("abc123"));
        assert!(preview.is_no_op());
        assert!(preview.lines().iter().any(|line| line.contains("§22")));
    }

    #[test]
    fn gaps_and_missing_profiles_surface_from_the_proposed_state() {
        let dev = id(DEV);
        let ci = id(CI);
        let names = BTreeMap::from([(dev, "dev-core".to_owned()), (ci, "ci".to_owned())]);
        let before = MembershipMap::new();
        let after = map(&[("kept", vec![dev])]);

        let mut state = proposed("def456");
        state.gaps.insert(
            SkillName::parse("missing-skill").expect("valid name"),
            vec![ci],
        );
        let preview = membership_preview(&names, &before, &after, &state);

        assert_eq!(preview.gaps.len(), 1);
        assert_eq!(preview.gaps[0].0, "missing-skill");
        assert_eq!(preview.gaps[0].1, vec!["ci".to_owned()]);
        assert!(preview.lines().iter().any(|line| line.contains("gap:")));
        assert_eq!(preview.resolved_commit, "def456");
    }

    #[test]
    fn unknown_ids_fall_back_to_the_bare_id() {
        let unknown = id(CI);
        let names = BTreeMap::new();
        let before = MembershipMap::new();
        let after = map(&[("testing", vec![unknown])]);
        let preview = membership_preview(&names, &before, &after, &proposed("abc123"));
        let row = preview.rows.first().expect("row");
        assert_eq!(row.after, vec![CI.to_owned()]);
    }

    #[test]
    fn preview_lines_cover_every_classification() {
        let dev = id(DEV);
        let rust = id(RUST);
        let names = BTreeMap::from([
            (dev, "dev-core".to_owned()),
            (rust, "rust-development".to_owned()),
        ]);
        let before = map(&[
            ("gone", vec![dev]),
            ("shared", vec![dev, rust]),
            ("kept", vec![rust]),
        ]);
        let after = map(&[
            ("fresh", vec![rust]),
            ("shared", vec![rust]),
            ("kept", vec![rust]),
        ]);
        let preview = membership_preview(&names, &before, &after, &proposed("abc123"));
        let lines = preview.lines().join("\n");
        assert!(lines.contains("install fresh"), "{lines}");
        assert!(lines.contains("retire gone"), "{lines}");
        assert!(lines.contains("membership change: shared"), "{lines}");
        assert!(lines.contains("retain"), "{lines}");
    }
}

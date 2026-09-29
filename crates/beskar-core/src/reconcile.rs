//! Reconciliation planning: what to do with each skill so a workspace
//! matches its enabled profiles, without losing local changes.
//!
//! Planning is a pure function of three fingerprints per skill:
//!
//! * **L**, the library version (if the skill is wanted),
//! * **R**, the recorded base: the library version the workspace copy was
//!   installed from, as kept in the registry,
//! * **W**, the workspace copy as it is now.
//!
//! This is a three-way comparison with R as the common ancestor, the same
//! rule a version control merge uses per file:
//!
//! | wanted? | situation               | action                                  |
//! |---------|-------------------------|-----------------------------------------|
//! | yes     | no W                    | install (restore, if R exists)          |
//! | yes     | W = L                   | nothing (record L if R differs)         |
//! | yes     | W = R, L changed        | update: nobody touched the copy         |
//! | yes     | W changed, L = R        | keep: the change is local only          |
//! | yes     | W, L both changed       | conflict                                |
//! | yes     | W present, no R, W ≠ L  | conflict: Beskar did not install it     |
//! | no      | W = R                   | remove                                  |
//! | no      | W changed               | conflict: removing would lose changes   |
//! | no      | no W, R exists          | forget the record                       |
//! | no      | W present, no R         | leave it: not Beskar's                  |
//!
//! Given the same library, profiles and workspace, the plan is the same.

use std::collections::{BTreeMap, BTreeSet};

use crate::fingerprint::Fingerprint;
use crate::names::{ProfileName, SkillId};

/// A skill the enabled profiles ask for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    /// The library version, or `None` if the library lacks the skill.
    pub library: Option<Fingerprint>,
    /// The enabled profiles that include the skill.
    pub profiles: Vec<ProfileName>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The workspace copy matches the library. Nothing to do.
    Unchanged,
    /// Wanted, not in the workspace: copy it in.
    Install,
    /// Installed before, deleted from the workspace since, still wanted:
    /// copy it in again.
    Restore,
    /// The library changed and the workspace copy is untouched: replace it.
    Update,
    /// No longer wanted and untouched: delete it.
    Remove,
    /// No longer wanted and already gone from the workspace: drop the
    /// record.
    Forget,
    /// The workspace copy already matches the library: record it as the
    /// installed version. No files change.
    Record,
    /// Changed in the workspace while the library stayed the same: leave
    /// the local change alone.
    KeepLocal,
    /// Going ahead would lose local changes; a decision is needed.
    Conflict(Conflict),
    /// Wanted, but the library has no such skill. Whatever is in the
    /// workspace stays.
    MissingSource,
    /// In the workspace, not installed by Beskar and not wanted. Beskar
    /// leaves it alone.
    Unmanaged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// Changed in the workspace, and the library has a newer version.
    Diverged,
    /// A skill directory Beskar did not install is where a wanted skill
    /// should go, and it differs from the library version.
    Untracked,
    /// Changed in the workspace, and no enabled profile wants it any more.
    Orphaned,
}

/// How to settle a conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Keep the workspace copy. For a wanted skill, the current library
    /// version counts as seen, so Beskar asks again only when the library
    /// changes again. An unwanted skill stays and Beskar stops managing it.
    Keep,
    /// Take the library's side: install the library version, or delete an
    /// unwanted copy.
    Replace,
    /// Copy the workspace version into the library, making it the version
    /// every workspace gets.
    Promote,
}

/// The plan for one skill, with the fingerprints it was based on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub skill: SkillId,
    pub action: Action,
    /// Enabled profiles that want the skill; empty if none does.
    pub profiles: Vec<ProfileName>,
    /// L: the library version, if the skill is wanted and in the library.
    pub library: Option<Fingerprint>,
    /// R: the recorded base version.
    pub recorded: Option<Fingerprint>,
    /// W: the workspace copy.
    pub present: Option<Fingerprint>,
}

impl Step {
    /// Whether carrying out the step changes files in the workspace.
    pub fn changes_files(&self) -> bool {
        matches!(
            self.action,
            Action::Install | Action::Restore | Action::Update | Action::Remove
        )
    }

    /// Whether the step changes anything, files or registry.
    pub fn changes_anything(&self) -> bool {
        self.changes_files() || matches!(self.action, Action::Forget | Action::Record)
    }
}

/// Plan every skill that is wanted, recorded or present. Steps are sorted
/// by skill name.
pub fn plan(
    wanted: &BTreeMap<SkillId, Wanted>,
    recorded: &BTreeMap<SkillId, Fingerprint>,
    present: &BTreeMap<SkillId, Fingerprint>,
) -> Vec<Step> {
    let skills: BTreeSet<&SkillId> = wanted
        .keys()
        .chain(recorded.keys())
        .chain(present.keys())
        .collect();
    skills
        .into_iter()
        .map(|skill| {
            let want = wanted.get(skill);
            let r = recorded.get(skill).copied();
            let w = present.get(skill).copied();
            let l = want.and_then(|want| want.library);
            Step {
                skill: skill.clone(),
                action: decide(want.is_some(), l, r, w),
                profiles: want.map(|want| want.profiles.clone()).unwrap_or_default(),
                library: l,
                recorded: r,
                present: w,
            }
        })
        .collect()
}

fn decide(
    wanted: bool,
    l: Option<Fingerprint>,
    r: Option<Fingerprint>,
    w: Option<Fingerprint>,
) -> Action {
    if wanted {
        let Some(l) = l else {
            return Action::MissingSource;
        };
        match (r, w) {
            (None, None) => Action::Install,
            (Some(_), None) => Action::Restore,
            (r, Some(w)) if w == l => {
                if r == Some(l) {
                    Action::Unchanged
                } else {
                    Action::Record
                }
            }
            (Some(r), Some(w)) if w == r => Action::Update,
            (Some(r), Some(_)) if l == r => Action::KeepLocal,
            (Some(_), Some(_)) => Action::Conflict(Conflict::Diverged),
            (None, Some(_)) => Action::Conflict(Conflict::Untracked),
        }
    } else {
        match (r, w) {
            (Some(r), Some(w)) if w == r => Action::Remove,
            (Some(_), Some(_)) => Action::Conflict(Conflict::Orphaned),
            (Some(_), None) => Action::Forget,
            (None, _) => Action::Unmanaged,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(byte: u8) -> Option<Fingerprint> {
        Some(Fingerprint::fake(byte))
    }

    const A: u8 = 1;
    const B: u8 = 2;
    const C: u8 = 3;

    #[test]
    fn decision_table() {
        use Action::*;
        let wanted = |l: u8, r: Option<u8>, w: Option<u8>| {
            decide(true, fp(l), r.and_then(fp), w.and_then(fp))
        };
        let unwanted =
            |r: Option<u8>, w: Option<u8>| decide(false, None, r.and_then(fp), w.and_then(fp));

        assert_eq!(wanted(A, None, None), Install);
        assert_eq!(wanted(A, Some(A), None), Restore);
        assert_eq!(wanted(A, Some(B), None), Restore);
        assert_eq!(wanted(A, Some(A), Some(A)), Unchanged);
        assert_eq!(
            wanted(A, Some(B), Some(A)),
            Record,
            "workspace already has the new library version"
        );
        assert_eq!(
            wanted(A, None, Some(A)),
            Record,
            "identical untracked copy is adopted"
        );
        assert_eq!(
            wanted(B, Some(A), Some(A)),
            Update,
            "library changed, copy untouched"
        );
        assert_eq!(
            wanted(A, Some(A), Some(B)),
            KeepLocal,
            "copy changed, library untouched"
        );
        assert_eq!(
            wanted(B, Some(A), Some(C)),
            Conflict(super::Conflict::Diverged)
        );
        assert_eq!(
            wanted(A, None, Some(B)),
            Conflict(super::Conflict::Untracked)
        );
        assert_eq!(decide(true, None, fp(A), fp(A)), MissingSource);
        assert_eq!(decide(true, None, None, None), MissingSource);

        assert_eq!(unwanted(Some(A), Some(A)), Remove);
        assert_eq!(
            unwanted(Some(A), Some(B)),
            Conflict(super::Conflict::Orphaned)
        );
        assert_eq!(unwanted(Some(A), None), Forget);
        assert_eq!(unwanted(None, Some(A)), Unmanaged);
    }

    #[test]
    fn the_brief_example() {
        // The workspace has git, testing and pdf; the profiles now want git,
        // testing and playwright.
        let id = |name: &str| SkillId::new(name).unwrap();
        let coding = ProfileName::new("coding").unwrap();
        let want = |l: u8| Wanted {
            library: fp(l),
            profiles: vec![coding.clone()],
        };
        let wanted = BTreeMap::from([
            (id("git"), want(A)),
            (id("testing"), want(B)),
            (id("playwright"), want(C)),
        ]);
        let recorded = BTreeMap::from([
            (id("git"), Fingerprint::fake(A)),
            (id("testing"), Fingerprint::fake(A)),
            (id("pdf"), Fingerprint::fake(A)),
        ]);
        let present = recorded.clone();

        let steps = plan(&wanted, &recorded, &present);
        let summary: Vec<(&str, Action)> =
            steps.iter().map(|s| (s.skill.as_str(), s.action)).collect();
        assert_eq!(
            summary,
            [
                ("git", Action::Unchanged),
                ("pdf", Action::Remove),
                ("playwright", Action::Install),
                ("testing", Action::Update),
            ]
        );
        assert_eq!(steps[2].profiles, [coding]);
        assert!(steps[1].profiles.is_empty());
        assert!(steps.iter().filter(|s| s.changes_files()).count() == 3);
    }

    #[test]
    fn duplicate_skills_across_profiles_collapse() {
        let id = SkillId::new("git").unwrap();
        let wanted = BTreeMap::from([(
            id.clone(),
            Wanted {
                library: fp(A),
                profiles: vec![
                    ProfileName::new("coding").unwrap(),
                    ProfileName::new("research").unwrap(),
                ],
            },
        )]);
        let steps = plan(&wanted, &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].action, Action::Install);
        assert_eq!(steps[0].profiles.len(), 2);
    }
}

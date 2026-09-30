//! Reconciliation planning: what to do with each skill so a workspace
//! matches its enabled profiles, without losing local changes.
//!
//! Planning is a pure function of three fingerprints per skill:
//!
//! * **L**, the library version (if the skill is wanted),
//! * **R**, the recorded base: the library version the workspace copy was
//!   installed from, as kept in the registry,
//! * **W**, the workspace copy as it is now,
//!
//! plus **K**, a library version the person chose not to take when they
//! kept their local copy (see [`Installation`]).
//!
//! This is a three-way comparison with R as the common ancestor, the same
//! rule a version control merge uses per file:
//!
//! | wanted? | situation                     | action                                 |
//! |---------|-------------------------------|----------------------------------------|
//! | yes     | no W                          | install (restore, if recorded)         |
//! | yes     | W = L                         | nothing (record L if R differs)        |
//! | yes     | W present, not recorded       | conflict: Beskar did not install it    |
//! | yes     | W ≠ L, L = K                  | keep: the person chose the local copy  |
//! | yes     | W ≠ L, kept without a base    | conflict: the library moved on         |
//! | yes     | W = R, L changed              | update: nobody touched the copy        |
//! | yes     | W changed, L = R              | keep: the change is local only         |
//! | yes     | W, L both changed             | conflict                               |
//! | no      | W = R                         | remove                                 |
//! | no      | W changed (or no R)           | conflict: removing would lose changes  |
//! | no      | no W, recorded                | forget the record                      |
//! | no      | W present, not recorded       | leave it: not Beskar's                 |
//!
//! Given the same library, profiles and workspace, the plan is the same.

use std::collections::{BTreeMap, BTreeSet};

use crate::fingerprint::Fingerprint;
use crate::names::{ProfileName, SkillId};
use crate::registry::Installation;

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
    /// No longer wanted and untouched, but the copy holds files that are
    /// not part of the skill (its own `.git`, a file the ignore patterns
    /// name), so deleting it would lose them: it stays where it is and
    /// Beskar stops managing it. Never decided by [`plan`], which cannot
    /// see files; the workspace planner turns a `Remove` into this.
    Release,
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
    /// version is recorded as declined, so Beskar asks again only when the
    /// library changes again; the recorded base stays, so a later promote
    /// still knows the library moved. An unwanted skill stays and Beskar
    /// stops managing it.
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
    /// What the registry records, if Beskar manages the skill here: R, the
    /// recorded base, and K, a library version the person declined.
    pub recorded: Option<Installation>,
    /// W: the workspace copy.
    pub present: Option<Fingerprint>,
}

impl Step {
    /// R: the recorded base, the library version the copy came from.
    pub fn base(&self) -> Option<Fingerprint> {
        self.recorded.and_then(|installation| installation.base)
    }

    /// Whether carrying out the step changes files in the workspace.
    pub fn changes_files(&self) -> bool {
        matches!(
            self.action,
            Action::Install | Action::Restore | Action::Update | Action::Remove
        )
    }

    /// Whether the step changes anything, files or registry.
    pub fn changes_anything(&self) -> bool {
        self.changes_files()
            || matches!(
                self.action,
                Action::Forget | Action::Record | Action::Release
            )
    }
}

/// Plan every skill that is wanted, recorded or present. Steps are sorted
/// by skill name.
pub fn plan(
    wanted: &BTreeMap<SkillId, Wanted>,
    recorded: &BTreeMap<SkillId, Installation>,
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
    recorded: Option<Installation>,
    w: Option<Fingerprint>,
) -> Action {
    let r = recorded.and_then(|installation| installation.base);
    let k = recorded.and_then(|installation| installation.kept);
    if wanted {
        let Some(l) = l else {
            return Action::MissingSource;
        };
        let Some(w) = w else {
            return if recorded.is_some() {
                Action::Restore
            } else {
                Action::Install
            };
        };
        if w == l {
            return if recorded == Some(Installation::of(l)) {
                Action::Unchanged
            } else {
                Action::Record
            };
        }
        if recorded.is_none() {
            return Action::Conflict(Conflict::Untracked);
        }
        if r == Some(w) {
            // Nobody changed the copy (or a kept change was undone).
            return Action::Update;
        }
        if k == Some(l) {
            return Action::KeepLocal;
        }
        match r {
            None => Action::Conflict(Conflict::Untracked),
            Some(r) if l == r => Action::KeepLocal,
            Some(_) => Action::Conflict(Conflict::Diverged),
        }
    } else {
        match (recorded, w) {
            (None, _) => Action::Unmanaged,
            (Some(_), None) => Action::Forget,
            (Some(_), Some(w)) if r == Some(w) => Action::Remove,
            (Some(_), Some(_)) => Action::Conflict(Conflict::Orphaned),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(byte: u8) -> Option<Fingerprint> {
        Some(Fingerprint::fake(byte))
    }

    fn installed(base: u8) -> Installation {
        Installation::of(Fingerprint::fake(base))
    }

    const A: u8 = 1;
    const B: u8 = 2;
    const C: u8 = 3;

    #[test]
    fn decision_table() {
        use Action::*;
        let wanted = |l: u8, r: Option<u8>, w: Option<u8>| {
            decide(true, fp(l), r.map(installed), w.and_then(fp))
        };
        let unwanted =
            |r: Option<u8>, w: Option<u8>| decide(false, None, r.map(installed), w.and_then(fp));

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
        assert_eq!(decide(true, None, Some(installed(A)), fp(A)), MissingSource);
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
    fn keeping_declines_one_library_version_and_keeps_the_base() {
        use Action::*;
        let kept = |base: Option<u8>, declined: u8| Installation {
            base: base.and_then(fp),
            kept: fp(declined),
        };
        // Changed here, library at B, the person kept their copy over B.
        assert_eq!(
            decide(true, fp(B), Some(kept(Some(A), B)), fp(C)),
            KeepLocal
        );
        // The library moves on to C: ask again.
        assert_eq!(
            decide(true, fp(C), Some(kept(Some(A), B)), fp(4)),
            Conflict(super::Conflict::Diverged)
        );
        // The person reverted their change: the copy is the base again, so
        // it simply updates.
        assert_eq!(decide(true, fp(B), Some(kept(Some(A), B)), fp(A)), Update);
        // A directory Beskar never installed, kept over B, has no base.
        assert_eq!(decide(true, fp(B), Some(kept(None, B)), fp(C)), KeepLocal);
        assert_eq!(
            decide(true, fp(4), Some(kept(None, B)), fp(C)),
            Conflict(super::Conflict::Untracked)
        );
        // Unwanted, a kept copy is never removed silently.
        assert_eq!(
            decide(false, None, Some(kept(None, B)), fp(C)),
            Conflict(super::Conflict::Orphaned)
        );
        assert_eq!(
            decide(false, None, Some(kept(Some(A), B)), fp(C)),
            Conflict(super::Conflict::Orphaned)
        );
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
            (id("git"), installed(A)),
            (id("testing"), installed(A)),
            (id("pdf"), installed(A)),
        ]);
        let present: BTreeMap<SkillId, Fingerprint> = recorded
            .iter()
            .map(|(id, installation)| (id.clone(), installation.base.unwrap()))
            .collect();

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

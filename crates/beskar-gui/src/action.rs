//! GUI-side pending mutations (spec §104, §113).
//!
//! The GUI owns its graphical state and its action vocabulary; every action
//! is fulfilled exclusively through the shared core services
//! ([`crate::services`]) the CLI and TUI also use, so UI state never alters
//! domain semantics (§105). Actions are first planned (dry-run through the
//! real planner), confirmed, then executed (§89, §135.38-39).

use std::path::PathBuf;

use beskar_core::editing::SkillPivot;
use beskar_core::ids::ProfileId;

/// One pending mutation, in planning order (§89).
#[derive(Debug, Clone, PartialEq)]
pub enum GuiAction {
    // ---- Installations (§52-§56, §78-§79, §104) ----
    /// The §104 staged workflow: attach several profiles to one existing
    /// installation, applied as sequential core `add` operations — each a
    /// complete plan→execute→persist transaction (§89, §22 idempotent).
    AttachProfiles {
        workspace: PathBuf,
        target: Option<String>,
        profiles: Vec<String>,
    },
    /// The inverse §104 workflow: detach several attachments, applied as
    /// sequential core `remove` operations (§52-§54).
    DetachProfiles {
        workspace: PathBuf,
        target: Option<String>,
        profiles: Vec<String>,
    },
    /// Single attach; also creates the installation when none is registered
    /// for the `(workspace, target)` pair yet (§21).
    AttachProfile {
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
    /// Presentation-only attachment reorder (§79): the full desired order by
    /// immutable Profile ID; must be a permutation of the current order.
    ReorderProfiles {
        workspace: PathBuf,
        target: Option<String>,
        order: Vec<ProfileId>,
    },

    // ---- Library skills (§69-§75) ----
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

    // ---- Library profiles (§76-§77) ----
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
        pivot: SkillPivot,
    },

    // ---- Git (§62, §65, §82) ----
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

impl GuiAction {
    /// The title shown in confirm dialogs and the Activity log.
    pub fn title(&self) -> String {
        match self {
            GuiAction::AttachProfiles { profiles, .. } => {
                format!("Attach {}", profiles.join(", "))
            }
            GuiAction::DetachProfiles { profiles, .. } => {
                format!("Detach {}", profiles.join(", "))
            }
            GuiAction::AttachProfile { profile, .. } => format!("Attach profile {profile}"),
            GuiAction::UpdateInstallation { .. } => "Update installation".to_owned(),
            GuiAction::UpdateAll => "Update all installations".to_owned(),
            GuiAction::RefSet { new_ref, .. } => format!("Change source ref to {new_ref}"),
            GuiAction::Unregister { keep_files, .. } => {
                if *keep_files {
                    "Unregister installation (keep files)".to_owned()
                } else {
                    "Unregister installation (retire managed skills)".to_owned()
                }
            }
            GuiAction::ReorderProfiles { .. } => "Reorder attached profiles".to_owned(),
            GuiAction::Ingest { source, bucket } => {
                format!("Ingest {} into {bucket}", source.display())
            }
            GuiAction::SkillMove { skill, bucket } => format!("Move {skill} to {bucket}"),
            GuiAction::SkillRename { old, new } => format!("Rename {old} to {new}"),
            GuiAction::SkillRemove { skill, cascade } => {
                if *cascade {
                    format!("Remove {skill} (cascade from profiles)")
                } else {
                    format!("Remove {skill}")
                }
            }
            GuiAction::SkillTag { skill, .. } => format!("Tag {skill}"),
            GuiAction::SkillRank { skill, .. } => format!("Rank {skill}"),
            GuiAction::ProfileCreate { name, .. } => format!("Create profile {name}"),
            GuiAction::ProfileDelete { name } => format!("Delete profile {name}"),
            GuiAction::ProfileRename { old, new } => format!("Rename profile {old} to {new}"),
            GuiAction::ProfileAddSkills { profile, skills } => {
                format!("Add {} to {profile}", skills.join(", "))
            }
            GuiAction::ProfileRemoveSkills { profile, skills } => {
                format!("Remove {} from {profile}", skills.join(", "))
            }
            GuiAction::ProfileMoveSkill { profile, skill, .. } => {
                format!("Reorder {skill} in {profile}")
            }
            GuiAction::Fetch { remote } => format!("Fetch from {remote}"),
            GuiAction::Push { branch, .. } => match branch {
                Some(branch) => format!("Push {branch}"),
                None => "Push current branch".to_owned(),
            },
            GuiAction::SwitchBranch { name } => format!("Switch to branch {name}"),
            GuiAction::CreateBranch { name } => format!("Create branch {name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_describe_the_action() {
        let action = GuiAction::AttachProfiles {
            workspace: PathBuf::from("/ws"),
            target: None,
            profiles: vec!["dev-core".into(), "rust-development".into()],
        };
        assert_eq!(action.title(), "Attach dev-core, rust-development");

        let action = GuiAction::Unregister {
            workspace: PathBuf::from("/ws"),
            target: None,
            keep_files: true,
        };
        assert_eq!(action.title(), "Unregister installation (keep files)");
    }
}

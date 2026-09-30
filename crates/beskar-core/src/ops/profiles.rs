//! Profiles: named sets of skills, kept in the library.

use std::path::PathBuf;

use crate::names::{ProfileName, SkillId};
use crate::profile::Profile;
use crate::usage;
use crate::{Beskar, Error, Result};

/// A profile as `profile list` shows it.
#[derive(Clone, Debug)]
pub struct ProfileSummary {
    pub name: ProfileName,
    /// The profile, or why its file does not load.
    pub profile: Result<Profile>,
    /// Workspaces that enable it.
    pub workspaces: usize,
}

/// One profile with its skills and where it is enabled.
#[derive(Clone, Debug)]
pub struct ProfileDetails {
    pub profile: Profile,
    /// Its skills that the library does not have.
    pub missing: Vec<SkillId>,
    /// Workspaces that enable it, sorted.
    pub enabled_in: Vec<PathBuf>,
}

/// What adding skills to or removing them from a profile did.
#[derive(Clone, Debug)]
pub struct ProfileEdit {
    pub name: ProfileName,
    /// Skills added or removed.
    pub changed: Vec<SkillId>,
    /// Skills that were in it already (adding) or not in it (removing).
    pub unchanged: Vec<SkillId>,
    /// For each skill that was not in the profile when removing, a close
    /// name that is, if any.
    pub suggestions: Vec<(SkillId, SkillId)>,
    /// Workspaces that enable the profile; their next update applies the
    /// change.
    pub enabled_in: usize,
}

/// A profile about to be deleted. Show it to the person, then pass it to
/// [`Beskar::delete_profile`].
#[derive(Clone, Debug)]
pub struct ProfileDeletion {
    pub name: ProfileName,
    pub path: PathBuf,
    /// Workspaces that enable it; deleting disables it there.
    pub enabled_in: Vec<PathBuf>,
}

/// What `delete_profile` did.
#[derive(Clone, Debug)]
pub struct ProfileDeleted {
    pub name: ProfileName,
    /// Workspaces it was disabled in.
    pub disabled_in: Vec<PathBuf>,
}

impl Beskar {
    /// Create a profile, optionally with skills and a description.
    pub fn create_profile(
        &self,
        name: &str,
        skills: &[String],
        description: Option<&str>,
    ) -> Result<Profile> {
        self.library.check()?;
        let name = ProfileName::new(name)?;
        let skills = skill_ids(skills)?;
        self.transact("profile create", |_| {
            self.library.create_profile(&name, description, &skills)
        })
    }

    /// Every profile, sorted by name, including ones that do not load.
    pub fn profiles(&self) -> Result<Vec<ProfileSummary>> {
        self.library.check()?;
        let registry = self.registry()?;
        Ok(self
            .library
            .profile_names()?
            .into_iter()
            .map(|name| ProfileSummary {
                workspaces: usage::profile_users(&registry, &name).len(),
                profile: self.library.profile(&name),
                name,
            })
            .collect())
    }

    pub fn profile_details(&self, name: &str) -> Result<ProfileDetails> {
        self.library.check()?;
        let name = self.library.find_profile(name)?;
        let profile = self.library.profile(&name)?;
        let registry = self.registry()?;
        Ok(ProfileDetails {
            missing: profile
                .skills
                .iter()
                .filter(|skill| !self.library.contains(skill))
                .cloned()
                .collect(),
            enabled_in: usage::profile_users(&registry, &name),
            profile,
        })
    }

    /// Add skills to a profile, keeping its comments and layout.
    pub fn add_to_profile(&self, name: &str, skills: &[String]) -> Result<ProfileEdit> {
        self.library.check()?;
        let name = self.library.find_profile(name)?;
        let skills = skill_ids(skills)?;
        self.transact("profile add", |registry| {
            let added = self.library.add_to_profile(&name, &skills)?;
            Ok(ProfileEdit {
                unchanged: unchanged(&skills, &added),
                changed: added,
                suggestions: Vec::new(),
                enabled_in: usage::profile_users(registry, &name).len(),
                name,
            })
        })
    }

    /// Remove skills from a profile, keeping its comments and layout.
    pub fn remove_from_profile(&self, name: &str, skills: &[String]) -> Result<ProfileEdit> {
        self.library.check()?;
        let name = self.library.find_profile(name)?;
        let skills = skill_ids(skills)?;
        self.transact("profile remove", |registry| {
            let before = self.library.profile(&name)?;
            let removed = self.library.remove_from_profile(&name, &skills)?;
            let unchanged = unchanged(&skills, &removed);
            let suggestions = unchanged
                .iter()
                .filter_map(|skill| {
                    let close =
                        bsk::closest(skill.as_str(), before.skills.iter().map(SkillId::as_str))?;
                    Some((skill.clone(), SkillId::new(close).ok()?))
                })
                .collect();
            Ok(ProfileEdit {
                changed: removed,
                unchanged,
                suggestions,
                enabled_in: usage::profile_users(registry, &name).len(),
                name,
            })
        })
    }

    /// Check that a profile can be deleted: it exists, and it is enabled
    /// nowhere unless `force` disables it there too.
    pub fn profile_deletion(&self, name: &str, force: bool) -> Result<ProfileDeletion> {
        self.library.check()?;
        let name = self.library.find_profile(name)?;
        let users = usage::profile_users(&self.registry()?, &name);
        if !users.is_empty() && !force {
            let places: Vec<String> = users.iter().map(|path| self.display(path)).collect();
            let mut error = Error::invalid(format!(
                "profile `{name}` is enabled in {}: {}",
                crate::count(users.len(), "workspace"),
                crate::join_and(&places)
            ));
            for place in places.iter().take(3) {
                error = error.hint(format!(
                    "`beskar repo disable {name} --repo {}` disables it there",
                    crate::shell_quote(place)
                ));
            }
            return Err(error.hint(format!(
                "or `beskar profile delete {name} --force --yes` deletes it and disables it everywhere"
            )));
        }
        Ok(ProfileDeletion {
            path: self.library.profile_path(&name),
            name,
            enabled_in: users,
        })
    }

    /// Delete a profile and disable it wherever it is enabled. Its skills
    /// stay in those workspaces until their next update.
    pub fn delete_profile(&self, deletion: &ProfileDeletion) -> Result<ProfileDeleted> {
        self.transact("profile delete", |registry| {
            let name = &deletion.name;
            let users = usage::profile_users(registry, name);
            if let Some(extra) = users.iter().find(|p| !deletion.enabled_in.contains(p)) {
                return Err(Error::conflict(format!(
                    "profile `{name}` was enabled in {} since beskar looked",
                    self.display(extra)
                ))
                .hint("run the command again to see the new state"));
            }
            self.library.delete_profile(name)?;
            for path in &users {
                if let Some(entry) = registry.get_mut(path) {
                    entry.profiles.retain(|profile| profile != name);
                }
            }
            Ok(ProfileDeleted {
                name: name.clone(),
                disabled_in: users,
            })
        })
    }
}

fn skill_ids(names: &[String]) -> Result<Vec<SkillId>> {
    names.iter().map(|name| SkillId::new(name)).collect()
}

/// The skills asked for that `changed` does not contain, once each.
fn unchanged(asked: &[SkillId], changed: &[SkillId]) -> Vec<SkillId> {
    let mut out: Vec<SkillId> = Vec::new();
    for skill in asked {
        if !changed.contains(skill) && !out.contains(skill) {
            out.push(skill.clone());
        }
    }
    out
}

//! Questions the registry answers: where a profile is used, where a skill
//! is installed and why, and overall counts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::Result;
use crate::library::Library;
use crate::names::{ProfileName, SkillId};
use crate::profile::Profile;
use crate::registry::Registry;

/// Workspaces that enable `profile`, sorted by path.
pub fn profile_users(registry: &Registry, profile: &ProfileName) -> Vec<PathBuf> {
    registry
        .repos()
        .filter(|repo| repo.profiles.contains(profile))
        .map(|repo| repo.path.clone())
        .collect()
}

/// One workspace's relation to a skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillUse {
    pub repo: PathBuf,
    /// Enabled profiles in this workspace that include the skill. Empty if
    /// the skill is installed but no longer wanted.
    pub profiles: Vec<ProfileName>,
    /// Whether Beskar has installed it there.
    pub installed: bool,
}

/// Where `skill` is installed or wanted, sorted by workspace path.
/// `profiles` are the library's profiles, by name.
pub fn skill_users(
    registry: &Registry,
    profiles: &BTreeMap<ProfileName, Profile>,
    skill: &SkillId,
) -> Vec<SkillUse> {
    registry
        .repos()
        .filter_map(|repo| {
            let via: Vec<ProfileName> = repo
                .profiles
                .iter()
                .filter(|name| {
                    profiles
                        .get(*name)
                        .is_some_and(|profile| profile.skills.contains(skill))
                })
                .cloned()
                .collect();
            let installed = repo.installed.contains_key(skill);
            (installed || !via.is_empty()).then(|| SkillUse {
                repo: repo.path.clone(),
                profiles: via,
                installed,
            })
        })
        .collect()
}

/// Profiles that include `skill`.
pub fn profiles_with(
    profiles: &BTreeMap<ProfileName, Profile>,
    skill: &SkillId,
) -> Vec<ProfileName> {
    profiles
        .values()
        .filter(|profile| profile.skills.contains(skill))
        .map(|profile| profile.name.clone())
        .collect()
}

/// Every profile in the library that loads, by name. Broken profiles are
/// left out; `beskar doctor` reports them.
pub fn loadable_profiles(library: &Library) -> BTreeMap<ProfileName, Profile> {
    library
        .profile_names()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|name| library.profile(&name).ok().map(|profile| (name, profile)))
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub repositories: usize,
    pub profiles: usize,
    pub library_skills: usize,
    /// Installed copies across all workspaces.
    pub installed_skills: usize,
    /// Library skills installed in no workspace.
    pub unused_skills: Vec<SkillId>,
    /// Library skills that no profile includes.
    pub unprofiled_skills: Vec<SkillId>,
    /// Profiles no workspace enables.
    pub unused_profiles: Vec<ProfileName>,
}

pub fn stats(library: &Library, registry: &Registry) -> Result<Stats> {
    let skills = library.skill_ids()?;
    let profiles = loadable_profiles(library);
    let installed: BTreeSet<&SkillId> = registry
        .repos()
        .flat_map(|repo| repo.installed.keys())
        .collect();
    let profiled: BTreeSet<&SkillId> = profiles
        .values()
        .flat_map(|profile| profile.skills.iter())
        .collect();
    let enabled: BTreeSet<&ProfileName> = registry
        .repos()
        .flat_map(|repo| repo.profiles.iter())
        .collect();
    Ok(Stats {
        repositories: registry.len(),
        profiles: library.profile_names()?.len(),
        library_skills: skills.len(),
        installed_skills: registry.repos().map(|repo| repo.installed.len()).sum(),
        unused_skills: skills
            .iter()
            .filter(|id| !installed.contains(id))
            .cloned()
            .collect(),
        unprofiled_skills: skills
            .iter()
            .filter(|id| !profiled.contains(id))
            .cloned()
            .collect(),
        unused_profiles: profiles
            .keys()
            .filter(|name| !enabled.contains(name))
            .cloned()
            .collect(),
    })
}

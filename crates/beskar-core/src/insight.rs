//! Read-only questions across library and registry: where is a profile used,
//! where is a skill installed, how big is everything.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::error::Result;
use crate::library::Library;
use crate::registry::Registry;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub repositories: usize,
    pub profiles: usize,
    pub library_skills: usize,
    /// Sum over repositories of recorded installations.
    pub installed_skills: usize,
    /// Library skills that no profile lists.
    pub unused_skills: usize,
    /// Library skills that no registered repository has installed.
    pub uninstalled_skills: usize,
}

pub fn stats(library: &Library, registry: &Registry) -> Result<Stats> {
    let skills = library.skills()?;
    let profiles = library.profiles()?;
    let in_profiles: BTreeSet<String> = profiles.iter().flat_map(|p| p.skills()).collect();
    let installed: BTreeSet<&str> = registry
        .repos()
        .iter()
        .flat_map(|r| r.installed.keys().map(String::as_str))
        .collect();
    Ok(Stats {
        repositories: registry.repos().len(),
        profiles: profiles.len(),
        library_skills: skills.len(),
        installed_skills: registry.repos().iter().map(|r| r.installed.len()).sum(),
        unused_skills: skills
            .iter()
            .filter(|s| !in_profiles.contains(&s.name))
            .count(),
        uninstalled_skills: skills
            .iter()
            .filter(|s| !installed.contains(s.name.as_str()))
            .count(),
    })
}

/// One repository's relationship with a skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillUse {
    pub repo: PathBuf,
    /// Enabled profiles of that repository which list the skill.
    pub via_profiles: Vec<String>,
    /// Whether Beskar has recorded an installation there.
    pub installed: bool,
}

/// Every repository that either wants the skill (through an enabled profile)
/// or has it installed.
pub fn skill_usage(library: &Library, registry: &Registry, skill: &str) -> Result<Vec<SkillUse>> {
    let mut out = Vec::new();
    for repo in registry.repos() {
        let resolved = library.resolve(&repo.profiles)?;
        let via = resolved.skills.get(skill).cloned().unwrap_or_default();
        let installed = repo.installed.contains_key(skill);
        if !via.is_empty() || installed {
            out.push(SkillUse {
                repo: repo.path.clone(),
                via_profiles: via,
                installed,
            });
        }
    }
    Ok(out)
}

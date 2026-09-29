//! Questions the registry should be able to answer: where is this profile
//! used, where is this skill installed and why, how big is everything.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::error::Result;
use crate::id::{ProfileId, SkillId};
use crate::library::Library;
use crate::profile::Profile;
use crate::registry::{Registry, Repository};

/// A repository that has a skill installed, and the enabled profiles that
/// currently account for it. `via` is empty when no enabled profile lists the
/// skill any more; the next update will remove it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub repo: PathBuf,
    pub via: Vec<ProfileId>,
}

/// Repositories that have `profile` enabled.
pub fn profile_users<'a>(registry: &'a Registry, profile: &ProfileId) -> Vec<&'a Repository> {
    registry.repositories().iter().filter(|r| r.is_enabled(profile)).collect()
}

/// Repositories where `skill` is installed.
pub fn skill_usage(library: &Library, registry: &Registry, skill: &SkillId) -> Result<Vec<Usage>> {
    let profiles = readable_profiles(library)?;
    Ok(registry
        .repositories()
        .iter()
        .filter(|r| r.installed(skill).is_some())
        .map(|repo| Usage {
            repo: repo.path.clone(),
            via: repo
                .enabled_profiles
                .iter()
                .filter(|id| profiles.iter().any(|p| &p.id == *id && p.skills.contains(skill)))
                .cloned()
                .collect(),
        })
        .collect())
}

/// Profiles in the library that parse. Broken ones are `doctor`'s business.
fn readable_profiles(library: &Library) -> Result<Vec<Profile>> {
    Ok(library.profile_ids()?.iter().filter_map(|id| library.profile(id).ok().flatten()).collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stats {
    pub repositories: usize,
    pub profiles: usize,
    pub library_skills: usize,
    /// Installations across all repositories, so a skill in three counts three times.
    pub installed_skills: usize,
    /// Library skills that no repository's enabled profiles ask for.
    pub unused_skills: Vec<SkillId>,
}

pub fn stats(library: &Library, registry: &Registry) -> Result<Stats> {
    let profiles = readable_profiles(library)?;
    let wanted: BTreeSet<&SkillId> = registry
        .repositories()
        .iter()
        .flat_map(|repo| repo.enabled_profiles.iter())
        .flat_map(|id| profiles.iter().filter(move |p| &p.id == id))
        .flat_map(|p| p.skills.iter())
        .collect();
    let all = library.skill_ids()?;
    Ok(Stats {
        repositories: registry.repositories().len(),
        profiles: library.profile_ids()?.len(),
        library_skills: all.len(),
        installed_skills: registry.repositories().iter().map(|r| r.installed_skills.len()).sum(),
        unused_skills: all.into_iter().filter(|s| !wanted.contains(s)).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Fingerprint;
    use crate::fsx::testutil::TempDir;

    fn sid(s: &str) -> SkillId {
        SkillId::new(s).unwrap()
    }

    fn pid(s: &str) -> ProfileId {
        ProfileId::new(s).unwrap()
    }

    fn fp() -> Fingerprint {
        Fingerprint::parse(&format!("fp1:{}", "0".repeat(64))).unwrap()
    }

    fn fixture() -> (TempDir, Library, Registry) {
        let dir = TempDir::new("inspect");
        let library = Library::new(dir.path().join("lib"));
        library.init().unwrap();
        for name in ["git", "pdf", "playwright", "orphan"] {
            std::fs::create_dir_all(library.skill_path(&sid(name))).unwrap();
        }
        library.create_profile(&pid("coding"), None).unwrap();
        library.create_profile(&pid("docs"), None).unwrap();
        library.add_to_profile(&pid("coding"), &[sid("git"), sid("playwright")]).unwrap();
        library.add_to_profile(&pid("docs"), &[sid("pdf")]).unwrap();

        let mut registry = Registry::new();
        registry.add("/r/api".into()).unwrap();
        registry.add("/r/site".into()).unwrap();
        let api = registry.find_mut(std::path::Path::new("/r/api")).unwrap();
        api.enable(pid("coding"));
        api.record_install(sid("git"), fp());
        api.record_install(sid("playwright"), fp());
        let site = registry.find_mut(std::path::Path::new("/r/site")).unwrap();
        site.enable(pid("coding"));
        site.enable(pid("docs"));
        site.record_install(sid("playwright"), fp());
        site.record_install(sid("pdf"), fp());
        (dir, library, registry)
    }

    #[test]
    fn profile_users_lists_repositories_with_it_enabled() {
        let (_dir, _library, registry) = fixture();
        let paths = |id: &str| -> Vec<PathBuf> {
            profile_users(&registry, &pid(id)).iter().map(|r| r.path.clone()).collect()
        };
        assert_eq!(paths("coding"), [PathBuf::from("/r/api"), PathBuf::from("/r/site")]);
        assert_eq!(paths("docs"), [PathBuf::from("/r/site")]);
        assert!(paths("nothing").is_empty());
    }

    #[test]
    fn skill_usage_says_where_and_why() {
        let (_dir, library, registry) = fixture();
        let usage = skill_usage(&library, &registry, &sid("playwright")).unwrap();
        assert_eq!(
            usage,
            [
                Usage { repo: "/r/api".into(), via: vec![pid("coding")] },
                Usage { repo: "/r/site".into(), via: vec![pid("coding")] },
            ]
        );
        let pdf = skill_usage(&library, &registry, &sid("pdf")).unwrap();
        assert_eq!(pdf, [Usage { repo: "/r/site".into(), via: vec![pid("docs")] }]);
        assert!(skill_usage(&library, &registry, &sid("orphan")).unwrap().is_empty());
    }

    #[test]
    fn a_skill_no_enabled_profile_lists_has_an_empty_via() {
        let (_dir, library, mut registry) = fixture();
        registry.find_mut(std::path::Path::new("/r/api")).unwrap().disable(&pid("coding"));
        let usage = skill_usage(&library, &registry, &sid("git")).unwrap();
        assert_eq!(usage, [Usage { repo: "/r/api".into(), via: vec![] }]);
    }

    #[test]
    fn stats_count_installations_and_find_unused_skills() {
        let (_dir, library, registry) = fixture();
        let stats = stats(&library, &registry).unwrap();
        assert_eq!(stats.repositories, 2);
        assert_eq!(stats.profiles, 2);
        assert_eq!(stats.library_skills, 4);
        assert_eq!(stats.installed_skills, 4);
        assert_eq!(stats.unused_skills, [sid("orphan")]);
    }

    #[test]
    fn a_broken_profile_does_not_break_stats() {
        let (_dir, library, registry) = fixture();
        std::fs::write(library.profile_path(&pid("docs")), "skils pdf\n").unwrap();
        let stats = stats(&library, &registry).unwrap();
        assert_eq!(stats.profiles, 2);
        assert!(stats.unused_skills.contains(&sid("pdf")));
    }
}

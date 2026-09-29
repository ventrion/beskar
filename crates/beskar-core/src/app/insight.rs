//! Observability: the registry answers "what is installed where, and why".

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::Beskar;
use crate::error::Result;
use crate::ids::{ProfileName, SkillId};
use crate::reconcile::{self, Action, Plan};
use crate::registry::Repository;

/// How a repository stands relative to its desired state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoHealth {
    /// Everything matches.
    UpToDate,
    /// Changes are waiting and none needs a decision.
    NeedsUpdate {
        /// Skills to install.
        add: usize,
        /// Skills to bring up to date.
        update: usize,
        /// Skills to remove.
        remove: usize,
    },
    /// Some installed skills have local changes that an update would overwrite.
    NeedsAttention {
        /// How many skills are in conflict.
        conflicts: usize,
        /// Changes that could go ahead regardless.
        pending: usize,
    },
    /// The repository folder no longer exists.
    Missing,
    /// The repository cannot be reconciled, for example because an enabled profile is gone.
    Broken(String),
}

/// One repository and how it stands.
#[derive(Clone, Debug)]
pub struct RepoSummary {
    /// The registry record.
    pub repo: Repository,
    /// Its standing.
    pub health: RepoHealth,
    /// The plan, when one could be made.
    pub plan: Option<Plan>,
}

/// Counts for the whole setup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    /// Registered repositories.
    pub repositories: usize,
    /// Profiles in the library.
    pub profiles: usize,
    /// Skills in the library.
    pub library_skills: usize,
    /// Installed skills, counted once per repository.
    pub installed_skills: usize,
    /// Library skills that are installed nowhere.
    pub unused_skills: Vec<SkillId>,
    /// Library skills that no profile lists, so they can never be installed.
    pub unassigned_skills: Vec<SkillId>,
}

/// Where a profile is enabled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileUsage {
    /// The profile.
    pub profile: ProfileName,
    /// Whether the library has it.
    pub exists: bool,
    /// The repositories where it is enabled.
    pub repos: Vec<PathBuf>,
}

/// One place a skill is, or should be, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillPlace {
    /// The repository.
    pub repo: PathBuf,
    /// The enabled profiles that select the skill there.
    pub via: BTreeSet<ProfileName>,
    /// Whether Beskar has installed it there.
    pub installed: bool,
}

/// Where a skill is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillUsage {
    /// The skill.
    pub skill: SkillId,
    /// Whether the library has it.
    pub in_library: bool,
    /// The profiles that list it.
    pub profiles: Vec<ProfileName>,
    /// The repositories where it is installed or selected.
    pub places: Vec<SkillPlace>,
}

impl Beskar {
    /// Where a profile is enabled.
    pub fn profile_usage(&self, profile: &ProfileName) -> Result<ProfileUsage> {
        let registry = self.store().read()?;
        Ok(ProfileUsage {
            exists: self.library().profile_path(profile).is_file(),
            repos: registry
                .using_profile(profile)
                .into_iter()
                .map(|r| r.path.clone())
                .collect(),
            profile: profile.clone(),
        })
    }

    /// Where a skill is installed, and through which profiles it is wanted.
    pub fn skill_usage(&self, skill: &SkillId) -> Result<SkillUsage> {
        let library = self.library();
        let registry = self.store().read()?;
        let profiles = library.profiles()?.valid;
        let listing: Vec<&ProfileName> = profiles
            .iter()
            .filter(|p| p.skills.contains(skill))
            .map(|p| &p.name)
            .collect();
        let places = registry
            .repos()
            .filter_map(|repo| {
                let via: BTreeSet<ProfileName> = listing
                    .iter()
                    .filter(|name| repo.profiles.contains(**name))
                    .map(|n| (*n).clone())
                    .collect();
                let installed = repo.installed.contains_key(skill);
                (installed || !via.is_empty()).then(|| SkillPlace {
                    repo: repo.path.clone(),
                    via,
                    installed,
                })
            })
            .collect();
        Ok(SkillUsage {
            skill: skill.clone(),
            in_library: library.has_skill(skill),
            profiles: listing.into_iter().cloned().collect(),
            places,
        })
    }

    /// How every repository stands. One broken repository does not hide the others.
    pub fn summarize_repos(&self) -> Result<Vec<RepoSummary>> {
        let registry = self.store().read()?;
        Ok(registry.repos().map(|repo| self.summarize(repo)).collect())
    }

    /// How one repository stands.
    pub fn summarize(&self, repo: &Repository) -> RepoSummary {
        if crate::fsx::is_gone(&repo.path) {
            return RepoSummary {
                repo: repo.clone(),
                health: RepoHealth::Missing,
                plan: None,
            };
        }
        match reconcile::plan(&self.library(), self.config(), repo) {
            Err(error) => RepoSummary {
                repo: repo.clone(),
                health: RepoHealth::Broken(error.message().to_string()),
                plan: None,
            },
            Ok(plan) => {
                let health = health_of(&plan);
                RepoSummary {
                    repo: repo.clone(),
                    health,
                    plan: Some(plan),
                }
            }
        }
    }

    /// Counts across the library and the registry.
    pub fn stats(&self) -> Result<Stats> {
        let library = self.library();
        let registry = self.store().read()?;
        let skills: Vec<SkillId> = library.skills()?.into_iter().map(|s| s.id).collect();
        let profiles = library.profiles()?;
        let listed: BTreeSet<&SkillId> = profiles
            .valid
            .iter()
            .flat_map(|p| p.skills.iter())
            .collect();
        let installed: BTreeSet<&SkillId> =
            registry.repos().flat_map(|r| r.installed.keys()).collect();
        Ok(Stats {
            repositories: registry.len(),
            profiles: profiles.valid.len() + profiles.broken.len(),
            library_skills: skills.len(),
            installed_skills: registry.repos().map(|r| r.installed.len()).sum(),
            unused_skills: skills
                .iter()
                .filter(|s| !installed.contains(s))
                .cloned()
                .collect(),
            unassigned_skills: skills
                .iter()
                .filter(|s| !listed.contains(s))
                .cloned()
                .collect(),
        })
    }

    /// Forgets repositories whose folders no longer exist. Files are never touched.
    /// With `dry_run`, only reports which ones it would forget.
    pub fn prune(&self, dry_run: bool) -> Result<Vec<PathBuf>> {
        let stale = |repo: &Repository| crate::fsx::is_gone(&repo.path);
        if dry_run {
            return Ok(self
                .store()
                .read()?
                .repos()
                .filter(|r| stale(r))
                .map(|r| r.path.clone())
                .collect());
        }
        self.store().update(|registry| {
            let gone: Vec<PathBuf> = registry
                .repos()
                .filter(|r| stale(r))
                .map(|r| r.path.clone())
                .collect();
            for path in &gone {
                registry.remove(path);
            }
            Ok(gone)
        })
    }

    /// Group installed skills by how many repositories hold each. Useful for cleanup decisions.
    pub fn install_counts(&self) -> Result<BTreeMap<SkillId, usize>> {
        let registry = self.store().read()?;
        let mut counts = BTreeMap::new();
        for repo in registry.repos() {
            for skill in repo.installed.keys() {
                *counts.entry(skill.clone()).or_insert(0) += 1;
            }
        }
        Ok(counts)
    }
}

fn health_of(plan: &Plan) -> RepoHealth {
    let problems = plan.problems();
    if !problems.is_empty() {
        return RepoHealth::Broken(
            problems
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        );
    }
    let count = |wanted: Action| plan.entries.iter().filter(|e| e.action == wanted).count();
    let (add, update, remove) = (
        count(Action::Add),
        count(Action::Update),
        count(Action::Remove),
    );
    let conflicts = plan.conflicts().count();
    if conflicts > 0 {
        RepoHealth::NeedsAttention {
            conflicts,
            pending: add + update + remove,
        }
    } else if add + update + remove > 0 {
        RepoHealth::NeedsUpdate {
            add,
            update,
            remove,
        }
    } else {
        RepoHealth::UpToDate
    }
}

#[cfg(test)]
mod tests {
    use super::super::repos::{ProfileChange, UpdateOptions};
    use super::super::testing::World;
    use super::*;
    use crate::config::ConflictPolicy;
    use crate::time::Timestamp;

    fn name(text: &str) -> ProfileName {
        ProfileName::parse(text).unwrap()
    }

    fn id(text: &str) -> SkillId {
        SkillId::parse(text).unwrap()
    }

    fn register(world: &World, project: &str, profiles: &[&str]) -> PathBuf {
        let path = world.project(project);
        world.beskar.add_repo(&path, &world.cwd()).unwrap();
        let names: Vec<ProfileName> = profiles.iter().map(|p| name(p)).collect();
        if !names.is_empty() {
            world
                .beskar
                .change_profiles(&path, ProfileChange::Enable, &names)
                .unwrap();
        }
        path
    }

    fn update_all(world: &World) {
        let options = UpdateOptions {
            dry_run: false,
            now: Timestamp::from_secs(1),
        };
        world
            .beskar
            .update_all(
                &options,
                &mut world.beskar.policy_resolver(Some(ConflictPolicy::Keep)),
            )
            .unwrap();
    }

    #[test]
    fn health_distinguishes_up_to_date_pending_attention_missing_and_broken() {
        let world = World::new();
        world
            .dir
            .write("user/.beskar/library/profiles/temp.bsk", "skill git\n");
        register(&world, "ok", &["coding"]);
        let pending = register(&world, "pending", &["coding"]);
        register(&world, "attention", &["coding"]);
        let vanished = register(&world, "vanished", &["coding"]);
        register(&world, "broken", &["temp"]);
        update_all(&world);

        world
            .beskar
            .change_profiles(&pending, ProfileChange::Enable, &[name("research")])
            .unwrap();
        world
            .dir
            .write("projects/attention/.agents/skills/git/SKILL.md", "edited");
        std::fs::remove_dir_all(&vanished).unwrap();
        std::fs::remove_file(
            world
                .dir
                .path()
                .join("user/.beskar/library/profiles/temp.bsk"),
        )
        .unwrap();

        let summaries = world.beskar.summarize_repos().unwrap();
        let health = |n: &str| {
            summaries
                .iter()
                .find(|s| s.repo.path.ends_with(n))
                .unwrap()
                .health
                .clone()
        };
        assert_eq!(health("ok"), RepoHealth::UpToDate);
        assert_eq!(
            health("pending"),
            RepoHealth::NeedsUpdate {
                add: 1,
                update: 0,
                remove: 0
            }
        );
        assert_eq!(
            health("attention"),
            RepoHealth::NeedsAttention {
                conflicts: 1,
                pending: 0
            }
        );
        assert_eq!(health("vanished"), RepoHealth::Missing);
        assert!(matches!(health("broken"), RepoHealth::Broken(m) if m.contains("profile 'temp'")));
    }

    #[test]
    fn a_repository_with_waiting_changes_reports_the_counts() {
        let world = World::new();
        let project = register(&world, "api", &["coding"]);
        update_all(&world);
        world
            .dir
            .write("user/.beskar/library/skills/git/SKILL.md", "v2");
        world
            .beskar
            .change_profiles(&project, ProfileChange::Enable, &[name("research")])
            .unwrap();
        let summary = &world.beskar.summarize_repos().unwrap()[0];
        assert_eq!(
            summary.health,
            RepoHealth::NeedsUpdate {
                add: 1,
                update: 1,
                remove: 0
            }
        );
    }

    #[test]
    fn stats_count_what_the_brief_asks_for() {
        let world = World::new();
        register(&world, "api", &["coding"]);
        register(&world, "docs", &["research"]);
        update_all(&world);
        world.dir.write(
            "user/.beskar/library/skills/orphan/SKILL.md",
            "in no profile",
        );
        let stats = world.beskar.stats().unwrap();
        assert_eq!(stats.repositories, 2);
        assert_eq!(stats.profiles, 2);
        assert_eq!(stats.library_skills, 4);
        assert_eq!(
            stats.installed_skills, 4,
            "git and testing in api; pdf and git in docs"
        );
        assert_eq!(stats.unused_skills, [id("orphan")]);
        assert_eq!(stats.unassigned_skills, [id("orphan")]);
    }

    #[test]
    fn stats_of_an_empty_setup() {
        let world = World::new();
        let stats = world.beskar.stats().unwrap();
        assert_eq!((stats.repositories, stats.installed_skills), (0, 0));
        assert_eq!(stats.unused_skills.len(), 3);
        assert!(stats.unassigned_skills.is_empty());
    }

    #[test]
    fn profile_usage_lists_the_repositories() {
        let world = World::new();
        let api = register(&world, "api", &["coding"]);
        let site = register(&world, "site", &["coding", "research"]);
        register(&world, "docs", &["research"]);
        let usage = world.beskar.profile_usage(&name("coding")).unwrap();
        assert!(usage.exists);
        assert_eq!(usage.repos, [api, site]);
        let none = world.beskar.profile_usage(&name("ghost")).unwrap();
        assert!(!none.exists && none.repos.is_empty());
    }

    #[test]
    fn skill_usage_says_where_a_skill_is_and_through_which_profile() {
        let world = World::new();
        let api = register(&world, "api", &["coding"]);
        let docs = register(&world, "docs", &["research"]);
        let site = register(&world, "site", &["coding", "research"]);
        register(&world, "empty", &[]);
        update_all(&world);
        world
            .beskar
            .change_profiles(&site, ProfileChange::Disable, &[name("coding")])
            .unwrap();

        let usage = world.beskar.skill_usage(&id("git")).unwrap();
        assert!(usage.in_library);
        assert_eq!(usage.profiles, [name("coding"), name("research")]);
        let places: Vec<(PathBuf, Vec<&str>, bool)> = usage
            .places
            .iter()
            .map(|p| {
                (
                    p.repo.clone(),
                    p.via.iter().map(ProfileName::as_str).collect(),
                    p.installed,
                )
            })
            .collect();
        assert_eq!(
            places,
            [
                (api, vec!["coding"], true),
                (docs, vec!["research"], true),
                (site, vec!["research"], true),
            ]
        );
        let pdf = world.beskar.skill_usage(&id("pdf")).unwrap();
        assert_eq!(pdf.places.len(), 2);
        let ghost = world.beskar.skill_usage(&id("ghost")).unwrap();
        assert!(!ghost.in_library && ghost.places.is_empty());
    }

    #[test]
    fn a_skill_installed_but_no_longer_selected_still_shows_up() {
        let world = World::new();
        let api = register(&world, "api", &["coding"]);
        update_all(&world);
        world
            .beskar
            .change_profiles(&api, ProfileChange::Disable, &[name("coding")])
            .unwrap();
        let usage = world.beskar.skill_usage(&id("git")).unwrap();
        assert_eq!(usage.places.len(), 1);
        assert!(usage.places[0].installed && usage.places[0].via.is_empty());
    }

    #[test]
    fn prune_forgets_only_vanished_repositories_and_never_touches_files() {
        let world = World::new();
        let keep = register(&world, "keep", &[]);
        let gone = register(&world, "gone", &[]);
        std::fs::remove_dir_all(&gone).unwrap();
        assert_eq!(
            world.beskar.prune(true).unwrap(),
            std::slice::from_ref(&gone)
        );
        assert_eq!(
            world.beskar.store().read().unwrap().len(),
            2,
            "a dry run changes nothing"
        );
        assert_eq!(world.beskar.prune(false).unwrap(), [gone]);
        let remaining: Vec<PathBuf> = world
            .beskar
            .store()
            .read()
            .unwrap()
            .repos()
            .map(|r| r.path.clone())
            .collect();
        assert_eq!(remaining, [keep]);
        assert!(world.beskar.prune(false).unwrap().is_empty());
    }

    #[test]
    fn install_counts_group_by_skill() {
        let world = World::new();
        register(&world, "a", &["coding"]);
        register(&world, "b", &["coding", "research"]);
        update_all(&world);
        let counts = world.beskar.install_counts().unwrap();
        assert_eq!(counts[&id("git")], 2);
        assert_eq!(counts[&id("pdf")], 1);
    }
}

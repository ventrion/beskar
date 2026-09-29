//! Repository use cases: register, enable profiles, update, remove.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::Beskar;
use crate::config::ConflictPolicy;
use crate::error::{Error, ErrorKind, Result};
use crate::fsx;
use crate::ids::ProfileName;
use crate::reconcile::{self, Outcome, Plan, PolicyResolver, Resolver};
use crate::registry::{Registry, Repository};
use crate::time::Timestamp;

/// How to change the enabled profiles of a repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileChange {
    /// Turn the profiles on.
    Enable,
    /// Turn the profiles off.
    Disable,
    /// Flip each profile: on becomes off and off becomes on.
    Toggle,
}

/// What a profile change did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileChangeReport {
    /// The repository.
    pub repo: PathBuf,
    /// Profiles that are now enabled and were not before.
    pub enabled: Vec<ProfileName>,
    /// Profiles that are now disabled and were not before.
    pub disabled: Vec<ProfileName>,
    /// Profiles that already were as requested.
    pub unchanged: Vec<ProfileName>,
    /// The enabled profiles after the change.
    pub profiles: BTreeSet<ProfileName>,
}

/// What `update` should do.
#[derive(Clone, Copy, Debug)]
pub struct UpdateOptions {
    /// Work out and report the changes without making them.
    pub dry_run: bool,
    /// The time to record as the moment of synchronisation.
    pub now: Timestamp,
}

/// The result of updating one repository.
#[derive(Clone, Debug)]
pub struct RepoUpdate {
    /// The repository as registered before the update.
    pub repo: Repository,
    /// What was planned.
    pub plan: Plan,
    /// What happened. `None` for a dry run.
    pub outcome: Option<Outcome>,
}

/// One repository's result when updating several.
#[derive(Debug)]
pub struct RepoResult {
    /// The repository's path.
    pub path: PathBuf,
    /// What happened to it.
    pub result: Result<RepoUpdate>,
}

/// What removing a repository did.
#[derive(Clone, Debug)]
pub struct RemoveReport {
    /// The repository that was forgotten.
    pub repo: Repository,
    /// When `--purge` was used: what happened to the installed skills.
    pub purge: Option<Outcome>,
}

impl Beskar {
    /// Registers a repository. Returns it, and whether it was newly registered.
    pub fn add_repo(&self, path: &Path, cwd: &Path) -> Result<(Repository, bool)> {
        let absolute = self.absolute(path, cwd);
        if !absolute.exists() {
            return Err(
                Error::not_found(format!("'{}' does not exist", absolute.display()))
                    .with_hint("check the path, or run 'beskar repo add .' from inside the folder"),
            );
        }
        if !absolute.is_dir() {
            return Err(
                Error::invalid(format!("'{}' is not a directory", absolute.display()))
                    .with_hint("register the folder that contains it"),
            );
        }
        let library_root = fsx::canonicalize(self.library().root())
            .unwrap_or_else(|_| self.library().root().to_path_buf());
        if absolute.starts_with(&library_root) {
            return Err(
                Error::invalid(format!("'{}' is inside the library", absolute.display()))
                    .with_hint(
                        "the library is where skills are kept; register the projects that use them",
                    ),
            );
        }
        reconcile::refuse_overlap(&self.library(), &absolute.join(&self.config().agent_skills))?;
        self.store().update(|registry| {
            if let Some(existing) = registry.get(&absolute) {
                return Ok((existing.clone(), false));
            }
            let repo = Repository::new(absolute.clone());
            // Fail now, with a clear message, if the path cannot be written to the registry.
            let mut probe = Registry::new();
            probe.insert(repo.clone());
            probe.render()?;
            registry.insert(repo.clone());
            Ok((repo, true))
        })
    }

    /// Finds the registered repository for a path: the path itself or the closest registered ancestor.
    /// Without a path, the current directory is used.
    pub fn find_repo(
        &self,
        registry: &Registry,
        target: Option<&Path>,
        cwd: &Path,
    ) -> Result<Repository> {
        let (start, explicit) = match target {
            Some(path) => (self.absolute(path, cwd), true),
            None => (self.absolute(cwd, cwd), false),
        };
        if let Some(exact) = registry.get(&start) {
            // Exactly a registered path, even if its folder has vanished: that is how it gets cleaned up.
            return Ok(exact.clone());
        }
        if explicit && !start.exists() {
            // A path that does not exist is a typo, and must not quietly mean the enclosing repository.
            return Err(
                Error::not_found(format!("'{}' does not exist", start.display())).with_hint(
                    "check the path, or list the registered repositories with 'beskar repo list'",
                ),
            );
        }
        registry
            .containing(&start)
            .cloned()
            .ok_or_else(|| self.not_registered(&start, explicit))
    }

    /// Forgets a repository. With a resolver, first removes the skills Beskar installed there.
    ///
    /// Without `purge`, files in the repository are left exactly as they are.
    pub fn remove_repo(
        &self,
        target: Option<&Path>,
        cwd: &Path,
        purge: Option<(&mut dyn Resolver, Timestamp)>,
    ) -> Result<RemoveReport> {
        let registry = self.store().read()?;
        let repo = match target {
            Some(path) => {
                let absolute = self.absolute(path, cwd);
                match registry.get(&absolute) {
                    Some(exact) => exact.clone(),
                    // A folder that is not there is a typo, whatever it is inside of.
                    None if !absolute.exists() => {
                        return Err(Error::not_found(format!(
                            "'{}' does not exist",
                            absolute.display()
                        ))
                        .with_hint(
                            "check the path, or list the registered repositories with 'beskar repo list'",
                        ));
                    }
                    None => {
                        let mut error = self.not_registered(&absolute, true);
                        if let Some(parent) = registry.containing(&absolute) {
                            error = Error::not_found(format!(
                                "'{}' is inside the registered repository '{}' but is not one itself",
                                absolute.display(),
                                parent.path.display()
                            ))
                            .with_hint(format!(
                                "to stop managing that repository, name it: beskar repo remove {}",
                                crate::text::shell_quote(&parent.path.to_string_lossy())
                            ));
                        }
                        return Err(error);
                    }
                }
            }
            None => self.find_repo(&registry, None, cwd)?,
        };
        let mut outcome = None;
        if let Some((resolver, now)) = purge
            && repo.path.is_dir()
        {
            let mut emptied = repo.clone();
            emptied.profiles.clear();
            let library = self.library();
            let plan = reconcile::plan(&library, self.config(), &emptied)?;
            outcome = Some(reconcile::apply(&library, &plan, &emptied, resolver, now)?);
        }
        self.store().update(|registry| {
            registry.remove(&repo.path);
            Ok(())
        })?;
        Ok(RemoveReport {
            repo,
            purge: outcome,
        })
    }

    /// Enables, disables or toggles profiles in a registered repository. Files are not touched;
    /// `update` does that.
    pub fn change_profiles(
        &self,
        repo_path: &Path,
        change: ProfileChange,
        names: &[ProfileName],
    ) -> Result<ProfileChangeReport> {
        let library = self.library();
        let current = self
            .store()
            .read()?
            .get(repo_path)
            .cloned()
            .ok_or_else(|| self.not_registered(repo_path, true))?;
        // Only profiles that would be switched on must exist. Switching off a profile that has
        // been deleted from the library is how a repository is cleaned up.
        for name in names {
            let would_enable = match change {
                ProfileChange::Enable => true,
                ProfileChange::Disable => false,
                ProfileChange::Toggle => !current.profiles.contains(name),
            };
            if would_enable {
                library.profile(name)?;
            } else if !current.profiles.contains(name) && !library.profile_path(name).is_file() {
                return Err(library.unknown_profile(name.as_str()));
            }
        }
        self.store().update(|registry| {
            let repo = registry.get_mut(repo_path).ok_or_else(|| {
                Error::new(
                    ErrorKind::NotFound,
                    "the repository was removed while this command was running",
                )
            })?;
            let mut report = ProfileChangeReport {
                repo: repo_path.to_path_buf(),
                enabled: Vec::new(),
                disabled: Vec::new(),
                unchanged: Vec::new(),
                profiles: BTreeSet::new(),
            };
            let mut seen = BTreeSet::new();
            for name in names {
                if !seen.insert(name.clone()) {
                    continue;
                }
                let on = repo.profiles.contains(name);
                let turn_on = match change {
                    ProfileChange::Enable => true,
                    ProfileChange::Disable => false,
                    ProfileChange::Toggle => !on,
                };
                match (on, turn_on) {
                    (false, true) => {
                        repo.profiles.insert(name.clone());
                        report.enabled.push(name.clone());
                    }
                    (true, false) => {
                        repo.profiles.remove(name);
                        report.disabled.push(name.clone());
                    }
                    _ => report.unchanged.push(name.clone()),
                }
            }
            report.profiles = repo.profiles.clone();
            Ok(report)
        })
    }

    /// Works out what an update would do to a registered repository, without changing anything.
    pub fn plan_repo(&self, repo: &Repository) -> Result<Plan> {
        require_folder(repo)?;
        reconcile::plan(&self.library(), self.config(), repo)
    }

    /// Reconciles one registered repository with its enabled profiles.
    ///
    /// The registry is updated with what was installed. Enabled profiles and the repository's own
    /// registration are left as they are, even if they changed while this ran.
    pub fn update_repo(
        &self,
        repo_path: &Path,
        options: &UpdateOptions,
        resolver: &mut dyn Resolver,
    ) -> Result<RepoUpdate> {
        let repo = self
            .store()
            .read()?
            .get(repo_path)
            .cloned()
            .ok_or_else(|| self.not_registered(repo_path, true))?;
        require_folder(&repo)?;
        let library = self.library();
        let plan = reconcile::plan(&library, self.config(), &repo)?;
        if options.dry_run {
            return Ok(RepoUpdate {
                repo,
                plan,
                outcome: None,
            });
        }
        let outcome = reconcile::apply(&library, &plan, &repo, resolver, options.now)?;
        self.store().update(|registry| {
            if let Some(current) = registry.get_mut(&repo.path) {
                current.installed = outcome.repo.installed.clone();
                current.synced = outcome.repo.synced;
            }
            Ok(())
        })?;
        Ok(RepoUpdate {
            repo,
            plan,
            outcome: Some(outcome),
        })
    }

    /// Reconciles every registered repository, one after another. One repository failing does not stop the rest.
    pub fn update_all(
        &self,
        options: &UpdateOptions,
        resolver: &mut dyn Resolver,
    ) -> Result<Vec<RepoResult>> {
        let paths: Vec<PathBuf> = self
            .store()
            .read()?
            .repos()
            .map(|r| r.path.clone())
            .collect();
        Ok(paths
            .into_iter()
            .map(|path| RepoResult {
                result: self.update_repo(&path, options, resolver),
                path,
            })
            .collect())
    }

    /// A resolver that applies the configured policy, or the one given, without asking anybody.
    pub fn policy_resolver(&self, policy: Option<ConflictPolicy>) -> PolicyResolver {
        PolicyResolver(policy.unwrap_or(self.config().on_conflict))
    }
}

fn require_folder(repo: &Repository) -> Result<()> {
    if !crate::fsx::is_gone(&repo.path) {
        Ok(())
    } else {
        let path = crate::text::shell_quote(&repo.path.to_string_lossy());
        Err(Error::not_found(format!(
            "the repository folder {} no longer exists",
            repo.path.display()
        ))
        .with_hint(format!(
            "if it moved, register the new location with 'beskar repo add'. To forget this entry, run 'beskar registry prune' (every missing folder) or, for this one: beskar repo remove {path}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::World;
    use super::*;
    use crate::config::ConflictPolicy;
    use crate::reconcile::{Action, Done};

    fn name(text: &str) -> ProfileName {
        ProfileName::parse(text).unwrap()
    }

    fn options(dry_run: bool) -> UpdateOptions {
        UpdateOptions {
            dry_run,
            now: Timestamp::from_secs(1_785_320_100),
        }
    }

    fn registered(world: &World, project: &str) -> PathBuf {
        let path = world.project(project);
        world.beskar.add_repo(&path, &world.cwd()).unwrap();
        path
    }

    #[test]
    fn adding_a_repository_is_idempotent_and_accepts_relative_paths() {
        let world = World::new();
        let project = world.project("api");
        let (repo, added) = world
            .beskar
            .add_repo(Path::new("projects/api"), &world.cwd())
            .unwrap();
        assert!(added);
        assert_eq!(repo.path, project);
        let (_, again) = world.beskar.add_repo(&project, &world.cwd()).unwrap();
        assert!(!again);
        assert_eq!(world.beskar.store().read().unwrap().len(), 1);
    }

    #[test]
    fn adding_rejects_missing_paths_files_and_the_library() {
        let world = World::new();
        assert_eq!(
            world
                .beskar
                .add_repo(Path::new("nope"), &world.cwd())
                .unwrap_err()
                .kind(),
            ErrorKind::NotFound
        );
        world.dir.write("file.txt", "x");
        assert_eq!(
            world
                .beskar
                .add_repo(Path::new("file.txt"), &world.cwd())
                .unwrap_err()
                .kind(),
            ErrorKind::Invalid
        );
        let library = world.beskar.library().root().to_path_buf();
        let e = world.beskar.add_repo(&library, &world.cwd()).unwrap_err();
        assert!(e.message().contains("is inside the library"));
        let e = world
            .beskar
            .add_repo(&library.join("skills/git"), &world.cwd())
            .unwrap_err();
        assert!(e.message().contains("is inside the library"));
    }

    #[test]
    fn adding_rejects_paths_the_registry_cannot_store() {
        let world = World::new();
        let odd = world.dir.mkdir("projects/trailing space ");
        let e = world.beskar.add_repo(&odd, &world.cwd()).unwrap_err();
        assert!(
            e.message().contains("cannot record the repository path"),
            "{}",
            e.message()
        );
        assert!(world.beskar.store().read().unwrap().is_empty());
    }

    #[test]
    fn find_repo_matches_the_repository_or_a_folder_inside_it() {
        let world = World::new();
        let project = registered(&world, "api");
        let inside = world.dir.mkdir("projects/api/src/deep");
        let registry = world.beskar.store().read().unwrap();
        assert_eq!(
            world
                .beskar
                .find_repo(&registry, None, &project)
                .unwrap()
                .path,
            project
        );
        assert_eq!(
            world
                .beskar
                .find_repo(&registry, None, &inside)
                .unwrap()
                .path,
            project
        );
        assert_eq!(
            world
                .beskar
                .find_repo(&registry, Some(Path::new("src")), &project)
                .unwrap()
                .path,
            project
        );
        assert_eq!(
            world
                .beskar
                .find_repo(&registry, Some(&project), Path::new("/"))
                .unwrap()
                .path,
            project
        );
    }

    #[test]
    fn find_repo_explains_how_to_register() {
        let world = World::new();
        let other = world.project("other");
        let registry = world.beskar.store().read().unwrap();
        let e = world.beskar.find_repo(&registry, None, &other).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert!(
            e.message()
                .contains("is not inside a registered repository")
        );
        assert_eq!(
            e.hint(),
            Some("register it with 'beskar repo add .', or name a repository with a path")
        );
        let e = world
            .beskar
            .find_repo(&registry, Some(&other), &other)
            .unwrap_err();
        assert!(
            e.hint()
                .unwrap()
                .starts_with("register it with: beskar repo add ")
        );
    }

    #[test]
    fn enable_disable_and_toggle_change_only_the_registry() {
        let world = World::new();
        let project = registered(&world, "api");
        let enabled = world
            .beskar
            .change_profiles(
                &project,
                ProfileChange::Enable,
                &[name("coding"), name("research")],
            )
            .unwrap();
        assert_eq!(enabled.enabled.len(), 2);
        assert!(
            !world.dir.exists("projects/api/.agents"),
            "enabling must not touch files"
        );

        let again = world
            .beskar
            .change_profiles(&project, ProfileChange::Enable, &[name("coding")])
            .unwrap();
        assert_eq!(again.unchanged, [name("coding")]);
        assert!(again.enabled.is_empty());

        world
            .beskar
            .library()
            .create_profile(&name("spare"), None, &[])
            .unwrap();
        let off = world
            .beskar
            .change_profiles(
                &project,
                ProfileChange::Disable,
                &[name("research"), name("spare")],
            )
            .unwrap();
        assert_eq!(off.disabled, [name("research")]);
        assert_eq!(
            off.unchanged,
            [name("spare")],
            "a real profile that was not on is harmless"
        );
        assert_eq!(off.profiles, [name("coding")].into_iter().collect());

        let flipped = world
            .beskar
            .change_profiles(
                &project,
                ProfileChange::Toggle,
                &[name("coding"), name("research")],
            )
            .unwrap();
        assert_eq!(flipped.disabled, [name("coding")]);
        assert_eq!(flipped.enabled, [name("research")]);
        assert_eq!(flipped.profiles, [name("research")].into_iter().collect());
    }

    #[test]
    fn enabling_needs_an_existing_profile_and_disabling_a_typo_is_an_error() {
        let world = World::new();
        let project = registered(&world, "api");
        let e = world
            .beskar
            .change_profiles(&project, ProfileChange::Enable, &[name("codng")])
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert_eq!(e.hint(), Some("did you mean 'coding'?"));
        let e = world
            .beskar
            .change_profiles(&project, ProfileChange::Toggle, &[name("codng")])
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        let e = world
            .beskar
            .change_profiles(&project, ProfileChange::Disable, &[name("codng")])
            .unwrap_err();
        assert_eq!(
            e.kind(),
            ErrorKind::NotFound,
            "a name that is neither enabled nor in the library is a typo"
        );
        assert_eq!(e.hint(), Some("did you mean 'coding'?"));
        let registry = world.beskar.store().read().unwrap();
        assert!(
            registry.get(&project).unwrap().profiles.is_empty(),
            "a failed request must not half-apply"
        );
    }

    #[test]
    fn a_profile_deleted_from_the_library_can_still_be_disabled_to_clean_up() {
        let world = World::new();
        let project = registered(&world, "api");
        world
            .beskar
            .change_profiles(&project, ProfileChange::Enable, &[name("coding")])
            .unwrap();
        std::fs::remove_file(
            world
                .dir
                .path()
                .join("user/.beskar/library/profiles/coding.bsk"),
        )
        .unwrap();
        let report = world
            .beskar
            .change_profiles(&project, ProfileChange::Disable, &[name("coding")])
            .unwrap();
        assert_eq!(report.disabled, [name("coding")]);
    }

    #[test]
    fn a_typo_in_the_repository_path_does_not_mean_the_enclosing_repository() {
        let world = World::new();
        let project = registered(&world, "api");
        let registry = world.beskar.store().read().unwrap();
        let e = world
            .beskar
            .find_repo(&registry, Some(Path::new("typo-dir")), &project)
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert!(e.message().contains("does not exist"), "{}", e.message());
        // An existing folder inside the repository is fine for read-only lookups ...
        let inside = world.dir.mkdir("projects/api/src");
        assert_eq!(
            world
                .beskar
                .find_repo(&registry, Some(&inside), &project)
                .unwrap()
                .path,
            project
        );
        // ... but removing needs the repository itself.
        let e = world
            .beskar
            .remove_repo(Some(&inside), &project, None)
            .unwrap_err();
        assert!(
            e.message().contains("is inside the registered repository"),
            "{}",
            e.message()
        );
        assert!(e.hint().unwrap().contains("beskar repo remove"));
        let e = world
            .beskar
            .remove_repo(Some(Path::new("typo-dir")), &project, None)
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert_eq!(
            world.beskar.store().read().unwrap().len(),
            1,
            "nothing was removed"
        );
    }

    #[test]
    fn a_vanished_repository_can_still_be_named_exactly_to_remove_it() {
        let world = World::new();
        let project = registered(&world, "gone");
        std::fs::remove_dir_all(&project).unwrap();
        world
            .beskar
            .remove_repo(Some(&project), world.dir.path(), None)
            .unwrap();
        assert!(world.beskar.store().read().unwrap().is_empty());
    }

    #[test]
    fn a_repository_whose_skills_folder_would_be_the_librarys_is_refused() {
        let world = World::new();
        let library = world.beskar.library().root().to_path_buf();
        // The Beskar home is not inside the library, but skills would land in the library's skills folder.
        std::fs::write(
            world.dir.path().join("user/.beskar/config.bsk"),
            "agent-skills library/skills\n",
        )
        .unwrap();
        let beskar =
            crate::Beskar::open(world.beskar.home().clone(), world.beskar.env().clone()).unwrap();
        let e = beskar
            .add_repo(library.parent().unwrap(), world.dir.path())
            .unwrap_err();
        assert!(
            e.message().contains("overlaps the library"),
            "{}",
            e.message()
        );
        assert!(beskar.store().read().unwrap().is_empty());
    }

    #[test]
    fn duplicates_in_one_request_count_once() {
        let world = World::new();
        let project = registered(&world, "api");
        let report = world
            .beskar
            .change_profiles(
                &project,
                ProfileChange::Toggle,
                &[name("coding"), name("coding")],
            )
            .unwrap();
        assert_eq!(report.enabled, [name("coding")]);
        assert!(report.disabled.is_empty());
    }

    #[test]
    fn changing_an_unregistered_repository_fails() {
        let world = World::new();
        let e = world
            .beskar
            .change_profiles(
                &world.project("x"),
                ProfileChange::Enable,
                &[name("coding")],
            )
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn the_workflow_from_the_brief_end_to_end() {
        let world = World::new();
        let project = registered(&world, "beskar");
        world
            .beskar
            .change_profiles(&project, ProfileChange::Enable, &[name("coding")])
            .unwrap();

        let dry = world
            .beskar
            .update_repo(
                &project,
                &options(true),
                &mut world.beskar.policy_resolver(None),
            )
            .unwrap();
        assert!(dry.outcome.is_none());
        assert_eq!(
            dry.plan
                .entries
                .iter()
                .filter(|e| e.action == Action::Add)
                .count(),
            2
        );
        assert!(
            !world.dir.exists("projects/beskar/.agents"),
            "a dry run writes nothing"
        );
        assert!(
            world
                .beskar
                .store()
                .read()
                .unwrap()
                .get(&project)
                .unwrap()
                .installed
                .is_empty()
        );

        let run = world
            .beskar
            .update_repo(
                &project,
                &options(false),
                &mut world.beskar.policy_resolver(None),
            )
            .unwrap();
        let outcome = run.outcome.unwrap();
        assert!(outcome.applied.iter().all(|a| a.done == Done::Added));
        assert!(
            world
                .dir
                .exists("projects/beskar/.agents/skills/git/SKILL.md")
        );
        assert!(
            world
                .dir
                .exists("projects/beskar/.agents/skills/testing/SKILL.md")
        );
        assert!(!world.dir.exists("projects/beskar/.agents/skills/pdf"));

        let stored = world.beskar.store().read().unwrap();
        let repo = stored.get(&project).unwrap();
        assert_eq!(repo.installed.len(), 2);
        assert_eq!(repo.synced, Some(Timestamp::from_secs(1_785_320_100)));
        assert_eq!(repo.profiles.len(), 1);
    }

    #[test]
    fn a_library_change_reaches_every_repository_that_uses_the_skill() {
        let world = World::new();
        let api = registered(&world, "api");
        let docs = registered(&world, "docs");
        let site = registered(&world, "site");
        world
            .beskar
            .change_profiles(&api, ProfileChange::Enable, &[name("coding")])
            .unwrap();
        world
            .beskar
            .change_profiles(&docs, ProfileChange::Enable, &[name("research")])
            .unwrap();
        world
            .beskar
            .change_profiles(&site, ProfileChange::Enable, &[name("research")])
            .unwrap();
        world
            .beskar
            .update_all(&options(false), &mut world.beskar.policy_resolver(None))
            .unwrap();

        world.dir.write(
            "user/.beskar/library/skills/testing/SKILL.md",
            "---\nname: testing\n---\nv2\n",
        );
        let results = world
            .beskar
            .update_all(&options(false), &mut world.beskar.policy_resolver(None))
            .unwrap();
        assert_eq!(results.len(), 3);
        let changed: Vec<bool> = results
            .iter()
            .map(|r| {
                r.result
                    .as_ref()
                    .unwrap()
                    .outcome
                    .as_ref()
                    .unwrap()
                    .changed_files()
            })
            .collect();
        assert_eq!(
            changed,
            [true, false, false],
            "only the repository that has testing needs modification"
        );
        assert_eq!(
            world
                .dir
                .read("projects/api/.agents/skills/testing/SKILL.md"),
            "---\nname: testing\n---\nv2\n"
        );
    }

    #[test]
    fn update_all_reports_each_repository_on_its_own() {
        let world = World::new();
        let good = registered(&world, "good");
        let gone = registered(&world, "gone");
        let ghost = registered(&world, "ghost");
        world
            .beskar
            .change_profiles(&good, ProfileChange::Enable, &[name("coding")])
            .unwrap();
        world
            .beskar
            .change_profiles(&ghost, ProfileChange::Enable, &[name("coding")])
            .unwrap();
        std::fs::remove_dir_all(&gone).unwrap();
        world
            .dir
            .write("user/.beskar/library/profiles/ghostly.bsk", "skill pdf\n");
        world
            .beskar
            .change_profiles(&ghost, ProfileChange::Enable, &[name("ghostly")])
            .unwrap();
        std::fs::remove_file(
            world
                .dir
                .path()
                .join("user/.beskar/library/profiles/ghostly.bsk"),
        )
        .unwrap();

        let results = world
            .beskar
            .update_all(&options(false), &mut world.beskar.policy_resolver(None))
            .unwrap();
        let summary: Vec<(String, bool)> = results
            .iter()
            .map(|r| {
                (
                    r.path.file_name().unwrap().to_string_lossy().into_owned(),
                    r.result.is_ok(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("ghost".to_string(), false),
                ("gone".to_string(), false),
                ("good".to_string(), true)
            ]
        );
        let gone_error = results[1].result.as_ref().unwrap_err();
        assert!(gone_error.message().contains("no longer exists"));
        assert!(world.dir.exists("projects/good/.agents/skills/git"));
    }

    #[test]
    fn update_does_not_undo_a_profile_change_made_meanwhile() {
        let world = World::new();
        let project = registered(&world, "api");
        world
            .beskar
            .change_profiles(&project, ProfileChange::Enable, &[name("coding")])
            .unwrap();
        struct Sneaky<'a>(&'a World, PathBuf);
        impl Resolver for Sneaky<'_> {
            fn resolve(&mut self, _: &reconcile::Conflict<'_>) -> Result<reconcile::Decision> {
                self.0
                    .beskar
                    .change_profiles(
                        &self.1,
                        ProfileChange::Enable,
                        &[ProfileName::parse("research").unwrap()],
                    )
                    .unwrap();
                Ok(reconcile::Decision::Resolve(reconcile::Resolution::Keep))
            }
        }
        // Create a conflict so the resolver runs between reading the registry and committing to it.
        world
            .beskar
            .update_repo(
                &project,
                &options(false),
                &mut world.beskar.policy_resolver(None),
            )
            .unwrap();
        world
            .dir
            .write("projects/api/.agents/skills/git/SKILL.md", "edited");
        world
            .beskar
            .update_repo(
                &project,
                &options(false),
                &mut Sneaky(&world, project.clone()),
            )
            .unwrap();
        let stored = world.beskar.store().read().unwrap();
        assert_eq!(
            stored.get(&project).unwrap().profiles.len(),
            2,
            "the concurrent enable must survive"
        );
    }

    #[test]
    fn removing_a_repository_leaves_its_files_by_default() {
        let world = World::new();
        let project = registered(&world, "api");
        world
            .beskar
            .change_profiles(&project, ProfileChange::Enable, &[name("coding")])
            .unwrap();
        world
            .beskar
            .update_repo(
                &project,
                &options(false),
                &mut world.beskar.policy_resolver(None),
            )
            .unwrap();
        let report = world
            .beskar
            .remove_repo(Some(&project), &world.cwd(), None)
            .unwrap();
        assert_eq!(report.repo.path, project);
        assert!(report.purge.is_none());
        assert!(world.beskar.store().read().unwrap().is_empty());
        assert!(world.dir.exists("projects/api/.agents/skills/git/SKILL.md"));
    }

    #[test]
    fn removing_with_purge_deletes_untouched_skills_and_respects_conflicts() {
        let world = World::new();
        let project = registered(&world, "api");
        world
            .beskar
            .change_profiles(&project, ProfileChange::Enable, &[name("coding")])
            .unwrap();
        world
            .beskar
            .update_repo(
                &project,
                &options(false),
                &mut world.beskar.policy_resolver(None),
            )
            .unwrap();
        world
            .dir
            .write("projects/api/.agents/skills/git/SKILL.md", "edited");
        world.dir.write(
            "projects/api/.agents/skills/mine/SKILL.md",
            "not installed by beskar",
        );

        let mut fail = world.beskar.policy_resolver(Some(ConflictPolicy::Fail));
        let e = world
            .beskar
            .remove_repo(
                Some(&project),
                &world.cwd(),
                Some((&mut fail, Timestamp::from_secs(1))),
            )
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert_eq!(
            world.beskar.store().read().unwrap().len(),
            1,
            "a failed purge keeps the registration"
        );
        assert!(
            world.dir.exists("projects/api/.agents/skills/testing"),
            "and removes nothing"
        );

        let mut keep = world.beskar.policy_resolver(Some(ConflictPolicy::Keep));
        let report = world
            .beskar
            .remove_repo(
                Some(&project),
                &world.cwd(),
                Some((&mut keep, Timestamp::from_secs(1))),
            )
            .unwrap();
        assert!(world.beskar.store().read().unwrap().is_empty());
        assert!(report.purge.is_some());
        assert!(!world.dir.exists("projects/api/.agents/skills/testing"));
        assert_eq!(
            world.dir.read("projects/api/.agents/skills/git/SKILL.md"),
            "edited"
        );
        assert!(
            world
                .dir
                .exists("projects/api/.agents/skills/mine/SKILL.md")
        );
    }

    #[test]
    fn plan_repo_reports_a_vanished_folder_helpfully() {
        let world = World::new();
        let project = registered(&world, "api");
        std::fs::remove_dir_all(&project).unwrap();
        let repo = world
            .beskar
            .store()
            .read()
            .unwrap()
            .get(&project)
            .cloned()
            .unwrap();
        let e = world.beskar.plan_repo(&repo).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert!(e.hint().unwrap().contains("beskar registry prune"));
    }
}

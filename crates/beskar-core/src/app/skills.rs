//! Working with an installed skill: reviewing local changes and promoting them into the library.

use std::path::{Path, PathBuf};

use super::Beskar;
use crate::diff::{self, TreeDiff};
use crate::error::{Error, ErrorKind, Result};
use crate::fingerprint::{Fingerprint, Ignore};
use crate::ids::SkillId;
pub use crate::reconcile::PromotionRisk;
use crate::reconcile::{Facts, promotion_risk};

/// The difference between the library's version of a skill and the copy in a repository.
#[derive(Clone, Debug)]
pub struct SkillDiff {
    /// The repository.
    pub repo: PathBuf,
    /// The skill.
    pub skill: SkillId,
    /// The library's directory for the skill, if the library has it.
    pub library: Option<PathBuf>,
    /// The skill's directory in the repository.
    pub workspace: PathBuf,
    /// Which paths differ.
    pub changes: TreeDiff,
    /// The differences as a unified diff. The library is the left side, the workspace the right.
    pub text: String,
}

/// A local skill copy that could be promoted into the library.
#[derive(Clone, Debug)]
pub struct Promotion {
    /// The repository.
    pub repo: PathBuf,
    /// The skill.
    pub skill: SkillId,
    /// The skill's directory in the repository.
    pub workspace: PathBuf,
    /// What differs between the library and the copy.
    pub changes: TreeDiff,
    /// What promoting would overwrite.
    pub risk: PromotionRisk,
    /// Whether the library already has the skill.
    pub exists_in_library: bool,
    /// Whether Beskar installed this copy. Only a tracked copy stays tracked after promotion; a
    /// folder somebody made stays theirs, so that the next update does not remove it.
    pub tracked: bool,
    /// Other repositories where the skill is installed and would differ from the library afterwards.
    pub other_repos: Vec<PathBuf>,
    /// The library's version when this was prepared, which promotion must still find.
    library_fingerprint: Option<Fingerprint>,
    /// The workspace copy when this was prepared, which promotion must still find.
    workspace_fingerprint: Fingerprint,
}

impl Promotion {
    /// True if the copy already equals the library's version.
    pub fn is_nothing_to_do(&self) -> bool {
        self.changes.is_empty()
    }
}

impl Beskar {
    fn workspace_copy(&self, repo: &Path, skill: &SkillId) -> Result<PathBuf> {
        let skills_dir = repo.join(&self.config().agent_skills);
        let workspace = skills_dir.join(skill.as_str());
        if workspace.is_dir() {
            Ok(workspace)
        } else {
            Err(
                Error::not_found(format!("there is no '{skill}' in {}", skills_dir.display()))
                    .with_hint("see what is installed with 'beskar repo status'"),
            )
        }
    }

    /// Shows how a repository's copy of a skill differs from the library's version.
    pub fn skill_diff(
        &self,
        skill: &SkillId,
        target: Option<&Path>,
        cwd: &Path,
    ) -> Result<SkillDiff> {
        let library = self.library();
        let registry = self.store().read()?;
        let repo = self.find_repo(&registry, target, cwd)?;
        let workspace = self.workspace_copy(&repo.path, skill)?;
        let library_dir = library.has_skill(skill).then(|| library.skill_path(skill));
        let left = library.skill_path(skill);
        let rules = Ignore::workspace();
        Ok(SkillDiff {
            repo: repo.path,
            skill: skill.clone(),
            changes: diff::compare(&left, &workspace, library.ignore(), &rules)?,
            text: diff::render(
                &left,
                &workspace,
                "library",
                "workspace",
                library.ignore(),
                &rules,
            )?,
            library: library_dir,
            workspace,
        })
    }

    /// Looks at a repository's copy of a skill and says what promoting it would do.
    pub fn prepare_promotion(
        &self,
        skill: &SkillId,
        target: Option<&Path>,
        cwd: &Path,
    ) -> Result<Promotion> {
        let library = self.library();
        let registry = self.store().read()?;
        let repo = self.find_repo(&registry, target, cwd)?;
        let workspace = self.workspace_copy(&repo.path, skill)?;
        let rules = Ignore::workspace();
        let workspace_fingerprint = Fingerprint::of_dir(&workspace, &rules)?;
        let facts = Facts {
            library: library.fingerprint_if_present(skill)?,
            recorded: repo.installed.get(skill).copied(),
            workspace: Some(workspace_fingerprint),
        };
        let changes = diff::compare(
            &library.skill_path(skill),
            &workspace,
            library.ignore(),
            &rules,
        )?;
        let other_repos = registry
            .repos()
            .filter(|other| other.path != repo.path)
            .filter(|other| other.installed.contains_key(skill))
            .map(|other| other.path.clone())
            .collect();
        Ok(Promotion {
            repo: repo.path,
            skill: skill.clone(),
            workspace,
            changes,
            risk: promotion_risk(&facts),
            exists_in_library: facts.library.is_some(),
            tracked: facts.recorded.is_some(),
            other_repos,
            library_fingerprint: facts.library,
            workspace_fingerprint,
        })
    }

    /// Copies a repository's version of a skill into the library, replacing the library's version.
    ///
    /// When the library changed since the copy was installed, or the copy is unrelated to the library,
    /// this refuses unless `allow_overwrite` is set. It also refuses if the library or the copy changed
    /// since [`Beskar::prepare_promotion`] looked at them, because what was reviewed is no longer what
    /// would be promoted. Afterwards a tracked copy counts as clean. Other repositories are not touched;
    /// `update` brings them up to date.
    pub fn promote(&self, promotion: &Promotion, allow_overwrite: bool) -> Result<Fingerprint> {
        if promotion.risk != PromotionRisk::None && !allow_overwrite {
            let why = match promotion.risk {
                PromotionRisk::OverwritesLibraryChanges => {
                    "the library changed since this copy was installed, and promoting would discard those changes"
                }
                _ => {
                    "beskar did not install this copy, so it cannot tell how it relates to the library's version"
                }
            };
            return Err(Error::new(
                ErrorKind::Conflict,
                format!("'{}' was not promoted: {why}", promotion.skill),
            )
            .with_hint(format!(
                "confirm with --force after reviewing the difference: beskar skill diff {} --repo {}",
                promotion.skill,
                crate::text::shell_quote(&promotion.repo.to_string_lossy())
            )));
        }
        let library = self.library();
        library.init()?;
        let now = Fingerprint::of_dir(&promotion.workspace, &Ignore::workspace())?;
        if now != promotion.workspace_fingerprint {
            return Err(Error::new(
                ErrorKind::Conflict,
                format!(
                    "'{}' changed after it was reviewed, so it was not promoted",
                    promotion.skill
                ),
            )
            .with_hint("run the command again"));
        }
        let fingerprint = library.replace_skill(
            &promotion.skill,
            &promotion.workspace,
            promotion.library_fingerprint,
        )?;
        if promotion.tracked {
            let skill = promotion.skill.clone();
            self.store().update(|registry| {
                if let Some(repo) = registry.get_mut(&promotion.repo) {
                    repo.installed.insert(skill, fingerprint);
                }
                Ok(())
            })?;
        }
        Ok(fingerprint)
    }
}

#[cfg(test)]
mod tests {
    use super::super::repos::{ProfileChange, UpdateOptions};
    use super::super::testing::World;
    use super::*;
    use crate::config::ConflictPolicy;
    use crate::ids::ProfileName;
    use crate::reconcile::SkillState;
    use crate::time::Timestamp;

    fn id(text: &str) -> SkillId {
        SkillId::parse(text).unwrap()
    }

    fn setup(world: &World, project: &str) -> PathBuf {
        let path = world.project(project);
        world.beskar.add_repo(&path, &world.cwd()).unwrap();
        world
            .beskar
            .change_profiles(
                &path,
                ProfileChange::Enable,
                &[ProfileName::parse("coding").unwrap()],
            )
            .unwrap();
        let options = UpdateOptions {
            dry_run: false,
            now: Timestamp::from_secs(1),
        };
        world
            .beskar
            .update_repo(&path, &options, &mut world.beskar.policy_resolver(None))
            .unwrap();
        path
    }

    fn state(world: &World, project: &Path, skill: &str) -> SkillState {
        let repo = world
            .beskar
            .store()
            .read()
            .unwrap()
            .get(project)
            .cloned()
            .unwrap();
        world
            .beskar
            .plan_repo(&repo)
            .unwrap()
            .entry(&id(skill))
            .unwrap()
            .state()
    }

    #[test]
    fn diff_shows_local_edits_against_the_library() {
        let world = World::new();
        let project = setup(&world, "api");
        world.dir.write(
            "projects/api/.agents/skills/git/SKILL.md",
            "---\nname: git\ndescription: The git skill\n---\nv1\nlocal line\n",
        );
        world
            .dir
            .write("projects/api/.agents/skills/git/notes.md", "mine\n");
        let diff = world.beskar.skill_diff(&id("git"), None, &project).unwrap();
        assert!(
            diff.text
                .contains("--- library/SKILL.md\n+++ workspace/SKILL.md\n"),
            "{}",
            diff.text
        );
        assert!(diff.text.contains("+local line\n"));
        assert!(diff.text.contains("+++ workspace/notes.md\n"));
        assert_eq!(
            diff.changes.summary(),
            "2 paths changed (1 added, 1 modified)"
        );
        assert!(diff.library.is_some());
    }

    #[test]
    fn diff_of_a_clean_copy_is_empty() {
        let world = World::new();
        let project = setup(&world, "api");
        let diff = world.beskar.skill_diff(&id("git"), None, &project).unwrap();
        assert!(diff.changes.is_empty() && diff.text.is_empty());
    }

    #[test]
    fn diff_of_a_skill_the_library_lacks_shows_everything_as_added() {
        let world = World::new();
        let project = setup(&world, "api");
        world
            .dir
            .write("projects/api/.agents/skills/mine/SKILL.md", "new\n");
        let diff = world
            .beskar
            .skill_diff(&id("mine"), None, &project)
            .unwrap();
        assert!(diff.library.is_none());
        assert_eq!(
            diff.text,
            "--- /dev/null\n+++ workspace/SKILL.md\n@@ -0,0 +1 @@\n+new\n"
        );
    }

    #[test]
    fn diff_of_a_skill_that_is_not_installed_says_so() {
        let world = World::new();
        let project = setup(&world, "api");
        let error = world
            .beskar
            .skill_diff(&id("pdf"), None, &project)
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert!(error.message().contains("there is no 'pdf' in"));
    }

    #[test]
    fn promoting_a_local_edit_updates_the_library_and_cleans_the_copy() {
        let world = World::new();
        let api = setup(&world, "api");
        let site = setup(&world, "site");
        world
            .dir
            .write("projects/api/.agents/skills/git/SKILL.md", "improved\n");
        assert_eq!(state(&world, &api, "git"), SkillState::LocalDrift);

        let promotion = world
            .beskar
            .prepare_promotion(&id("git"), None, &api)
            .unwrap();
        assert_eq!(promotion.risk, PromotionRisk::None);
        assert!(!promotion.is_nothing_to_do());
        assert_eq!(promotion.other_repos, std::slice::from_ref(&site));
        world.beskar.promote(&promotion, false).unwrap();

        assert_eq!(
            world.dir.read("user/.beskar/library/skills/git/SKILL.md"),
            "improved\n"
        );
        assert_eq!(state(&world, &api, "git"), SkillState::Clean);
        assert_eq!(
            state(&world, &site, "git"),
            SkillState::LibraryChanged,
            "the other repository can now update"
        );

        let options = UpdateOptions {
            dry_run: false,
            now: Timestamp::from_secs(2),
        };
        world
            .beskar
            .update_all(
                &options,
                &mut world.beskar.policy_resolver(Some(ConflictPolicy::Fail)),
            )
            .unwrap();
        assert_eq!(
            world.dir.read("projects/site/.agents/skills/git/SKILL.md"),
            "improved\n"
        );
    }

    #[test]
    fn promoting_an_unmodified_copy_is_nothing_to_do() {
        let world = World::new();
        let api = setup(&world, "api");
        let promotion = world
            .beskar
            .prepare_promotion(&id("git"), None, &api)
            .unwrap();
        assert!(promotion.is_nothing_to_do());
    }

    #[test]
    fn promoting_over_library_changes_needs_explicit_permission() {
        let world = World::new();
        let api = setup(&world, "api");
        world
            .dir
            .write("projects/api/.agents/skills/git/SKILL.md", "local edit\n");
        world
            .dir
            .write("user/.beskar/library/skills/git/SKILL.md", "library edit\n");
        let promotion = world
            .beskar
            .prepare_promotion(&id("git"), None, &api)
            .unwrap();
        assert_eq!(promotion.risk, PromotionRisk::OverwritesLibraryChanges);
        let error = world.beskar.promote(&promotion, false).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Conflict);
        assert!(error.message().contains("would discard those changes"));
        assert_eq!(
            world.dir.read("user/.beskar/library/skills/git/SKILL.md"),
            "library edit\n"
        );

        world.beskar.promote(&promotion, true).unwrap();
        assert_eq!(
            world.dir.read("user/.beskar/library/skills/git/SKILL.md"),
            "local edit\n"
        );
    }

    #[test]
    fn promoting_an_untracked_copy_needs_permission_when_the_library_has_the_skill() {
        let world = World::new();
        let api = setup(&world, "api");
        world.dir.write(
            "projects/api/.agents/skills/pdf/SKILL.md",
            "hand made pdf skill\n",
        );
        let promotion = world
            .beskar
            .prepare_promotion(&id("pdf"), None, &api)
            .unwrap();
        assert_eq!(promotion.risk, PromotionRisk::UnrelatedToLibrary);
        assert!(world.beskar.promote(&promotion, false).is_err());
    }

    #[test]
    fn promoting_a_brand_new_skill_adds_it_to_the_library_and_leaves_the_folder_alone() {
        let world = World::new();
        let api = setup(&world, "api");
        world.dir.write(
            "projects/api/.agents/skills/fresh/SKILL.md",
            "---\nname: fresh\n---\n",
        );
        let promotion = world
            .beskar
            .prepare_promotion(&id("fresh"), None, &api)
            .unwrap();
        assert!(!promotion.exists_in_library && !promotion.tracked);
        assert_eq!(promotion.risk, PromotionRisk::None);
        world.beskar.promote(&promotion, false).unwrap();
        assert!(world.beskar.library().has_skill(&id("fresh")));
        let repo = world
            .beskar
            .store()
            .read()
            .unwrap()
            .get(&api)
            .cloned()
            .unwrap();
        assert!(
            !repo.installed.contains_key(&id("fresh")),
            "a folder somebody made stays theirs"
        );

        // The next update must not delete it just because no profile lists it yet.
        let options = UpdateOptions {
            dry_run: false,
            now: Timestamp::from_secs(2),
        };
        world
            .beskar
            .update_repo(
                &api,
                &options,
                &mut world.beskar.policy_resolver(Some(ConflictPolicy::Fail)),
            )
            .unwrap();
        assert!(
            world
                .dir
                .exists("projects/api/.agents/skills/fresh/SKILL.md")
        );
    }

    #[test]
    fn promotion_refuses_when_the_library_changed_after_it_was_prepared() {
        let world = World::new();
        let api = setup(&world, "api");
        world
            .dir
            .write("projects/api/.agents/skills/git/SKILL.md", "local edit\n");
        let promotion = world
            .beskar
            .prepare_promotion(&id("git"), None, &api)
            .unwrap();
        assert_eq!(promotion.risk, PromotionRisk::None);
        // While the person was deciding, somebody improved the library's version.
        world.dir.write(
            "user/.beskar/library/skills/git/SKILL.md",
            "library edit made meanwhile\n",
        );
        let e = world.beskar.promote(&promotion, false).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert!(
            e.message().contains("changed while beskar was working"),
            "{}",
            e.message()
        );
        assert_eq!(
            world.dir.read("user/.beskar/library/skills/git/SKILL.md"),
            "library edit made meanwhile\n"
        );
    }

    #[test]
    fn promotion_refuses_when_the_copy_changed_after_it_was_reviewed() {
        let world = World::new();
        let api = setup(&world, "api");
        world.dir.write(
            "projects/api/.agents/skills/git/SKILL.md",
            "what was reviewed\n",
        );
        let promotion = world
            .beskar
            .prepare_promotion(&id("git"), None, &api)
            .unwrap();
        world.dir.write(
            "projects/api/.agents/skills/git/SKILL.md",
            "edited again after the review\n",
        );
        let e = world.beskar.promote(&promotion, false).unwrap_err();
        assert!(
            e.message().contains("changed after it was reviewed"),
            "{}",
            e.message()
        );
        assert_ne!(
            world.dir.read("user/.beskar/library/skills/git/SKILL.md"),
            "edited again after the review\n"
        );
    }

    #[test]
    fn a_git_folder_in_the_library_skill_survives_promotion() {
        let world = World::new();
        let api = setup(&world, "api");
        world.dir.write(
            "user/.beskar/library/skills/git/.git/HEAD",
            "ref: refs/heads/main",
        );
        world
            .dir
            .write("projects/api/.agents/skills/git/SKILL.md", "local edit\n");
        world.dir.write(
            "projects/api/.agents/skills/git/.git/HEAD",
            "ref: refs/heads/somewhere-else",
        );
        let promotion = world
            .beskar
            .prepare_promotion(&id("git"), None, &api)
            .unwrap();
        world.beskar.promote(&promotion, false).unwrap();
        assert_eq!(
            world.dir.read("user/.beskar/library/skills/git/.git/HEAD"),
            "ref: refs/heads/main",
            "the library's own .git is kept"
        );
        assert_eq!(
            world.dir.read("user/.beskar/library/skills/git/SKILL.md"),
            "local edit\n"
        );
    }

    #[test]
    fn diff_shows_a_git_folder_in_the_copy_as_a_single_line() {
        let world = World::new();
        let api = setup(&world, "api");
        world
            .dir
            .write("projects/api/.agents/skills/git/.git/HEAD", "ref");
        world
            .dir
            .write("projects/api/.agents/skills/git/.git/objects/aa/bb", "blob");
        let diff = world.beskar.skill_diff(&id("git"), None, &api).unwrap();
        assert_eq!(
            diff.text,
            "Only in workspace: .git/ (a version control folder, not compared)\n"
        );
    }
}

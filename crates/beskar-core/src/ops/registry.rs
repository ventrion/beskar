//! The machine-wide view: which workspaces Beskar manages, what state each
//! one is in, and where profiles and skills are used.

use std::path::PathBuf;

use crate::names::{ProfileName, SkillId};
use crate::reconcile::Action;
use crate::registry::RepoEntry;
use crate::sync::{self, RepoPlan};
use crate::usage::{self, SkillUse, Stats};
use crate::{Beskar, Error, Result};

/// Where a profile is enabled.
#[derive(Clone, Debug)]
pub struct ProfileUsage {
    pub profile: ProfileName,
    /// Workspaces that enable it, sorted.
    pub workspaces: Vec<PathBuf>,
}

/// Where a skill is installed or wanted, and through which profiles.
#[derive(Clone, Debug)]
pub struct SkillUsage {
    pub skill: SkillId,
    pub uses: Vec<SkillUse>,
}

/// One workspace's state at a glance.
#[derive(Clone, Debug)]
pub struct RepoHealth {
    pub entry: RepoEntry,
    pub state: Health,
}

#[derive(Clone, Debug)]
pub enum Health {
    /// The workspace directory is gone.
    Gone,
    /// Beskar cannot plan it, for example because an enabled profile does
    /// not load.
    Broken(Error),
    /// The plan, with counts by kind.
    Planned(RepoPlan, Counts),
}

/// How many skills of a workspace are in each state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub to_install: usize,
    pub to_update: usize,
    pub to_remove: usize,
    /// Unwanted copies that stay in place because they hold files that
    /// are not part of the skill.
    pub to_release: usize,
    pub conflicts: usize,
    pub missing: usize,
    pub changed_here: usize,
    pub blocked: usize,
}

impl Counts {
    pub fn of(plan: &RepoPlan) -> Counts {
        let n = |f: &dyn Fn(Action) -> bool| {
            plan.steps
                .iter()
                .filter(|s| f(s.action) && !plan.blocked.contains_key(&s.skill))
                .count()
        };
        Counts {
            to_install: n(&|a| matches!(a, Action::Install | Action::Restore)),
            to_update: n(&|a| a == Action::Update),
            to_remove: n(&|a| a == Action::Remove),
            to_release: n(&|a| a == Action::Release),
            conflicts: n(&|a| matches!(a, Action::Conflict(_))),
            missing: n(&|a| a == Action::MissingSource),
            changed_here: n(&|a| a == Action::KeepLocal),
            blocked: plan.blocked.len(),
        }
    }

    /// Whether a person has to act: a conflict, a missing skill or a
    /// blocked one.
    pub fn needs_attention(&self) -> bool {
        self.conflicts > 0 || self.missing > 0 || self.blocked > 0
    }

    /// Whether an update would change something by itself.
    pub fn pending(&self) -> bool {
        self.to_install + self.to_update + self.to_remove + self.to_release > 0
    }
}

impl Beskar {
    /// Every registered workspace, sorted by path.
    pub fn repos(&self) -> Result<Vec<RepoEntry>> {
        Ok(self.registry()?.repos().cloned().collect())
    }

    /// Where a profile is enabled.
    pub fn profile_usage(&self, name: &str) -> Result<ProfileUsage> {
        let profile = ProfileName::new(name)?;
        let workspaces = usage::profile_users(&self.registry()?, &profile);
        if workspaces.is_empty() && !self.library.has_profile(&profile) {
            self.library.find_profile(profile.as_str())?;
        }
        Ok(ProfileUsage {
            profile,
            workspaces,
        })
    }

    /// Where a skill is installed or wanted.
    pub fn skill_usage(&self, name: &str) -> Result<SkillUsage> {
        let skill = SkillId::new(name)?;
        let profiles = usage::loadable_profiles(&self.library);
        let uses = usage::skill_users(&self.registry()?, &profiles, &skill);
        if uses.is_empty() && !self.library.contains(&skill) {
            self.library.find_skill(name)?;
        }
        Ok(SkillUsage { skill, uses })
    }

    /// Every registered workspace's state.
    pub fn health(&self) -> Result<Vec<RepoHealth>> {
        Ok(self
            .registry()?
            .repos()
            .map(|entry| RepoHealth {
                state: if crate::fsx::is_gone(&entry.path) {
                    Health::Gone
                } else {
                    match sync::plan_repo(self, entry) {
                        Ok(plan) => {
                            let counts = Counts::of(&plan);
                            Health::Planned(plan, counts)
                        }
                        Err(error) => Health::Broken(error),
                    }
                },
                entry: entry.clone(),
            })
            .collect())
    }

    /// Counts across the library and every workspace.
    pub fn stats(&self) -> Result<Stats> {
        self.library.check()?;
        usage::stats(&self.library, &self.registry()?)
    }

    /// Forget workspaces whose directories are gone. Returns them. A
    /// directory that exists but cannot be read is not gone.
    pub fn prune(&self, dry_run: bool) -> Result<Vec<PathBuf>> {
        let gone = |registry: &crate::Registry| -> Vec<PathBuf> {
            registry
                .repos()
                .filter(|r| crate::fsx::is_gone(&r.path))
                .map(|r| r.path.clone())
                .collect()
        };
        if dry_run {
            return Ok(gone(&self.registry()?));
        }
        self.transact("registry prune", |registry| {
            let gone = gone(registry);
            for path in &gone {
                registry.remove(path);
            }
            Ok(gone)
        })
    }
}

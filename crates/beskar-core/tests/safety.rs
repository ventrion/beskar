//! A randomized check of the safety principle: Beskar never silently destroys a locally modified skill.
//!
//! Each scenario applies a long random sequence of things that happen in real life: people and
//! agents editing installed skills, deleting them, adding their own folders, the library changing,
//! profiles being edited and enabled, and updates running under every conflict policy. After each
//! step it checks the promises Beskar makes.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use beskar_core::app::{InitOptions, ProfileChange, UpdateOptions};
use beskar_core::reconcile::{Action, ConflictKind, Decision, Resolution, Resolver};
use beskar_core::testing::TempDir;
use beskar_core::{Beskar, ConflictPolicy, Env, Home, ProfileName, SkillId, Timestamp};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const SKILLS: [&str; 4] = ["alpha", "bravo", "charlie", "delta"];
const PROFILES: [&str; 3] = ["one", "two", "three"];
const REPOS: [&str; 2] = ["r1", "r2"];

fn skill(name: &str) -> SkillId {
    SkillId::parse(name).unwrap()
}

fn profile(name: &str) -> ProfileName {
    ProfileName::parse(name).unwrap()
}

/// Every file under `dir`, with content.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(base).unwrap().display().to_string(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// A resolver that gives a scripted answer for every conflict.
struct Scripted(Decision);

impl Resolver for Scripted {
    fn resolve(
        &mut self,
        _: &beskar_core::reconcile::Conflict<'_>,
    ) -> beskar_core::Result<Decision> {
        Ok(self.0)
    }
}

struct Scenario {
    dir: TempDir,
    beskar: Beskar,
    rng: Rng,
    repos: Vec<PathBuf>,
    /// What a person last wrote into `<repo>/.agents/skills/<skill>/SKILL.md` and has not yet been
    /// resolved. Beskar must never lose it unless told to.
    protected: BTreeMap<(usize, String), String>,
    /// Folders somebody made that Beskar did not install.
    unmanaged: BTreeMap<(usize, String), String>,
    counter: u32,
}

impl Scenario {
    fn new(seed: u64) -> Scenario {
        let dir = TempDir::new("safety");
        let env = Env {
            user_home: Some(dir.path().join("user")),
            beskar_home: None,
        };
        let (beskar, _) = Beskar::init(
            Home::at(dir.path().join("user/.beskar")),
            env,
            &InitOptions::default(),
        )
        .unwrap();
        let mut scenario = Scenario {
            dir,
            beskar,
            rng: Rng(seed),
            repos: Vec::new(),
            protected: BTreeMap::new(),
            unmanaged: BTreeMap::new(),
            counter: 0,
        };
        for name in SKILLS {
            scenario.write_library(name);
        }
        let library = scenario.beskar.library();
        library
            .create_profile(&profile("one"), None, &[skill("alpha"), skill("bravo")])
            .unwrap();
        library
            .create_profile(&profile("two"), None, &[skill("bravo"), skill("charlie")])
            .unwrap();
        library
            .create_profile(&profile("three"), None, &[skill("delta")])
            .unwrap();
        for name in REPOS {
            let path = scenario.dir.mkdir(&format!("projects/{name}"));
            scenario
                .beskar
                .add_repo(&path, scenario.dir.path())
                .unwrap();
            scenario.repos.push(path);
        }
        scenario
    }

    fn fresh_text(&mut self, what: &str) -> String {
        self.counter += 1;
        format!("{what} #{}\n", self.counter)
    }

    fn write_library(&mut self, name: &str) {
        let text = self.fresh_text(&format!("library version of {name}"));
        let dir = self.beskar.library().skill_path(&skill(name));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), text).unwrap();
    }

    fn skill_file(&self, repo: usize, name: &str) -> PathBuf {
        self.repos[repo]
            .join(".agents/skills")
            .join(name)
            .join("SKILL.md")
    }

    fn update(
        &mut self,
        repo: usize,
        resolver: &mut dyn Resolver,
    ) -> beskar_core::Result<beskar_core::app::RepoUpdate> {
        let options = UpdateOptions {
            dry_run: false,
            now: Timestamp::from_secs(1_785_320_100),
        };
        self.beskar
            .update_repo(&self.repos[repo], &options, resolver)
    }

    /// Performs one random real-life event.
    fn step(&mut self) -> String {
        let repo = self.rng.below(REPOS.len());
        let name = *self.rng.pick(&SKILLS);
        match self.rng.below(14) {
            0 | 1 => {
                // Somebody edits an installed skill.
                if self.skill_file(repo, name).exists()
                    && !self.unmanaged.contains_key(&(repo, name.to_string()))
                {
                    let text = self.fresh_text(&format!("user edit in {name}"));
                    fs::write(self.skill_file(repo, name), &text).unwrap();
                    self.protected.insert((repo, name.to_string()), text);
                    return format!("edit {name} in r{}", repo + 1);
                }
                "edit (nothing to edit)".into()
            }
            2 => {
                // Somebody deletes an installed skill.
                let dir = self.repos[repo].join(".agents/skills").join(name);
                if dir.exists() && !self.unmanaged.contains_key(&(repo, name.to_string())) {
                    fs::remove_dir_all(dir).unwrap();
                    self.protected.remove(&(repo, name.to_string()));
                    return format!("delete {name} in r{}", repo + 1);
                }
                "delete (nothing to delete)".into()
            }
            3 => {
                // Somebody makes a folder of their own.
                let dir = self.repos[repo].join(".agents/skills").join(name);
                if !dir.exists() {
                    let text = self.fresh_text("hand made");
                    fs::create_dir_all(&dir).unwrap();
                    fs::write(dir.join("SKILL.md"), &text).unwrap();
                    self.unmanaged.insert((repo, name.to_string()), text);
                    return format!("hand-made {name} in r{}", repo + 1);
                }
                "hand-made (exists)".into()
            }
            4 | 5 => {
                self.write_library(name);
                format!("library edit {name}")
            }
            6 => {
                let profile_name = profile(self.rng.pick(&PROFILES));
                let library = self.beskar.library();
                if self.rng.below(2) == 0 {
                    let _ = library.profile_add(&profile_name, &[skill(name)]);
                    format!("profile {profile_name} += {name}")
                } else {
                    let _ = library.profile_remove(&profile_name, &[skill(name)]);
                    format!("profile {profile_name} -= {name}")
                }
            }
            7 | 8 => {
                let profile_name = profile(self.rng.pick(&PROFILES));
                let change = *self.rng.pick(&[
                    ProfileChange::Enable,
                    ProfileChange::Disable,
                    ProfileChange::Toggle,
                ]);
                let _ = self.beskar.change_profiles(
                    &self.repos[repo],
                    change,
                    std::slice::from_ref(&profile_name),
                );
                format!("{change:?} {profile_name} in r{}", repo + 1)
            }
            9 => {
                // The library loses a skill, and profiles stop listing it.
                let _ = self.beskar.library().remove_skill(&skill(name), true);
                format!("library remove {name}")
            }
            10 => {
                if !self.beskar.library().has_skill(&skill(name)) {
                    self.write_library(name);
                    return format!("library re-add {name}");
                }
                "library re-add (exists)".into()
            }
            11 => self.update_with_fail(repo),
            12 => self.update_with_keep(repo),
            _ => self.update_with_abort(repo),
        }
    }

    fn workspace(&self, repo: usize) -> BTreeMap<String, Vec<u8>> {
        snapshot(&self.repos[repo].join(".agents"))
    }

    fn update_with_fail(&mut self, repo: usize) -> String {
        let before = self.workspace(repo);
        let registry_before = self
            .beskar
            .store()
            .read()
            .unwrap()
            .get(&self.repos[repo])
            .cloned();
        let result = self.update(repo, &mut Scripted(Decision::Unresolved));
        if result.is_err() {
            assert_eq!(
                self.workspace(repo),
                before,
                "a failed update must leave the repository exactly as it was"
            );
            assert_eq!(
                self.beskar
                    .store()
                    .read()
                    .unwrap()
                    .get(&self.repos[repo])
                    .cloned(),
                registry_before,
                "and its registry record"
            );
        }
        format!(
            "update r{} (fail): {}",
            repo + 1,
            if result.is_ok() { "ok" } else { "refused" }
        )
    }

    fn update_with_abort(&mut self, repo: usize) -> String {
        let before = self.workspace(repo);
        let result = self.update(repo, &mut Scripted(Decision::Abort));
        if result.is_err() {
            assert_eq!(
                self.workspace(repo),
                before,
                "an aborted update must leave the repository exactly as it was"
            );
        }
        format!("update r{} (abort)", repo + 1)
    }

    fn update_with_keep(&mut self, repo: usize) -> String {
        let result = self.update(
            repo,
            &mut beskar_core::reconcile::PolicyResolver(ConflictPolicy::Keep),
        );
        if let Ok(update) = result {
            // After a keep-run, only conflicts may remain to be done.
            let plan = self
                .beskar
                .plan_repo(
                    &self
                        .beskar
                        .store()
                        .read()
                        .unwrap()
                        .get(&self.repos[repo])
                        .cloned()
                        .unwrap(),
                )
                .unwrap();
            for entry in &plan.entries {
                let acceptable = matches!(
                    entry.action,
                    Action::Unchanged | Action::Adopt | Action::Forget | Action::Conflict(_)
                );
                assert!(
                    acceptable || !update.plan.problems().is_empty() || !plan.problems().is_empty(),
                    "left over work after a keep-update: {entry:?}"
                );
            }
        }
        format!("update r{} (keep)", repo + 1)
    }

    /// The promises. Called after every step.
    fn check(&self, history: &[String]) {
        let context = || {
            format!(
                "after: {}",
                history
                    .iter()
                    .rev()
                    .take(6)
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" -> ")
            )
        };
        for ((repo, name), text) in &self.protected {
            let file = self.skill_file(*repo, name);
            let found = fs::read_to_string(&file).unwrap_or_else(|_| {
                panic!(
                    "the edited skill {name} in r{} vanished\n{}",
                    repo + 1,
                    context()
                )
            });
            assert_eq!(
                &found,
                text,
                "the edited skill {name} in r{} was overwritten\n{}",
                repo + 1,
                context()
            );
        }
        for ((repo, name), text) in &self.unmanaged {
            let found = fs::read_to_string(self.skill_file(*repo, name))
                .unwrap_or_else(|_| panic!("the hand-made skill {name} vanished\n{}", context()));
            assert_eq!(
                &found,
                text,
                "the hand-made skill {name} was overwritten\n{}",
                context()
            );
        }
    }

    /// Something a person says "yes, replace it" to: protection ends for that repository.
    fn replace_everything(&mut self, repo: usize) {
        let result = self.update(
            repo,
            &mut beskar_core::reconcile::PolicyResolver(ConflictPolicy::Replace),
        );
        if result.is_ok() {
            self.protected.retain(|(r, _), _| *r != repo);
            self.unmanaged.retain(|(r, name), text| {
                // Untracked folders that differ from the library are replaced under Replace.
                *r != repo
                    || fs::read_to_string(
                        self.repos[*r]
                            .join(".agents/skills")
                            .join(name)
                            .join("SKILL.md"),
                    )
                    .is_ok_and(|now| &now == text)
            });
        }
    }
}

#[test]
fn edits_are_never_lost_under_any_sequence_of_events() {
    for seed in 1..=60u64 {
        let mut scenario = Scenario::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut history = Vec::new();
        for _ in 0..50 {
            let what = scenario.step();
            history.push(what);
            scenario.check(&history);
        }
    }
}

#[test]
fn replace_is_the_only_way_to_lose_an_edit_and_it_leaves_the_repository_converged() {
    for seed in 1..=40u64 {
        let mut scenario = Scenario::new(seed.wrapping_mul(0xD1B5_4A32_D192_ED03));
        let mut history = Vec::new();
        for _ in 0..30 {
            history.push(scenario.step());
            scenario.check(&history);
        }
        for repo in 0..REPOS.len() {
            // Make every profile resolvable so the update can run.
            for name in SKILLS {
                if !scenario.beskar.library().has_skill(&skill(name)) {
                    scenario.write_library(name);
                }
            }
            scenario.replace_everything(repo);
            let stored = scenario
                .beskar
                .store()
                .read()
                .unwrap()
                .get(&scenario.repos[repo])
                .cloned()
                .unwrap();
            if let Ok(plan) = scenario.beskar.plan_repo(&stored)
                && plan.problems().is_empty()
            {
                let unresolved: Vec<_> = plan
                    .entries
                    .iter()
                    .filter(|e| {
                        matches!(
                            e.action,
                            Action::Add
                                | Action::Update
                                | Action::Remove
                                | Action::Conflict(
                                    ConflictKind::LocalDrift
                                        | ConflictKind::Diverged
                                        | ConflictKind::ModifiedRemoval
                                )
                        )
                    })
                    .collect();
                assert!(
                    unresolved.is_empty(),
                    "seed {seed} r{}: work left after a replace-update: {unresolved:?}\n{history:?}",
                    repo + 1
                );
            }
        }
    }
}

#[test]
fn promote_never_loses_an_edit_either() {
    let mut scenario = Scenario::new(0xABCDEF);
    scenario
        .update(0, &mut Scripted(Decision::Resolve(Resolution::Keep)))
        .ok();
    scenario
        .beskar
        .change_profiles(
            &scenario.repos[0].clone(),
            ProfileChange::Enable,
            &[profile("one")],
        )
        .unwrap();
    scenario
        .update(0, &mut Scripted(Decision::Unresolved))
        .unwrap();
    let edited = "promoted text\n";
    fs::write(scenario.skill_file(0, "alpha"), edited).unwrap();
    let result = scenario
        .update(0, &mut Scripted(Decision::Resolve(Resolution::Promote)))
        .unwrap();
    assert!(result.outcome.unwrap().failures().is_empty());
    assert_eq!(
        fs::read_to_string(
            scenario
                .beskar
                .library()
                .skill_path(&skill("alpha"))
                .join("SKILL.md")
        )
        .unwrap(),
        edited
    );
    assert_eq!(
        fs::read_to_string(scenario.skill_file(0, "alpha")).unwrap(),
        edited
    );
}

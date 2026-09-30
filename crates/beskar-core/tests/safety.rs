//! Randomized safety tests: Beskar never loses a local change silently.
//!
//! Each case builds a scratch Beskar home with five library skills, two
//! overlapping profiles and two registered workspaces, then takes a
//! deterministic random walk (xorshift64*, one seed per case): editing the
//! library, editing workspace copies by hand, switching profiles, updating
//! with every conflict policy, dry runs, promotes, restores and purges.
//!
//! A shadow model records what every skill directory should hold, which
//! copies Beskar manages (with the content it put there, the recorded
//! base) and which library versions the person declined by keeping their
//! copy. After every step the files, the library, the registry and the
//! plans are compared with it:
//!
//! 1. A copy changed by hand keeps exactly that content until the person
//!    replaces it (policy `replace`), restores it, deletes it or reverts it.
//!    An unwanted copy with local changes is never deleted without
//!    `replace`.
//! 2. A dry run, and an update stopped by an undecided conflict, change no
//!    file in the workspace and leave the registry byte for byte the same.
//! 3. After a clean update with `replace`, and after any update that finds
//!    nothing to do, every wanted skill matches the library (unless it is a
//!    local change the library never moved past), and Beskar no longer
//!    manages any unwanted copy that is still there.
//! 4. Directories under names no profile wants are never touched.
//! 5. No `.beskar` work directory or `.beskar-*` temporary entry remains.
//! 6. The registry always loads and agrees with the model; a recorded skill
//!    that is gone is planned to be forgotten or restored.
//! 7. Planning the same state twice gives the same plan.
//!
//! On top of that, every plan is checked against the decision table in
//! `reconcile`, recomputed from the model's file contents rather than from
//! fingerprints, and an update that went through leaves nothing to do.
//!
//! A failure names its seed and lists the steps that led to it. Set
//! `BESKAR_SAFETY_SEED=<n>` to run that one case.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use beskar_core::init::init;
use beskar_core::library::Imported;
use beskar_core::names::{ProfileName, SkillId};
use beskar_core::ops::repos::{
    ProfileChange, Promoted, RepoRemoved, RepoStatus, RepoUpdate, Restored, UpdateResult,
};
use beskar_core::ops::{RepoRef, Resolver};
use beskar_core::reconcile::{Action, Conflict};
use beskar_core::registry::Registry;
use beskar_core::sync::{Done, Outcome, RepoPlan};
use beskar_core::{Beskar, ConflictPolicy, ErrorKind, Fingerprint, Result};

/// Random walks, and random steps in each.
const CASES: u64 = 100;
const STEPS: usize = 30;
/// Threads the walks run on.
const THREADS: u64 = 10;

const SKILLS: [&str; 5] = ["alpha", "beta", "gamma", "delta", "epsilon"];
const PROFILES: [(&str, &[&str]); 2] = [
    ("coding", &["alpha", "beta", "gamma"]),
    ("research", &["gamma", "delta"]),
];
const WORKSPACES: [&str; 2] = ["ws-a", "ws-b"];
/// Directories a person keeps in a skills directory under names that no
/// profile and no library skill ever has: one named like a skill, one not.
const FOREIGN: [&str; 2] = ["zeta-own", "Scratch Notes"];

/// A directory's files, relative path to contents. Directories themselves
/// do not count, as in a fingerprint.
type Tree = BTreeMap<String, Vec<u8>>;
/// Everything under a directory: `None` for a directory, contents for a
/// file.
type Listing = BTreeMap<String, Option<Vec<u8>>>;

// ----- Random walks -----

/// Every case, spread over threads: a walk spends much of its time waiting
/// for the disk, so more threads than cores still help.
#[test]
fn random_walks() {
    let seeds: Vec<u64> = match std::env::var("BESKAR_SAFETY_SEED") {
        Ok(seed) => vec![seed.parse().expect("BESKAR_SAFETY_SEED is a number")],
        Err(_) => (0..CASES).collect(),
    };
    std::thread::scope(|scope| {
        let threads: Vec<_> = (0..THREADS)
            .map(|thread| {
                let seeds = &seeds;
                scope.spawn(move || {
                    for &seed in seeds.iter().filter(|seed| *seed % THREADS == thread) {
                        walk(seed);
                    }
                })
            })
            .collect();
        for thread in threads {
            if let Err(panic) = thread.join() {
                std::panic::resume_unwind(panic);
            }
        }
    });
}

#[test]
fn a_seed_replays_the_same_walk() {
    assert_eq!(walk(7), walk(7));
}

/// Take one random walk and return its log.
fn walk(seed: u64) -> Vec<String> {
    let mut world = World::new(seed, &format!("walk-{seed}"));
    let mut rng = Rng::new(seed);
    world.switch(0, ProfileChange::Enable, "coding");
    world.switch(1, ProfileChange::Enable, "research");
    world.update(0, ConflictPolicy::Abort);
    world.update(1, ConflictPolicy::Abort);
    for _ in 0..STEPS {
        world.random_step(&mut rng);
    }
    std::mem::take(&mut world.log)
}

// ----- Targeted sequences -----

#[test]
fn keep_then_library_change_then_keep_again() {
    let mut world = World::new(1001, "keep-twice");
    world.switch(0, ProfileChange::Enable, "coding");
    world.update(0, ConflictPolicy::Abort);
    world.edit_copy(0, "alpha", "SKILL.md", "mine");
    world.edit_library("alpha", "SKILL.md", "library v2");
    assert_eq!(
        world.action(0, "alpha"),
        Some(Action::Conflict(Conflict::Diverged))
    );
    let update = world.update(0, ConflictPolicy::Keep);
    assert_eq!(done(&update, "alpha"), Some(Done::KeptLocal));
    assert_eq!(world.action(0, "alpha"), Some(Action::KeepLocal));
    let again = world.update(0, ConflictPolicy::Abort);
    assert!(matches!(again.result, UpdateResult::UpToDate), "{again:?}");

    world.edit_library("alpha", "SKILL.md", "library v3");
    assert_eq!(
        world.action(0, "alpha"),
        Some(Action::Conflict(Conflict::Diverged)),
        "a newer library version asks again"
    );
    let stopped = world.update(0, ConflictPolicy::Abort);
    assert!(matches!(stopped.result, UpdateResult::Stopped));
    let update = world.update(0, ConflictPolicy::Keep);
    assert_eq!(done(&update, "alpha"), Some(Done::KeptLocal));
    assert_eq!(world.copy_file(0, "alpha", "SKILL.md"), "mine");
    assert_eq!(world.action(0, "alpha"), Some(Action::KeepLocal));
}

#[test]
fn keep_then_revert_then_update_takes_the_new_library_version() {
    let mut world = World::new(1002, "keep-revert");
    world.switch(0, ProfileChange::Enable, "coding");
    world.update(0, ConflictPolicy::Abort);
    let installed = world.spaces[0].files["alpha"].clone();
    world.edit_copy(0, "alpha", "SKILL.md", "mine");
    world.edit_library("alpha", "SKILL.md", "library v2");
    let update = world.update(0, ConflictPolicy::Keep);
    assert_eq!(done(&update, "alpha"), Some(Done::KeptLocal));

    // The person undoes their change by hand: the copy is its base again.
    world.set_copy(0, "alpha", installed);
    assert_eq!(world.action(0, "alpha"), Some(Action::Update));
    let update = world.update(0, ConflictPolicy::Abort);
    assert_eq!(done(&update, "alpha"), Some(Done::Updated));
    assert_eq!(world.copy_file(0, "alpha", "SKILL.md"), "library v2");
    assert_eq!(world.action(0, "alpha"), Some(Action::Unchanged));
}

#[test]
fn a_kept_untracked_directory_conflicts_again_when_the_library_changes() {
    let mut world = World::new(1003, "untracked");
    // A directory Beskar never installed, under a name a profile will want.
    world.set_copy(0, "alpha", tree(&[("SKILL.md", "my own alpha")]));
    assert_eq!(world.action(0, "alpha"), Some(Action::Unmanaged));
    world.switch(0, ProfileChange::Enable, "coding");
    assert_eq!(
        world.action(0, "alpha"),
        Some(Action::Conflict(Conflict::Untracked))
    );
    let update = world.update(0, ConflictPolicy::Keep);
    assert_eq!(done(&update, "alpha"), Some(Done::KeptLocal));
    assert_eq!(done(&update, "beta"), Some(Done::Installed));
    assert_eq!(world.action(0, "alpha"), Some(Action::KeepLocal));
    // Kept over this library version, it is not a conflict, so even
    // `replace` leaves it alone.
    let update = world.update(0, ConflictPolicy::Replace);
    assert!(
        matches!(update.result, UpdateResult::UpToDate),
        "{update:?}"
    );

    world.edit_library("alpha", "SKILL.md", "library v2");
    assert_eq!(
        world.action(0, "alpha"),
        Some(Action::Conflict(Conflict::Untracked)),
        "a new library version is a conflict again, never an update"
    );
    let stopped = world.update(0, ConflictPolicy::Abort);
    assert!(matches!(stopped.result, UpdateResult::Stopped));
    let update = world.update(0, ConflictPolicy::Keep);
    assert_eq!(done(&update, "alpha"), Some(Done::KeptLocal));
    assert_eq!(world.copy_file(0, "alpha", "SKILL.md"), "my own alpha");
}

#[test]
fn promoting_a_kept_copy_without_force_fails() {
    let mut world = World::new(1004, "promote-kept");
    world.switch(0, ProfileChange::Enable, "coding");
    world.update(0, ConflictPolicy::Abort);
    world.edit_copy(0, "alpha", "SKILL.md", "mine");
    world.edit_library("alpha", "SKILL.md", "library v2");
    world.update(0, ConflictPolicy::Keep);

    let error = world.promote(0, "alpha").unwrap_err();
    assert_eq!(error.kind, ErrorKind::Conflict, "{}", error.message);
    assert_eq!(world.library_file("alpha", "SKILL.md"), "library v2");
    assert_eq!(world.copy_file(0, "alpha", "SKILL.md"), "mine");

    // With --force the person overrides the library on purpose.
    let promoted = world
        .beskar
        .promote(&world.at(0), "alpha", true)
        .expect("forced promote");
    assert_eq!(promoted.promotion.imported, Imported::Replaced);
    let copy = world.spaces[0].files["alpha"].clone();
    world.library.insert("alpha", copy.clone());
    world.spaces[0].settle("alpha", copy);
    world.check();
    assert_eq!(world.library_file("alpha", "SKILL.md"), "mine");
    assert_eq!(world.action(0, "alpha"), Some(Action::Unchanged));
}

#[test]
fn replace_deletes_an_orphaned_copy_and_keep_releases_it() {
    let mut world = World::new(1005, "orphans");
    for w in 0..2 {
        world.switch(w, ProfileChange::Enable, "coding");
        world.update(w, ConflictPolicy::Abort);
    }
    world.edit_copy(0, "alpha", "SKILL.md", "mine in a");
    world.edit_copy(1, "alpha", "SKILL.md", "mine in b");
    world.remove_from_profile("coding", "alpha");
    for w in 0..2 {
        assert_eq!(
            world.action(w, "alpha"),
            Some(Action::Conflict(Conflict::Orphaned))
        );
    }
    let stopped = world.update(0, ConflictPolicy::Abort);
    assert!(matches!(stopped.result, UpdateResult::Stopped));
    let stopped = world.update(0, ConflictPolicy::Ask);
    assert!(matches!(stopped.result, UpdateResult::Stopped));

    let update = world.update(0, ConflictPolicy::Replace);
    assert_eq!(done(&update, "alpha"), Some(Done::Removed));
    assert!(!world.copy_dir(0, "alpha").exists());

    let update = world.update(1, ConflictPolicy::Keep);
    assert_eq!(done(&update, "alpha"), Some(Done::Released));
    assert_eq!(world.copy_file(1, "alpha", "SKILL.md"), "mine in b");
    let registry = world.beskar.registry().unwrap();
    let entry = registry.get(&world.spaces[1].root).unwrap();
    assert!(!entry.installed.contains_key(&id("alpha")));
    assert_eq!(world.action(1, "alpha"), Some(Action::Unmanaged));
    let update = world.update(1, ConflictPolicy::Replace);
    assert!(
        matches!(update.result, UpdateResult::UpToDate),
        "{update:?}"
    );
    assert_eq!(world.copy_file(1, "alpha", "SKILL.md"), "mine in b");
}

// ----- Bugs found -----

/// `sync::promote` records the promoted copy in the registry but never sets
/// the workspace's `skills_dir`, which updates and restores do. Once
/// `skills-dir` changes in the config, Beskar no longer refuses to plan (as
/// it does for a workspace whose first copy came from an update): the next
/// update installs a second copy under the new directory and leaves the
/// promoted one behind, no longer managed.
#[test]
fn a_promote_that_records_the_first_installation_records_the_skills_directory() {
    let scratch = Scratch::new("promote-skills-dir");
    let home = scratch.0.join(".beskar");
    init(&home, Some(&scratch.0), None).expect("init");
    let beskar = Beskar::load(&home, Some(&scratch.0)).expect("load");
    // A profile names a skill the library does not have yet; the person
    // writes it in a workspace and promotes it from there.
    write_file(
        &beskar.library.profiles_dir().join("coding.bsk"),
        b"skill: fresh\n",
    );
    let repo = scratch.0.join("repo");
    write_file(
        &repo.join(".agents/skills/fresh/SKILL.md"),
        b"---\nname: fresh\ndescription: written here\n---\n",
    );
    let at = RepoRef::named(repo.clone());
    beskar.add_repo(&repo).expect("register");
    beskar
        .change_profiles(&at, ProfileChange::Enable, &["coding".to_string()])
        .expect("enable");
    let promoted = beskar.promote(&at, "fresh", false).expect("promote");
    assert_eq!(promoted.promotion.imported, Imported::Added);
    let registry = beskar.registry().expect("registry");
    let entry = registry.get(&repo).expect("registered");
    assert!(entry.installed.contains_key(&id("fresh")));
    assert_eq!(
        entry.skills_dir.as_deref(),
        Some(Path::new(".agents/skills")),
        "the registry records `fresh` but not the skills directory it is in"
    );

    // What that protects: with another skills-dir, planning is refused
    // rather than installing a second copy elsewhere.
    let config = fs::read_to_string(&beskar.config.path).expect("read config");
    let config = config.replace("skills-dir: .agents/skills", "skills-dir: .claude/skills");
    fs::write(&beskar.config.path, config).expect("write config");
    let beskar = Beskar::load(&home, Some(&scratch.0)).expect("load");
    let error = beskar.repo_status(&repo).unwrap_err();
    assert!(
        error.message.contains("config now says"),
        "{}",
        error.message
    );
}

// ----- The model -----

/// What the model knows about one workspace.
#[derive(Default)]
struct Space {
    root: PathBuf,
    enabled: BTreeSet<&'static str>,
    /// What each library skill's directory here holds; absent if there is
    /// none.
    files: BTreeMap<&'static str, Tree>,
    /// Copies Beskar manages with a recorded base: the content it put
    /// there.
    base: BTreeMap<&'static str, Tree>,
    /// Library versions the person declined by keeping their copy.
    declined: BTreeMap<&'static str, Tree>,
    /// Directories under names no profile ever wants.
    foreign: BTreeMap<&'static str, Tree>,
}

impl Space {
    fn recorded(&self, skill: &str) -> bool {
        self.base.contains_key(skill) || self.declined.contains_key(skill)
    }

    /// Whether the copy holds something Beskar did not put there.
    fn changed(&self, skill: &str) -> bool {
        self.files
            .get(skill)
            .is_some_and(|files| self.base.get(skill) != Some(files))
    }

    /// Beskar put `files` in place and recorded them as the base.
    fn settle(&mut self, skill: &'static str, files: Tree) {
        self.files.insert(skill, files.clone());
        self.base.insert(skill, files);
        self.declined.remove(skill);
    }

    /// Beskar no longer manages the skill here.
    fn forget(&mut self, skill: &str) {
        self.base.remove(skill);
        self.declined.remove(skill);
    }
}

/// The workspace listing and the registry before an operation.
struct Snapshot {
    listing: Listing,
    registry: Vec<u8>,
}

struct World {
    seed: u64,
    _scratch: Scratch,
    beskar: Beskar,
    /// Every library skill's files.
    library: BTreeMap<&'static str, Tree>,
    profiles: BTreeMap<&'static str, BTreeSet<&'static str>>,
    spaces: Vec<Space>,
    serial: u64,
    log: Vec<String>,
}

impl World {
    /// A library with every skill in `SKILLS`, the profiles in `PROFILES`
    /// and the registered workspaces in `WORKSPACES`, none of which has a
    /// profile enabled yet.
    fn new(seed: u64, name: &str) -> World {
        let scratch = Scratch::new(name);
        let home = scratch.0.join(".beskar");
        init(&home, Some(&scratch.0), None).expect("init");
        let beskar = Beskar::load(&home, Some(&scratch.0)).expect("load");
        let mut library = BTreeMap::new();
        for skill in SKILLS {
            let files = tree(&[
                (
                    "SKILL.md",
                    &format!("---\nname: {skill}\ndescription: {skill}, first version\n---\n"),
                ),
                ("scripts/run.sh", &format!("echo {skill}\n")),
            ]);
            write_tree(&beskar.library.skills_dir().join(skill), &files);
            library.insert(skill, files);
        }
        let mut profiles = BTreeMap::new();
        for (profile, skills) in PROFILES {
            let names: Vec<String> = skills.iter().map(|s| s.to_string()).collect();
            beskar
                .create_profile(profile, &names, None)
                .expect("create profile");
            profiles.insert(profile, skills.iter().copied().collect());
        }
        let mut spaces = Vec::new();
        for workspace in WORKSPACES {
            let root = scratch.0.join(workspace);
            fs::create_dir_all(&root).expect("create workspace");
            beskar.add_repo(&root).expect("register workspace");
            spaces.push(Space {
                root,
                ..Space::default()
            });
        }
        let world = World {
            seed,
            _scratch: scratch,
            beskar,
            library,
            profiles,
            spaces,
            serial: 0,
            log: Vec::new(),
        };
        world.check();
        world
    }

    // ----- Reporting -----

    #[track_caller]
    fn fail(&self, what: String) -> ! {
        let steps: String = self
            .log
            .iter()
            .enumerate()
            .map(|(i, step)| format!("  {:>3}. {step}\n", i + 1))
            .collect();
        panic!("seed {}: {what}\nsteps:\n{steps}", self.seed);
    }

    #[track_caller]
    fn ensure(&self, ok: bool, what: impl FnOnce() -> String) {
        if !ok {
            self.fail(what());
        }
    }

    fn note(&mut self, step: String) {
        self.log.push(step);
    }

    /// Add the result of the step just noted.
    fn noted(&mut self, result: String) {
        if let Some(step) = self.log.last_mut() {
            step.push_str(" -> ");
            step.push_str(&result);
        }
    }

    fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    // ----- Paths and reads -----

    fn library_dir(&self, skill: &str) -> PathBuf {
        self.beskar.library.skills_dir().join(skill)
    }

    fn skills_dir(&self, w: usize) -> PathBuf {
        self.beskar
            .workspace(&self.spaces[w].root)
            .skills_dir()
            .to_path_buf()
    }

    fn copy_dir(&self, w: usize, name: &str) -> PathBuf {
        self.skills_dir(w).join(name)
    }

    fn copy_file(&self, w: usize, skill: &str, rel: &str) -> String {
        fs::read_to_string(self.copy_dir(w, skill).join(rel)).expect("read workspace file")
    }

    fn library_file(&self, skill: &str, rel: &str) -> String {
        fs::read_to_string(self.library_dir(skill).join(rel)).expect("read library file")
    }

    fn at(&self, w: usize) -> RepoRef {
        RepoRef::named(self.spaces[w].root.clone())
    }

    fn fingerprint(&self, path: &Path) -> Fingerprint {
        Fingerprint::of(path, self.beskar.ignore()).expect("fingerprint")
    }

    fn registry_bytes(&self) -> Vec<u8> {
        fs::read(&self.beskar.config.registry).expect("read registry")
    }

    fn snapshot(&self, w: usize) -> Snapshot {
        Snapshot {
            listing: listing(&self.spaces[w].root),
            registry: self.registry_bytes(),
        }
    }

    fn wanted(&self, w: usize) -> BTreeSet<&'static str> {
        self.spaces[w]
            .enabled
            .iter()
            .flat_map(|profile| self.profiles[profile].iter().copied())
            .collect()
    }

    fn status(&self, w: usize) -> RepoStatus {
        self.beskar
            .repo_status(&self.spaces[w].root)
            .unwrap_or_else(|error| {
                self.fail(format!(
                    "{}: status failed: {}",
                    WORKSPACES[w], error.message
                ))
            })
    }

    fn action(&self, w: usize, skill: &str) -> Option<Action> {
        step_action(&self.status(w).plan, skill)
    }

    // ----- What a person does by hand -----

    fn edit_library(&mut self, skill: &'static str, rel: &str, text: &str) {
        self.note(format!("library: write {skill}/{rel}"));
        write_file(&self.library_dir(skill).join(rel), text.as_bytes());
        self.library
            .get_mut(skill)
            .expect("a library skill")
            .insert(rel.to_string(), text.as_bytes().to_vec());
        self.check();
    }

    fn delete_library_file(&mut self, skill: &'static str, rel: &str) {
        self.note(format!("library: delete {skill}/{rel}"));
        fs::remove_file(self.library_dir(skill).join(rel)).expect("delete library file");
        self.library
            .get_mut(skill)
            .expect("a library skill")
            .remove(rel);
        self.check();
    }

    fn edit_copy(&mut self, w: usize, skill: &'static str, rel: &str, text: &str) {
        self.note(format!("{}: write {skill}/{rel}", WORKSPACES[w]));
        write_file(&self.copy_dir(w, skill).join(rel), text.as_bytes());
        self.spaces[w]
            .files
            .entry(skill)
            .or_default()
            .insert(rel.to_string(), text.as_bytes().to_vec());
        self.check();
    }

    fn delete_copy_file(&mut self, w: usize, skill: &'static str, rel: &str) {
        self.note(format!("{}: delete {skill}/{rel}", WORKSPACES[w]));
        fs::remove_file(self.copy_dir(w, skill).join(rel)).expect("delete workspace file");
        self.spaces[w]
            .files
            .get_mut(skill)
            .expect("a copy")
            .remove(rel);
        self.check();
    }

    fn delete_copy(&mut self, w: usize, skill: &'static str) {
        self.note(format!("{}: delete the copy of {skill}", WORKSPACES[w]));
        fs::remove_dir_all(self.copy_dir(w, skill)).expect("delete copy");
        self.spaces[w].files.remove(skill);
        self.check();
    }

    /// Make the copy hold exactly `files`: a directory made by hand, or a
    /// change undone.
    fn set_copy(&mut self, w: usize, skill: &'static str, files: Tree) {
        self.note(format!(
            "{}: make {skill} hold {}",
            WORKSPACES[w],
            show(Some(&files))
        ));
        let dir = self.copy_dir(w, skill);
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("clear copy");
        }
        write_tree(&dir, &files);
        self.spaces[w].files.insert(skill, files);
        self.check();
    }

    fn edit_foreign(&mut self, w: usize, name: &'static str, rel: &str, text: &str) {
        self.note(format!("{}: write {name}/{rel}", WORKSPACES[w]));
        write_file(&self.copy_dir(w, name).join(rel), text.as_bytes());
        self.spaces[w]
            .foreign
            .entry(name)
            .or_default()
            .insert(rel.to_string(), text.as_bytes().to_vec());
        self.check();
    }

    // ----- What a person asks Beskar to do -----

    fn switch(&mut self, w: usize, change: ProfileChange, profile: &'static str) {
        self.note(format!("{}: {change:?} {profile}", WORKSPACES[w]));
        let was = self.spaces[w].enabled.contains(profile);
        let changed = self
            .beskar
            .change_profiles(&self.at(w), change, &[profile.to_string()])
            .unwrap_or_else(|error| self.fail(format!("change_profiles: {}", error.message)));
        let enable = match change {
            ProfileChange::Enable => true,
            ProfileChange::Disable => false,
            ProfileChange::Toggle => !was,
        };
        let names = |list: &[ProfileName]| -> Vec<String> {
            list.iter().map(|name| name.as_str().to_string()).collect()
        };
        let only = |yes: bool| -> Vec<String> {
            if yes {
                vec![profile.to_string()]
            } else {
                Vec::new()
            }
        };
        self.ensure(
            names(&changed.enabled) == only(enable && !was)
                && names(&changed.disabled) == only(!enable && was),
            || format!("change_profiles reported {changed:?}"),
        );
        if enable {
            self.spaces[w].enabled.insert(profile);
        } else {
            self.spaces[w].enabled.remove(profile);
        }
        let pending = changed
            .pending
            .as_ref()
            .unwrap_or_else(|error| self.fail(format!("pending plan: {}", error.message)));
        let plans = self.check();
        // Invariant 7.
        self.ensure(same_plan(pending, &plans[w]), || {
            "the plan change_profiles reports differs from status".to_string()
        });
    }

    fn add_to_profile(&mut self, profile: &'static str, skill: &'static str) {
        self.note(format!("profile {profile}: add {skill}"));
        let edit = self
            .beskar
            .add_to_profile(profile, &[skill.to_string()])
            .unwrap_or_else(|error| self.fail(format!("add_to_profile: {}", error.message)));
        let fresh = self
            .profiles
            .get_mut(profile)
            .expect("a profile")
            .insert(skill);
        let changed: Vec<&str> = edit.changed.iter().map(SkillId::as_str).collect();
        self.ensure(changed == if fresh { vec![skill] } else { vec![] }, || {
            format!("add_to_profile reported {changed:?}")
        });
        self.check();
    }

    fn remove_from_profile(&mut self, profile: &'static str, skill: &'static str) {
        self.note(format!("profile {profile}: remove {skill}"));
        let edit = self
            .beskar
            .remove_from_profile(profile, &[skill.to_string()])
            .unwrap_or_else(|error| self.fail(format!("remove_from_profile: {}", error.message)));
        let was = self
            .profiles
            .get_mut(profile)
            .expect("a profile")
            .remove(skill);
        let changed: Vec<&str> = edit.changed.iter().map(SkillId::as_str).collect();
        self.ensure(changed == if was { vec![skill] } else { vec![] }, || {
            format!("remove_from_profile reported {changed:?}")
        });
        self.check();
    }

    fn update(&mut self, w: usize, policy: ConflictPolicy) -> RepoUpdate {
        self.note(format!("{}: update ({})", WORKSPACES[w], policy.as_str()));
        let before = self.snapshot(w);
        let wanted = self.wanted(w);
        let mut resolver = policy;
        let update = self
            .beskar
            .update_repo(&self.spaces[w].root, false, &mut resolver)
            .unwrap_or_else(|error| self.fail(format!("update failed: {}", error.message)));
        self.noted(summary(&update));
        self.absorb(w, policy, &update, &before, &wanted, false);
        let plans = self.check();
        let next = &plans[w];
        if matches!(update.result, UpdateResult::Stopped) {
            // Invariant 7: nothing changed, so planning again under no lock
            // gives the plan the update made under the lock.
            self.ensure(same_plan(&update.plan, next), || {
                format!("{}: a stopped update planned differently", WORKSPACES[w])
            });
        } else {
            self.ensure(next.is_up_to_date(), || {
                format!(
                    "{}: the update left work to do: {}",
                    WORKSPACES[w],
                    plan_summary(next)
                )
            });
        }
        update
    }

    fn dry_run(&mut self, w: usize, policy: ConflictPolicy) -> RepoUpdate {
        self.note(format!("{}: dry run ({})", WORKSPACES[w], policy.as_str()));
        let before = self.snapshot(w);
        let mut resolver = policy;
        let update = self
            .beskar
            .update_repo(&self.spaces[w].root, true, &mut resolver)
            .unwrap_or_else(|error| self.fail(format!("dry run failed: {}", error.message)));
        self.noted(summary(&update));
        self.check_dry_run(w, &update, &before, &self.wanted(w));
        let plans = self.check();
        // Invariant 7.
        self.ensure(same_plan(&update.plan, &plans[w]), || {
            "a dry run planned differently from status".to_string()
        });
        update
    }

    /// Promote without `--force`.
    fn promote(&mut self, w: usize, skill: &'static str) -> Result<Promoted> {
        self.note(format!("{}: promote {skill}", WORKSPACES[w]));
        let library = self.library[skill].clone();
        let space = &self.spaces[w];
        let copy = space.files.get(skill).cloned();
        let base = space.base.get(skill).cloned();
        let tracked = space.recorded(skill);
        let wanted = self.wanted(w).contains(skill);
        let result = self.beskar.promote(&self.at(w), skill, false);
        self.noted(match &result {
            Ok(promoted) => format!("{:?}", promoted.promotion.imported),
            Err(error) => format!("refused ({:?})", error.kind),
        });
        let Some(copy) = copy else {
            match &result {
                Err(error) if error.kind == ErrorKind::NotFound => {}
                _ => self.fail(format!("promoting a missing copy gave {result:?}")),
            }
            self.check();
            return result;
        };
        // Without --force, the library changes only if it is still the
        // version the copy is based on.
        let expected = if copy == library {
            Some(Imported::Unchanged)
        } else if base.as_ref() == Some(&library) {
            Some(Imported::Replaced)
        } else {
            None
        };
        match (&result, expected) {
            (Ok(promoted), Some(imported)) => self.ensure(
                promoted.promotion.imported == imported && promoted.promotion.wanted == wanted,
                || format!("promote reported {promoted:?}, expected {imported:?}"),
            ),
            (Err(error), None) => self.ensure(error.kind == ErrorKind::Conflict, || {
                format!("promote failed oddly: {}", error.message)
            }),
            (Ok(promoted), None) => self.fail(format!(
                "promote without --force overwrote library changes to {skill}: {promoted:?}"
            )),
            (Err(error), Some(_)) => self.fail(format!("promote refused: {}", error.message)),
        }
        if result.is_ok() {
            self.library.insert(skill, copy.clone());
            if tracked || wanted {
                self.spaces[w].settle(skill, copy);
            }
        }
        self.check();
        result
    }

    /// Restore through the preview a front end shows first.
    fn restore(&mut self, w: usize, skill: &'static str) -> Result<Restored> {
        self.note(format!("{}: restore {skill}", WORKSPACES[w]));
        let preview = self
            .beskar
            .restore_preview(&self.at(w), skill)
            .unwrap_or_else(|error| self.fail(format!("restore preview: {}", error.message)));
        let library = self.library[skill].clone();
        let space = &self.spaces[w];
        let copy = space.files.get(skill);
        let discards =
            copy.is_some_and(|copy| *copy != library && space.base.get(skill) != Some(copy));
        self.ensure(
            preview.discards_changes == discards && preview.present.is_some() == copy.is_some(),
            || {
                format!(
                    "the restore preview says {preview:?}; the copy is {}",
                    show(copy)
                )
            },
        );
        let expected = match copy {
            Some(copy) if *copy == library => Done::Recorded,
            Some(_) => Done::Replaced,
            None if space.recorded(skill) => Done::Restored,
            None => Done::Installed,
        };
        let wanted = self.wanted(w).contains(skill);
        let result = self.beskar.restore(&preview);
        self.noted(match &result {
            Ok(restored) => format!("{:?}", restored.done),
            Err(error) => format!("refused ({:?})", error.kind),
        });
        match &result {
            Ok(restored) => {
                self.ensure(wanted && restored.done == expected, || {
                    format!("restore did {restored:?}, expected {expected:?} (wanted: {wanted})")
                });
                self.spaces[w].settle(skill, library);
            }
            Err(error) => self.ensure(!wanted && error.kind == ErrorKind::Invalid, || {
                format!("restore failed: {}", error.message)
            }),
        }
        self.check();
        result
    }

    /// `repo remove --purge`, registering the workspace again afterwards so
    /// the walk can go on.
    fn purge(&mut self, w: usize, policy: ConflictPolicy, dry_run: bool) -> RepoRemoved {
        self.note(format!(
            "{}: purge ({}{})",
            WORKSPACES[w],
            policy.as_str(),
            if dry_run { ", dry run" } else { "" }
        ));
        let before = self.snapshot(w);
        let none = BTreeSet::new();
        let mut resolver = policy;
        let removed = self
            .beskar
            .remove_repo(
                &self.spaces[w].root,
                Some(&mut resolver as &mut dyn Resolver),
                dry_run,
            )
            .unwrap_or_else(|error| self.fail(format!("purge failed: {}", error.message)));
        let update = removed
            .purge
            .clone()
            .unwrap_or_else(|| self.fail("a purge of an existing workspace has no plan".into()));
        self.noted(format!(
            "{}{}",
            summary(&update),
            if removed.unregistered {
                ", unregistered"
            } else {
                ""
            }
        ));
        if dry_run {
            self.check_dry_run(w, &update, &before, &none);
            self.ensure(!removed.unregistered, || {
                "a dry-run purge unregistered the workspace".into()
            });
        } else {
            self.absorb(w, policy, &update, &before, &none, true);
            let registered = self
                .beskar
                .registry()
                .expect("registry")
                .get(&self.spaces[w].root)
                .is_some();
            self.ensure(registered != removed.unregistered, || {
                format!("purge says unregistered: {}", removed.unregistered)
            });
            if removed.unregistered {
                let space = &self.spaces[w];
                self.ensure(space.base.is_empty() && space.declined.is_empty(), || {
                    "a purge unregistered a workspace whose installations were not all settled"
                        .into()
                });
                let added = self
                    .beskar
                    .add_repo(&self.spaces[w].root)
                    .unwrap_or_else(|error| self.fail(format!("add_repo: {}", error.message)));
                self.ensure(added.new, || {
                    "the purged workspace was still registered".into()
                });
                self.spaces[w].enabled.clear();
            } else {
                self.ensure(matches!(update.result, UpdateResult::Stopped), || {
                    "a purge that went through left the workspace registered".into()
                });
            }
        }
        self.check();
        removed
    }

    // ----- Checking operations -----

    /// Check an update's (or a purge's) result against the model, then
    /// bring the model up to date with what it did.
    fn absorb(
        &mut self,
        w: usize,
        policy: ConflictPolicy,
        update: &RepoUpdate,
        before: &Snapshot,
        wanted: &BTreeSet<&'static str>,
        purge: bool,
    ) {
        let label = WORKSPACES[w];
        // The plan made under the lock is the model's, before the update.
        self.check_plan(w, &update.plan, wanted);
        match &update.result {
            UpdateResult::Planned => self.fail(format!("{label}: a real update only planned")),
            UpdateResult::Stopped => {
                self.ensure(
                    matches!(policy, ConflictPolicy::Abort | ConflictPolicy::Ask),
                    || format!("{label}: policy {} stopped", policy.as_str()),
                );
                self.ensure(update.plan.conflicts().next().is_some(), || {
                    format!("{label}: stopped without a conflict")
                });
                // Invariant 2.
                self.ensure(listing(&self.spaces[w].root) == before.listing, || {
                    format!("{label}: a stopped update changed the workspace")
                });
                self.ensure(self.registry_bytes() == before.registry, || {
                    format!("{label}: a stopped update changed the registry")
                });
            }
            UpdateResult::UpToDate => {
                self.ensure(update.plan.is_up_to_date(), || {
                    format!("{label}: up to date with work left in the plan")
                });
                let now = listing(&self.spaces[w].root);
                // A purge may tidy away the empty skills directory.
                let same = if purge {
                    files_of(&now) == files_of(&before.listing)
                } else {
                    now == before.listing
                };
                self.ensure(same, || {
                    format!("{label}: an update with nothing to do changed files")
                });
                self.check_converged(w, &update.plan, wanted);
            }
            UpdateResult::Applied(outcomes) => {
                // Exactly the steps that do something report an outcome.
                let acting: BTreeSet<&str> = update
                    .plan
                    .steps
                    .iter()
                    .filter(|step| {
                        !matches!(
                            step.action,
                            Action::Unchanged
                                | Action::KeepLocal
                                | Action::Unmanaged
                                | Action::MissingSource
                        )
                    })
                    .map(|step| step.skill.as_str())
                    .collect();
                let reported: Vec<&str> = outcomes.iter().map(|o| o.skill.as_str()).collect();
                self.ensure(
                    reported.iter().copied().collect::<BTreeSet<_>>() == acting
                        && reported.len() == acting.len(),
                    || format!("{label}: outcomes for {reported:?}, steps for {acting:?}"),
                );
                for outcome in outcomes {
                    self.absorb_outcome(w, policy, update, outcome);
                }
                if policy == ConflictPolicy::Replace {
                    self.check_converged(w, &update.plan, wanted);
                }
            }
        }
    }

    fn absorb_outcome(
        &mut self,
        w: usize,
        policy: ConflictPolicy,
        update: &RepoUpdate,
        outcome: &Outcome,
    ) {
        let label = WORKSPACES[w];
        let name = outcome.skill.as_str();
        let Some(skill) = SKILLS.iter().copied().find(|skill| *skill == name) else {
            self.fail(format!(
                "{label}: an outcome for {name}, which is not a library skill"
            ))
        };
        let done = match &outcome.result {
            Ok(done) => *done,
            Err(error) => self.fail(format!("{label}: {skill} failed: {}", error.message)),
        };
        let action = step_action(&update.plan, skill).unwrap_or_else(|| {
            self.fail(format!("{label}: an outcome for {skill} without a step"))
        });
        let library = self.library[skill].clone();
        let space = &self.spaces[w];
        let was = space.files.get(skill).cloned();
        let changed = space.changed(skill);
        let now = read_tree(&self.copy_dir(w, skill));
        let conflict = matches!(action, Action::Conflict(_));
        let context = || {
            format!(
                "{label}: {skill} {done:?} under {} (step {action:?}); it held {}, now {}",
                policy.as_str(),
                show(was.as_ref()),
                show(now.as_ref())
            )
        };
        match done {
            Done::Installed | Done::Restored | Done::Updated | Done::Recorded | Done::Replaced => {
                self.ensure(now.as_ref() == Some(&library), || {
                    format!("{} - which is not the library version", context())
                });
                if done == Done::Replaced {
                    self.ensure(policy == ConflictPolicy::Replace && conflict, context);
                } else {
                    // Invariant 1: only `replace` overwrites local changes.
                    self.ensure(!changed || done == Done::Recorded, || {
                        format!("{} - local changes were overwritten", context())
                    });
                }
                self.spaces[w].settle(skill, library);
            }
            Done::Removed => {
                self.ensure(now.is_none(), context);
                // Invariant 1: only `replace` deletes local changes.
                self.ensure(
                    !changed
                        || (policy == ConflictPolicy::Replace
                            && action == Action::Conflict(Conflict::Orphaned)),
                    || format!("{} - local changes were deleted", context()),
                );
                let space = &mut self.spaces[w];
                space.files.remove(skill);
                space.forget(skill);
            }
            Done::Forgotten => {
                self.ensure(was.is_none() && now.is_none(), context);
                self.spaces[w].forget(skill);
            }
            Done::KeptLocal => {
                self.ensure(
                    policy == ConflictPolicy::Keep
                        && matches!(
                            action,
                            Action::Conflict(Conflict::Diverged | Conflict::Untracked)
                        )
                        && now == was,
                    context,
                );
                self.spaces[w].declined.insert(skill, library);
            }
            Done::Released => {
                self.ensure(
                    now == was
                        && ((policy == ConflictPolicy::Keep
                            && action == Action::Conflict(Conflict::Orphaned))
                            || action == Action::Release),
                    context,
                );
                self.spaces[w].forget(skill);
            }
            Done::Promoted => self.fail(format!("{} - no policy promotes", context())),
        }
    }

    fn check_dry_run(
        &self,
        w: usize,
        update: &RepoUpdate,
        before: &Snapshot,
        wanted: &BTreeSet<&'static str>,
    ) {
        let label = WORKSPACES[w];
        let consistent = match update.result {
            UpdateResult::Planned => !update.plan.is_up_to_date(),
            UpdateResult::UpToDate => update.plan.is_up_to_date(),
            _ => false,
        };
        self.ensure(consistent, || {
            format!("{label}: a dry run returned {:?}", update.result)
        });
        // Invariant 2.
        self.ensure(listing(&self.spaces[w].root) == before.listing, || {
            format!("{label}: a dry run changed the workspace")
        });
        self.ensure(self.registry_bytes() == before.registry, || {
            format!("{label}: a dry run changed the registry")
        });
        self.check_plan(w, &update.plan, wanted);
    }

    /// Invariant 3: every wanted skill matches the library, unless it is a
    /// local change the library has not moved past, and Beskar manages no
    /// unwanted copy that is still there.
    fn check_converged(&self, w: usize, plan: &RepoPlan, wanted: &BTreeSet<&'static str>) {
        let label = WORKSPACES[w];
        let space = &self.spaces[w];
        for skill in SKILLS {
            let now = read_tree(&self.copy_dir(w, skill));
            if wanted.contains(skill) {
                let local =
                    step_action(plan, skill) == Some(Action::KeepLocal) && space.changed(skill);
                self.ensure(now.as_ref() == Some(&self.library[skill]) || local, || {
                    format!(
                        "{label}: wanted {skill} holds {}, the library {}",
                        show(now.as_ref()),
                        show(Some(&self.library[skill]))
                    )
                });
            } else if now.is_some() {
                self.ensure(!space.recorded(skill), || {
                    format!("{label}: unwanted {skill} is still managed")
                });
            }
        }
    }

    // ----- Invariants that hold between operations -----

    /// Check every invariant against the files, the registry and fresh
    /// plans, and return each workspace's plan.
    fn check(&self) -> Vec<RepoPlan> {
        // Invariant 6: the registry loads.
        let registry = self.beskar.registry().unwrap_or_else(|error| {
            self.fail(format!("the registry does not load: {}", error.message))
        });
        for skill in SKILLS {
            let files = read_tree(&self.library_dir(skill));
            self.ensure(files.as_ref() == Some(&self.library[skill]), || {
                format!(
                    "the library's {skill} holds {}, the model {}",
                    show(files.as_ref()),
                    show(Some(&self.library[skill]))
                )
            });
        }
        for (profile, skills) in &self.profiles {
            let loaded = self
                .beskar
                .library
                .profile(&ProfileName::new(profile).expect("profile name"))
                .unwrap_or_else(|error| self.fail(format!("profile {profile}: {}", error.message)));
            let listed: BTreeSet<&str> = loaded.skills.iter().map(SkillId::as_str).collect();
            self.ensure(listed == *skills, || {
                format!("profile {profile} lists {listed:?}, the model {skills:?}")
            });
        }
        // Invariant 5.
        let mut dirs = vec![
            self.beskar.config.home.clone(),
            self.beskar.library.skills_dir(),
            self.beskar.library.profiles_dir(),
        ];
        dirs.extend((0..self.spaces.len()).map(|w| self.skills_dir(w)));
        for dir in &dirs {
            self.check_no_leftovers(dir);
        }
        let plans: Vec<RepoPlan> = (0..self.spaces.len())
            .map(|w| self.check_space(w, &registry))
            .collect();
        // Invariant 7: planning the same state again gives the same plan.
        // Planning is the slow part of a check, so each step plans one
        // workspace twice; dry runs, stopped updates and profile changes
        // compare their own plans with these too.
        let w = self.log.len() % self.spaces.len();
        self.ensure(same_plan(&plans[w], &self.status(w).plan), || {
            format!("{}: planning twice gave two plans", WORKSPACES[w])
        });
        plans
    }

    fn check_no_leftovers(&self, dir: &Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let name = entry.expect("list directory").file_name();
            let name = name.to_string_lossy();
            self.ensure(name != ".beskar" && !name.starts_with(".beskar-"), || {
                format!("{} holds the leftover {name}", dir.display())
            });
        }
    }

    fn check_space(&self, w: usize, registry: &Registry) -> RepoPlan {
        let label = WORKSPACES[w];
        let space = &self.spaces[w];
        // Invariant 1: every copy holds what the model expects, including
        // every change made by hand that nothing settled since.
        for skill in SKILLS {
            let files = read_tree(&self.copy_dir(w, skill));
            self.ensure(files.as_ref() == space.files.get(skill), || {
                format!(
                    "{label}: {skill} holds {}, the model expects {}",
                    show(files.as_ref()),
                    show(space.files.get(skill))
                )
            });
        }
        // Invariant 4.
        for (name, expected) in &space.foreign {
            let files = read_tree(&self.copy_dir(w, name));
            self.ensure(files.as_ref() == Some(expected), || {
                format!("{label}: {name}, which no profile wants, was touched")
            });
        }

        // The registry agrees with the model.
        let entry = registry
            .get(&space.root)
            .unwrap_or_else(|| self.fail(format!("{label} is not registered")));
        let enabled: BTreeSet<&str> = entry.profiles.iter().map(ProfileName::as_str).collect();
        self.ensure(enabled == space.enabled, || {
            format!(
                "{label}: the registry enables {enabled:?}, the model {:?}",
                space.enabled
            )
        });
        for skill in entry.installed.keys() {
            self.ensure(SKILLS.contains(&skill.as_str()), || {
                format!("{label}: the registry records {skill}, which is not a library skill")
            });
        }
        // Whatever Beskar records here, it records where the copies are.
        self.ensure(
            entry.installed.is_empty()
                || entry.skills_dir.as_deref() == Some(self.beskar.config.skills_dir.as_path()),
            || format!("{label}: the registry records skills but not their skills directory"),
        );
        let status = self.status(w);
        let plan = status.plan;
        self.ensure(status.leftovers.is_empty(), || {
            format!("{label}: leftovers {:?}", status.leftovers)
        });
        for skill in SKILLS {
            let installation = entry.installed.get(&id(skill));
            let base = installation.and_then(|i| i.base);
            let declined = installation.and_then(|i| i.kept);
            self.ensure(
                installation.is_some() == space.recorded(skill)
                    && base.is_some() == space.base.contains_key(skill)
                    && declined.is_some() == space.declined.contains_key(skill),
                || {
                    format!(
                        "{label}: the registry records {skill} as {installation:?}; the model has base {} and declined {}",
                        show(space.base.get(skill)),
                        show(space.declined.get(skill))
                    )
                },
            );
            // The fingerprints the registry holds are those of the contents
            // the model says they stand for. The plan has the copy's and
            // (for a wanted skill) the library's fingerprint already.
            let step = plan.steps.iter().find(|step| step.skill.as_str() == skill);
            let copy_fp = step.and_then(|step| step.present);
            let library_fp = || {
                step.and_then(|step| step.library)
                    .unwrap_or_else(|| self.fingerprint(&self.library_dir(skill)))
            };
            if let (Some(base), Some(files)) = (base, space.base.get(skill)) {
                if space.files.get(skill) == Some(files) {
                    self.ensure(Some(base) == copy_fp, || {
                        format!("{label}: the recorded base of {skill} is not the untouched copy")
                    });
                }
                if self.library[skill] == *files {
                    self.ensure(base == library_fp(), || {
                        format!("{label}: the recorded base of {skill} is not the library version")
                    });
                }
            }
            if let (Some(declined), Some(files)) = (declined, space.declined.get(skill))
                && self.library[skill] == *files
            {
                self.ensure(declined == library_fp(), || {
                    format!("{label}: the declined version of {skill} is not the library's")
                });
            }
        }

        // Invariant 6: a recorded skill that is gone is forgotten or
        // restored.
        for skill in entry.installed.keys() {
            if !self.copy_dir(w, skill.as_str()).exists() {
                let action = step_action(&plan, skill.as_str());
                self.ensure(
                    matches!(action, Some(Action::Forget | Action::Restore)),
                    || format!("{label}: recorded {skill} is gone and planned as {action:?}"),
                );
            }
        }
        self.check_plan(w, &plan, &self.wanted(w));
        plan
    }

    /// Check a plan against the decision table, recomputed from the model.
    fn check_plan(&self, w: usize, plan: &RepoPlan, wanted: &BTreeSet<&'static str>) {
        let label = WORKSPACES[w];
        let space = &self.spaces[w];
        self.ensure(
            plan.steps
                .windows(2)
                .all(|pair| pair[0].skill < pair[1].skill),
            || format!("{label}: plan steps are not sorted by skill"),
        );
        self.ensure(
            plan.blocked.is_empty()
                && plan.stays.is_empty()
                && plan.missing_sources().next().is_none(),
            || format!("{label}: nothing here is blocked, released or missing: {plan:?}"),
        );
        for step in &plan.steps {
            let name = step.skill.as_str();
            self.ensure(
                SKILLS.contains(&name) || space.foreign.contains_key(name),
                || format!("{label}: the plan has a step for {name}"),
            );
            self.ensure(step.profiles.is_empty() != wanted.contains(name), || {
                format!("{label}: {name} is wanted by {:?}", step.profiles)
            });
        }
        for skill in SKILLS {
            let actual = step_action(plan, skill);
            let expected = self.expected_action(w, skill, wanted.contains(skill));
            self.ensure(actual == expected, || {
                format!(
                    "{label}: {skill} is planned as {actual:?}, the decision table says {expected:?} (copy {}, base {}, declined {}, library {})",
                    show(space.files.get(skill)),
                    show(space.base.get(skill)),
                    show(space.declined.get(skill)),
                    show(Some(&self.library[skill]))
                )
            });
        }
        for name in space.foreign.keys() {
            let ok = match SkillId::new(name) {
                Ok(_) => step_action(plan, name) == Some(Action::Unmanaged),
                Err(_) => plan.others.iter().any(|other| other == name),
            };
            self.ensure(ok, || format!("{label}: {name} is not left alone"));
        }
    }

    /// The reconciliation decision table, in terms of the model's contents.
    fn expected_action(&self, w: usize, skill: &str, wanted: bool) -> Option<Action> {
        let space = &self.spaces[w];
        let library = &self.library[skill];
        let base = space.base.get(skill);
        let declined = space.declined.get(skill);
        let recorded = space.recorded(skill);
        let Some(copy) = space.files.get(skill) else {
            return match (wanted, recorded) {
                (true, true) => Some(Action::Restore),
                (true, false) => Some(Action::Install),
                (false, true) => Some(Action::Forget),
                (false, false) => None,
            };
        };
        Some(if wanted {
            if copy == library {
                if base == Some(library) && declined.is_none() {
                    Action::Unchanged
                } else {
                    Action::Record
                }
            } else if !recorded {
                Action::Conflict(Conflict::Untracked)
            } else if base == Some(copy) {
                Action::Update
            } else if declined == Some(library) {
                Action::KeepLocal
            } else {
                match base {
                    None => Action::Conflict(Conflict::Untracked),
                    Some(base) if base == library => Action::KeepLocal,
                    Some(_) => Action::Conflict(Conflict::Diverged),
                }
            }
        } else if !recorded {
            Action::Unmanaged
        } else if base == Some(copy) {
            Action::Remove
        } else {
            Action::Conflict(Conflict::Orphaned)
        })
    }

    // ----- One random step -----

    fn random_step(&mut self, rng: &mut Rng) {
        let w = rng.below(self.spaces.len());
        let policy = rng.pick(&ConflictPolicy::ALL);
        let skill = rng.pick(&SKILLS);
        let n = self.next_serial();
        let present: Vec<&'static str> = self.spaces[w].files.keys().copied().collect();
        let absent: Vec<&'static str> = SKILLS
            .iter()
            .copied()
            .filter(|skill| !present.contains(skill))
            .collect();
        // A skill with a copy here, for steps that act on one.
        let copy = (!present.is_empty()).then(|| rng.pick(&present));
        match rng.below(100) {
            0..=7 => {
                let rel = any_file(rng, &self.library[skill]).unwrap_or_else(|| "SKILL.md".into());
                self.edit_library(skill, &rel, &format!("{skill}: library edit {n}\n"));
            }
            8..=11 => self.edit_library(
                skill,
                &format!("refs/r{n}.md"),
                &format!("{skill}: new library file {n}\n"),
            ),
            12..=14 => {
                let extra: Vec<String> = self.library[skill]
                    .keys()
                    .filter(|rel| *rel != "SKILL.md")
                    .cloned()
                    .collect();
                if extra.is_empty() {
                    self.edit_library(skill, &format!("refs/r{n}.md"), &format!("{n}\n"));
                } else {
                    self.delete_library_file(skill, &rng.pick(&extra));
                }
            }
            15..=23 => match copy {
                Some(copy) => {
                    let rel = any_file(rng, &self.spaces[w].files[copy])
                        .unwrap_or_else(|| "SKILL.md".into());
                    self.edit_copy(w, copy, &rel, &format!("{copy}: local edit {n}\n"));
                }
                None => self.make_by_hand(w, skill, n),
            },
            24..=27 => match copy {
                Some(copy) => self.edit_copy(
                    w,
                    copy,
                    &format!("notes/n{n}.md"),
                    &format!("{copy}: local notes {n}\n"),
                ),
                None => self.make_by_hand(w, skill, n),
            },
            28..=30 => match copy.map(|copy| (copy, any_file(rng, &self.spaces[w].files[copy]))) {
                Some((copy, Some(rel))) => self.delete_copy_file(w, copy, &rel),
                Some((copy, None)) => self.delete_copy(w, copy),
                None => self.make_by_hand(w, skill, n),
            },
            31..=33 => match copy {
                Some(copy) => self.delete_copy(w, copy),
                None => self.make_by_hand(w, skill, n),
            },
            34..=37 => {
                let skill = if absent.is_empty() {
                    skill
                } else {
                    rng.pick(&absent)
                };
                self.make_by_hand(w, skill, n);
            }
            38..=41 => {
                let space = &self.spaces[w];
                let changed: Vec<&'static str> = space
                    .base
                    .keys()
                    .copied()
                    .filter(|skill| space.files.get(skill) != space.base.get(skill))
                    .collect();
                if changed.is_empty() {
                    self.dry_run(w, policy);
                } else {
                    let skill = rng.pick(&changed);
                    let base = self.spaces[w].base[skill].clone();
                    self.set_copy(w, skill, base);
                }
            }
            42..=43 => {
                let name = rng.pick(&FOREIGN);
                let rel = self.spaces[w]
                    .foreign
                    .get(name)
                    .and_then(|files| any_file(rng, files))
                    .unwrap_or_else(|| format!("notes-{n}.md"));
                self.edit_foreign(w, name, &rel, &format!("{name}: kept by hand {n}\n"));
            }
            44..=50 => {
                let change = rng.pick(&[
                    ProfileChange::Enable,
                    ProfileChange::Disable,
                    ProfileChange::Toggle,
                ]);
                let profile = rng.pick(&PROFILES).0;
                self.switch(w, change, profile);
            }
            51..=56 => {
                let profile = rng.pick(&PROFILES).0;
                if self.profiles[profile].contains(skill) {
                    self.remove_from_profile(profile, skill);
                } else {
                    self.add_to_profile(profile, skill);
                }
            }
            57..=74 | 97..=99 => {
                self.update(w, policy);
            }
            75..=80 => {
                self.dry_run(w, policy);
            }
            81..=86 => {
                let _ = self.promote(w, copy.unwrap_or(skill));
            }
            87..=93 => {
                let skill = if rng.below(4) == 0 {
                    skill
                } else {
                    copy.unwrap_or(skill)
                };
                let _ = self.restore(w, skill);
            }
            _ => {
                let dry_run = rng.below(3) == 0;
                self.purge(w, policy, dry_run);
            }
        }
    }

    /// Put a directory Beskar never installed where `skill` goes.
    fn make_by_hand(&mut self, w: usize, skill: &'static str, n: u64) {
        let text = format!("---\nname: {skill}\ndescription: made by hand {n}\n---\n");
        self.set_copy(w, skill, tree(&[("SKILL.md", &text)]));
    }
}

// ----- Helpers -----

fn id(name: &str) -> SkillId {
    SkillId::new(name).expect("a valid skill name")
}

fn tree(files: &[(&str, &str)]) -> Tree {
    files
        .iter()
        .map(|(rel, text)| (rel.to_string(), text.as_bytes().to_vec()))
        .collect()
}

fn step_action(plan: &RepoPlan, skill: &str) -> Option<Action> {
    plan.steps
        .iter()
        .find(|step| step.skill.as_str() == skill)
        .map(|step| step.action)
}

/// Whether two plans are the same, step for step.
fn same_plan(a: &RepoPlan, b: &RepoPlan) -> bool {
    a.repo == b.repo
        && a.steps == b.steps
        && a.blocked.keys().eq(b.blocked.keys())
        && a.stays == b.stays
        && a.others == b.others
}

fn done(update: &RepoUpdate, skill: &str) -> Option<Done> {
    match &update.result {
        UpdateResult::Applied(outcomes) => outcomes
            .iter()
            .find(|outcome| outcome.skill.as_str() == skill)
            .and_then(|outcome| outcome.result.as_ref().ok().copied()),
        _ => None,
    }
}

fn plan_summary(plan: &RepoPlan) -> String {
    let steps: Vec<String> = plan
        .steps
        .iter()
        .filter(|step| !matches!(step.action, Action::Unchanged | Action::Unmanaged))
        .map(|step| format!("{} {:?}", step.skill, step.action))
        .collect();
    format!("[{}]", steps.join(", "))
}

fn summary(update: &RepoUpdate) -> String {
    match &update.result {
        UpdateResult::UpToDate => "up to date".into(),
        UpdateResult::Planned => format!("planned {}", plan_summary(&update.plan)),
        UpdateResult::Stopped => format!("stopped {}", plan_summary(&update.plan)),
        UpdateResult::Applied(outcomes) => {
            let outcomes: Vec<String> = outcomes
                .iter()
                .map(|outcome| match &outcome.result {
                    Ok(done) => format!("{} {done:?}", outcome.skill),
                    Err(error) => format!("{} failed: {}", outcome.skill, error.message),
                })
                .collect();
            format!("applied [{}]", outcomes.join(", "))
        }
    }
}

fn show(files: Option<&Tree>) -> String {
    match files {
        None => "nothing".into(),
        Some(files) => {
            let files: Vec<String> = files
                .iter()
                .map(|(rel, bytes)| format!("{rel}: {:?}", String::from_utf8_lossy(bytes)))
                .collect();
            format!("{{{}}}", files.join(", "))
        }
    }
}

fn any_file(rng: &mut Rng, files: &Tree) -> Option<String> {
    let names: Vec<&String> = files.keys().collect();
    (!names.is_empty()).then(|| rng.pick(&names).clone())
}

// ----- Files -----

/// The files under `dir`, or `None` if nothing is there.
fn read_tree(dir: &Path) -> Option<Tree> {
    let metadata = fs::symlink_metadata(dir).ok()?;
    assert!(metadata.is_dir(), "{} is not a directory", dir.display());
    let mut files = Tree::new();
    for (rel, contents) in listing(dir) {
        if let Some(contents) = contents {
            files.insert(rel, contents);
        }
    }
    Some(files)
}

/// Everything under `root`, directories included.
fn listing(root: &Path) -> Listing {
    fn walk(dir: &Path, prefix: &str, out: &mut Listing) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let entry = entry.expect("list directory");
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.file_type().expect("file type").is_dir() {
                out.insert(rel.clone(), None);
                walk(&entry.path(), &rel, out);
            } else {
                out.insert(rel, Some(fs::read(entry.path()).expect("read file")));
            }
        }
    }
    let mut out = Listing::new();
    walk(root, "", &mut out);
    out
}

fn files_of(listing: &Listing) -> BTreeMap<&String, &Vec<u8>> {
    listing
        .iter()
        .filter_map(|(rel, contents)| contents.as_ref().map(|contents| (rel, contents)))
        .collect()
}

fn write_tree(dir: &Path, files: &Tree) {
    fs::create_dir_all(dir).expect("create directory");
    for (rel, contents) in files {
        write_file(&dir.join(rel), contents);
    }
}

fn write_file(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().expect("a file has a parent")).expect("create directory");
    fs::write(path, contents).expect("write file");
}

/// A directory under the system temp dir, deleted on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "beskar-safety-{}-{}-{name}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create scratch directory");
        Scratch(fs::canonicalize(&path).expect("canonicalize scratch directory"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// xorshift64*: small, fast and the same everywhere, so a seed always
/// replays the same walk.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        // splitmix64 spreads neighbouring seeds apart; the state is never 0.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Rng((z ^ (z >> 31)) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    fn pick<T: Clone>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())].clone()
    }
}

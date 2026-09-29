//! CLI command implementations. These modules are thin: they parse no
//! arguments (cli.rs did), print through `Ui`, and delegate domain work to
//! the library modules. Swapping the CLI for a TUI means replacing only
//! this tree.

pub mod doctor;
pub mod init;
pub mod library;
pub mod profiles;
pub mod registries;
pub mod repos;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cli::{Cmd, Global};
use crate::config::{Config, ConflictPolicy};
use crate::error::{Error, Exit, Result};
use crate::library::Library;
use crate::registry::Registry;
use crate::reconcile::{self, Applier, Summary, Workspace};
use crate::ui::Ui;
use crate::util;

/// Per-invocation context handed to every command.
pub struct Ctx {
    pub global: Global,
    pub ui: Ui,
}

impl Ctx {
    pub fn new(global: Global) -> Ctx {
        let ui = if global.color { Ui::detect() } else { Ui { color: false } };
        Ctx { global, ui }
    }

    /// Load the beskar config, pointing the user at `init` when absent.
    pub fn config(&self) -> Result<Config> {
        let home = Config::resolve_home(self.global.home.as_deref())?;
        Config::load(&home)
    }

    /// Yes unless the user asked us not to assume it.
    pub fn assume_yes(&self) -> bool {
        self.global.yes
    }
}

/// Dispatch a parsed command.
pub fn dispatch(ctx: &Ctx, cmd: Cmd) -> Result<Exit> {
    match cmd {
        Cmd::Init => init::run(ctx),
        Cmd::Doctor => doctor::run(ctx),
        Cmd::Library(sub) => library::run(ctx, sub),
        Cmd::Profile(sub) => profiles::run(ctx, sub),
        Cmd::Repo(sub) => repos::run(ctx, sub),
        Cmd::Registry(sub) => registries::run(ctx, sub),
        Cmd::Update { all, dry_run, conflict } => {
            if all {
                registries::update_all(ctx, None, dry_run, conflict)
            } else {
                let cfg = ctx.config()?;
                let lib = Library::new(&cfg.library_path);
                repos::update(ctx, &cfg, &lib, None, dry_run, conflict, false)
            }
        }
        Cmd::Status { path } => {
            // Inside a registered repo: repo status. Otherwise: registry status.
            let cwd = std::env::current_dir().map_err(|e| Error::io(".", e))?;
            let cfg = ctx.config()?;
            let reg = Registry::load(&cfg.registry_path)?;
            let here = match &path {
                Some(p) => p.clone(),
                None => cwd,
            };
            if reg.repo(&here).is_some() {
                repos::status(ctx, path)
            } else {
                let lib = Library::new(&cfg.library_path);
                registries::status(ctx, &cfg, &lib, &reg)
            }
        }
    }
}

/// The effective conflict policy for a run: flag > config > interactivity,
/// with a printed notice when ask degrades to skip.
pub fn effective_policy(ctx: &Ctx, cfg: &Config, flag: Option<ConflictPolicy>) -> (ConflictPolicy, bool) {
    let interactive = Ui::interactive_stdin();
    let (policy, degraded) = reconcile::resolve_policy(flag, cfg.conflict_policy, interactive);
    if degraded {
        ctx.ui.warn(
            "no interactive terminal and no explicit --conflict policy: conflicts will be \
             kept local (use --conflict replace|promote|skip|abort to be explicit)",
        );
    }
    (policy, degraded)
}

/// What the enabled profiles of a repo want, cross-checked with the library.
pub struct Desired {
    /// (skill id, current library fingerprint). An empty fingerprint is
    /// the sentinel for "referenced by a profile but missing in the
    /// library"; the planner turns those into warnings.
    pub skills: Vec<(String, String)>,
    /// skill id -> profiles that bring it
    pub bringers: BTreeMap<String, Vec<String>>,
    /// enabled profile names that do not exist in the library
    pub unknown_profiles: Vec<String>,
}

pub fn resolve_desired(lib: &Library, enabled: &[String]) -> Result<Desired> {
    let mut bringers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut unknown_profiles = Vec::new();

    for pname in enabled {
        match lib.load_profile(pname) {
            Ok(profile) => {
                for s in &profile.skills {
                    bringers.entry(s.clone()).or_default().push(profile.name.clone());
                }
            }
            Err(e) => {
                if lib.profile_path(pname).is_file() {
                    // The file is there but does not parse: a hard error,
                    // never a silent downgrade. Treating a broken profile
                    // as "gone" would plan to remove everything it
                    // installed and exit 0.
                    return Err(e);
                }
                unknown_profiles.push(pname.clone());
            }
        }
    }

    // An empty fingerprint marks skills the library does not have.
    let mut skills = Vec::new();
    for id in bringers.keys() {
        let fp = if lib.has_skill(id) { lib.fingerprint(id)? } else { String::new() };
        skills.push((id.clone(), fp));
    }

    Ok(Desired { skills, bringers, unknown_profiles })
}

/// Locate the repo record for a path argument (default: cwd).
pub fn repo_path_arg(path: Option<&Path>) -> Result<PathBuf> {
    match path {
        Some(p) => Ok(p.to_path_buf()),
        None => std::env::current_dir().map_err(|e| Error::io(".", &e)),
    }
}

/// Run reconciliation for one registered repo: print the plan, apply it
/// (unless dry-run), and persist the new registry records. Returns the
/// summary.
pub fn reconcile_one(
    ctx: &Ctx,
    cfg: &Config,
    lib: &Library,
    registry: &mut Registry,
    repo_path: &Path,
    dry_run: bool,
    conflict: Option<ConflictPolicy>,
) -> Result<Summary> {
    let rec_path = repo_path.to_path_buf();
    let rec = registry
        .repo(&rec_path)
        .cloned()
        .ok_or_else(|| {
            Error::msg(format!(
                "{} is not registered — run `beskar repo add {}` first",
                util::display_path(&rec_path),
                util::display_path(&rec_path)
            ))
        })?;

    let (policy, _degraded) = effective_policy(ctx, cfg, conflict);
    let desired = resolve_desired(lib, &rec.profiles)?;

    let skills_dir = rec.path.join(&cfg.agent_skills_dir);
    let workspace = Workspace::scan(&skills_dir)?;

    // Warn about enabled profiles that no longer exist; plan() warns about
    // wanted-but-missing skills itself.
    let mut actions: Vec<reconcile::Action> = desired
        .unknown_profiles
        .iter()
        .map(|p| reconcile::Action::Warn {
            skill: format!("profile {p}"),
            msg: "profile not found in library".to_string(),
        })
        .collect();
    actions.extend(reconcile::plan(&desired.skills, &rec.installed, &workspace));

    ctx.ui.header(&format!(
        "Repository: {}",
        util::display_path(&rec.path)
    ));

    let mut applier = Applier {
        library: lib,
        skills_dir: skills_dir.clone(),
        ui: &ctx.ui,
        policy,
        dry_run,
        promoted: BTreeMap::new(),
    };
    let (records, summary) = applier.apply(&actions, &rec)?;

    // On-disk directories that beskar neither installed nor wants.
    let known: Vec<&str> = desired
        .skills
        .iter()
        .map(|(id, _)| id.as_str())
        .chain(rec.installed.iter().map(|r| r.id.as_str()))
        .collect();
    let untracked: Vec<&String> = workspace
        .fingerprints
        .keys()
        .filter(|k| !known.contains(&k.as_str()))
        .collect();
    if !untracked.is_empty() {
        let list = untracked
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        ctx.ui.warn(&format!(
            "untracked directories in {}: {list} (left alone; remove or add them to a profile)",
            util::display_path(&skills_dir)
        ));
    }

    if !dry_run {
        if let Some(rec) = registry.repo_mut(&rec_path) {
            rec.installed = records;
            if !summary.failed {
                rec.last_sync = Some(util::now_iso());
            }
        }
        registry.save()?;
    }

    print_summary(ctx, &summary, dry_run);
    Ok(summary)
}

pub fn print_summary(ctx: &Ctx, s: &Summary, dry_run: bool) {
    if dry_run {
        ctx.ui.note("No files changed (dry run).");
        return;
    }
    if s.quiet() && s.warnings == 0 {
        ctx.ui.ok("already up to date");
        return;
    }
    let mut parts: Vec<String> = Vec::new();
    if s.installed > 0 {
        parts.push(format!("{} installed", s.installed));
    }
    if s.adopted > 0 {
        parts.push(format!("{} adopted", s.adopted));
    }
    if s.updated > 0 {
        parts.push(format!("{} updated", s.updated));
    }
    if s.removed > 0 {
        parts.push(format!("{} removed", s.removed));
    }
    if s.kept > 0 {
        parts.push(format!("{} unchanged", s.kept));
    }
    if s.conflicts > 0 {
        parts.push(ctx.ui.yellow(&format!("{} conflicts kept local", s.conflicts)));
    }
    if s.warnings > 0 {
        parts.push(ctx.ui.yellow(&format!("{} warnings", s.warnings)));
    }
    if parts.is_empty() {
        ctx.ui.ok("done");
    } else {
        println!("{}", parts.join(", "));
    }
}

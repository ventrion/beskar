//! `beskar repo ...` — deployment targets.

use std::path::PathBuf;

use crate::cli::RepoCmd;
use crate::config::{Config, ConflictPolicy};
use crate::error::{Error, Exit, Result};
use crate::library::Library;
use crate::registry::Registry;
use crate::reconcile::{self, SkillStatus, Workspace};
use crate::util;

use super::{reconcile_one, resolve_desired, repo_path_arg, Ctx};

pub fn run(ctx: &Ctx, cmd: RepoCmd) -> Result<Exit> {
    let cfg = ctx.config()?;
    let lib = Library::new(&cfg.library_path);
    let mut registry = Registry::load(&cfg.registry_path)?;
    match cmd {
        RepoCmd::Add { path } => add(ctx, &cfg, &mut registry, path),
        RepoCmd::Remove { path, purge } => remove(ctx, &cfg, &mut registry, path, purge),
        RepoCmd::List => list(ctx, &registry),
        RepoCmd::Status { path } => status(ctx, path),
        RepoCmd::Enable { profile, path } => {
            toggle_profile(ctx, &lib, &mut registry, path, profile, Toggle::Enable)
        }
        RepoCmd::Disable { profile, path } => {
            toggle_profile(ctx, &lib, &mut registry, path, profile, Toggle::Disable)
        }
        RepoCmd::Toggle { profile, path } => {
            toggle_profile(ctx, &lib, &mut registry, path, profile, Toggle::Flip)
        }
        RepoCmd::Update { path, all, dry_run, conflict } => {
            update(ctx, &cfg, &lib, path, dry_run, conflict, all)
        }
    }
}

fn add(ctx: &Ctx, cfg: &Config, registry: &mut Registry, path: Option<PathBuf>) -> Result<Exit> {
    let target = repo_path_arg(path.as_deref())?;
    if util::is_subpath(&target, &cfg.home) {
        return Err(Error::msg(format!(
            "refusing to register a repo inside the beskar home ({})",
            util::display_path(&cfg.home)
        )));
    }
    if !target.is_dir() {
        return Err(Error::msg(format!(
            "{} is not a directory — repos are filesystem workspaces",
            util::display_path(&target)
        )));
    }
    match registry.add_repo(&target)? {
        true => {
            registry.save()?;
            ctx.ui.ok(&format!(
                "registered {} — enable profiles with `beskar repo enable <profile>`, \
                 then `beskar repo update`",
                util::display_path(registry.repo(&target).expect("just added").path.as_path())
            ));
        }
        false => {
            ctx.ui.note(&format!(
                "{} is already registered",
                util::display_path(registry.repo(&target).expect("already present").path.as_path())
            ));
        }
    }
    Ok(Exit::Ok)
}

fn remove(
    ctx: &Ctx,
    cfg: &Config,
    registry: &mut Registry,
    path: Option<PathBuf>,
    purge: bool,
) -> Result<Exit> {
    let target = repo_path_arg(path.as_deref())?;
    let rec = registry
        .repo(&target)
        .cloned()
        .ok_or_else(|| Error::msg(format!("{} is not registered", util::display_path(&target))))?;

    let confirmed = if ctx.assume_yes() {
        true
    } else if crate::ui::Ui::interactive_stdin() {
        let note = if purge { " and remove its managed skills" } else { " (installed files are kept)" };
        ctx.ui.confirm(&format!("Forget {}{note}?", util::display_path(&rec.path)), false)?
    } else {
        true
    };
    if !confirmed {
        ctx.ui.note("cancelled");
        return Ok(Exit::Ok);
    }

    if purge {
        let skills_dir = rec.path.join(&cfg.agent_skills_dir);
        let workspace = Workspace::scan(&skills_dir)?;
        let mut removed = 0usize;
        for inst in &rec.installed {
            let Some(ws_fp) = workspace.fingerprints.get(&inst.id) else {
                continue; // nothing on disk
            };
            if ws_fp == &inst.installed_fingerprint {
                util::remove_tree(&skills_dir.join(&inst.id))?;
                removed += 1;
            } else {
                ctx.ui.warn(&format!(
                    "kept `{}` — locally modified (remove it yourself if you mean it)",
                    inst.id
                ));
            }
        }
        if removed > 0 {
            ctx.ui.note(&format!(
                "removed {removed} managed skill director{y} from the workspace",
                y = if removed == 1 { "y" } else { "ies" }
            ));
        }
        // Remove the skills dir when nothing is left in it.
        if skills_dir.is_dir()
            && std::fs::read_dir(&skills_dir).map(|mut d| d.next().is_none()).unwrap_or(false)
        {
            std::fs::remove_dir(&skills_dir).ok();
        }
    }

    registry.repos.retain(|r| r.path != rec.path);
    registry.save()?;
    ctx.ui.ok(&format!("unregistered {}", util::display_path(&rec.path)));
    Ok(Exit::Ok)
}

pub fn list(ctx: &Ctx, registry: &Registry) -> Result<Exit> {
    if registry.repos.is_empty() {
        ctx.ui.note("no repositories registered — `beskar repo add [path]`");
        return Ok(Exit::Ok);
    }
    let mut repos: Vec<&crate::registry::RepoRecord> = registry.repos.iter().collect();
    repos.sort_by_key(|r| r.path.to_string_lossy().into_owned());
    println!("{:<52} {}", ctx.ui.bold("REPOSITORY"), ctx.ui.bold("PROFILES"));
    for r in repos {
        let profiles = if r.profiles.is_empty() {
            ctx.ui.dim("(none)")
        } else {
            r.profiles.join(", ")
        };
        println!("{:<52} {}", util::display_path(&r.path), profiles);
    }
    Ok(Exit::Ok)
}

pub fn status(ctx: &Ctx, path: Option<PathBuf>) -> Result<Exit> {
    let cfg = ctx.config()?;
    let lib = Library::new(&cfg.library_path);
    let registry = Registry::load(&cfg.registry_path)?;
    let target = repo_path_arg(path.as_deref())?;
    let rec = registry
        .repo(&target)
        .cloned()
        .ok_or_else(|| Error::msg(format!("{} is not registered", util::display_path(&target))))?;

    let desired = resolve_desired(&lib, &rec.profiles)?;
    let skills_dir = rec.path.join(&cfg.agent_skills_dir);
    let workspace = Workspace::scan(&skills_dir)?;

    ctx.ui.header(&format!("Repository: {}", util::display_path(&rec.path)));
    if !rec.profiles.is_empty() {
        ctx.ui.note(&format!("profiles: {}", rec.profiles.join(", ")));
    }
    match &rec.last_sync {
        Some(ts) => ctx.ui.note(&format!("last sync: {} ({})", ts, util::rel_time(ts, util::now_epoch()))),
        None => ctx.ui.note("never synchronized — run `beskar repo update`"),
    }
    println!();

    // All ids of interest: installed, on disk, or desired.
    let mut ids: Vec<String> = Vec::new();
    for r in &rec.installed {
        ids.push(r.id.clone());
    }
    for k in workspace.fingerprints.keys() {
        if !ids.contains(k) {
            ids.push(k.clone());
        }
    }
    for (id, _) in &desired.skills {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    ids.sort();

    let mut pending = 0usize;
    let mut conflicts = 0usize;
    if ids.is_empty() {
        ctx.ui.note("no skills installed and none wanted");
    }
    for id in &ids {
        let lib_fp = if lib.has_skill(id) { Some(lib.fingerprint(id)?) } else { None };
        let ws_fp = workspace.fingerprints.get(id).cloned();
        let rec_i = rec.installed.iter().find(|r| &r.id == id);
        let status = reconcile::classify(rec_i, lib_fp, ws_fp);
        let bringers = desired
            .bringers
            .get(id)
            .map(|v| format!(" [via {}]", v.join(", ")))
            .unwrap_or_default();
        let extra = match status {
            SkillStatus::Clean => "",
            SkillStatus::LibraryChanged => {
                pending += 1;
                ""
            }
            SkillStatus::LocalDrift => "",
            SkillStatus::BothChanged => {
                conflicts += 1;
                " (library changed + local modifications)"
            }
            SkillStatus::MissingOnDisk => {
                pending += 1;
                " (reinstalled on update)"
            }
            SkillStatus::MissingLibrary => "",
            SkillStatus::Untracked => "",
        };
        let sym = status.symbol();
        println!(" {} {:<28} {}{}", ctx.ui.symbol(sym), ctx.ui.bold(id), status.label(), ctx.ui.dim(&(extra.to_string() + &bringers)));
    }

    // Desired but not yet installed (plan would install).
    let not_installed: Vec<&String> = desired
        .skills
        .iter()
        .map(|(id, _)| id)
        .filter(|id| {
            lib.has_skill(id)
                && !rec.installed.iter().any(|r| &r.id == *id)
                && !workspace.fingerprints.contains_key(*id)
        })
        .collect();
    if !not_installed.is_empty() {
        println!();
        for id in not_installed {
            pending += 1;
            println!(" {} {:<28} {}", ctx.ui.symbol("+"), ctx.ui.bold(id), "wanted, not installed");
        }
    }

    println!();
    if pending == 0 && conflicts == 0 {
        ctx.ui.ok("in sync");
    } else {
        let bits = [
            (pending > 0).then(|| format!("{pending} pending")),
            (conflicts > 0).then(|| format!("{conflicts} conflict(s)")),
        ];
        let text: Vec<String> = bits.into_iter().flatten().collect();
        ctx.ui.warn(&format!(
            "{} — run `beskar repo update{}` to reconcile",
            text.join(", "),
            if conflicts > 0 { " --conflict <policy>" } else { "" }
        ));
    }
    Ok(Exit::Ok)
}

enum Toggle {
    Enable,
    Disable,
    Flip,
}

fn toggle_profile(
    ctx: &Ctx,
    lib: &Library,
    registry: &mut Registry,
    path: Option<PathBuf>,
    profile: String,
    mode: Toggle,
) -> Result<Exit> {
    let target = repo_path_arg(path.as_deref())?;
    let canonical = std::fs::canonicalize(&target)
        .map_err(|e| Error::msg(format!("{}: {e} (is the repo directory still there?)", util::display_path(&target))))?;
    let rec = registry
        .repo(&canonical)
        .cloned()
        .ok_or_else(|| Error::msg(format!("{} is not registered — `beskar repo add` first", util::display_path(&canonical))))?;

    let currently = rec.has_profile(&profile);
    let enable = match mode {
        Toggle::Enable => true,
        Toggle::Disable => false,
        Toggle::Flip => !currently,
    };

    if enable {
        if !lib.profile_path(&profile).is_file() {
            let available: Vec<String> =
                lib.list_profiles()?.iter().map(|p| p.name.clone()).collect();
            return Err(Error::msg(format!(
                "no profile `{profile}` in the library{}",
                if available.is_empty() {
                    " (library has no profiles yet)".to_string()
                } else {
                    format!(" — available: {}", available.join(", "))
                }
            )));
        }
        if currently {
            ctx.ui.note(&format!("`{profile}` is already enabled here"));
            return Ok(Exit::Ok);
        }
        registry.repo_mut(&canonical).expect("checked above").profiles.push(profile.clone());
        registry.save()?;
        ctx.ui.ok(&format!(
            "enabled `{profile}` for {} — run `beskar repo update` to materialize",
            util::display_path(&canonical)
        ));
    } else {
        if !currently {
            return Err(Error::msg(format!(
                "`{profile}` is not enabled here (enabled: {})",
                if rec.profiles.is_empty() { "none".to_string() } else { rec.profiles.join(", ") }
            )));
        }
        let rec_mut = registry.repo_mut(&canonical).expect("checked above");
        rec_mut.profiles.retain(|p| p != &profile);
        registry.save()?;
        ctx.ui.ok(&format!(
            "disabled `{profile}` for {} — run `beskar repo update` to remove its skills",
            util::display_path(&canonical)
        ));
    }
    Ok(Exit::Ok)
}

#[allow(clippy::too_many_arguments)]
pub fn update(
    ctx: &Ctx,
    cfg: &Config,
    lib: &Library,
    path: Option<PathBuf>,
    dry_run: bool,
    conflict: Option<ConflictPolicy>,
    all: bool,
) -> Result<Exit> {
    if all {
        return super::registries::update_all(ctx, None, dry_run, conflict);
    }
    let target = repo_path_arg(path.as_deref())?;
    let mut registry = Registry::load(&cfg.registry_path)?;
    let summary = reconcile_one(ctx, cfg, lib, &mut registry, &target, dry_run, conflict)?;
    if summary.failed || summary.conflicts > 0 {
        return Ok(Exit::Failed);
    }
    Ok(Exit::Ok)
}

//! `beskar registry ...` — machine-local deployment state, observability,
//! and fleet-wide updates.

use std::path::PathBuf;

use crate::cli::RegCmd;
use crate::config::{Config, ConflictPolicy};
use crate::error::{Error, Exit, Result};
use crate::library::Library;
use crate::registry::Registry;
use crate::reconcile::{self, SkillStatus, Workspace};
use crate::util;

use super::{reconcile_one, Ctx};

pub fn run(ctx: &Ctx, cmd: RegCmd) -> Result<Exit> {
    let cfg = ctx.config()?;
    let lib = Library::new(&cfg.library_path);
    let mut registry = Registry::load(&cfg.registry_path)?;
    match cmd {
        RegCmd::List => list(ctx, &registry),
        RegCmd::Status => status(ctx, &cfg, &lib, &registry),
        RegCmd::Stats => stats(ctx, &lib, &registry),
        RegCmd::Update { path, dry_run, conflict } => update_all(ctx, path, dry_run, conflict),
        RegCmd::Prune { assumed_yes } => prune(ctx, &mut registry, assumed_yes),
    }
}

fn list(ctx: &Ctx, registry: &Registry) -> Result<Exit> {
    super::repos::list(ctx, registry)
}

pub fn status(ctx: &Ctx, cfg: &Config, lib: &Library, registry: &Registry) -> Result<Exit> {
    if registry.repos.is_empty() {
        ctx.ui.note("no repositories registered");
        return Ok(Exit::Ok);
    }
    let mut paths: Vec<&crate::registry::RepoRecord> = registry.repos.iter().collect();
    paths.sort_by_key(|r| r.path.to_string_lossy().into_owned());

    let mut any_problem = false;
    for rec in paths {
        if !rec.path.is_dir() {
            ctx.ui.fail(&format!(
                "{} — path missing (consider `beskar registry prune`)",
                util::display_path(&rec.path)
            ));
            any_problem = true;
            continue;
        }
        let skills_dir = rec.path.join(&cfg.agent_skills_dir);
        let workspace = Workspace::scan(&skills_dir)?;
        let desired = super::resolve_desired(lib, &rec.profiles)?;
        let desired_ids: Vec<String> = desired.skills.iter().map(|(id, _)| id.clone()).collect();

        let mut clean = 0;
        let mut pending = 0;
        let mut drifted = 0;
        let mut missing = 0;
        for inst in &rec.installed {
            let lib_fp = if lib.has_skill(&inst.id) {
                Some(lib.fingerprint(&inst.id)?)
            } else {
                None
            };
            match reconcile::classify(
                Some(inst),
                lib_fp,
                workspace.fingerprints.get(&inst.id).cloned(),
            ) {
                SkillStatus::Clean => clean += 1,
                SkillStatus::LibraryChanged | SkillStatus::MissingOnDisk => pending += 1,
                SkillStatus::LocalDrift | SkillStatus::BothChanged => drifted += 1,
                SkillStatus::MissingLibrary => missing += 1,
                SkillStatus::Untracked => {}
            }
        }
        // Wanted but not yet present (missing-from-library skills are
        // reported separately, not as pending).
        let not_installed = desired_ids
            .iter()
            .filter(|id| {
                lib.has_skill(id)
                    && !rec.installed.iter().any(|r| &r.id == *id)
                    && !workspace.fingerprints.contains_key(*id)
            })
            .count();
        pending += not_installed;

        let unknown = desired.unknown_profiles.len();
        let mut bits: Vec<String> = Vec::new();
        bits.push(format!("{clean} clean"));
        if pending > 0 {
            bits.push(ctx.ui.yellow(&format!("{pending} pending")));
        }
        if drifted > 0 {
            bits.push(ctx.ui.yellow(&format!("{drifted} with local edits")));
        }
        if missing > 0 {
            bits.push(ctx.ui.red(&format!("{missing} missing in library")));
        }
        if unknown > 0 {
            bits.push(ctx.ui.red(&format!("{unknown} unknown profile(s)")));
        }
        if pending + drifted + missing + unknown > 0 {
            any_problem = true;
        }
        println!("{} — {}", util::display_path(&rec.path), bits.join(", "));
    }
    if !any_problem {
        ctx.ui.ok("all repositories in sync");
    } else {
        ctx.ui.note("run `beskar registry update --all` to reconcile pending repos");
    }
    Ok(Exit::Ok)
}

fn stats(ctx: &Ctx, lib: &Library, registry: &Registry) -> Result<Exit> {
    let skills = lib.list_skills()?;
    let profiles = lib.list_profiles().unwrap_or_default();
    let installed: usize = registry.repos.iter().map(|r| r.installed.len()).sum();
    let referenced: Vec<String> = profiles.iter().flat_map(|p| p.skills.iter().cloned()).collect();
    let unused = skills.iter().filter(|s| !referenced.contains(&s.id)).count();

    println!("{:<18}{}", ctx.ui.bold("Repositories"), registry.repos.len());
    println!("{:<18}{}", ctx.ui.bold("Profiles"), profiles.len());
    println!("{:<18}{}", ctx.ui.bold("Library skills"), skills.len());
    println!("{:<18}{}", ctx.ui.bold("Installed skills"), installed);
    println!("{:<18}{}", ctx.ui.bold("Unused skills"), unused);

    if unused > 0 {
        let names: Vec<String> = skills
            .iter()
            .filter(|s| !referenced.contains(&s.id))
            .map(|s| s.id.clone())
            .collect();
        ctx.ui.note(&format!("not referenced by any profile: {}", names.join(", ")));
    }
    Ok(Exit::Ok)
}

/// Reconcile every registered repo (or all repos under `--all` forms).
/// `path` restricts the run to a single registered repo.
pub fn update_all(
    ctx: &Ctx,
    path: Option<PathBuf>,
    dry_run: bool,
    conflict: Option<ConflictPolicy>,
) -> Result<Exit> {
    let cfg = ctx.config()?;
    let lib = Library::new(&cfg.library_path);
    let mut registry = Registry::load(&cfg.registry_path)?;

    if registry.repos.is_empty() {
        ctx.ui.note("no repositories registered — nothing to update");
        return Ok(Exit::Ok);
    }

    let mut targets: Vec<PathBuf> = registry.repos.iter().map(|r| r.path.clone()).collect();
    targets.sort();
    if let Some(p) = &path {
        let canonical = std::fs::canonicalize(p)
            .map_err(|e| Error::msg(format!("{}: {e}", util::display_path(p))))?;
        if registry.repo(&canonical).is_none() {
            return Err(Error::msg(format!("{} is not registered", util::display_path(&canonical))));
        }
        targets.retain(|t| t == &canonical);
    }

    let mut failed = 0usize;
    let mut conflicted = 0usize;
    for (i, repo_path) in targets.iter().enumerate() {
        if i > 0 {
            println!();
        }
        if !repo_path.is_dir() {
            ctx.ui.fail(&format!(
                "Repository: {} — path missing, skipped (consider `beskar registry prune`)",
                util::display_path(repo_path)
            ));
            continue;
        }
        let summary = match reconcile_one(ctx, &cfg, &lib, &mut registry, repo_path, dry_run, conflict) {
            Ok(s) => s,
            Err(e) => {
                ctx.ui.fail(&format!("failed: {e}"));
                failed += 1;
                continue;
            }
        };
        if summary.failed || summary.conflicts > 0 {
            conflicted += 1;
        }
    }

    println!();
    let total = targets.len();
    let text = format!(
        "{} repositor{} processed",
        total,
        if total == 1 { "y" } else { "ies" }
    );
    if failed == 0 && conflicted == 0 {
        ctx.ui.ok(&text);
        Ok(Exit::Ok)
    } else {
        let mut bits: Vec<String> = Vec::new();
        if failed > 0 {
            bits.push(format!("{failed} failed"));
        }
        if conflicted > 0 {
            bits.push(format!("{conflicted} with unresolved conflicts"));
        }
        ctx.ui.warn(&format!("{text} — {}", bits.join(", ")));
        Ok(Exit::Failed)
    }
}

fn prune(ctx: &Ctx, registry: &mut Registry, assumed_yes: bool) -> Result<Exit> {
    let gone: Vec<PathBuf> = registry
        .repos
        .iter()
        .filter(|r| !r.path.is_dir())
        .map(|r| r.path.clone())
        .collect();
    if gone.is_empty() {
        ctx.ui.ok("nothing to prune — every registered path exists");
        return Ok(Exit::Ok);
    }
    ctx.ui.header(&format!("{} registered path(s) no longer exist:", gone.len()));
    for g in &gone {
        println!("  {}", util::display_path(g));
    }
    let confirmed = if assumed_yes || ctx.assume_yes() {
        true
    } else if crate::ui::Ui::interactive_stdin() {
        ctx.ui.confirm("Forget these repositories from the registry?", true)?
    } else {
        ctx.ui.warn("non-interactive without --yes: nothing removed");
        false
    };
    if !confirmed {
        ctx.ui.note("prune cancelled");
        return Ok(Exit::Ok);
    }
    registry.repos.retain(|r| r.path.is_dir());
    registry.save()?;
    ctx.ui.ok(&format!("pruned {} repositor{}", gone.len(), if gone.len() == 1 { "y" } else { "ies" }));
    Ok(Exit::Ok)
}

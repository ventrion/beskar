//! `beskar doctor` — verify the whole deployment picture.
//!
//! Every check prints a line; problems (things that make reconciliation
//! wrong) count toward the exit code, warnings (drift, orphans) do not.

use crate::error::{Exit, Result};
use crate::library::Library;
use crate::registry::Registry;
use crate::reconcile::{self, Workspace};
use crate::util;

use super::Ctx;

pub fn run(ctx: &Ctx) -> Result<Exit> {
    let cfg = match ctx.config() {
        Ok(cfg) => cfg,
        Err(e) => {
            ctx.ui.fail(&e.to_string());
            return Ok(Exit::Failed);
        }
    };

    let mut problems = 0usize;
    let mut warnings = 0usize;
    println!(
        "{}",
        ctx.ui.bold(&format!("beskar doctor — {}", util::display_path(&cfg.home)))
    );
    println!();

    // -- config ----------------------------------------------------------
    println!(
        "{} config      {}",
        ctx.ui.green("✓"),
        util::display_path(&cfg.path)
    );
    println!(
        "  library     {}",
        util::display_path(&cfg.library_path)
    );
    println!(
        "  registry    {}",
        util::display_path(&cfg.registry_path)
    );
    println!(
        "  skills dir  <repo>/{}",
        cfg.agent_skills_dir
    );

    // -- library ----------------------------------------------------------
    println!();
    println!("{}", ctx.ui.bold("Library"));
    let lib = Library::new(&cfg.library_path);
    if !lib.root.is_dir() {
        ctx.ui.fail(&format!("library directory missing: {}", util::display_path(&lib.root)));
        problems += 1;
        println!();
        println!(
            "{}",
            ctx.ui.yellow(&format!(
                "{problems} problem(s), {warnings} warning(s)"
            ))
        );
        return Ok(Exit::Failed);
    }
    lib.init_dirs()?;
    let skills = lib.list_skills()?;
    let profiles = match lib.list_profiles() {
        Ok(p) => p,
        Err(e) => {
            ctx.ui.fail(&format!("profiles unreadable: {e}"));
            problems += 1;
            Vec::new()
        }
    };
    ctx.ui.ok(&format!(
        "{} skills, {} profiles in {}",
        skills.len(),
        profiles.len(),
        util::display_path(&lib.root)
    ));
    let known_profiles: Vec<String> = profiles.iter().map(|p| p.name.clone()).collect();
    let known_skills: Vec<String> = skills.iter().map(|s| s.id.clone()).collect();

    // Dangling profile -> skill references.
    for p in &profiles {
        for s in &p.skills {
            if !known_skills.contains(s) {
                ctx.ui.warn(&format!(
                    "profile `{}` references missing skill `{s}`",
                    p.name
                ));
                warnings += 1;
            }
        }
        if p.skills.is_empty() {
            ctx.ui.warn(&format!("profile `{}` has no skills", p.name));
            warnings += 1;
        }
    }

    // -- registry & repos ---------------------------------------------------
    println!();
    println!("{}", ctx.ui.bold("Registry"));
    let registry = match Registry::load(&cfg.registry_path) {
        Ok(r) => r,
        Err(e) => {
            ctx.ui.fail(&format!("registry unreadable: {e}"));
            problems += 1;
            println!();
            println!(
                "{}",
                ctx.ui.yellow(&format!("{problems} problem(s), {warnings} warning(s)"))
            );
            return Ok(Exit::Failed);
        }
    };
    if registry.repos.is_empty() {
        ctx.ui.note("no repositories registered yet");
    }
    let repo_paths: Vec<std::path::PathBuf> = registry.repos.iter().map(|r| r.path.clone()).collect();

    for path in &repo_paths {
        println!();
        println!("{}", ctx.ui.cyan(&util::display_path(path)));
        let rec = registry.repo(path).cloned().expect("path from registry");
        if !path.is_dir() {
            ctx.ui.fail("path does not exist (consider `beskar registry prune`)");
            problems += 1;
            continue;
        }

        // Enabled profiles exist?
        for p in &rec.profiles {
            if known_profiles.contains(p) {
                ctx.ui.ok(&format!("profile `{p}` enabled"));
            } else {
                ctx.ui.fail(&format!(
                    "profile `{p}` enabled but missing from library"
                ));
                problems += 1;
            }
        }
        if rec.profiles.is_empty() {
            ctx.ui.warn("no profiles enabled");
            warnings += 1;
        }

        // Installed vs library vs workspace.
        let skills_dir = path.join(&cfg.agent_skills_dir);
        let workspace = match Workspace::scan(&skills_dir) {
            Ok(w) => w,
            Err(e) => {
                ctx.ui.fail(&format!("workspace scan failed: {e}"));
                problems += 1;
                continue;
            }
        };
        for inst in &rec.installed {
            let lib_fp = if lib.has_skill(&inst.id) {
                Some(lib.fingerprint(&inst.id)?)
            } else {
                None
            };
            let ws_fp = workspace.fingerprints.get(&inst.id).cloned();
            let status = reconcile::classify(Some(inst), lib_fp, ws_fp);
            match status {
                reconcile::SkillStatus::Clean => {
                    ctx.ui.ok(&format!("{} clean", inst.id));
                }
                reconcile::SkillStatus::LibraryChanged => {
                    ctx.ui.warn(&format!(
                        "{} library changed — run `beskar repo update`",
                        inst.id
                    ));
                    warnings += 1;
                }
                reconcile::SkillStatus::LocalDrift => {
                    ctx.ui.warn(&format!(
                        "{} has local modifications (promote or replace before updating)",
                        inst.id
                    ));
                    warnings += 1;
                }
                reconcile::SkillStatus::BothChanged => {
                    ctx.ui.fail(&format!(
                        "{} changed in the library AND locally — resolve the conflict",
                        inst.id
                    ));
                    problems += 1;
                }
                reconcile::SkillStatus::MissingOnDisk => {
                    ctx.ui.warn(&format!(
                        "{} registered but missing on disk — run `beskar repo update`",
                        inst.id
                    ));
                    warnings += 1;
                }
                reconcile::SkillStatus::MissingLibrary => {
                    ctx.ui.fail(&format!(
                        "{} installed but gone from the library",
                        inst.id
                    ));
                    problems += 1;
                }
                reconcile::SkillStatus::Untracked => unreachable!("installed ids are tracked"),
            }
        }

        // Untracked directories.
        let tracked: Vec<&str> = rec.installed.iter().map(|r| r.id.as_str()).collect();
        let mut untracked: Vec<&String> = workspace
            .fingerprints
            .keys()
            .filter(|k| !tracked.contains(&k.as_str()))
            .collect();
        untracked.sort();
        for u in untracked {
            ctx.ui.warn(&format!(
                "`{u}` sits in {} but is not managed by beskar",
                util::display_path(&skills_dir)
            ));
            warnings += 1;
        }
    }

    // Summary.
    println!();
    if problems == 0 && warnings == 0 {
        println!("{}", ctx.ui.green("All checks passed."));
    } else {
        let text = format!("{problems} problem(s), {warnings} warning(s)");
        println!("{}", if problems > 0 { ctx.ui.red(&text) } else { ctx.ui.yellow(&text) });
    }
    Ok(if problems > 0 { Exit::Failed } else { Exit::Ok })
}

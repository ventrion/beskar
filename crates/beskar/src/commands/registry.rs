//! `beskar registry ...` plus the `status` / `update --all` aliases.

use beskar_core::fsutil::display_path;
use beskar_core::reconcile::{self, Action};
use beskar_core::{insight, Beskar, Home, Result};

use super::{done, parse_cmd, policy_from, unknown_sub, update_repo};
use crate::args;
use crate::{help, ui, Outcome};

pub fn run(home: &Home, sub: Option<&str>, argv: &[String]) -> Result<Outcome> {
    match sub {
        Some("list") => list(home, argv, 2),
        Some("status") => status(home, argv, 2),
        Some("stats") => stats(home, argv),
        Some("update") => update(home, argv, 2),
        Some("prune") => prune(home, argv),
        other => unknown_sub("registry", other, help::REGISTRY),
    }
}

pub fn list(home: &Home, argv: &[String], skip: usize) -> Result<Outcome> {
    if let Err(o) = parse_cmd(argv, &[], skip, help::REGISTRY) {
        return Ok(o);
    }
    let beskar = Beskar::open(home.clone())?;
    let repos = beskar.registry.repos();
    if repos.is_empty() {
        println!("No repositories registered. Run `beskar repo add .` inside one.");
        return done();
    }
    let rows: Vec<Vec<String>> = repos
        .iter()
        .map(|r| {
            vec![
                display_path(&r.path),
                if r.profiles.is_empty() {
                    "(no profiles)".to_string()
                } else {
                    r.profiles.join(", ")
                },
                format!("{} installed", r.installed.len()),
                r.synced
                    .clone()
                    .unwrap_or_else(|| "never updated".to_string()),
            ]
        })
        .collect();
    print!("{}", ui::table(&rows));
    done()
}

pub fn status(home: &Home, argv: &[String], skip: usize) -> Result<Outcome> {
    if let Err(o) = parse_cmd(argv, &[], skip, help::REGISTRY) {
        return Ok(o);
    }
    let beskar = Beskar::open(home.clone())?;
    if beskar.registry.repos().is_empty() {
        println!("No repositories registered. Run `beskar repo add .` inside one.");
        return done();
    }
    let mut rows = Vec::new();
    let mut pending = 0;
    for repo in beskar.registry.repos() {
        let shown = display_path(&repo.path);
        if !repo.path.is_dir() {
            rows.push(vec![shown, "missing directory".to_string(), String::new()]);
            continue;
        }
        let plan = reconcile::plan(&beskar.library, repo, &beskar.config.skills_dir)?;
        let count =
            |f: &dyn Fn(&Action) -> bool| plan.items.iter().filter(|i| f(&i.action)).count();
        let mut parts = Vec::new();
        let add = count(&|a| matches!(a, Action::Add { .. }));
        let upd = count(&|a| matches!(a, Action::Update));
        let rem = count(&|a| matches!(a, Action::Remove | Action::Forget));
        let con = count(&|a| a.is_conflict());
        let modi = count(&|a| matches!(a, Action::Modified));
        let miss = count(&|a| matches!(a, Action::MissingInLibrary));
        let ok = count(&|a| matches!(a, Action::Unchanged));
        for (n, label) in [
            (add, "+"),
            (upd, "~"),
            (rem, "-"),
            (con, "!"),
            (modi, "M"),
            (miss, "?"),
        ] {
            if n > 0 {
                parts.push(format!("{label}{n}"));
            }
        }
        for p in &plan.missing_profiles {
            parts.push(format!("missing profile {p}"));
        }
        let summary = if parts.is_empty() {
            format!("in sync ({ok} skills)")
        } else {
            pending += 1;
            format!("{ok} ok, {}", parts.join(" "))
        };
        let profiles = if repo.profiles.is_empty() {
            "(no profiles)".to_string()
        } else {
            repo.profiles.join(", ")
        };
        rows.push(vec![shown, profiles, summary]);
    }
    print!("{}", ui::table(&rows));
    if pending > 0 {
        println!(
            "\n{pending} repositor{} attention; `beskar registry update` reconciles them all.",
            if pending == 1 { "y needs" } else { "ies need" }
        );
    }
    done()
}

fn stats(home: &Home, argv: &[String]) -> Result<Outcome> {
    if let Err(o) = parse_cmd(argv, &[], 2, help::REGISTRY) {
        return Ok(o);
    }
    let beskar = Beskar::open(home.clone())?;
    let s = insight::stats(&beskar.library, &beskar.registry)?;
    let rows = vec![
        vec!["Repositories".to_string(), s.repositories.to_string()],
        vec!["Profiles".to_string(), s.profiles.to_string()],
        vec!["Library skills".to_string(), s.library_skills.to_string()],
        vec![
            "Installed skills".to_string(),
            s.installed_skills.to_string(),
        ],
        vec![
            "Unused skills (in no profile)".to_string(),
            s.unused_skills.to_string(),
        ],
        vec![
            "Skills installed nowhere".to_string(),
            s.uninstalled_skills.to_string(),
        ],
    ];
    print!("{}", ui::table(&rows));
    done()
}

pub fn update(home: &Home, argv: &[String], skip: usize) -> Result<Outcome> {
    let parsed = match parse_cmd(
        argv,
        &[args::DRY_RUN, args::ON_CONFLICT, args::ALL, args::REPO],
        skip,
        help::REGISTRY,
    ) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    if !parsed.positional.is_empty() {
        return crate::usage(
            "registry update takes no positional arguments",
            help::REGISTRY,
        );
    }
    let mut beskar = Beskar::open(home.clone())?;
    let policy = policy_from(&parsed, &beskar)?;
    let dry_run = parsed.has(&args::DRY_RUN);
    let paths: Vec<std::path::PathBuf> = beskar
        .registry
        .repos()
        .iter()
        .map(|r| r.path.clone())
        .collect();
    if paths.is_empty() {
        println!("No repositories registered.");
        return done();
    }
    let mut failures = 0;
    let mut changed = 0;
    let mut promoted = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        if i > 0 {
            println!("\n{}", "-".repeat(60));
        }
        if !path.is_dir() {
            println!(
                "Repository: {}\n\nskipped: directory is missing (run `beskar registry prune`)",
                display_path(path)
            );
            continue;
        }
        match update_repo(&mut beskar, path, dry_run, policy) {
            Ok(summary) => {
                if summary.changed_files {
                    changed += 1;
                }
                promoted.extend(summary.promoted);
            }
            Err(e) => {
                failures += 1;
                println!("\nerror: {e}");
            }
        }
    }
    println!("\n{}", "=".repeat(60));
    println!(
        "{} repositories, {changed} changed, {failures} failed{}",
        paths.len(),
        if dry_run { " (dry run)" } else { "" }
    );
    if !promoted.is_empty() {
        println!(
            "promoted during this run: {}; run `beskar registry update` again to propagate",
            promoted.join(", ")
        );
    }
    Ok(if failures > 0 {
        Outcome::Failed
    } else {
        Outcome::Ok
    })
}

fn prune(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[args::DRY_RUN], 2, help::REGISTRY) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let mut beskar = Beskar::open(home.clone())?;
    let gone: Vec<std::path::PathBuf> = beskar
        .registry
        .repos()
        .iter()
        .filter(|r| !r.path.is_dir())
        .map(|r| r.path.clone())
        .collect();
    if gone.is_empty() {
        println!("Every registered repository exists; nothing to prune.");
        return done();
    }
    for p in &gone {
        println!(
            "{} {}",
            if parsed.has(&args::DRY_RUN) {
                "would forget"
            } else {
                "forgot"
            },
            display_path(p)
        );
        if !parsed.has(&args::DRY_RUN) {
            beskar.registry.remove(p);
        }
    }
    if !parsed.has(&args::DRY_RUN) {
        beskar.registry.save()?;
    }
    done()
}

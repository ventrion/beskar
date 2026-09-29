//! `beskar repo ...`

use std::path::Path;

use beskar_core::fsutil::{self, display_path};
use beskar_core::reconcile;
use beskar_core::{Beskar, Error, Home, Result};

use super::{done, parse_cmd, policy_from, print_plan, resolve_repo, unknown_sub, update_repo};
use crate::args;
use crate::{help, ui, Outcome};

pub fn run(home: &Home, sub: Option<&str>, argv: &[String]) -> Result<Outcome> {
    match sub {
        Some("add") => add(home, argv),
        Some("remove") => remove(home, argv),
        Some("list") => super::registry::list(home, argv, 2),
        Some("status") => status(home, argv),
        Some("enable") => toggle(home, argv, Some(true)),
        Some("disable") => toggle(home, argv, Some(false)),
        Some("toggle") => toggle(home, argv, None),
        Some("update") => update(home, argv, 2),
        Some("diff") => diff(home, argv),
        Some("promote") => promote(home, argv),
        other => unknown_sub("repo", other, help::REPO),
    }
}

fn add(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[], 2, help::REPO) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let path = match parsed.positional.as_slice() {
        [] => ".".to_string(),
        [p] => p.clone(),
        _ => return crate::usage("repo add takes at most one <path>", help::REPO),
    };
    let mut beskar = Beskar::open(home.clone())?;
    let abs = fsutil::absolute(Path::new(&path))?;
    if !abs.is_dir() {
        return Err(Error::not_found(format!(
            "{} is not a directory",
            display_path(&abs)
        )));
    }
    if let Some(existing) = beskar.registry.find_containing(&abs) {
        if existing.path != abs {
            println!(
                "note: {} is inside the registered repository {}",
                display_path(&abs),
                display_path(&existing.path)
            );
        }
    }
    beskar.registry.add(abs.clone())?;
    beskar.registry.save()?;
    println!("registered {}", display_path(&abs));
    println!("enable profiles with `beskar repo enable <profile>`, then `beskar repo update`");
    done()
}

fn remove(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[args::PURGE, args::YES], 2, help::REPO) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let explicit = match parsed.positional.as_slice() {
        [] => None,
        [p] => Some(p.as_str()),
        _ => return crate::usage("repo remove takes at most one <path>", help::REPO),
    };
    let mut beskar = Beskar::open(home.clone())?;
    let path = match explicit {
        // Allow forgetting a repository whose directory no longer exists.
        Some(p)
            if beskar
                .registry
                .get(&fsutil::absolute(Path::new(p))?)
                .is_some() =>
        {
            fsutil::absolute(Path::new(p))?
        }
        _ => resolve_repo(&beskar, &parsed, explicit)?,
    };
    let repo = beskar
        .registry
        .get(&path)
        .cloned()
        .expect("resolved repo is registered");
    if parsed.has(&args::PURGE) && !repo.installed.is_empty() {
        let plan = reconcile::plan(&beskar.library, &repo, &beskar.config.skills_dir)?;
        let modified: Vec<&str> = plan
            .items
            .iter()
            .filter(|i| i.recorded.is_some() && i.workspace.is_some() && i.recorded != i.workspace)
            .map(|i| i.skill.as_str())
            .collect();
        if !modified.is_empty() {
            println!("locally modified, will be deleted: {}", modified.join(", "));
        }
        let n = repo.installed.len();
        if !ui::confirm(
            &format!(
                "Delete {n} installed skill(s) from {}?",
                display_path(&path)
            ),
            false,
            parsed.has(&args::YES),
        )? {
            println!("Nothing changed.");
            return done();
        }
        for skill in repo.installed.keys() {
            let dir = plan.skills_dir.join(skill);
            if dir.is_dir() {
                fsutil::remove_dir(&dir)?;
                println!("deleted {}", display_path(&dir));
            }
        }
    }
    beskar.registry.remove(&path);
    beskar.registry.save()?;
    println!("unregistered {}", display_path(&path));
    if !parsed.has(&args::PURGE) && !repo.installed.is_empty() {
        println!("installed skills were left in place (use --purge to delete them)");
    }
    done()
}

fn status(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[args::REPO], 2, help::REPO) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let explicit = match parsed.positional.as_slice() {
        [] => None,
        [p] => Some(p.as_str()),
        _ => return crate::usage("repo status takes at most one <path>", help::REPO),
    };
    let beskar = Beskar::open(home.clone())?;
    let path = resolve_repo(&beskar, &parsed, explicit)?;
    let repo = beskar
        .registry
        .get(&path)
        .expect("resolved repo is registered");
    let plan = reconcile::plan(&beskar.library, repo, &beskar.config.skills_dir)?;
    print_plan(&plan, &repo.profiles, true);
    match &repo.synced {
        Some(t) => println!("\nLast update: {t}"),
        None => println!("\nNever updated."),
    }
    if plan.has_changes() {
        println!("Run `beskar repo update` to apply.");
    }
    done()
}

fn toggle(home: &Home, argv: &[String], enable: Option<bool>) -> Result<Outcome> {
    let verb = match enable {
        Some(true) => "enable",
        Some(false) => "disable",
        None => "toggle",
    };
    let parsed = match parse_cmd(argv, &[args::REPO], 2, help::REPO) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    if parsed.positional.is_empty() {
        return crate::usage(
            &format!("repo {verb} needs at least one <profile>"),
            help::REPO,
        );
    }
    let mut beskar = Beskar::open(home.clone())?;
    let path = resolve_repo(&beskar, &parsed, None)?;
    for name in &parsed.positional {
        if !beskar.library.has_profile(name) {
            return Err(Error::not_found(format!(
                "profile '{name}' does not exist (create it with `beskar profile create {name}`)"
            )));
        }
    }
    let repo = beskar
        .registry
        .get_mut(&path)
        .expect("resolved repo is registered");
    for name in &parsed.positional {
        let turn_on = enable.unwrap_or(!repo.has_profile(name));
        let changed = if turn_on {
            repo.enable(name)
        } else {
            repo.disable(name)
        };
        match (turn_on, changed) {
            (true, true) => println!("enabled {name}"),
            (true, false) => println!("{name} was already enabled"),
            (false, true) => println!("disabled {name}"),
            (false, false) => println!("{name} was not enabled"),
        }
    }
    let profiles = repo.profiles.clone();
    beskar.registry.save()?;
    println!(
        "{}: profiles = {}",
        display_path(&path),
        if profiles.is_empty() {
            "(none)".to_string()
        } else {
            profiles.join(", ")
        }
    );
    println!("run `beskar repo update` to apply");
    done()
}

/// `beskar repo update` and the `beskar update` alias (`skip` differs).
pub fn update(home: &Home, argv: &[String], skip: usize) -> Result<Outcome> {
    let parsed = match parse_cmd(
        argv,
        &[args::REPO, args::DRY_RUN, args::ON_CONFLICT, args::ALL],
        skip,
        help::REPO,
    ) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    if parsed.has(&args::ALL) {
        return super::registry::update(home, argv, skip);
    }
    let explicit = match parsed.positional.as_slice() {
        [] => None,
        [p] => Some(p.as_str()),
        _ => return crate::usage("repo update takes at most one <path>", help::REPO),
    };
    let mut beskar = Beskar::open(home.clone())?;
    let path = resolve_repo(&beskar, &parsed, explicit)?;
    let policy = policy_from(&parsed, &beskar)?;
    let summary = update_repo(&mut beskar, &path, parsed.has(&args::DRY_RUN), policy)?;
    if !summary.promoted.is_empty() {
        println!(
            "\nPromoted to the library: {}. Other repositories pick this up with `beskar registry update`.",
            summary.promoted.join(", ")
        );
    }
    done()
}

fn diff(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[args::REPO], 2, help::REPO) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let [skill] = parsed.positional.as_slice() else {
        return crate::usage("repo diff needs exactly one <skill>", help::REPO);
    };
    let beskar = Beskar::open(home.clone())?;
    let path = resolve_repo(&beskar, &parsed, None)?;
    let workspace = path.join(&beskar.config.skills_dir).join(skill);
    if !workspace.is_dir() {
        return Err(Error::not_found(format!(
            "skill '{skill}' is not installed in {}",
            display_path(&path)
        )));
    }
    let library = beskar.library.skill(skill)?;
    print!(
        "{}",
        beskar_core::diff::render(&library.path, &workspace, "library", "workspace")?
    );
    done()
}

fn promote(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[args::REPO, args::YES], 2, help::REPO) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let [skill] = parsed.positional.as_slice() else {
        return crate::usage("repo promote needs exactly one <skill>", help::REPO);
    };
    let mut beskar = Beskar::open(home.clone())?;
    let path = resolve_repo(&beskar, &parsed, None)?;
    let workspace = path.join(&beskar.config.skills_dir).join(skill);
    if !workspace.is_dir() {
        return Err(Error::not_found(format!(
            "skill '{skill}' is not installed in {}",
            display_path(&path)
        )));
    }
    if beskar.library.has_skill(skill) {
        let lib = beskar.library.skill_path(skill);
        let d = beskar_core::diff::dir_diff(&lib, &workspace)?;
        if d.is_empty() {
            println!("{skill}: the library already matches this workspace copy");
        } else {
            print!(
                "{}",
                beskar_core::diff::render(&lib, &workspace, "library", "workspace")?
            );
        }
        if !ui::confirm(
            &format!("Replace the library version of {skill} with this copy?"),
            false,
            parsed.has(&args::YES),
        )? {
            println!("Nothing changed.");
            return done();
        }
    } else {
        println!("{skill} is new to the library.");
    }
    let repo = beskar
        .registry
        .get_mut(&path)
        .expect("resolved repo is registered");
    let fp = reconcile::promote(&beskar.library, repo, &beskar.config.skills_dir, skill)?;
    beskar.registry.save()?;
    println!("promoted {skill} to the library ({})", fp.short());
    let others: Vec<String> = beskar
        .registry
        .repos_with_installed(skill)
        .iter()
        .filter(|r| r.path != path)
        .map(|r| display_path(&r.path))
        .collect();
    if !others.is_empty() {
        println!(
            "also installed in: {}\nrun `beskar registry update` to propagate",
            others.join(", ")
        );
    }
    done()
}

//! `beskar registry ...`

use std::collections::BTreeSet;
use std::path::PathBuf;

use beskar_core::{Result, SkillId, State};

use crate::args::Args;
use crate::ui::{self, Style, show};
use crate::update::{self, Options};
use crate::{Ctx, Exit};

pub fn list(ctx: &Ctx, _args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    println!("{} {}", ctx.ui.bold("Registry:"), show(b.registry.path()));
    if b.registry.is_empty() {
        println!("\nNo repositories registered.");
        return Ok(Exit::Ok);
    }
    for r in b.registry.repos() {
        println!();
        let missing = if r.path.is_dir() {
            String::new()
        } else {
            format!(" {}", ctx.ui.paint("(directory missing)", Style::Red))
        };
        println!("{}{missing}", ctx.ui.bold(&show(&r.path)));
        let installed: Vec<&str> = r.installed.keys().map(SkillId::as_str).collect();
        let mut rows = vec![
            vec!["  profiles".into(), if r.profiles.is_empty() { ctx.ui.dim("none") } else { r.profiles.join(", ") }],
            vec!["  installed".into(), if installed.is_empty() { ctx.ui.dim("none") } else { installed.join(", ") }],
        ];
        if let Some(dir) = &r.skills_dir {
            rows.push(vec!["  skills dir".into(), dir.display().to_string()]);
        }
        rows.push(vec!["  synced".into(), r.synced.clone().unwrap_or_else(|| ctx.ui.dim("never"))]);
        print!("{}", ui::table(&rows));
    }
    Ok(Exit::Ok)
}

pub fn status(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    if let Some(name) = args.value("profile") {
        b.library.profile(name)?;
        println!("{}", ctx.ui.bold(name));
        let users: Vec<_> = b.registry.using_profile(name).collect();
        if users.is_empty() {
            println!("  {}", ctx.ui.dim("not enabled anywhere"));
        }
        for r in users {
            println!("  {}", show(&r.path));
        }
        return Ok(Exit::Ok);
    }
    if let Some(raw) = args.value("skill") {
        let id = SkillId::new(raw)?;
        println!("{}", ctx.ui.bold(id.as_str()));
        let usage = b.skill_usage(&id)?;
        let mut rows = Vec::new();
        for (path, via) in &usage {
            let via = if via.is_empty() {
                ctx.ui.paint("[no enabled profile; removed on next update]", Style::Yellow)
            } else {
                format!("[profile: {}]", via.join(", "))
            };
            rows.push(vec![format!("  {}", show(path)), via]);
        }
        // Wanted but not installed yet.
        let installed: BTreeSet<&PathBuf> = usage.iter().map(|(p, _)| p).collect();
        for r in b.registry.repos().filter(|r| !installed.contains(&r.path)) {
            let via: Vec<&str> = r
                .profiles
                .iter()
                .filter(|p| b.library.profile(p).is_ok_and(|p| p.contains(&id)))
                .map(String::as_str)
                .collect();
            if !via.is_empty() {
                rows.push(vec![
                    format!("  {}", show(&r.path)),
                    ctx.ui.dim(&format!("[profile: {}] not installed yet", via.join(", "))),
                ]);
            }
        }
        if rows.is_empty() {
            println!("  {}", ctx.ui.dim("not installed anywhere"));
        }
        print!("{}", ui::table(&rows));
        return Ok(Exit::Ok);
    }

    if b.registry.is_empty() {
        println!("No repositories registered.");
        return Ok(Exit::Ok);
    }
    let mut rows = Vec::new();
    let mut exit = Exit::Ok;
    for r in b.registry.repos() {
        let profiles = if r.profiles.is_empty() { ctx.ui.dim("-") } else { r.profiles.join(", ") };
        let state = match b.plan(r) {
            Err(e) => {
                exit = exit.worst(Exit::Attention);
                let short = if r.path.is_dir() { "error" } else { "missing" };
                rows.push(vec![
                    format!("  {}", ctx.ui.paint(short, Style::Red)),
                    show(&r.path),
                    ctx.ui.dim(e.message()),
                ]);
                continue;
            }
            Ok(plan) => {
                let conflicts = plan.conflicts().count();
                let missing = plan.skills.iter().filter(|s| s.state == State::Missing).count();
                let pending = plan.pending().count() - conflicts;
                if conflicts + missing > 0 {
                    exit = exit.worst(Exit::Attention);
                    let n = conflicts + missing;
                    ctx.ui.paint(&ui::plural(n, "problem", "problems"), Style::Magenta)
                } else if pending > 0 {
                    ctx.ui.paint(&format!("{pending} pending"), Style::Cyan)
                } else {
                    ctx.ui.paint("up to date", Style::Green)
                }
            }
        };
        rows.push(vec![format!("  {state}"), show(&r.path), profiles]);
    }
    print!("{}", ui::table(&rows));
    Ok(exit)
}

pub fn stats(ctx: &Ctx, _args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let skills = b.library.skill_ids()?;
    let profiles = b.library.profiles()?;
    let installed: usize = b.registry.repos().map(|r| r.installed.len()).sum();
    let in_profile: BTreeSet<&SkillId> = profiles.iter().flat_map(|p| p.skills()).collect();
    let deployed: BTreeSet<&SkillId> = b.registry.repos().flat_map(|r| r.installed.keys()).collect();
    let not_in_profile = skills.iter().filter(|s| !in_profile.contains(s)).count();
    let unused = skills.iter().filter(|s| !deployed.contains(s)).count();
    let rows = vec![
        vec!["Repositories".to_string(), b.registry.len().to_string()],
        vec!["Profiles".to_string(), profiles.len().to_string()],
        vec!["Library skills".to_string(), skills.len().to_string()],
        vec!["Installed skills".to_string(), installed.to_string()],
        vec!["Unused skills".to_string(), unused.to_string(), ctx.ui.dim("installed nowhere")],
        vec!["Unprofiled skills".to_string(), not_in_profile.to_string(), ctx.ui.dim("in no profile")],
    ];
    // Right-align the numbers.
    let width = rows.iter().map(|r| r[1].len()).max().unwrap_or(0);
    let rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|mut r| {
            r[1] = format!("{:>width$}", r[1]);
            r
        })
        .collect();
    print!("{}", ui::table(&rows));
    Ok(Exit::Ok)
}

pub fn update(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let mut b = ctx.open()?;
    let opts = Options::from_args(args, &b)?;
    let repos: Vec<PathBuf> = b.registry.repos().map(|r| r.path.clone()).collect();
    Ok(update::run(ctx, &mut b, &repos, &opts))
}

pub fn prune(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let mut b = ctx.open()?;
    let gone: Vec<PathBuf> = b.registry.repos().filter(|r| !r.path.is_dir()).map(|r| r.path.clone()).collect();
    if gone.is_empty() {
        println!("Nothing to prune: every registered directory exists.");
        return Ok(Exit::Ok);
    }
    for p in &gone {
        println!("  {} {}", ctx.ui.paint("-", Style::Red), show(p));
        if !args.has("dry-run") {
            b.registry.remove(p);
        }
    }
    if args.has("dry-run") {
        println!("Would forget {}. Nothing changed.", ui::plural(gone.len(), "repository", "repositories"));
    } else {
        b.registry.save()?;
        println!("Forgot {}.", ui::plural(gone.len(), "repository", "repositories"));
    }
    Ok(Exit::Ok)
}

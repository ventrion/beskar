//! `beskar library ...`

use std::path::Path;

use beskar_core::library::Imported;
use beskar_core::scan::{self, CandidateStatus};
use beskar_core::skill::SKILL_FILE;
use beskar_core::{Beskar, Result, SkillId, err, fingerprint, paths};

use crate::args::Args;
use crate::ui::{self, Style, show};
use crate::{Ctx, Exit};

/// Names of profiles that include `id`.
fn profiles_with(b: &Beskar, id: &SkillId) -> Result<Vec<String>> {
    Ok(b.library.profiles()?.into_iter().filter(|p| p.contains(id)).map(|p| p.name().to_string()).collect())
}

pub fn list(ctx: &Ctx, _args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let skills = b.library.skills()?;
    if skills.is_empty() {
        println!("The library has no skills yet.");
        println!("{}", ctx.ui.dim("hint: import some with `beskar library add <dir>` or `beskar library scan <dir>`"));
        return Ok(Exit::Ok);
    }
    let profiles = b.library.profiles()?;
    let rows: Vec<Vec<String>> = skills
        .iter()
        .map(|s| {
            let used: Vec<&str> = profiles.iter().filter(|p| p.contains(&s.id)).map(|p| p.name()).collect();
            let used = if used.is_empty() { ctx.ui.dim("-") } else { used.join(", ") };
            vec![s.id.to_string(), used, ui::truncate(s.meta.description().unwrap_or(""), 70)]
        })
        .collect();
    let mut all = vec![vec![ctx.ui.dim("SKILL"), ctx.ui.dim("PROFILES"), ctx.ui.dim("DESCRIPTION")]];
    all.extend(rows);
    print!("{}", ui::table(&all));
    Ok(Exit::Ok)
}

pub fn add(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let src = paths::absolute(Path::new(&args.positional[0]))?;
    if !src.is_dir() {
        return Err(err!("{} is not a directory", show(&src)));
    }
    let name = match args.value("name") {
        Some(n) => n.to_string(),
        None => src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
    };
    let id =
        SkillId::new(&name).map_err(|e| err!("{}", e.message()).hint("rename the directory or pass --name <id>"))?;
    if !src.join(SKILL_FILE).is_file() {
        let nested = scan::find_skill_dirs(&src);
        if !nested.is_empty() {
            return Err(err!(
                "{} is not a skill, but contains {}",
                show(&src),
                ui::plural(nested.len(), "skill", "skills")
            )
            .hint(format!("import them with `beskar library scan {}`", args.positional[0])));
        }
        eprintln!(
            "{}",
            ctx.ui.paint(&format!("warning: {} has no {SKILL_FILE}; importing it anyway", show(&src)), Style::Yellow)
        );
    }
    match b.library.import(&src, &id, args.has("replace"))? {
        Imported::Added => println!("Added `{id}` to the library"),
        Imported::Replaced => {
            println!("Replaced `{id}` in the library");
            hint_propagate(ctx, &b, &id)?;
        }
        Imported::Unchanged => println!("`{id}` is already in the library with identical content"),
    }
    Ok(Exit::Ok)
}

/// After a library skill changed: say which workspaces will pick it up.
pub fn hint_propagate(ctx: &Ctx, b: &Beskar, id: &SkillId) -> Result<()> {
    let n = b.skill_usage(id)?.len();
    if n > 0 {
        println!(
            "{}",
            ctx.ui.dim(&format!(
                "hint: `{id}` is installed in {}; run `beskar update --all` to propagate",
                ui::plural(n, "workspace", "workspaces")
            ))
        );
    }
    Ok(())
}

pub fn scan(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let root = paths::absolute(Path::new(&args.positional[0]))?;
    if !root.is_dir() {
        return Err(err!("{} is not a directory", show(&root)));
    }
    let replace = args.has("replace");
    let candidates = scan::scan(&b.library, &root)?;
    if candidates.is_empty() {
        println!("No skills found in {} (a skill is a directory containing {SKILL_FILE})", show(&root));
        return Ok(Exit::Ok);
    }
    println!("Found {} in {}\n", ui::plural(candidates.len(), "skill", "skills"), show(&root));
    let mut rows = Vec::new();
    let mut importable = Vec::new();
    for c in &candidates {
        let rel = c.path.strip_prefix(&root).map(|p| p.display().to_string()).unwrap_or_default();
        let rel = if rel.is_empty() { ".".to_string() } else { rel };
        let (mark, note) = match &c.status {
            CandidateStatus::New => {
                importable.push(c);
                (
                    ctx.ui.paint("+", Style::Green),
                    c.description.as_deref().map(|d| ui::truncate(d, 60)).unwrap_or_default(),
                )
            }
            CandidateStatus::Identical => (ctx.ui.dim("="), ctx.ui.dim("already in the library")),
            CandidateStatus::Differs if replace => {
                importable.push(c);
                (ctx.ui.paint("~", Style::Cyan), "differs from the library; will replace".to_string())
            }
            CandidateStatus::Differs => {
                (ctx.ui.paint("~", Style::Yellow), "differs from the library; skipped (use --replace)".to_string())
            }
            CandidateStatus::InvalidName(m) => (ctx.ui.paint("!", Style::Red), format!("skipped: {m}")),
            CandidateStatus::Duplicate(first) => {
                let first = first.strip_prefix(&root).unwrap_or(first).display().to_string();
                (ctx.ui.paint("!", Style::Red), format!("skipped: same name as {first}"))
            }
        };
        rows.push(vec![format!("  {mark}"), c.name.clone(), ctx.ui.dim(&rel), note]);
    }
    print!("{}", ui::table(&rows));
    println!();
    if importable.is_empty() {
        println!("Nothing to import.");
        return Ok(Exit::Ok);
    }
    let what = ui::plural(importable.len(), "skill", "skills");
    if args.has("dry-run") {
        println!("Would import {what}. Nothing changed.");
        return Ok(Exit::Ok);
    }
    if !args.has("yes") {
        if !ctx.ui.interactive() {
            println!("Not importing without confirmation: re-run with --yes to import {what}.");
            return Ok(Exit::Attention);
        }
        if !ctx.ui.confirm(&format!("Import {what}?"), true) {
            println!("Nothing imported.");
            return Ok(Exit::Ok);
        }
    }
    let mut failed = 0;
    for c in &importable {
        let id = c.id().expect("importable candidates have valid ids");
        if let Err(e) = b.library.import(&c.path, &id, replace) {
            failed += 1;
            eprintln!("error: {id}: {}", e.message());
        }
    }
    println!("Imported {}.", ui::plural(importable.len() - failed, "skill", "skills"));
    Ok(if failed > 0 { Exit::Failure } else { Exit::Ok })
}

pub fn show_skill(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let id = b.library.find_skill(&args.positional[0])?;
    let skill = b.library.skill(&id)?;
    let fp = fingerprint::of_dir(&skill.path)?;
    let files = count_files(&skill.path);
    println!("{}", ctx.ui.bold(id.as_str()));
    let mut rows = Vec::new();
    if let Some(d) = skill.meta.description() {
        rows.push(vec!["  description".into(), d.to_string()]);
    }
    for (k, v) in skill.meta.fields.iter().filter(|(k, _)| !matches!(k.as_str(), "name" | "description")) {
        rows.push(vec![format!("  {k}"), ui::truncate(v, 80)]);
    }
    if !skill.meta.has_skill_file {
        rows.push(vec!["  note".into(), ctx.ui.paint(&format!("no {SKILL_FILE}"), Style::Yellow)]);
    }
    rows.push(vec!["  path".into(), show(&skill.path)]);
    rows.push(vec!["  fingerprint".into(), fp.to_string()]);
    rows.push(vec!["  files".into(), files.to_string()]);
    let profiles = profiles_with(&b, &id)?;
    rows.push(vec!["  profiles".into(), if profiles.is_empty() { ctx.ui.dim("none") } else { profiles.join(", ") }]);
    print!("{}", ui::table(&rows));
    let usage = b.skill_usage(&id)?;
    if usage.is_empty() {
        println!("  {}", ctx.ui.dim("not installed anywhere"));
    } else {
        println!("  installed in");
        let rows: Vec<Vec<String>> = usage
            .iter()
            .map(|(path, via)| {
                let via =
                    if via.is_empty() { ctx.ui.dim("[no enabled profile]") } else { format!("[{}]", via.join(", ")) };
                vec![format!("    {}", show(path)), via]
            })
            .collect();
        print!("{}", ui::table(&rows));
    }
    Ok(Exit::Ok)
}

fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            let p = e.path();
            if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) { count_files(&p) } else { 1 }
        })
        .sum()
}

pub fn remove(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let id = b.library.find_skill(&args.positional[0])?;
    let profiles = profiles_with(&b, &id)?;
    if !profiles.is_empty() && !args.has("force") {
        return Err(err!(
            "`{id}` is part of {}: {}",
            ui::plural(profiles.len(), "profile", "profiles"),
            profiles.join(", ")
        )
        .hint("remove it from those profiles first, or pass --force to do both"));
    }
    for name in &profiles {
        let mut p = b.library.profile(name)?;
        p.remove_skill(&id)?;
        p.save()?;
        println!("Removed `{id}` from profile `{name}`");
    }
    b.library.remove_skill(&id)?;
    println!("Removed `{id}` from the library");
    let installed = b.skill_usage(&id)?.len();
    if installed > 0 {
        println!(
            "{}",
            ctx.ui.dim(&format!(
                "hint: still installed in {}; `beskar update --all` removes unmodified copies",
                ui::plural(installed, "workspace", "workspaces")
            ))
        );
    }
    Ok(Exit::Ok)
}

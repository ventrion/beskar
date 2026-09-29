//! `beskar profile ...` — named groups of skills in the library.

use crate::cli::ProfileCmd;
use crate::error::{Error, Exit, Result};
use crate::library::Library;
use crate::profile::{validate_name, Profile};
use crate::registry::Registry;
use crate::util;

use super::Ctx;

pub fn run(ctx: &Ctx, cmd: ProfileCmd) -> Result<Exit> {
    let cfg = ctx.config()?;
    let lib = Library::new(&cfg.library_path);
    match cmd {
        ProfileCmd::Create { name, description } => create(ctx, &lib, name, description),
        ProfileCmd::Delete { name, force } => delete(ctx, &cfg, &lib, name, force),
        ProfileCmd::List => list(ctx, &lib),
        ProfileCmd::Show { name } => show(ctx, &cfg, &lib, name),
        ProfileCmd::Add { name, skills } => add(ctx, &lib, name, skills),
        ProfileCmd::Remove { name, skills } => remove(ctx, &lib, name, skills),
    }
}

fn create(ctx: &Ctx, lib: &Library, name: String, description: Option<String>) -> Result<Exit> {
    validate_name(&name)?;
    let path = lib.profile_path(&name);
    if path.exists() {
        return Err(Error::msg(format!(
            "profile `{name}` already exists ({}) — edit it or delete it first",
            util::display_path(&path)
        )));
    }
    let p = Profile::new(path.clone(), &name, description.as_deref());
    p.save()?;
    ctx.ui.ok(&format!(
        "created profile `{name}` — add skills with `beskar profile add {name} <skill>...`"
    ));
    Ok(Exit::Ok)
}

fn delete(ctx: &Ctx, cfg: &crate::config::Config, lib: &Library, name: String, force: bool) -> Result<Exit> {
    let path = lib.profile_path(&name);
    if !path.is_file() {
        return Err(Error::msg(format!("no profile `{name}`")));
    }
    // Who would lose this profile?
    let registry = Registry::load(&cfg.registry_path)?;
    let repos_using: Vec<String> = registry
        .repos
        .iter()
        .filter(|r| r.has_profile(&name))
        .map(|r| util::display_path(&r.path))
        .collect();
    if !repos_using.is_empty() && !force {
        return Err(Error::msg(format!(
            "profile `{name}` is enabled in: {} — disable it there first, or use --force \
             (those repos will warn about a missing profile until updated)",
            repos_using.join(", ")
        )));
    }
    let confirmed = if ctx.assume_yes() || force {
        true
    } else if crate::ui::Ui::interactive_stdin() {
        ctx.ui.confirm(&format!("Delete profile `{name}`?"), false)?
    } else {
        repos_using.is_empty()
    };
    if !confirmed {
        ctx.ui.note("cancelled");
        return Ok(Exit::Ok);
    }
    std::fs::remove_file(&path).map_err(|e| Error::io(&path, &e))?;
    ctx.ui.ok(&format!("deleted profile `{name}`"));
    Ok(Exit::Ok)
}

fn list(ctx: &Ctx, lib: &Library) -> Result<Exit> {
    let profiles = lib.list_profiles()?;
    if profiles.is_empty() {
        ctx.ui.note("no profiles yet — create one with `beskar profile create <name>`");
        return Ok(Exit::Ok);
    }
    println!("{:<24} {:<8} {}", ctx.ui.bold("NAME"), ctx.ui.bold("SKILLS"), ctx.ui.bold("DESCRIPTION"));
    for p in &profiles {
        let desc = p.description.clone().unwrap_or_default();
        let desc: String = desc.chars().take(60).collect();
        println!("{:<24} {:<8} {}", p.name, p.skills.len(), ctx.ui.dim(&desc));
    }
    Ok(Exit::Ok)
}

fn show(ctx: &Ctx, cfg: &crate::config::Config, lib: &Library, name: String) -> Result<Exit> {
    let p = lib.load_profile(&name)?;
    println!("{:<14}{}", ctx.ui.bold("name"), p.name);
    if let Some(d) = &p.description {
        println!("{:<14}{}", ctx.ui.bold("description"), d);
    }
    println!("{:<14}{}", ctx.ui.bold("file"), util::display_path(&p.path));
    println!();
    if p.skills.is_empty() {
        ctx.ui.note("(no skills yet)");
    } else {
        println!("{}", ctx.ui.bold("skills"));
        for s in &p.skills {
            let marker = if lib.has_skill(s) {
                ctx.ui.green("✓")
            } else {
                ctx.ui.red("✗ missing")
            };
            println!("  {marker} {s}");
        }
    }
    // Repos using this profile.
    let registry = Registry::load(&cfg.registry_path)?;
    let repos: Vec<String> = registry
        .repos
        .iter()
        .filter(|r| r.has_profile(&name))
        .map(|r| util::display_path(&r.path))
        .collect();
    println!();
    if repos.is_empty() {
        println!("{:<14}{}", ctx.ui.bold("enabled in"), ctx.ui.dim("(no repos)"));
    } else {
        println!("{}", ctx.ui.bold("enabled in"));
        for r in repos {
            println!("  {r}");
        }
    }
    Ok(Exit::Ok)
}

fn add(ctx: &Ctx, lib: &Library, name: String, skills: Vec<String>) -> Result<Exit> {
    let mut p = lib.load_profile(&name)?;
    // Validate all skills before touching the file.
    let mut missing = Vec::new();
    for s in &skills {
        if !lib.has_skill(s) && !missing.contains(s) {
            missing.push(s.clone());
        }
    }
    if !missing.is_empty() {
        return Err(Error::msg(format!(
            "not in the library: {} — import them first (`beskar library list` shows what you have)",
            missing.join(", ")
        )));
    }
    let fresh: Vec<String> = skills.iter().filter(|s| !p.has_skill(s)).cloned().collect();
    if fresh.is_empty() {
        ctx.ui.note("all requested skills are already in the profile");
        return Ok(Exit::Ok);
    }
    p.add_skills(&fresh);
    p.save()?;
    ctx.ui.ok(&format!(
        "added {} to profile `{name}` (now {} skills)",
        fresh.join(", "),
        p.skills.len()
    ));
    Ok(Exit::Ok)
}

fn remove(ctx: &Ctx, lib: &Library, name: String, skills: Vec<String>) -> Result<Exit> {
    let mut p = lib.load_profile(&name)?;
    let removed = p.remove_skills(&skills);
    if removed == 0 {
        ctx.ui.note("none of those skills were in the profile");
        return Ok(Exit::Ok);
    }
    p.save()?;
    ctx.ui.ok(&format!(
        "removed {removed} skill(s) from profile `{name}` ({} left)",
        p.skills.len()
    ));
    Ok(Exit::Ok)
}

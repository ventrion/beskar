//! `beskar library ...` — manage the canonical skill collection.

use std::path::PathBuf;

use crate::cli::LibCmd;
use crate::error::{Error, Exit, Result};
use crate::library::{discover_skills, Library};
use crate::profile::Profile;
use crate::registry::Registry;
use crate::util;

use super::Ctx;

pub fn run(ctx: &Ctx, cmd: LibCmd) -> Result<Exit> {
    let cfg = ctx.config()?;
    let lib = Library::new(&cfg.library_path);
    match cmd {
        LibCmd::Init { path } => init(ctx, &cfg, &lib, path),
        LibCmd::Add { path, name, force } => add(ctx, &cfg, &lib, path, name, force),
        LibCmd::Scan { path, force } => scan(ctx, &cfg, &lib, path, force),
        LibCmd::List => list(ctx, &lib),
        LibCmd::Show { skill } => show(ctx, &cfg, &lib, skill),
        LibCmd::Remove { skill, force } => remove(ctx, &lib, skill, force),
    }
}

fn init(ctx: &Ctx, cfg: &crate::config::Config, _lib: &Library, path: Option<PathBuf>) -> Result<Exit> {
    let mut lib = Library::new(&cfg.library_path);
    if let Some(p) = path {
        let abs = if p.is_absolute() { p } else { std::env::current_dir().map_err(|e| Error::io(".", &e))?.join(p) };
        let mut cfg = cfg.clone();
        cfg.library_path = abs.clone();
        cfg.save()?;
        lib = Library::new(&abs);
        ctx.ui.note("library-path updated in config");
    }
    lib.init_dirs()?;
    let skills = lib.list_skills()?.len();
    let profiles = lib.list_profiles()?.len();
    ctx.ui.ok(&format!(
        "library ready at {} ({} skills, {} profiles)",
        util::display_path(&lib.root),
        skills,
        profiles
    ));
    Ok(Exit::Ok)
}

fn add(
    ctx: &Ctx,
    _cfg: &crate::config::Config,
    lib: &Library,
    path: PathBuf,
    name: Option<String>,
    force: bool,
) -> Result<Exit> {
    let src = if path.is_absolute() { path.clone() } else {
        std::env::current_dir().map_err(|e| Error::io(".", &e))?.join(&path)
    };
    if !src.is_dir() {
        return Err(Error::msg(format!("{} is not a directory", util::display_path(&src))));
    }
    let meta = crate::skillmd::from_dir(&src);
    let id = name.unwrap_or_else(|| {
        src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    });
    if id.is_empty() {
        return Err(Error::msg("cannot derive a skill name — pass --name"));
    }
    let (files, fp) = lib.import_skill(&src, &id, force)?;
    let mut line = format!("added `{id}` ({} files, {})", files, &fp[..13.min(fp.len())]);
    if let Some(d) = &meta.description {
        line.push_str(&format!(" — {d}"));
    }
    ctx.ui.ok(&line);
    Ok(Exit::Ok)
}

fn scan(
    ctx: &Ctx,
    _cfg: &crate::config::Config,
    lib: &Library,
    path: PathBuf,
    force: bool,
) -> Result<Exit> {
    let src = if path.is_absolute() { path.clone() } else {
        std::env::current_dir().map_err(|e| Error::io(".", &e))?.join(&path)
    };
    if !src.is_dir() {
        return Err(Error::msg(format!("{} is not a directory", util::display_path(&src))));
    }
    let found = discover_skills(&src)?;
    if found.is_empty() {
        ctx.ui.note(&format!(
            "no skills found under {} (a skill is a directory containing SKILL.md)",
            util::display_path(&src)
        ));
        return Ok(Exit::Failed);
    }

    ctx.ui.header(&format!("Found {} skill{}.", found.len(), if found.len() == 1 { "" } else { "s" }));
    let mut importable = Vec::new();
    let mut collisions = 0usize;
    for f in &found {
        let exists = lib.has_skill(&f.id);
        let marker = if exists { ctx.ui.yellow("±") } else { ctx.ui.green("✓") };
        let desc = f.description.clone().unwrap_or_default();
        let note = if exists { " (already in library)" } else { "" };
        println!(" {} {:<24}{}{}", marker, f.id, if desc.is_empty() { String::new() } else { desc }, ctx.ui.dim(note));
        if exists && !force {
            collisions += 1;
        } else {
            importable.push(f);
        }
    }

    if importable.is_empty() {
        ctx.ui.note("nothing new to import");
        return Ok(Exit::Ok);
    }

    let proceed = if ctx.assume_yes() {
        true
    } else if crate::ui::Ui::interactive_stdin() {
        ctx.ui.confirm(
            &format!("Import {} skill{}?", importable.len(), if importable.len() == 1 { "" } else { "s" }),
            true,
        )?
    } else {
        ctx.ui.warn("non-interactive without --yes: skipping import (run with --yes to import)");
        false
    };
    if !proceed {
        ctx.ui.note("import cancelled");
        return Ok(Exit::Ok);
    }

    let mut imported = 0usize;
    for f in &importable {
        match lib.import_skill(&f.path, &f.id, force) {
            Ok((files, _)) => {
                imported += 1;
                ctx.ui.ok(&format!(
                    "imported `{}` ({} file{})",
                    f.id,
                    files,
                    if files == 1 { "" } else { "s" }
                ));
            }
            Err(e) => ctx.ui.fail(&format!("could not import `{}`: {e}", f.id)),
        }
    }
    if collisions > 0 {
        ctx.ui.note(&format!(
            "{collisions} already in the library — rerun with --force to overwrite them"
        ));
    }
    println!();
    ctx.ui.ok(&format!("{imported} skill(s) imported into {}", util::display_path(&lib.skills_dir())));
    Ok(Exit::Ok)
}

fn list(ctx: &Ctx, lib: &Library) -> Result<Exit> {
    let skills = lib.list_skills()?;
    if skills.is_empty() {
        ctx.ui.note(&format!(
            "library is empty — import some with `beskar library add <path>` or `beskar library scan <path>`"
        ));
        return Ok(Exit::Ok);
    }
    let profiles = lib.list_profiles().unwrap_or_default();
    println!("{:<28} {:<6} {}", ctx.ui.bold("NAME"), ctx.ui.bold("USED-BY"), ctx.ui.bold("DESCRIPTION"));
    for s in &skills {
        let used = profiles.iter().filter(|p| p.has_skill(&s.id)).count();
        let desc = s.description.clone().unwrap_or_default();
        let desc: String = desc.chars().take(60).collect();
        println!("{:<28} {:<6} {}", s.id, used, ctx.ui.dim(&desc));
    }
    Ok(Exit::Ok)
}

fn show(ctx: &Ctx, cfg: &crate::config::Config, lib: &Library, skill: String) -> Result<Exit> {
    let skills = lib.list_skills()?;
    let Some(s) = skills.iter().find(|s| s.id == skill) else {
        return Err(Error::msg(format!(
            "skill `{skill}` is not in the library ({} skills present)",
            skills.len()
        )));
    };
    println!("{:<14}{}", ctx.ui.bold("name"), s.display());
    if let Some(d) = &s.description {
        println!("{:<14}{}", ctx.ui.bold("description"), d);
    }
    if !s.has_skill_md {
        println!("{:<14}{}", ctx.ui.bold("skill.md"), ctx.ui.dim("missing — bare directory"));
    }
    println!("{:<14}{}", ctx.ui.bold("path"), util::display_path(&s.path));
    let fp = lib.fingerprint(&skill)?;
    println!("{:<14}{}", ctx.ui.bold("fingerprint"), fp);

    let items = util::walk_sorted(&s.path)?;
    let mut files = 0usize;
    let mut bytes = 0u64;
    for it in &items {
        if let util::Item::File { rel } = it {
            files += 1;
            bytes += std::fs::metadata(s.path.join(rel)).map(|m| m.len()).unwrap_or(0);
        }
    }
    println!("{:<14}{} files, {} bytes", ctx.ui.bold("content"), files, bytes);

    // Who uses it.
    let profiles: Vec<Profile> = lib.list_profiles().unwrap_or_default();
    let users: Vec<&Profile> = profiles.iter().filter(|p| p.has_skill(&skill)).collect();
    if users.is_empty() {
        println!("{:<14}{}", ctx.ui.bold("in profiles"), ctx.ui.dim("(none)"));
    } else {
        println!("{:<14}{}", ctx.ui.bold("in profiles"), users.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", "));
    }
    let registry = Registry::load(&cfg.registry_path)?;
    let mut repos = Vec::new();
    for r in &registry.repos {
        if r.installed.iter().any(|i| i.id == skill) {
            repos.push(util::display_path(&r.path));
        }
    }
    if repos.is_empty() {
        println!("{:<14}{}", ctx.ui.bold("installed in"), ctx.ui.dim("(no repos)"));
    } else {
        println!("{:<14}{}", ctx.ui.bold("installed in"), repos.join(", "));
    }

    // File listing.
    println!();
    println!("{}", ctx.ui.bold("files"));
    for it in &items {
        match it {
            util::Item::File { rel } => println!("  {}", rel.display()),
            util::Item::Dir { rel } => println!("  {}/", util::display_path(rel.as_path())),
            util::Item::Symlink { rel, target } => {
                println!("  {} -> {}", rel.display(), target)
            }
        }
    }
    Ok(Exit::Ok)
}

fn remove(ctx: &Ctx, lib: &Library, skill: String, force: bool) -> Result<Exit> {
    if !lib.has_skill(&skill) {
        return Err(Error::msg(format!("skill `{skill}` is not in the library")));
    }
    // Usage check.
    let profiles = lib.list_profiles().unwrap_or_default();
    let used_in: Vec<&Profile> = profiles.iter().filter(|p| p.has_skill(&skill)).collect();
    if !used_in.is_empty() && !force {
        let names: Vec<&str> = used_in.iter().map(|p| p.name.as_str()).collect();
        return Err(Error::msg(format!(
            "skill `{skill}` is referenced by profile(s): {} — \
             remove it from the profiles first, or use --force (references will dangle)",
            names.join(", ")
        )));
    }
    let confirmed = if ctx.assume_yes() || force {
        true
    } else if crate::ui::Ui::interactive_stdin() {
        ctx.ui.confirm(&format!("Remove skill `{skill}` from the library?"), false)?
    } else if used_in.is_empty() {
        true // unreferenced removal in a script is fine
    } else {
        false
    };
    if !confirmed {
        ctx.ui.note("cancelled");
        return Ok(Exit::Ok);
    }
    lib.remove_skill(&skill)?;
    if !used_in.is_empty() {
        ctx.ui.warn("profile references were left in place and now dangle");
    }
    ctx.ui.ok(&format!("removed `{skill}`"));
    Ok(Exit::Ok)
}

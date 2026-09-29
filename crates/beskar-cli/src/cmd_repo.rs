//! `beskar repo ...`, plus the `status` and `update` aliases.

use std::path::{Path, PathBuf};

use beskar_core::reconcile::State;
use beskar_core::{Beskar, RepoEntry, Result, config, err, fingerprint, fsops, paths};

use crate::args::Args;
use crate::ui::{self, Style, show};
use crate::update::{self, Options};
use crate::{Ctx, Exit};

/// The repository selected by `--repo`, or the one containing the current directory.
pub fn target(b: &Beskar, args: &Args) -> Result<PathBuf> {
    let start = match args.value("repo") {
        Some(p) => PathBuf::from(p),
        None => std::env::current_dir().map_err(|e| err!("cannot read the current directory: {e}"))?,
    };
    Ok(b.repo_containing(&start)?.path.clone())
}

fn check_profiles(b: &Beskar, names: &[String]) -> Result<()> {
    for name in names {
        b.library.profile(name)?;
    }
    Ok(())
}

pub fn add(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let mut b = ctx.open()?;
    let raw = args.positional.first().map_or(".", String::as_str);
    let path = paths::absolute(Path::new(raw))?;
    if !path.is_dir() {
        return Err(err!("{} is not a directory", show(&path)));
    }
    let library = paths::absolute(b.library.root())?;
    if path.starts_with(&library) {
        return Err(err!("{} is inside the library; the library is not a workspace", show(&path)));
    }
    let enable: Vec<String> = args.values("enable").to_vec();
    check_profiles(&b, &enable)?;
    let skills_dir = args.value("skills-dir").map(config::parse_skills_dir).transpose().map_err(|m| err!("{m}"))?;
    if b.registry.add(RepoEntry::new(path.clone())) {
        println!("Registered {}", show(&path));
    } else {
        println!("{} is already registered", show(&path));
    }
    let repo = b.registry.get_mut(&path).expect("just added");
    if let Some(dir) = skills_dir {
        println!("  skills dir: {}", dir.display());
        repo.skills_dir = Some(dir);
    }
    for name in enable {
        if !repo.has_profile(&name) {
            println!("  enabled {name}");
            repo.profiles.push(name);
        }
    }
    let has_profiles = !repo.profiles.is_empty();
    b.registry.save()?;
    let next = if has_profiles {
        "run `beskar repo update` to install its skills"
    } else {
        "enable a profile with `beskar repo enable <profile>`"
    };
    println!("{}", ctx.ui.dim(&format!("hint: {next}")));
    Ok(Exit::Ok)
}

pub fn remove(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let mut b = ctx.open()?;
    let path = match args.positional.first() {
        Some(p) => {
            let abs = paths::absolute(Path::new(p))?;
            if b.registry.get(&abs).is_none() {
                return Err(err!("{} is not registered", show(&abs)).hint("see `beskar repo list`"));
            }
            abs
        }
        None => target(&b, args)?,
    };
    let entry = b.registry.get(&path).expect("checked").clone();
    let mut exit = Exit::Ok;
    if args.has("purge") {
        let dir = entry.skills_path(&b.config.skills_dir);
        for (id, recorded) in &entry.installed {
            let skill = dir.join(id.as_str());
            if !skill.is_dir() {
                continue;
            }
            if fingerprint::of_dir(&skill)? == *recorded {
                fsops::remove_dir(&skill)?;
                println!("  {} {id}", ctx.ui.symbol(State::Remove));
            } else {
                println!("  {} {id}  modified locally; left in place", ctx.ui.paint("!", Style::Magenta));
                exit = Exit::Attention;
            }
        }
    }
    b.registry.remove(&path);
    b.registry.save()?;
    println!("Unregistered {}", show(&path));
    if !args.has("purge") && !entry.installed.is_empty() {
        println!("{}", ctx.ui.dim(&format!("note: {} installed skills were left in place", entry.installed.len())));
    }
    Ok(exit)
}

pub fn list(ctx: &Ctx, _args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    if b.registry.is_empty() {
        println!("No repositories registered.");
        println!("{}", ctx.ui.dim("hint: `beskar repo add <path>`"));
        return Ok(Exit::Ok);
    }
    let mut rows =
        vec![vec![ctx.ui.dim("REPOSITORY"), ctx.ui.dim("PROFILES"), ctx.ui.dim("SKILLS"), ctx.ui.dim("SYNCED")]];
    for r in b.registry.repos() {
        let path = if r.path.is_dir() {
            show(&r.path)
        } else {
            format!("{} {}", show(&r.path), ctx.ui.paint("(missing)", Style::Red))
        };
        rows.push(vec![
            path,
            if r.profiles.is_empty() { ctx.ui.dim("-") } else { r.profiles.join(", ") },
            r.installed.len().to_string(),
            r.synced.clone().unwrap_or_else(|| ctx.ui.dim("never")),
        ]);
    }
    print!("{}", ui::table(&rows));
    Ok(Exit::Ok)
}

pub fn status(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let path = target(&b, args)?;
    let repo = b.registry.get(&path).expect("target is registered");
    let skills_dir = repo.skills_dir.as_deref().unwrap_or(&b.config.skills_dir);
    let profiles = if repo.profiles.is_empty() { ctx.ui.dim("none") } else { repo.profiles.join(", ") };
    print!(
        "{}",
        ui::table(&[
            vec![ctx.ui.bold("Repository"), show(&repo.path)],
            vec![ctx.ui.bold("Profiles"), profiles],
            vec![ctx.ui.bold("Skills dir"), skills_dir.display().to_string()],
            vec![ctx.ui.bold("Synced"), repo.synced.clone().unwrap_or_else(|| ctx.ui.dim("never"))],
        ])
    );
    let plan = b.plan(repo)?;
    println!();
    let mut rows = Vec::new();
    for s in &plan.skills {
        let via = if s.profiles.is_empty() { ctx.ui.dim("-") } else { s.profiles.join(", ") };
        let note = match s.state {
            State::Clean => String::new(),
            State::Install => "not installed yet".into(),
            State::Restore => "deleted locally; will be restored".into(),
            State::Update => "library changed".into(),
            State::Modified => "edited locally; library unchanged".into(),
            State::Adopt => "matches the library; not yet recorded".into(),
            State::Remove => "no longer in an enabled profile".into(),
            State::Forget => "no longer wanted; already gone".into(),
            State::Untracked => "not managed by Beskar".into(),
            State::Missing => "not in the library".into(),
            State::Conflict(kind) => kind.describe().into(),
        };
        let label = ctx.ui.paint(s.state.label(), ui::Ui::state_style(s.state));
        let fp = if args.has("verbose") {
            s.workspace.as_ref().map(|f| ctx.ui.dim(f.short())).unwrap_or_default()
        } else {
            String::new()
        };
        rows.push(vec![format!("  {label}"), s.id.to_string(), via, note, fp]);
    }
    if rows.is_empty() {
        println!("  {}", ctx.ui.dim("no skills"));
    } else {
        print!("{}", ui::table(&rows));
    }
    println!();
    let pending = plan.pending().filter(|s| !matches!(s.state, State::Conflict(_))).count();
    let conflicts = plan.conflicts().count();
    let missing = plan.skills.iter().filter(|s| s.state == State::Missing).count();
    let mut parts = Vec::new();
    if pending > 0 {
        parts.push(ui::plural(pending, "change pending", "changes pending"));
    }
    if conflicts > 0 {
        parts.push(ui::plural(conflicts, "conflict", "conflicts"));
    }
    if missing > 0 {
        parts.push(format!("{missing} missing from the library"));
    }
    if parts.is_empty() {
        println!("Up to date.");
        Ok(Exit::Ok)
    } else {
        println!("{}. Run `beskar repo update` to reconcile.", parts.join(", "));
        Ok(if conflicts + missing > 0 { Exit::Attention } else { Exit::Ok })
    }
}

fn hint_update(ctx: &Ctx) {
    println!("{}", ctx.ui.dim("hint: run `beskar repo update` to apply"));
}

pub fn enable(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let mut b = ctx.open()?;
    let path = target(&b, args)?;
    check_profiles(&b, &args.positional)?;
    let repo = b.registry.get_mut(&path).expect("target is registered");
    let mut changed = false;
    for name in &args.positional {
        if repo.has_profile(name) {
            println!("`{name}` is already enabled in {}", show(&path));
        } else {
            repo.profiles.push(name.clone());
            println!("Enabled `{name}` in {}", show(&path));
            changed = true;
        }
    }
    b.registry.save()?;
    if changed {
        hint_update(ctx);
    }
    Ok(Exit::Ok)
}

pub fn disable(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let mut b = ctx.open()?;
    let path = target(&b, args)?;
    let repo = b.registry.get_mut(&path).expect("target is registered");
    let mut changed = false;
    for name in &args.positional {
        if repo.has_profile(name) {
            repo.profiles.retain(|p| p != name);
            println!("Disabled `{name}` in {}", show(&path));
            changed = true;
        } else {
            println!("`{name}` is not enabled in {}", show(&path));
        }
    }
    b.registry.save()?;
    if changed {
        hint_update(ctx);
    }
    Ok(Exit::Ok)
}

pub fn toggle(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let mut b = ctx.open()?;
    let path = target(&b, args)?;
    let enabled: Vec<bool> = {
        let repo = b.registry.get(&path).expect("target is registered");
        args.positional.iter().map(|n| repo.has_profile(n)).collect()
    };
    let to_enable: Vec<String> =
        args.positional.iter().zip(&enabled).filter(|(_, e)| !**e).map(|(n, _)| n.clone()).collect();
    check_profiles(&b, &to_enable)?;
    let repo = b.registry.get_mut(&path).expect("target is registered");
    for (name, was) in args.positional.iter().zip(enabled) {
        if was {
            repo.profiles.retain(|p| p != name);
            println!("Disabled `{name}` in {}", show(&path));
        } else {
            repo.profiles.push(name.clone());
            println!("Enabled `{name}` in {}", show(&path));
        }
    }
    b.registry.save()?;
    hint_update(ctx);
    Ok(Exit::Ok)
}

pub fn update(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let mut b = ctx.open()?;
    let opts = Options::from_args(args, &b)?;
    let repos: Vec<PathBuf> = if args.has("all") {
        if args.has("repo") {
            return Err(err!("--all and --repo cannot be combined"));
        }
        b.registry.repos().map(|r| r.path.clone()).collect()
    } else {
        vec![target(&b, args)?]
    };
    Ok(update::run(ctx, &mut b, &repos, &opts))
}

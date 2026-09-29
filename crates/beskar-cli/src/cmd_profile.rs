//! `beskar profile ...`

use beskar_core::{Beskar, Result, SkillId, err};

use crate::args::Args;
use crate::ui::{self, Style, show};
use crate::{Ctx, Exit};

/// Resolve skill names against the library; all must exist.
fn resolve_skills(b: &Beskar, names: &[String]) -> Result<Vec<SkillId>> {
    names.iter().map(|n| b.library.find_skill(n)).collect()
}

fn hint_update(ctx: &Ctx, b: &Beskar, profile: &str) {
    let n = b.registry.using_profile(profile).count();
    if n > 0 {
        println!(
            "{}",
            ctx.ui.dim(&format!(
                "hint: `{profile}` is enabled in {}; run `beskar update --all` to apply",
                ui::plural(n, "workspace", "workspaces")
            ))
        );
    }
}

pub fn create(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let name = &args.positional[0];
    let skills = resolve_skills(&b, &args.positional[1..])?;
    let mut profile = b.library.create_profile(name, args.value("description"))?;
    for id in &skills {
        profile.add_skill(id)?;
    }
    profile.save()?;
    println!("Created profile `{name}` ({})", show(profile.path()));
    if skills.is_empty() {
        println!("{}", ctx.ui.dim(&format!("hint: add skills with `beskar profile add {name} <skill>...`")));
    } else {
        println!("  skills: {}", skills.iter().map(SkillId::as_str).collect::<Vec<_>>().join(", "));
    }
    Ok(Exit::Ok)
}

pub fn delete(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let mut b = ctx.open()?;
    let name = args.positional[0].clone();
    b.library.profile(&name)?;
    let users: Vec<_> = b.registry.using_profile(&name).map(|r| r.path.clone()).collect();
    if !users.is_empty() && !args.has("force") {
        let list: Vec<String> = users.iter().map(|p| show(p)).collect();
        return Err(err!(
            "`{name}` is enabled in {}: {}",
            ui::plural(users.len(), "workspace", "workspaces"),
            list.join(", ")
        )
        .hint("disable it there first, or pass --force to do both"));
    }
    for path in &users {
        let repo = b.registry.get_mut(path).expect("listed above");
        repo.profiles.retain(|p| p != &name);
        println!("Disabled `{name}` in {}", show(path));
    }
    b.registry.save()?;
    b.library.delete_profile(&name)?;
    println!("Deleted profile `{name}`");
    if !users.is_empty() {
        println!("{}", ctx.ui.dim("hint: run `beskar update --all` to remove its skills from those workspaces"));
    }
    Ok(Exit::Ok)
}

pub fn list(ctx: &Ctx, _args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let profiles = b.library.profiles()?;
    if profiles.is_empty() {
        println!("No profiles yet.");
        println!("{}", ctx.ui.dim("hint: `beskar profile create <name> <skill>...`"));
        return Ok(Exit::Ok);
    }
    let mut rows =
        vec![vec![ctx.ui.dim("PROFILE"), ctx.ui.dim("SKILLS"), ctx.ui.dim("WORKSPACES"), ctx.ui.dim("DESCRIPTION")]];
    for p in &profiles {
        rows.push(vec![
            p.name().to_string(),
            p.skills().len().to_string(),
            b.registry.using_profile(p.name()).count().to_string(),
            ui::truncate(p.description().unwrap_or(""), 60),
        ]);
    }
    print!("{}", ui::table(&rows));
    Ok(Exit::Ok)
}

pub fn show_profile(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let p = b.library.profile(&args.positional[0])?;
    match p.description() {
        Some(d) => println!("{} — {d}", ctx.ui.bold(p.name())),
        None => println!("{}", ctx.ui.bold(p.name())),
    }
    println!("  {}", ctx.ui.dim(&show(p.path())));
    println!("\nskills ({})", p.skills().len());
    for id in p.skills() {
        if b.library.has_skill(id) {
            println!("  {id}");
        } else {
            println!("  {id}  {}", ctx.ui.paint("missing from the library", Style::Magenta));
        }
    }
    let users: Vec<_> = b.registry.using_profile(p.name()).collect();
    println!("\nenabled in ({})", users.len());
    for r in users {
        println!("  {}", show(&r.path));
    }
    Ok(Exit::Ok)
}

pub fn add(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let name = &args.positional[0];
    let mut profile = b.library.profile(name)?;
    let skills = resolve_skills(&b, &args.positional[1..])?;
    let (mut added, mut already) = (Vec::new(), Vec::new());
    for id in skills {
        if profile.add_skill(&id)? { added.push(id.to_string()) } else { already.push(id.to_string()) }
    }
    profile.save()?;
    if !added.is_empty() {
        println!("Added to `{name}`: {}", added.join(", "));
    }
    if !already.is_empty() {
        println!("Already in `{name}`: {}", already.join(", "));
    }
    if !added.is_empty() {
        hint_update(ctx, &b, name);
    }
    Ok(Exit::Ok)
}

pub fn remove(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let b = ctx.open()?;
    let name = &args.positional[0];
    let mut profile = b.library.profile(name)?;
    let (mut removed, mut absent) = (Vec::new(), Vec::new());
    for raw in &args.positional[1..] {
        let id = SkillId::new(raw)?;
        if profile.remove_skill(&id)? { removed.push(id.to_string()) } else { absent.push(id.to_string()) }
    }
    profile.save()?;
    if !removed.is_empty() {
        println!("Removed from `{name}`: {}", removed.join(", "));
        hint_update(ctx, &b, name);
    }
    if !absent.is_empty() {
        println!("Not in `{name}`: {}", absent.join(", "));
    }
    Ok(Exit::Ok)
}

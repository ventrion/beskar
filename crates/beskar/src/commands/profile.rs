//! `beskar profile ...`

use beskar_core::fsutil::display_path;
use beskar_core::{Beskar, Error, Home, Result};

use super::{done, parse_cmd, unknown_sub};
use crate::args;
use crate::{help, ui, Outcome};

pub fn run(home: &Home, sub: Option<&str>, argv: &[String]) -> Result<Outcome> {
    match sub {
        Some("list") => list(home, argv),
        Some("show") => show(home, argv),
        Some("create") => create(home, argv),
        Some("delete") => delete(home, argv),
        Some("add") => edit(home, argv, true),
        Some("remove") => edit(home, argv, false),
        other => unknown_sub("profile", other, help::PROFILE),
    }
}

fn list(home: &Home, argv: &[String]) -> Result<Outcome> {
    if let Err(o) = parse_cmd(argv, &[], 2, help::PROFILE) {
        return Ok(o);
    }
    let beskar = Beskar::open(home.clone())?;
    let profiles = beskar.library.profiles()?;
    if profiles.is_empty() {
        println!("No profiles yet. Create one with `beskar profile create <name>`.");
        return done();
    }
    let rows: Vec<Vec<String>> = profiles
        .iter()
        .map(|p| {
            let repos = beskar.registry.repos_with_profile(&p.name).len();
            vec![
                p.name.clone(),
                format!("{} skill(s)", p.skills().len()),
                format!("{repos} repo(s)"),
                ui::truncate(p.description().unwrap_or(""), 60),
            ]
        })
        .collect();
    print!("{}", ui::table(&rows));
    done()
}

fn show(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[], 2, help::PROFILE) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let [name] = parsed.positional.as_slice() else {
        return crate::usage("profile show needs exactly one <profile>", help::PROFILE);
    };
    let beskar = Beskar::open(home.clone())?;
    let profile = beskar.library.profile(name)?;
    println!("Profile:      {}", profile.name);
    println!("File:         {}", display_path(&profile.path));
    if let Some(d) = profile.description() {
        println!("Description:  {d}");
    }
    println!("\nSkills:");
    let skills = profile.skills();
    if skills.is_empty() {
        println!("  (none; add some with `beskar profile add {name} <skill>...`)");
    }
    for s in &skills {
        if beskar.library.has_skill(s) {
            println!("  {s}");
        } else {
            println!("  {s}   ! not in the library");
        }
    }
    println!("\nUsed by:");
    let repos = beskar.registry.repos_with_profile(name);
    if repos.is_empty() {
        println!("  (no repository has this profile enabled)");
    }
    for r in repos {
        println!("  {}", display_path(&r.path));
    }
    done()
}

fn create(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[args::DESCRIPTION], 2, help::PROFILE) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let [name] = parsed.positional.as_slice() else {
        return crate::usage("profile create needs exactly one <profile>", help::PROFILE);
    };
    let beskar = Beskar::open(home.clone())?;
    let mut profile = beskar.library.create_profile(name)?;
    if let Some(d) = parsed.value(&args::DESCRIPTION) {
        profile.set_description(d);
        profile.save()?;
    }
    println!(
        "created profile {} ({})",
        profile.name,
        display_path(&profile.path)
    );
    done()
}

fn delete(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[args::FORCE], 2, help::PROFILE) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let [name] = parsed.positional.as_slice() else {
        return crate::usage("profile delete needs exactly one <profile>", help::PROFILE);
    };
    let mut beskar = Beskar::open(home.clone())?;
    beskar.library.profile(name)?;
    let using: Vec<std::path::PathBuf> = beskar
        .registry
        .repos_with_profile(name)
        .iter()
        .map(|r| r.path.clone())
        .collect();
    if !using.is_empty() {
        if !parsed.has(&args::FORCE) {
            let shown: Vec<String> = using.iter().map(|p| display_path(p)).collect();
            return Err(Error::invalid(format!(
                "profile '{name}' is enabled in {}; disable it there first or pass --force",
                shown.join(", ")
            )));
        }
        for path in &using {
            if let Some(repo) = beskar.registry.get_mut(path) {
                repo.disable(name);
                println!("disabled {name} in {}", display_path(path));
            }
        }
        beskar.registry.save()?;
    }
    beskar.library.delete_profile(name)?;
    println!("deleted profile {name}");
    if !using.is_empty() {
        println!("run `beskar registry update` to remove skills those repositories no longer need");
    }
    done()
}

fn edit(home: &Home, argv: &[String], adding: bool) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[], 2, help::PROFILE) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let verb = if adding { "add" } else { "remove" };
    let [name, skills @ ..] = parsed.positional.as_slice() else {
        return crate::usage(
            &format!("profile {verb} needs <profile> <skill>..."),
            help::PROFILE,
        );
    };
    if skills.is_empty() {
        return crate::usage(
            &format!("profile {verb} needs at least one <skill>"),
            help::PROFILE,
        );
    }
    let beskar = Beskar::open(home.clone())?;
    let mut profile = beskar.library.profile(name)?;
    if adding {
        let missing: Vec<&String> = skills
            .iter()
            .filter(|s| !beskar.library.has_skill(s))
            .collect();
        if let Some(first) = missing.first() {
            let mut msg = format!("skill '{first}' is not in the library");
            if missing.len() > 1 {
                msg.push_str(&format!(" (nor are {} other(s))", missing.len() - 1));
            }
            msg.push_str("; import it with `beskar library add <dir>` first");
            return Err(Error::not_found(msg));
        }
    }
    for s in skills {
        beskar_core::names::validate("skill", s)?;
        let changed = if adding {
            profile.add_skill(s)
        } else {
            profile.remove_skill(s)
        };
        match (adding, changed) {
            (true, true) => println!("added {s} to {name}"),
            (true, false) => println!("{s} is already in {name}"),
            (false, true) => println!("removed {s} from {name}"),
            (false, false) => println!("{s} was not in {name}"),
        }
    }
    profile.save()?;
    let repos = beskar.registry.repos_with_profile(name);
    if !repos.is_empty() {
        println!(
            "{} repositor{} use this profile; run `beskar registry update` to apply the change",
            repos.len(),
            if repos.len() == 1 { "y" } else { "ies" }
        );
    }
    done()
}

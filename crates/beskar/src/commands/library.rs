//! `beskar library ...`

use std::path::Path;

use beskar_core::fsutil::display_path;
use beskar_core::reconcile::{self, Action};
use beskar_core::{insight, skill, Beskar, Error, Home, Library, Result};

use super::{done, parse_cmd, unknown_sub};
use crate::args;
use crate::{help, ui, Outcome};

pub fn run(home: &Home, sub: Option<&str>, argv: &[String]) -> Result<Outcome> {
    match sub {
        Some("list") => list(home, argv),
        Some("show") => show(home, argv),
        Some("add") => add(home, argv),
        Some("scan") => scan(home, argv),
        Some("remove") => remove(home, argv),
        Some("init") => init(home, argv),
        other => unknown_sub("library", other, help::LIBRARY),
    }
}

fn list(home: &Home, argv: &[String]) -> Result<Outcome> {
    if let Err(o) = parse_cmd(argv, &[], 2, help::LIBRARY) {
        return Ok(o);
    }
    let beskar = Beskar::open(home.clone())?;
    let skills = beskar.library.skills()?;
    if skills.is_empty() {
        println!("The library is empty. Import skills with `beskar library add <dir>` or `beskar library scan <dir>`.");
        return done();
    }
    let profiles = beskar.library.profiles()?;
    let rows: Vec<Vec<String>> = skills
        .iter()
        .map(|s| {
            let used_by = profiles.iter().filter(|p| p.has_skill(&s.name)).count();
            let usage = match used_by {
                0 => "-".to_string(),
                1 => "1 profile".to_string(),
                n => format!("{n} profiles"),
            };
            vec![s.name.clone(), usage, ui::truncate(s.description(), 70)]
        })
        .collect();
    print!("{}", ui::table(&rows));
    done()
}

fn show(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[], 2, help::LIBRARY) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let [name] = parsed.positional.as_slice() else {
        return crate::usage("library show needs exactly one <skill>", help::LIBRARY);
    };
    let beskar = Beskar::open(home.clone())?;
    let s = beskar.library.skill(name)?;
    println!("Skill:        {}", s.name);
    println!("Path:         {}", display_path(&s.path));
    if let Some(n) = &s.metadata.name {
        if n != &s.name {
            println!("Declared as:  {n}");
        }
    }
    if !s.description().is_empty() {
        println!("Description:  {}", s.description());
    }
    if !s.has_skill_file() {
        println!("Note:         no SKILL.md");
    }
    println!("Fingerprint:  {}", s.fingerprint()?.short());
    let files = beskar_core::fsutil::list_files(&s.path)?;
    println!("Files:        {}", files.len());

    let profiles = beskar.library.profiles_with_skill(name)?;
    println!("\nProfiles:");
    if profiles.is_empty() {
        println!("  (none)");
    }
    for p in &profiles {
        println!("  {}", p.name);
    }

    let uses = insight::skill_usage(&beskar.library, &beskar.registry, name)?;
    println!("\nRepositories:");
    if uses.is_empty() {
        println!("  (none)");
    }
    let mut rows = Vec::new();
    for u in &uses {
        let repo = beskar.registry.get(&u.repo).expect("listed repo exists");
        let state = match reconcile::plan(&beskar.library, repo, &beskar.config.skills_dir) {
            Ok(plan) => plan
                .items
                .iter()
                .find(|i| &i.skill == name)
                .map(|i| match &i.action {
                    Action::Unchanged => "up to date".to_string(),
                    other => other.to_string(),
                })
                .unwrap_or_default(),
            Err(e) => format!("({e})"),
        };
        let via = if u.via_profiles.is_empty() {
            "not wanted by any enabled profile".to_string()
        } else {
            format!("[profile: {}]", u.via_profiles.join(", "))
        };
        rows.push(vec![format!("  {}", display_path(&u.repo)), via, state]);
    }
    print!("{}", ui::table(&rows));
    done()
}

fn add(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[args::NAME, args::REPLACE], 2, help::LIBRARY) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let [path] = parsed.positional.as_slice() else {
        return crate::usage("library add needs exactly one <dir>", help::LIBRARY);
    };
    let beskar = Beskar::open(home.clone())?;
    let src = Path::new(path);
    let name = match parsed.value(&args::NAME) {
        Some(n) => n.to_string(),
        None => beskar_core::fsutil::absolute(src)?
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| {
                Error::invalid(format!(
                    "cannot derive a skill name from {path}; pass --name"
                ))
            })?,
    };
    let s = beskar
        .library
        .import_skill(src, &name, parsed.has(&args::REPLACE))?;
    println!("imported {} -> {}", s.name, display_path(&s.path));
    if !s.has_skill_file() {
        println!(
            "note: {} has no SKILL.md; agents may not recognise it as a skill",
            s.name
        );
    }
    done()
}

fn scan(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[args::YES], 2, help::LIBRARY) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let [path] = parsed.positional.as_slice() else {
        return crate::usage("library scan needs exactly one <dir>", help::LIBRARY);
    };
    let beskar = Beskar::open(home.clone())?;
    let root = beskar_core::fsutil::absolute(Path::new(path))?;
    if !root.is_dir() {
        return Err(Error::not_found(format!(
            "{} is not a directory",
            display_path(&root)
        )));
    }
    let found = skill::discover(&root)?;
    if found.is_empty() {
        println!(
            "No skills found under {} (looking for directories with a SKILL.md).",
            display_path(&root)
        );
        return done();
    }
    println!(
        "Found {} skill(s) under {}.\n",
        found.len(),
        display_path(&root)
    );
    let mut to_import = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for dir in &found {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let meta = skill::read_metadata(dir);
        let desc = ui::truncate(meta.description.as_deref().unwrap_or(""), 60);
        if !beskar_core::names::is_valid(&name) {
            println!("  x {name:<28} skipped: not a valid skill name");
        } else if beskar.library.has_skill(&name) {
            println!("  = {name:<28} already in the library");
        } else if !seen.insert(name.clone()) {
            println!("  x {name:<28} skipped: another directory with this name was found first");
        } else {
            println!("  + {name:<28} {desc}");
            to_import.push((name, dir.clone()));
        }
    }
    if to_import.is_empty() {
        println!("\nNothing new to import.");
        return done();
    }
    println!();
    if !ui::confirm(
        &format!("Import {} skill(s)?", to_import.len()),
        true,
        parsed.has(&args::YES),
    )? {
        println!("Nothing imported.");
        return done();
    }
    for (name, dir) in &to_import {
        beskar.library.import_skill(dir, name, false)?;
        println!("imported {name}");
    }
    println!("\nAdd them to a profile with `beskar profile add <profile> <skill>...`.");
    done()
}

fn remove(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[args::FORCE], 2, help::LIBRARY) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let [name] = parsed.positional.as_slice() else {
        return crate::usage("library remove needs exactly one <skill>", help::LIBRARY);
    };
    let beskar = Beskar::open(home.clone())?;
    beskar.library.skill(name)?;
    let profiles = beskar.library.profiles_with_skill(name)?;
    if !profiles.is_empty() {
        let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
        if !parsed.has(&args::FORCE) {
            return Err(Error::invalid(format!(
                "skill '{name}' is listed in profile(s) {}; remove it from them first or pass --force",
                names.join(", ")
            )));
        }
        for mut p in profiles {
            p.remove_skill(name);
            p.save()?;
            println!("removed {name} from profile {}", p.name);
        }
    }
    beskar.library.remove_skill(name)?;
    println!("removed {name} from the library");
    let installed = beskar.registry.repos_with_installed(name);
    if !installed.is_empty() {
        println!("\nStill installed in {} repositories; `beskar registry update` will remove those copies:", installed.len());
        for r in installed {
            println!("  {}", display_path(&r.path));
        }
    }
    done()
}

fn init(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[], 2, help::LIBRARY) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    let mut config = beskar_core::Config::load(home)?;
    let target = match parsed.positional.as_slice() {
        [] => config.library.clone(),
        [p] => beskar_core::fsutil::absolute(Path::new(p))?,
        _ => return crate::usage("library init takes at most one <dir>", help::LIBRARY),
    };
    let existed = Library::is_initialised(&target);
    let lib = Library::init(&target)?;
    if lib.root != config.library {
        config.set(home, "library", &display_path(&lib.root))?;
        println!("config now points at {}", display_path(&lib.root));
    }
    println!(
        "{} library at {}",
        if existed { "found" } else { "created" },
        display_path(&lib.root)
    );
    done()
}

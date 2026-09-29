//! `beskar profile ...`: named sets of skills.

use beskar_core::{Error, ProfileName, Result, SkillId, usage};

use super::{count, counted, join_and};
use crate::app::{App, EXIT_OK, Outcome};
use crate::args::Matches;
use crate::output::{Cell, table, truncate};

fn skill_ids(names: &[String]) -> Result<Vec<SkillId>> {
    names.iter().map(|name| SkillId::new(name)).collect()
}

pub fn create(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    beskar.library.check()?;
    let name = ProfileName::new(&m.args[0])?;
    let skills = skill_ids(&m.args[1..])?;
    let _lock = beskar.lock("profile create")?;
    let profile = beskar
        .library
        .create_profile(&name, m.value("description"), &skills)?;
    let style = app.out.style();
    app.out.line(format!(
        "Created profile {} with {} ({}).",
        style.bold(name.as_str()),
        count(profile.skills.len(), "skill"),
        app.display(&profile.path)
    ));
    if profile.skills.is_empty() {
        app.out.line(format!(
            "Add skills with `beskar profile add {name} <skill>...`."
        ));
    } else {
        app.out.line(format!(
            "Enable it in a workspace with `beskar repo enable {name}`."
        ));
    }
    Ok(EXIT_OK)
}

pub fn delete(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    beskar.library.check()?;
    let name = beskar.library.find_profile(&m.args[0])?;
    let users = usage::profile_users(&beskar.registry()?, &name);
    if !users.is_empty() && !m.has("force") {
        let places: Vec<String> = users.iter().map(|path| app.display(path)).collect();
        let mut error = Error::invalid(format!(
            "profile `{name}` is enabled in {}: {}",
            count(users.len(), "workspace"),
            join_and(&places)
        ));
        for path in users.iter().take(3) {
            error = error.hint(format!(
                "`beskar repo disable {name} --repo {}` disables it there",
                app.arg(path)
            ));
        }
        return Err(error
            .hint(format!(
                "or `beskar profile delete {name} --force --yes` deletes it and disables it everywhere"
            ))
            .into());
    }
    if !m.has("yes") {
        let question = format!(
            "Delete profile `{name}` ({})?",
            app.display(&beskar.library.profile_path(&name))
        );
        match app.confirm(&question, false) {
            Some(true) => {}
            Some(false) => {
                app.out.line("Nothing deleted.");
                return Ok(EXIT_OK);
            }
            None => {
                return Err(Error::conflict(format!(
                    "deleting profile `{name}` needs confirmation"
                ))
                .hint("pass --yes to delete it")
                .into());
            }
        }
    }
    let _lock = beskar.lock("profile delete")?;
    let mut registry = beskar.registry()?;
    let users = usage::profile_users(&registry, &name);
    for path in &users {
        if let Some(entry) = registry.get_mut(path) {
            entry.profiles.retain(|profile| *profile != name);
        }
    }
    beskar.library.delete_profile(&name)?;
    if !users.is_empty() {
        registry.save()?;
    }
    let style = app.out.style();
    app.out
        .line(format!("Deleted profile {}.", style.bold(name.as_str())));
    if !users.is_empty() {
        app.out.line(format!(
            "Disabled it in {}; `beskar update --all` removes its skills there.",
            count(users.len(), "workspace")
        ));
    }
    Ok(EXIT_OK)
}

pub fn list(app: &mut App, _m: &Matches) -> Outcome {
    let beskar = app.load()?;
    beskar.library.check()?;
    let names = beskar.library.profile_names()?;
    if names.is_empty() {
        app.out
            .line("No profiles yet. Create one with `beskar profile create <name> <skill>...`.");
        return Ok(EXIT_OK);
    }
    let registry = beskar.registry()?;
    let style = app.out.style();
    let mut broken = 0;
    let rows = names
        .iter()
        .map(|name| {
            let users = usage::profile_users(&registry, name).len();
            let used = if users == 0 {
                style.dim("unused")
            } else {
                format!("used in {}", count(users, "workspace"))
            };
            match beskar.library.profile(name) {
                Ok(profile) => {
                    let description = profile.description.unwrap_or_default();
                    let description = match app.width() {
                        Some(width) => truncate(&description, width.saturating_sub(50).max(20)),
                        None => description,
                    };
                    vec![
                        Cell::styled(name.to_string(), |t| style.bold(t)),
                        Cell::plain(count(profile.skills.len(), "skill")),
                        Cell::plain(used),
                        Cell::plain(description),
                    ]
                }
                Err(error) => {
                    broken += 1;
                    vec![
                        Cell::styled(name.to_string(), |t| style.bold(t)),
                        Cell::styled("unreadable", |t| style.red(t)),
                        Cell::plain(used),
                        Cell::plain(error.message),
                    ]
                }
            }
        })
        .collect();
    for line in table(rows, "") {
        app.out.line(line);
    }
    if broken > 0 {
        app.out.blank();
        app.out
            .line("Run `beskar doctor` to see where the unreadable profiles go wrong.");
    }
    Ok(EXIT_OK)
}

pub fn show(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    beskar.library.check()?;
    let name = beskar.library.find_profile(&m.args[0])?;
    let profile = beskar.library.profile(&name)?;
    let registry = beskar.registry()?;
    let users = usage::profile_users(&registry, &name);
    let style = app.out.style();

    app.out.line(match &profile.description {
        Some(description) => format!("{}: {description}", style.bold(name.as_str())),
        None => style.bold(name.as_str()),
    });
    app.out.line(style.dim(&app.display(&profile.path)));
    app.out.blank();
    app.out
        .line(style.bold(&format!("Skills ({})", profile.skills.len())));
    if profile.skills.is_empty() {
        app.out.line(format!(
            "  none yet; add some with `beskar profile add {name} <skill>...`"
        ));
    }
    let mut skills = profile.skills.clone();
    skills.sort();
    let rows = skills
        .iter()
        .map(|skill| {
            let note = if beskar.library.contains(skill) {
                String::new()
            } else {
                style.red("✗ not in the library")
            };
            vec![Cell::plain(skill.to_string()), Cell::plain(note)]
        })
        .collect();
    for line in table(rows, "  ") {
        app.out.line(line);
    }
    app.out.blank();
    app.out
        .line(style.bold(&format!("Enabled in ({})", users.len())));
    if users.is_empty() {
        app.out.line(format!(
            "  no workspace yet; enable it with `beskar repo enable {name}`"
        ));
    }
    for path in &users {
        app.out.line(format!("  {}", app.display(path)));
    }
    Ok(EXIT_OK)
}

pub fn add(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    beskar.library.check()?;
    let name = beskar.library.find_profile(&m.args[0])?;
    let skills = skill_ids(&m.args[1..])?;
    let _lock = beskar.lock("profile add")?;
    let added = beskar.library.add_to_profile(&name, &skills)?;
    let style = app.out.style();
    let already: Vec<&str> = skills
        .iter()
        .filter(|s| !added.contains(s))
        .map(SkillId::as_str)
        .collect();
    if !added.is_empty() {
        let names: Vec<&str> = added.iter().map(SkillId::as_str).collect();
        app.out.line(format!(
            "Added {} to {}.",
            join_and(&names),
            style.bold(name.as_str())
        ));
    }
    if !already.is_empty() {
        app.out.line(format!(
            "{} already had {}.",
            style.bold(name.as_str()),
            join_and(&already)
        ));
    }
    let users = usage::profile_users(&beskar.registry()?, &name).len();
    if !added.is_empty() && users > 0 {
        app.out.line(format!(
            "{} {name}; `beskar update --all` installs the new skills there.",
            counted(users, "workspace", "enables", "enable")
        ));
    }
    Ok(EXIT_OK)
}

pub fn remove(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    beskar.library.check()?;
    let name = beskar.library.find_profile(&m.args[0])?;
    let skills = skill_ids(&m.args[1..])?;
    let _lock = beskar.lock("profile remove")?;
    let before = beskar.library.profile(&name)?;
    let removed = beskar.library.remove_from_profile(&name, &skills)?;
    let style = app.out.style();
    if !removed.is_empty() {
        let names: Vec<&str> = removed.iter().map(SkillId::as_str).collect();
        app.out.line(format!(
            "Removed {} from {}.",
            join_and(&names),
            style.bold(name.as_str())
        ));
    }
    for skill in skills.iter().filter(|s| !removed.contains(s)) {
        let mut note = format!("{} has no skill `{skill}`", style.bold(name.as_str()));
        if let Some(close) = bsk::closest(skill.as_str(), before.skills.iter().map(SkillId::as_str))
        {
            note += &format!("; did you mean `{close}`?");
        }
        app.out.line(format!("{note}."));
    }
    let users = usage::profile_users(&beskar.registry()?, &name).len();
    if !removed.is_empty() && users > 0 {
        app.out.line(format!(
            "{} {name}; `beskar update --all` removes the skills no other enabled profile includes.",
            counted(users, "workspace", "enables", "enable")
        ));
    }
    Ok(EXIT_OK)
}

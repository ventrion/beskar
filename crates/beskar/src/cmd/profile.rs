//! `beskar profile ...`: named sets of skills.

use beskar_core::ops::profiles::{ProfileEdit, ProfileSummary};
use beskar_core::profile::Profile;
use beskar_core::{Error, SkillId};

use super::{count, counted, join_and};
use crate::app::{App, EXIT_OK, Outcome};
use crate::args::Matches;
use crate::json::{self, Json};
use crate::output::{Cell, clean, table, truncate};

fn profile_json(profile: &Profile) -> Json {
    Json::obj([
        ("profile", Json::from(profile.name.as_str())),
        ("path", Json::path(&profile.path)),
        ("description", Json::from(profile.description.clone())),
        ("skills", Json::strings(&profile.skills)),
    ])
}

pub fn create(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let profile = beskar.create_profile(&m.args[0], &m.args[1..], m.value("description"))?;
    app.data(|| profile_json(&profile));
    let style = app.out.style();
    let name = &profile.name;
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
    let deletion = beskar.profile_deletion(&m.args[0], m.has("force"))?;
    let name = deletion.name.clone();
    if !m.has("yes") {
        let question = format!("Delete profile `{name}` ({})?", app.display(&deletion.path));
        match app.confirm(&question, false) {
            Some(true) => {}
            Some(false) => {
                app.data(|| {
                    Json::obj([
                        ("profile", Json::from(name.as_str())),
                        ("deleted", Json::Bool(false)),
                    ])
                });
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
    let deleted = beskar.delete_profile(&deletion)?;
    app.data(|| {
        Json::obj([
            ("profile", Json::from(name.as_str())),
            ("deleted", Json::Bool(true)),
            ("disabled_in", Json::paths(&deleted.disabled_in)),
        ])
    });
    let style = app.out.style();
    app.out
        .line(format!("Deleted profile {}.", style.bold(name.as_str())));
    if !deleted.disabled_in.is_empty() {
        app.out.line(format!(
            "Disabled it in {}; `beskar update --all` removes its skills there.",
            count(deleted.disabled_in.len(), "workspace")
        ));
    }
    Ok(EXIT_OK)
}

pub fn list(app: &mut App, _m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let profiles = beskar.profiles()?;
    app.data(|| Json::arr(profiles.iter().map(summary_json)));
    if profiles.is_empty() {
        app.out
            .line("No profiles yet. Create one with `beskar profile create <name> <skill>...`.");
        return Ok(EXIT_OK);
    }
    let style = app.out.style();
    let mut broken = 0;
    let rows = profiles
        .iter()
        .map(|summary| {
            let used = if summary.workspaces == 0 {
                style.dim("unused")
            } else {
                format!("used in {}", count(summary.workspaces, "workspace"))
            };
            let name = Cell::styled(summary.name.to_string(), |t| style.bold(t));
            match &summary.profile {
                Ok(profile) => {
                    let description = clean(profile.description.as_deref().unwrap_or_default());
                    let description = match app.width() {
                        Some(width) => truncate(&description, width.saturating_sub(50).max(20)),
                        None => description,
                    };
                    vec![
                        name,
                        Cell::plain(count(profile.skills.len(), "skill")),
                        Cell::plain(used),
                        Cell::plain(description),
                    ]
                }
                Err(error) => {
                    broken += 1;
                    vec![
                        name,
                        Cell::styled("unreadable", |t| style.red(t)),
                        Cell::plain(used),
                        Cell::plain(clean(&error.message)),
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

fn summary_json(summary: &ProfileSummary) -> Json {
    let base = match &summary.profile {
        Ok(profile) => profile_json(profile),
        Err(error) => Json::obj([
            ("profile", Json::from(summary.name.as_str())),
            ("error", json::error(error)),
        ]),
    };
    base.with("workspaces", Json::count(summary.workspaces))
}

pub fn show(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let details = beskar.profile_details(&m.args[0])?;
    app.data(|| {
        profile_json(&details.profile)
            .with("missing", Json::strings(&details.missing))
            .with("enabled_in", Json::paths(&details.enabled_in))
    });
    let (profile, name) = (&details.profile, &details.profile.name);
    let style = app.out.style();

    app.out.line(match &profile.description {
        Some(description) => format!("{}: {}", style.bold(name.as_str()), clean(description)),
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
            let note = if details.missing.contains(skill) {
                style.red("✗ not in the library")
            } else {
                String::new()
            };
            vec![Cell::plain(skill.to_string()), Cell::plain(note)]
        })
        .collect();
    for line in table(rows, "  ") {
        app.out.line(line);
    }
    app.out.blank();
    app.out
        .line(style.bold(&format!("Enabled in ({})", details.enabled_in.len())));
    if details.enabled_in.is_empty() {
        app.out.line(format!(
            "  no workspace yet; enable it with `beskar repo enable {name}`"
        ));
    }
    for path in &details.enabled_in {
        app.out.line(format!("  {}", app.display(path)));
    }
    Ok(EXIT_OK)
}

fn edit_json(edit: &ProfileEdit, changed: &str, unchanged: &str) -> Json {
    Json::obj([
        ("profile", Json::from(edit.name.as_str())),
        (changed, Json::strings(&edit.changed)),
        (unchanged, Json::strings(&edit.unchanged)),
        ("enabled_in", Json::count(edit.enabled_in)),
    ])
}

fn names(skills: &[SkillId]) -> String {
    join_and(&skills.iter().map(SkillId::as_str).collect::<Vec<_>>())
}

pub fn add(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let edit = beskar.add_to_profile(&m.args[0], &m.args[1..])?;
    app.data(|| edit_json(&edit, "added", "already_there"));
    let style = app.out.style();
    let name = style.bold(edit.name.as_str());
    if !edit.changed.is_empty() {
        app.out
            .line(format!("Added {} to {name}.", names(&edit.changed)));
    }
    if !edit.unchanged.is_empty() {
        app.out
            .line(format!("{name} already had {}.", names(&edit.unchanged)));
    }
    if !edit.changed.is_empty() && edit.enabled_in > 0 {
        app.out.line(format!(
            "{} {}; `beskar update --all` installs {} there.",
            counted(edit.enabled_in, "workspace", "enables", "enable"),
            edit.name,
            if edit.changed.len() == 1 {
                "the new skill"
            } else {
                "the new skills"
            }
        ));
    }
    Ok(EXIT_OK)
}

pub fn remove(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let edit = beskar.remove_from_profile(&m.args[0], &m.args[1..])?;
    app.data(|| edit_json(&edit, "removed", "not_there"));
    let style = app.out.style();
    let name = style.bold(edit.name.as_str());
    if !edit.changed.is_empty() {
        app.out
            .line(format!("Removed {} from {name}.", names(&edit.changed)));
    }
    for skill in &edit.unchanged {
        let mut note = format!("{name} has no skill `{skill}`");
        if let Some((_, close)) = edit.suggestions.iter().find(|(asked, _)| asked == skill) {
            note += &format!("; did you mean `{close}`?");
        }
        app.out.line(format!("{note}."));
    }
    if !edit.changed.is_empty() && edit.enabled_in > 0 {
        app.out.line(format!(
            "{} {}; `beskar update --all` removes the skills no other enabled profile includes.",
            counted(edit.enabled_in, "workspace", "enables", "enable"),
            edit.name
        ));
    }
    Ok(EXIT_OK)
}

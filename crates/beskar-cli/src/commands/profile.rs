//! `profile create|delete|list|show|add|remove`.

use beskar_core::{Error, ErrorKind, ProfileName, SkillId};

use super::{Failure, Result, confirm, emit, open, profile_name, skill_id};
use crate::args::Parsed;
use crate::context::{Context, clip, count, names, sanitize, table};
use crate::json::Json;

fn skill_ids(parsed: &Parsed, texts: &[String]) -> Result<Vec<SkillId>> {
    texts.iter().map(|t| skill_id(parsed, t)).collect()
}

pub fn create(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let library = beskar.library();
    let name = profile_name(parsed, parsed.arg(0).unwrap_or(""))?;
    let skills = skill_ids(parsed, &parsed.args()[1..])?;
    let profile = library.create_profile(&name, parsed.value("description"), &skills)?;
    ctx.say(format!(
        "Created profile '{name}' with {}.",
        count(profile.skills.len(), "skill")
    ));
    ctx.say(format!("  {}", ctx.tilde(&library.profile_path(&name))));
    ctx.say(format!(
        "Add skills with 'beskar profile add {name} <skill>...'. Enable it in a repository with 'beskar repo enable {name}'."
    ));
    Ok(0)
}

pub fn delete(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let library = beskar.library();
    let name = profile_name(parsed, parsed.arg(0).unwrap_or(""))?;
    if !library.profile_path(&name).is_file() {
        return Err(library.unknown_profile(name.as_str()).into());
    }
    let usage = beskar.profile_usage(&name)?;
    if !usage.repos.is_empty() && !parsed.flag("force") {
        let repos: Vec<String> = usage.repos.iter().map(|r| ctx.tilde(r)).collect();
        return Err(Failure::Core(
            Error::new(
                ErrorKind::Conflict,
                format!("profile '{name}' is enabled in {}: {}", count(repos.len(), "repository"), repos.join(", ")),
            )
            .with_hint(format!("disable it there first with 'beskar repo disable {name}', or use --force and disable it afterwards")),
        ));
    }
    if !confirm(
        ctx,
        parsed.flag("yes"),
        &format!("Delete profile '{name}'?"),
        false,
    )? {
        ctx.say("Nothing was deleted.");
        return Ok(0);
    }
    library.delete_profile(&name)?;
    ctx.say(format!("Deleted profile '{name}'."));
    if !usage.repos.is_empty() {
        ctx.say(format!(
            "It is still enabled in {}; run 'beskar repo disable {name}' there, because 'beskar repo update' cannot continue while an enabled profile is missing.",
            count(usage.repos.len(), "repository")
        ));
    }
    Ok(0)
}

pub fn list(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let library = beskar.library();
    let profiles = library.profiles()?;
    let registry = beskar.store().read()?;
    let uses = |name: &ProfileName| registry.using_profile(name).len();
    if parsed.flag("json") {
        let mut items: Vec<Json> = profiles
            .valid
            .iter()
            .map(|p| {
                Json::obj([
                    ("name", p.name.to_string().into()),
                    ("description", p.description.clone().into()),
                    ("skills", Json::strings(&p.skills)),
                    (
                        "repositories",
                        Json::arr(
                            registry
                                .using_profile(&p.name)
                                .into_iter()
                                .map(|r| Json::path(&r.path)),
                        ),
                    ),
                    ("error", Json::Null),
                ])
            })
            .collect();
        items.extend(profiles.broken.iter().map(|b| {
            Json::obj([
                ("name", b.name.clone().into()),
                ("description", Json::Null),
                ("skills", Json::Null),
                ("repositories", Json::arr([])),
                ("error", b.error.message().to_string().into()),
            ])
        }));
        emit(ctx, Json::Arr(items));
        return Ok(0);
    }
    if profiles.valid.is_empty() && profiles.broken.is_empty() {
        ctx.say("There are no profiles yet.");
        ctx.say("Create one with 'beskar profile create <name> [skill...]'.");
        return Ok(0);
    }
    let truncate = ctx.truncate;
    let mut rows: Vec<Vec<String>> = profiles
        .valid
        .iter()
        .map(|p| {
            vec![
                p.name.to_string(),
                p.skills.len().to_string(),
                uses(&p.name).to_string(),
                clip(
                    &sanitize(p.description.as_deref().unwrap_or("")),
                    60,
                    truncate,
                ),
            ]
        })
        .collect();
    rows.extend(profiles.broken.iter().map(|b| {
        vec![
            b.name.clone(),
            "?".into(),
            "?".into(),
            "unreadable; see 'beskar doctor'".into(),
        ]
    }));
    rows.sort();
    let text = table(
        &["PROFILE", "SKILLS", "REPOS", "DESCRIPTION"],
        &rows,
        ctx.style(),
        0,
    );
    ctx.say(text);
    for broken in &profiles.broken {
        ctx.say_err(format!(
            "warning: {}",
            broken.error.message().lines().next().unwrap_or("")
        ));
    }
    Ok(0)
}

pub fn show(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let library = beskar.library();
    let name = profile_name(parsed, parsed.arg(0).unwrap_or(""))?;
    let profile = library.profile(&name)?;
    let usage = beskar.profile_usage(&name)?;
    let described: Vec<(SkillId, Option<String>, bool)> = profile
        .skills
        .iter()
        .map(|id| match library.skill(id) {
            Ok(info) => (id.clone(), info.description, true),
            Err(_) => (id.clone(), None, false),
        })
        .collect();
    if parsed.flag("json") {
        emit(
            ctx,
            Json::obj([
                ("name", name.to_string().into()),
                ("description", profile.description.clone().into()),
                ("file", Json::path(&library.profile_path(&name))),
                (
                    "skills",
                    Json::arr(described.iter().map(|(id, d, present)| {
                        Json::obj([
                            ("id", id.to_string().into()),
                            ("description", d.clone().into()),
                            ("in_library", (*present).into()),
                        ])
                    })),
                ),
                (
                    "repositories",
                    Json::arr(usage.repos.iter().map(|r| Json::path(r))),
                ),
            ]),
        );
        return Ok(0);
    }
    let style = ctx.style();
    ctx.say(style.bold(name.as_str()));
    if let Some(description) = &profile.description {
        ctx.say(format!(
            "  {}  {}",
            style.dim("description"),
            sanitize(description)
        ));
    }
    ctx.say(format!(
        "  {}         {}",
        style.dim("file"),
        ctx.tilde(&library.profile_path(&name))
    ));
    ctx.say("");
    ctx.say(style.bold(&format!("Skills ({})", described.len())));
    let width = described
        .iter()
        .map(|(id, _, _)| id.as_str().len())
        .max()
        .unwrap_or(0);
    for (id, description, present) in &described {
        let note = if *present {
            clip(
                &sanitize(description.as_deref().unwrap_or("")),
                70,
                ctx.truncate,
            )
        } else {
            style.red("not in the library")
        };
        ctx.say(format!("  {:<width$}  {note}", id.as_str()).trim_end());
    }
    ctx.say("");
    ctx.say(style.bold(&format!("Enabled in ({})", usage.repos.len())));
    if usage.repos.is_empty() {
        ctx.say(format!(
            "  nowhere yet; enable it with 'beskar repo enable {name}'"
        ));
    }
    for repo in &usage.repos {
        ctx.say(format!("  {}", ctx.tilde(repo)));
    }
    Ok(0)
}

fn report_edit(
    ctx: &mut Context,
    name: &ProfileName,
    edit: &beskar_core::library::ProfileEdit,
    changed_verb: &str,
    unchanged_phrase: &str,
) -> Result {
    if !edit.changed.is_empty() {
        ctx.say(format!("{changed_verb} '{name}': {}", names(&edit.changed)));
    }
    if !edit.unchanged.is_empty() {
        ctx.say(format!(
            "{unchanged_phrase} '{name}': {}",
            names(&edit.unchanged)
        ));
    }
    Ok(0)
}

pub fn add(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let name = profile_name(parsed, parsed.arg(0).unwrap_or(""))?;
    let skills = skill_ids(parsed, &parsed.args()[1..])?;
    let edit = beskar.library().profile_add(&name, &skills)?;
    report_edit(ctx, &name, &edit, "Added to", "Already in")?;
    hint_propagation(&beskar, ctx, &name, !edit.changed.is_empty())
}

pub fn remove(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let name = profile_name(parsed, parsed.arg(0).unwrap_or(""))?;
    let skills = skill_ids(parsed, &parsed.args()[1..])?;
    let edit = beskar.library().profile_remove(&name, &skills)?;
    report_edit(ctx, &name, &edit, "Removed from", "Was not in")?;
    hint_propagation(&beskar, ctx, &name, !edit.changed.is_empty())
}

/// After a profile changes, tells the user how many repositories will notice.
fn hint_propagation(
    beskar: &beskar_core::Beskar,
    ctx: &mut Context,
    name: &ProfileName,
    changed: bool,
) -> Result {
    if !changed {
        return Ok(0);
    }
    let users = beskar.profile_usage(name)?.repos.len();
    if users > 0 {
        ctx.say(format!(
            "{} {} '{name}'. Apply the change with 'beskar registry update --all'.",
            count(users, "repository"),
            if users == 1 { "uses" } else { "use" }
        ));
    }
    Ok(0)
}

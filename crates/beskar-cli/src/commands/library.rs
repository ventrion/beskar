//! `library init|add|scan|list|show|remove`.

use std::collections::BTreeMap;

use beskar_core::SkillId;
use beskar_core::library::{AddOutcome, Candidate, CandidateStatus};

use super::{Result, confirm, emit, open, resolve_path, skill_id};
use crate::args::Parsed;
use crate::context::{Context, bytes, clip, count, names, sanitize, table};
use crate::json::Json;

pub fn init(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let library = beskar.library();
    let created = library.init()?;
    if created.is_empty() {
        ctx.say(format!(
            "The library at {} is already set up.",
            ctx.tilde(library.root())
        ));
    }
    for path in created {
        ctx.say(format!("created  {}", ctx.tilde(&path)));
    }
    Ok(0)
}

pub fn add(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let path = resolve_path(ctx, parsed.arg(0).unwrap_or("."));
    let added = beskar
        .library()
        .add_skill(&path, parsed.value("name"), parsed.flag("force"))?;
    for warning in &added.warnings {
        ctx.say_err(format!("warning: {warning}"));
    }
    let message = match added.outcome {
        AddOutcome::Added => format!("Added skill '{}' to the library.", added.id),
        AddOutcome::Replaced => format!("Replaced skill '{}' in the library.", added.id),
        AddOutcome::Unchanged => format!(
            "Skill '{}' is already in the library with the same content.",
            added.id
        ),
    };
    ctx.say(message);
    if added.outcome == AddOutcome::Added {
        ctx.say(format!(
            "Group it with others using 'beskar profile add <profile> {}'.",
            added.id
        ));
    }
    if added.outcome == AddOutcome::Replaced {
        ctx.say("Repositories pick up the new version with 'beskar registry update --all'.");
    }
    Ok(0)
}

fn candidate_line(candidate: &Candidate, force: bool, ctx: &Context) -> String {
    let style = ctx.style();
    let name = &sanitize(&candidate.dir_name);
    match &candidate.status {
        CandidateStatus::New(_) => format!("{} {name}", style.green("+")),
        CandidateStatus::Identical(_) => format!(
            "{} {name}  {}",
            style.dim("="),
            style.dim("already in the library")
        ),
        CandidateStatus::Differs(_) if force => {
            format!(
                "{} {name}  {}",
                style.yellow("~"),
                "differs from the library; will be replaced"
            )
        }
        CandidateStatus::Differs(_) => {
            format!(
                "{} {name}  {}",
                style.yellow("~"),
                "differs from the library; skipped, use --force to replace it"
            )
        }
        CandidateStatus::InvalidName(reason) => {
            format!("{} {name}  skipped: {}", style.red("!"), sanitize(reason))
        }
        CandidateStatus::Duplicate(_, first) => {
            format!(
                "{} {name}  skipped: same name as {}",
                style.red("!"),
                ctx.tilde(first)
            )
        }
        CandidateStatus::TooLarge(size) => format!(
            "{} {name}  skipped: too big to be a skill ({} files, {})",
            style.red("!"),
            size.files,
            bytes(size.bytes)
        ),
    }
}

fn status_word(status: &CandidateStatus) -> &'static str {
    match status {
        CandidateStatus::New(_) => "new",
        CandidateStatus::Identical(_) => "identical",
        CandidateStatus::Differs(_) => "differs",
        CandidateStatus::InvalidName(_) => "invalid-name",
        CandidateStatus::Duplicate(..) => "duplicate",
        CandidateStatus::TooLarge(_) => "too-large",
    }
}

pub fn scan(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let library = beskar.library();
    let root = resolve_path(ctx, parsed.arg(0).unwrap_or("."));
    let force = parsed.flag("force");
    let json = parsed.flag("json");
    let candidates = library.scan(&root)?;
    let importable: Vec<&Candidate> = candidates.iter().filter(|c| c.will_import(force)).collect();

    if !json {
        if candidates.is_empty() {
            ctx.say(format!("No skills found below {}.", ctx.tilde(&root)));
            ctx.say("A skill is a folder that contains a SKILL.md file.");
            return Ok(0);
        }
        ctx.say(format!("Found {}.", count(candidates.len(), "skill")));
        ctx.say("");
        for candidate in &candidates {
            let line = candidate_line(candidate, force, ctx);
            ctx.say(format!("  {line}"));
        }
        ctx.say("");
        if importable.is_empty() {
            ctx.say("Nothing to import.");
            return Ok(0);
        }
    }
    let dry_run = parsed.flag("dry-run");
    let mut imported: Vec<String> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    if !dry_run && !importable.is_empty() {
        let question = format!("Import {}?", count(importable.len(), "skill"));
        let go = confirm(ctx, parsed.flag("yes"), &question, true)?;
        if go {
            for candidate in &importable {
                match library.import(candidate, force) {
                    Ok(added) => imported.push(added.id.to_string()),
                    Err(error) => {
                        failures.push((candidate.dir_name.clone(), error.message().to_string()))
                    }
                }
            }
        }
    }
    if json {
        let found = candidates.iter().map(|c| {
            Json::obj([
                ("name", c.dir_name.clone().into()),
                ("path", Json::path(&c.path)),
                ("status", status_word(&c.status).into()),
                ("will_import", c.will_import(force).into()),
            ])
        });
        emit(
            ctx,
            Json::obj([
                ("path", Json::path(&root)),
                ("dry_run", dry_run.into()),
                ("found", Json::arr(found)),
                ("imported", Json::strings(&imported)),
                (
                    "failed",
                    Json::arr(failures.iter().map(|(n, e)| {
                        Json::obj([("name", n.clone().into()), ("error", e.clone().into())])
                    })),
                ),
            ]),
        );
    } else if dry_run {
        ctx.say("Dry run: nothing was imported.");
    } else if imported.is_empty() && failures.is_empty() {
        ctx.say("Nothing imported.");
    } else {
        ctx.say(format!("Imported {}.", count(imported.len(), "skill")));
        if !imported.is_empty() {
            ctx.say("Next: group them with 'beskar profile create <name> <skill>...'.");
        }
    }
    for (name, error) in &failures {
        ctx.say_err(format!("error: could not import '{name}': {error}"));
    }
    Ok(i32::from(!failures.is_empty()))
}

pub fn list(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let library = beskar.library();
    let skills = library.skills()?;
    let profiles = library.profiles()?;
    let mut membership: BTreeMap<&SkillId, Vec<&str>> = BTreeMap::new();
    for profile in &profiles.valid {
        for skill in &profile.skills {
            membership
                .entry(skill)
                .or_default()
                .push(profile.name.as_str());
        }
    }
    if parsed.flag("json") {
        let items = skills.iter().map(|s| {
            Json::obj([
                ("id", s.id.to_string().into()),
                ("path", Json::path(&s.path)),
                ("name", s.name.clone().into()),
                ("description", s.description.clone().into()),
                ("has_skill_file", s.has_skill_file.into()),
                (
                    "profiles",
                    Json::strings(membership.get(&s.id).cloned().unwrap_or_default()),
                ),
            ])
        });
        emit(ctx, Json::arr(items));
        return Ok(0);
    }
    if skills.is_empty() {
        ctx.say("The library has no skills yet.");
        ctx.say(
            "Import some with 'beskar library scan <folder>' or 'beskar library add <folder>'.",
        );
        return Ok(0);
    }
    let truncate = ctx.truncate;
    // The description goes last: the last column is never padded, so one long description does not
    // stretch every line.
    let rows: Vec<Vec<String>> = skills
        .iter()
        .map(|s| {
            vec![
                s.id.to_string(),
                membership
                    .get(&s.id)
                    .map(|p| p.join(", "))
                    .unwrap_or_default(),
                clip(&sanitize(&one_line(s.description.as_deref())), 60, truncate),
            ]
        })
        .collect();
    let text = table(&["SKILL", "PROFILES", "DESCRIPTION"], &rows, ctx.style(), 0);
    ctx.say(text);
    Ok(0)
}

pub fn show(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let library = beskar.library();
    let id = skill_id(parsed, parsed.arg(0).unwrap_or(""))?;
    let info = library.skill(&id)?;
    let files = library.files(&id)?;
    let total: u64 = files.iter().map(|(_, size)| size).sum();
    let fingerprint = library.fingerprint(&id)?;
    let usage = beskar.skill_usage(&id)?;

    if parsed.flag("json") {
        emit(
            ctx,
            Json::obj([
                ("id", id.to_string().into()),
                ("path", Json::path(&info.path)),
                ("name", info.name.clone().into()),
                ("description", info.description.clone().into()),
                (
                    "metadata",
                    Json::Obj(
                        info.metadata
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone().into()))
                            .collect(),
                    ),
                ),
                ("fingerprint", fingerprint.to_string().into()),
                (
                    "files",
                    Json::arr(files.iter().map(|(p, s)| {
                        Json::obj([("path", p.clone().into()), ("bytes", (*s as i64).into())])
                    })),
                ),
                ("profiles", Json::strings(&usage.profiles)),
                (
                    "used_in",
                    Json::arr(usage.places.iter().map(|p| {
                        Json::obj([
                            ("repository", Json::path(&p.repo)),
                            ("installed", p.installed.into()),
                            ("via", Json::strings(&p.via)),
                        ])
                    })),
                ),
            ]),
        );
        return Ok(0);
    }
    let style = ctx.style();
    ctx.say(style.bold(id.as_str()));
    let mut rows: Vec<(String, String)> = vec![("path".into(), ctx.tilde(&info.path))];
    if let Some(name) = &info.name {
        rows.push(("name".into(), sanitize(name)));
    }
    if let Some(description) = &info.description {
        rows.push(("description".into(), sanitize(description)));
    }
    rows.extend(
        info.metadata
            .iter()
            .map(|(key, value)| (sanitize(key), sanitize(value))),
    );
    rows.push(("fingerprint".into(), fingerprint.short()));
    rows.push((
        "files".into(),
        format!("{}, {}", count(files.len(), "file"), bytes(total)),
    ));
    rows.push(("profiles".into(), names(&usage.profiles)));
    let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (key, value) in rows {
        ctx.say(format!(
            "  {}  {value}",
            style.dim(&format!("{key:<width$}"))
        ));
    }
    if !info.has_skill_file {
        ctx.say(format!(
            "  {}",
            style.yellow(
                "note: this folder has no SKILL.md, so agents will not recognise it as a skill"
            )
        ));
    }
    ctx.say("");
    ctx.say(style.bold(&format!("Files ({})", files.len())));
    for (path, size) in files.iter().take(30) {
        ctx.say(format!(
            "  {}  {}",
            sanitize(path),
            style.dim(&bytes(*size))
        ));
    }
    if files.len() > 30 {
        ctx.say(format!("  ... and {} more", files.len() - 30));
    }
    ctx.say("");
    ctx.say(style.bold(&format!("Used in ({})", usage.places.len())));
    if usage.places.is_empty() {
        ctx.say("  no repository has it installed or asks for it");
    }
    let width = usage
        .places
        .iter()
        .map(|p| ctx.tilde(&p.repo).chars().count())
        .max()
        .unwrap_or(0);
    for place in &usage.places {
        let why = match (place.via.is_empty(), place.installed) {
            (false, true) => format!("[profile: {}]", names(&place.via)),
            (false, false) => format!("[profile: {}, not installed yet]", names(&place.via)),
            (true, _) => "[installed, no longer selected by a profile]".to_string(),
        };
        ctx.say(format!(
            "  {:<width$}  {}",
            ctx.tilde(&place.repo),
            style.dim(&why)
        ));
    }
    Ok(0)
}

pub fn remove(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let library = beskar.library();
    let id = skill_id(parsed, parsed.arg(0).unwrap_or(""))?;
    library.skill(&id)?;
    let question = format!("Delete skill '{id}' from the library?");
    if !confirm(ctx, parsed.flag("yes"), &question, false)? {
        ctx.say("Nothing was removed.");
        return Ok(0);
    }
    let installed = beskar
        .skill_usage(&id)?
        .places
        .iter()
        .filter(|p| p.installed)
        .count();
    let removal = library.remove_skill(&id, parsed.flag("force"))?;
    ctx.say(format!("Removed skill '{id}' from the library."));
    if !removal.profiles_edited.is_empty() {
        ctx.say(format!(
            "Took it out of {}: {}.",
            count(removal.profiles_edited.len(), "profile"),
            names(&removal.profiles_edited)
        ));
    }
    if installed > 0 {
        ctx.say(format!(
            "{} still {} a copy. 'beskar registry update --all' removes the copies that nobody edited.",
            count(installed, "repository"),
            if installed == 1 { "has" } else { "have" }
        ));
    }
    Ok(0)
}

/// A description on a single line, for tables. A multi-line description keeps only its first line.
fn one_line(text: Option<&str>) -> String {
    text.and_then(|t| t.lines().next())
        .unwrap_or("")
        .trim()
        .to_string()
}

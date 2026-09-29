use std::io::{self, IsTerminal};
use std::path::Path;

use beskar_core::fsx;
use beskar_core::inspect;
use beskar_core::library::{AddOutcome, ImportStatus, Library};
use beskar_core::{Error, SkillId};

use crate::app::{CliError, Ctx, Outcome, usage};
use crate::args::Parsed;
use crate::prompt::confirm;
use crate::render::{columns, plural, truncate};

fn skill_id(text: &str) -> Result<SkillId, CliError> {
    Ok(SkillId::new(text)?)
}

pub fn init(ctx: &Ctx, _parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let created = beskar.library().init()?;
    say!(
        "Library at {} {}.",
        ctx.show(beskar.library().root()),
        if created { "created" } else { "already set up" }
    );
    Ok(0)
}

pub fn add(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let path = Path::new(parsed.positional(0).unwrap_or_default());
    let name = parsed.value("name").map(skill_id).transpose()?;
    let outcome = beskar.library().add_skill(path, name, parsed.has("replace"))?;
    let label = |id: &beskar_core::Fingerprint| id.short().to_string();
    match &outcome {
        AddOutcome::Added(fp) => say!("Added to the library ({}).", label(fp)),
        AddOutcome::Replaced { from, to } => {
            say!("Replaced the library copy ({} -> {}).", label(from), label(to));
        }
        AddOutcome::Unchanged(fp) => say!("Already in the library, unchanged ({}).", label(fp)),
    }
    if !path.join("SKILL.md").is_file() {
        say!(
            "note: {} has no SKILL.md, so agents will not recognise it as a skill.",
            path.display()
        );
    }
    Ok(0)
}

pub fn scan(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let library = beskar.library();
    library.require_initialized()?;
    let root = Path::new(parsed.positional(0).unwrap_or_default());
    let candidates = Library::discover(root)?;
    if candidates.is_empty() {
        say!(
            "No skills found under {}. A skill is a directory that contains SKILL.md.",
            root.display()
        );
        return Ok(0);
    }
    let replace = parsed.has("replace");
    let items = library.plan_import(candidates)?;
    say!("Found {} under {}.\n", plural(items.len(), "skill"), ctx.show(root));

    let mut rows = Vec::new();
    for item in &items {
        let name = item.candidate.id.as_ref().map_or_else(
            |_| {
                item.candidate
                    .path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            },
            ToString::to_string,
        );
        let (marker, text) = match &item.status {
            ImportStatus::New => ('+', "new".to_string()),
            ImportStatus::Identical => ('=', "already in the library, identical".to_string()),
            ImportStatus::Differs if replace => {
                ('~', "differs from the library, will be replaced".to_string())
            }
            ImportStatus::Differs => {
                ('~', "differs from the library, skipped (use --replace to overwrite)".to_string())
            }
            ImportStatus::Invalid(reason) => ('!', reason.clone()),
            ImportStatus::Duplicate(first) => {
                ('!', format!("same name as {}, skipped", ctx.show(first)))
            }
        };
        rows.push(vec![marker.to_string(), name, text]);
    }
    put!("{}", columns(&rows, "  "));

    let importable: Vec<_> = items
        .iter()
        .filter(|i| {
            matches!(i.status, ImportStatus::New)
                || (replace && matches!(i.status, ImportStatus::Differs))
        })
        .collect();
    if importable.is_empty() {
        say!("\nNothing to import.");
        return Ok(0);
    }
    let count = plural(importable.len(), "skill");
    if !parsed.has("yes") {
        if !ctx.stdin_is_terminal || !io::stdout().is_terminal() {
            say!(
                "\n{count} can be imported. There is no terminal to ask on, so nothing was imported."
            );
            say!("Run the command again with --yes to import.");
            return Ok(0);
        }
        let stdin = io::stdin();
        let confirmed =
            confirm(&mut stdin.lock(), &mut io::stdout(), &format!("\nImport {count}?"), true)
                .map_err(|e| Error::io("cannot read the answer", &e))?;
        if !confirmed {
            say!("Nothing imported.");
            return Ok(0);
        }
    }

    let mut imported = 0;
    let mut failed = 0;
    for item in importable {
        let Ok(id) = &item.candidate.id else { continue };
        match library.add_skill(&item.candidate.path, Some(id.clone()), replace) {
            Ok(_) => imported += 1,
            Err(error) => {
                failed += 1;
                complain!("error: {id}: {}", error.message());
            }
        }
    }
    say!("\nImported {}.", plural(imported, "skill"));
    Ok(i32::from(failed > 0))
}

pub fn list(ctx: &Ctx, _parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    beskar.library().require_initialized()?;
    let skills = beskar.library().skills()?;
    if skills.is_empty() {
        say!(
            "The library has no skills yet. Import some with `beskar library add <dir>` or `beskar library scan <dir>`."
        );
        return Ok(0);
    }
    let rows: Vec<Vec<String>> = skills
        .iter()
        .map(|s| vec![s.id.to_string(), truncate(s.description().unwrap_or(""), 72)])
        .collect();
    put!("{}", columns(&rows, ""));
    say!("\n{}", plural(skills.len(), "skill"));
    Ok(0)
}

pub fn show(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let library = beskar.library();
    let id = skill_id(parsed.positional(0).unwrap_or_default())?;
    let Some(skill) = library.skill(&id)? else {
        let known = library.skill_ids()?;
        let names: Vec<&str> = known.iter().map(SkillId::as_str).collect();
        let mut error = Error::not_found(format!("no skill named `{id}` in the library"));
        if let Some(near) = beskar_lines::closest(id.as_str(), &names) {
            error = error.with_hint(format!("did you mean `{near}`?"));
        }
        return Err(error.into());
    };

    say!("{}", skill.id);
    let mut rows = vec![vec!["path".to_string(), ctx.show(&skill.path)]];
    if let Some(meta) = &skill.metadata {
        if let Some(name) = &meta.name {
            rows.push(vec!["name".to_string(), name.clone()]);
        }
        if let Some(description) = &meta.description {
            rows.push(vec!["description".to_string(), description.clone()]);
        }
        for (key, value) in &meta.fields {
            rows.push(vec![key.clone(), value.clone()]);
        }
    } else {
        rows.push(vec!["note".to_string(), "no SKILL.md".to_string()]);
    }
    rows.push(vec!["fingerprint".to_string(), skill.fingerprint.short().to_string()]);
    put!("{}", columns(&rows, "  "));

    let profiles: Vec<String> = library
        .profile_ids()?
        .iter()
        .filter_map(|p| library.profile(p).ok().flatten())
        .filter(|p| p.skills.contains(&id))
        .map(|p| p.id.to_string())
        .collect();
    say!(
        "\nIn profiles: {}",
        if profiles.is_empty() { "none".to_string() } else { profiles.join(", ") }
    );

    let registry = beskar.load_registry()?;
    let usage = inspect::skill_usage(library, &registry, &id)?;
    if usage.is_empty() {
        say!("Installed in: no repository");
    } else {
        say!("Installed in:");
        let rows: Vec<Vec<String>> = usage
            .iter()
            .map(|u| {
                let via = if u.via.is_empty() {
                    "[no enabled profile lists it; the next update removes it]".to_string()
                } else {
                    let names: Vec<&str> = u.via.iter().map(|p| p.as_str()).collect();
                    format!("[profile: {}]", names.join(", "))
                };
                vec![ctx.show(&u.repo), via]
            })
            .collect();
        put!("{}", columns(&rows, "  "));
    }

    say!("\nFiles:");
    for entry in fsx::walk(&skill.path)? {
        say!("  {}", entry.rel);
    }
    Ok(0)
}

pub fn remove(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let id = skill_id(parsed.positional(0).ok_or_else(|| usage("missing <skill>"))?)?;
    let stripped = beskar.library().remove_skill(&id, parsed.has("force"))?;
    say!("Removed `{id}` from the library.");
    if !stripped.is_empty() {
        let names: Vec<&str> = stripped.iter().map(|p| p.as_str()).collect();
        say!("Also removed it from: {}.", names.join(", "));
    }
    say!("Repositories that have it keep their copy until `beskar registry update` removes it.");
    Ok(0)
}

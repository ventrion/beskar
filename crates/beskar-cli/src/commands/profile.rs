use beskar_core::inspect;
use beskar_core::{Error, ProfileId, SkillId};

use crate::app::{CliError, Ctx, Outcome};
use crate::args::Parsed;
use crate::render::{columns, plural, truncate};

fn profile_id(text: &str) -> Result<ProfileId, CliError> {
    Ok(ProfileId::new(text)?)
}

fn skill_ids(texts: &[String]) -> Result<Vec<SkillId>, CliError> {
    texts.iter().map(|t| Ok(SkillId::new(t.as_str())?)).collect()
}

pub fn create(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let id = profile_id(parsed.positional(0).unwrap_or_default())?;
    beskar.library().create_profile(&id, parsed.value("description"))?;
    say!("Created profile `{id}` at {}.", ctx.show(&beskar.library().profile_path(&id)));
    say!("Add skills with `beskar profile add {id} <skill>...`.");
    Ok(0)
}

pub fn delete(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let id = profile_id(parsed.positional(0).unwrap_or_default())?;
    let affected = beskar.delete_profile(&id, parsed.has("force"))?;
    say!("Deleted profile `{id}`.");
    if !affected.is_empty() {
        say!("Disabled it in:");
        for repo in &affected {
            say!("  {}", ctx.show(repo));
        }
        say!("Run `beskar registry update` to remove the skills only it wanted.");
    }
    Ok(0)
}

pub fn list(ctx: &Ctx, _parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let library = beskar.library();
    library.require_initialized()?;
    let registry = beskar.load_registry()?;
    let ids = library.profile_ids()?;
    if ids.is_empty() {
        say!("No profiles yet. Create one with `beskar profile create <name>`.");
        return Ok(0);
    }
    let mut rows = Vec::new();
    for id in &ids {
        let used = inspect::profile_users(&registry, id).len();
        let (skills, description) = match library.profile(id) {
            Ok(Some(profile)) => (
                plural(profile.skills.len(), "skill"),
                truncate(profile.description.as_deref().unwrap_or(""), 60),
            ),
            _ => ("unreadable".to_string(), "run `beskar doctor`".to_string()),
        };
        rows.push(vec![
            id.to_string(),
            skills,
            format!(
                "used in {}",
                plural(used, "repository").replace("repositorys", "repositories")
            ),
            description,
        ]);
    }
    put!("{}", columns(&rows, ""));
    Ok(0)
}

pub fn show(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let library = beskar.library();
    let id = profile_id(parsed.positional(0).unwrap_or_default())?;
    library.require_profile(&id)?;
    let Some(profile) = library.profile(&id)? else {
        return Err(Error::not_found(format!("no profile named `{id}`")).into());
    };
    say!("{id}");
    let mut rows = vec![vec!["file".to_string(), ctx.show(&library.profile_path(&id))]];
    if let Some(description) = &profile.description {
        rows.push(vec!["description".to_string(), description.clone()]);
    }
    put!("{}", columns(&rows, "  "));

    say!("\nSkills ({})", profile.skills.len());
    let mut rows = Vec::new();
    for skill in &profile.skills {
        let description = match library.skill(skill)? {
            Some(found) => truncate(found.description().unwrap_or(""), 72),
            None => "(not in the library)".to_string(),
        };
        rows.push(vec![skill.to_string(), description]);
    }
    put!("{}", columns(&rows, "  "));

    let registry = beskar.load_registry()?;
    let users = inspect::profile_users(&registry, &id);
    say!("\nEnabled in ({})", users.len());
    for repo in users {
        say!("  {}", ctx.show(&repo.path));
    }
    Ok(0)
}

pub fn add(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let id = profile_id(parsed.positional(0).unwrap_or_default())?;
    let skills = skill_ids(&parsed.positionals[1..])?;
    let added = beskar.library().add_to_profile(&id, &skills)?;
    let already: Vec<&str> =
        skills.iter().filter(|s| !added.contains(s)).map(SkillId::as_str).collect();
    if !added.is_empty() {
        let names: Vec<&str> = added.iter().map(SkillId::as_str).collect();
        say!("Added {} to `{id}`.", names.join(", "));
    }
    if !already.is_empty() {
        say!("Already in `{id}`: {}.", already.join(", "));
    }
    if !added.is_empty() {
        say!(
            "Repositories that enable `{id}` get the new skills on their next `beskar repo update`."
        );
    }
    Ok(0)
}

pub fn remove(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let id = profile_id(parsed.positional(0).unwrap_or_default())?;
    let skills = skill_ids(&parsed.positionals[1..])?;
    let removed = beskar.library().remove_from_profile(&id, &skills)?;
    let names: Vec<&str> = removed.iter().map(SkillId::as_str).collect();
    say!("Removed {} from `{id}`.", names.join(", "));
    say!(
        "Repositories that enable `{id}` drop them on their next `beskar repo update`, unless another profile wants them."
    );
    Ok(0)
}

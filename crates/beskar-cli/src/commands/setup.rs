//! `init`, `doctor` and `config`.

use std::path::Path;

use beskar_core::app::InitOptions;
use beskar_core::config::KEYS;
use beskar_core::doctor::{self, Severity};
use beskar_core::{Beskar, Config, ConflictPolicy};

use super::{Failure, Result, emit, locate_home, open, resolve_path};
use crate::args::Parsed;
use crate::context::{Context, count, sanitize, table};
use crate::json::Json;

pub fn init(parsed: &Parsed, ctx: &mut Context) -> Result {
    let home = locate_home(ctx, parsed)?;
    let options = InitOptions {
        library: parsed.value("library").map(|p| resolve_path(ctx, p)),
        force: parsed.flag("force"),
    };
    let (_beskar, report) = Beskar::init(home, ctx.env.clone(), &options)?;
    let style = ctx.style();
    let fresh = !report.created.is_empty();
    ctx.say(if fresh {
        style.bold("Beskar is ready.")
    } else {
        style.bold("Beskar is already set up.")
    });
    ctx.say("");
    let mut rows: Vec<(&str, String)> = report
        .created
        .iter()
        .map(|p| ("created", ctx.tilde(p)))
        .collect();
    rows.extend(report.existing.iter().map(|p| ("exists", ctx.tilde(p))));
    rows.sort_by(|a, b| a.1.cmp(&b.1));
    for (what, path) in rows {
        let label = if what == "created" {
            style.green("created")
        } else {
            style.dim("exists ")
        };
        ctx.say(format!("  {label}  {path}"));
    }
    if fresh {
        ctx.say("");
        ctx.say("Next steps:");
        ctx.say("  beskar library scan <folder>    import skills you already have");
        ctx.say("  beskar profile create <name>    group skills into a profile");
        ctx.say("  beskar repo add .               register the current folder as a repository");
    }
    Ok(0)
}

pub fn doctor(parsed: &Parsed, ctx: &mut Context) -> Result {
    let home = locate_home(ctx, parsed)?;
    let report = doctor::run(&home, &ctx.env);
    let (errors, warnings) = (report.errors(), report.warnings());
    if parsed.flag("json") {
        let findings = report.findings.iter().map(|f| {
            Json::obj([
                ("severity", severity_word(f.severity).into()),
                ("area", f.area.into()),
                ("message", f.message.clone().into()),
                ("hint", f.hint.clone().into()),
            ])
        });
        emit(
            ctx,
            Json::obj([
                ("healthy", report.is_healthy().into()),
                ("errors", errors.into()),
                ("warnings", warnings.into()),
                ("findings", Json::arr(findings)),
            ]),
        );
        return Ok(i32::from(!report.is_healthy()));
    }
    let style = ctx.style();
    let area_width = report
        .findings
        .iter()
        .map(|f| f.area.len())
        .max()
        .unwrap_or(0);
    for finding in &report.findings {
        let label = match finding.severity {
            Severity::Ok => style.green("ok   "),
            Severity::Warning => style.yellow("warn "),
            Severity::Error => style.red("error"),
        };
        let indent = " ".repeat(5 + 1 + area_width + 2);
        let message = sanitize(&finding.message);
        let mut lines = message.lines();
        ctx.say(format!(
            "{label} {:<area_width$}  {}",
            finding.area,
            lines.next().unwrap_or("")
        ));
        for line in lines {
            ctx.say(format!("{indent}{line}"));
        }
        if let Some(hint) = &finding.hint {
            let hint = sanitize(hint);
            ctx.say(format!("{indent}{}", style.dim(&format!("hint: {hint}"))));
        }
    }
    ctx.say("");
    if errors == 0 && warnings == 0 {
        ctx.say(style.green("Everything looks good."));
    } else {
        let plural = |n: usize, word: &str| {
            if n == 1 {
                format!("{n} {word}")
            } else {
                format!("{n} {word}s")
            }
        };
        ctx.say(format!(
            "{}, {}.",
            plural(errors, "error"),
            plural(warnings, "warning")
        ));
    }
    Ok(i32::from(!report.is_healthy()))
}

fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::Ok => "ok",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

pub fn config_show(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let config = beskar.config();
    let file = beskar.home().config_file();
    if parsed.flag("json") {
        emit(
            ctx,
            Json::obj([
                ("home", Json::path(beskar.home().root())),
                ("file", Json::path(&file)),
                ("library", Json::path(&config.library)),
                ("registry", Json::path(&config.registry)),
                ("agent-skills", Json::path(&config.agent_skills)),
                ("on-conflict", config.on_conflict.name().into()),
            ]),
        );
        return Ok(0);
    }
    let rows = vec![
        vec!["settings".to_string(), ctx.tilde(&file)],
        vec!["library".to_string(), ctx.tilde(&config.library)],
        vec!["registry".to_string(), ctx.tilde(&config.registry)],
        vec![
            "agent-skills".to_string(),
            config.agent_skills.display().to_string(),
        ],
        vec![
            "on-conflict".to_string(),
            format!(
                "{}  ({})",
                config.on_conflict,
                config.on_conflict.describe()
            ),
        ],
    ];
    let text = table(&["SETTING", "VALUE"], &rows, ctx.style(), 0);
    ctx.say(text);
    Ok(0)
}

pub fn config_set(parsed: &Parsed, ctx: &mut Context) -> Result {
    let home = locate_home(ctx, parsed)?;
    let (Some(key), Some(value)) = (parsed.arg(0), parsed.arg(1)) else {
        return Err(Failure::usage(
            parsed,
            "config set needs a key and a value",
            None,
        ));
    };
    // A setting that does not exist, or a policy that does not exist, is a mistake in the command line.
    if !KEYS.contains(&key) {
        let hint = match bsk::closest(key, KEYS) {
            Some(near) => format!("did you mean '{near}'?"),
            None => format!("settings: {}", KEYS.join(", ")),
        };
        return Err(Failure::usage(
            parsed,
            format!("unknown setting '{key}'"),
            Some(hint),
        ));
    }
    if key == "on-conflict" {
        value
            .parse::<ConflictPolicy>()
            .map_err(|error| Failure::typed(parsed, error))?;
    }
    let before = Config::load(&home, &ctx.env).ok();
    let after = Config::set(&home, &ctx.env, key, value)?;
    ctx.say(format!("Set {key} to {value}."));
    if let Some(before) = before
        && key == "agent-skills"
        && before.agent_skills != after.agent_skills
    {
        warn_about_old_copies(parsed, ctx, &before.agent_skills, &after.agent_skills);
    }
    Ok(0)
}

/// Installed copies do not move when the skills folder changes. Says where they stayed.
fn warn_about_old_copies(parsed: &Parsed, ctx: &mut Context, old: &Path, new: &Path) {
    let Ok(beskar) = open(ctx, parsed) else {
        return;
    };
    let Ok(registry) = beskar.store().read() else {
        return;
    };
    let affected = registry
        .repos()
        .filter(|repo| !repo.installed.is_empty())
        .count();
    if affected == 0 {
        return;
    }
    ctx.say(format!(
        "{} {} skills installed under {}. Those copies stay where they are and Beskar no longer manages them. \
Delete them yourself if you do not want them, then run 'beskar registry update --all' to install into {}.",
        count(affected, "repository"),
        if affected == 1 { "has" } else { "have" },
        old.display(),
        new.display()
    ));
}

pub fn config_path(parsed: &Parsed, ctx: &mut Context) -> Result {
    let home = locate_home(ctx, parsed)?;
    ctx.say(home.config_file().display().to_string());
    Ok(0)
}

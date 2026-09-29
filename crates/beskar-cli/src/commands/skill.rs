//! `skill diff` and `skill promote`.

use beskar_core::app::PromotionRisk;
use beskar_core::diff::ChangeKind;

use super::{Result, confirm, open, resolve_path, skill_id};
use crate::args::Parsed;
use crate::context::{Context, count};

pub fn diff(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let id = skill_id(parsed, parsed.arg(0).unwrap_or(""))?;
    let target = parsed.value("repo").map(|p| resolve_path(ctx, p));
    let diff = beskar.skill_diff(&id, target.as_deref(), &ctx.cwd)?;
    if diff.changes.is_empty() {
        ctx.say(format!(
            "'{id}' in {} is identical to the library's version.",
            ctx.tilde(&diff.repo)
        ));
        return Ok(0);
    }
    ctx.say(diff.text.trim_end());
    ctx.say_err(diff.changes.summary());
    Ok(0)
}

pub fn promote(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let id = skill_id(parsed, parsed.arg(0).unwrap_or(""))?;
    let target = parsed.value("repo").map(|p| resolve_path(ctx, p));
    let promotion = beskar.prepare_promotion(&id, target.as_deref(), &ctx.cwd)?;
    let repo = ctx.tilde(&promotion.repo);
    if promotion.is_nothing_to_do() {
        ctx.say(format!(
            "'{id}' in {repo} is identical to the library's version. Nothing to promote."
        ));
        return Ok(0);
    }
    let style = ctx.style();
    ctx.say(style.bold(&format!("Promote '{id}' from {repo} into the library")));
    for change in &promotion.changes.changes {
        let mark = match change.kind {
            ChangeKind::Added => style.green("A"),
            ChangeKind::Removed => style.red("D"),
            ChangeKind::Modified | ChangeKind::ModeChanged => style.yellow("M"),
        };
        ctx.say(format!(
            "  {mark} {}{}",
            change.path,
            if change.is_dir { "/" } else { "" }
        ));
    }
    ctx.say(format!("  {}", promotion.changes.summary()));
    if !promotion.exists_in_library {
        ctx.say("  The library does not have this skill yet; promoting adds it.");
    }

    let assume_yes = parsed.flag("yes");
    let mut allow_overwrite = parsed.flag("force");
    if promotion.risk != PromotionRisk::None {
        let warning = if promotion.risk == PromotionRisk::OverwritesLibraryChanges {
            "The library changed since this copy was installed. Promoting discards those changes."
        } else {
            "Beskar did not install this copy, so it cannot tell how it relates to the library's version. Promoting replaces the library's version."
        };
        ctx.say(format!("  {}", style.yellow(warning)));
        if !allow_overwrite {
            if ctx.interactive && !assume_yes {
                if !confirm(ctx, false, "Promote anyway?", false)? {
                    ctx.say("Nothing was promoted.");
                    return Ok(0);
                }
                allow_overwrite = true;
            } else {
                // Explains, with a hint, why --yes is not enough here.
                beskar.promote(&promotion, false)?;
            }
        }
    }
    if !confirm(
        ctx,
        assume_yes || allow_overwrite,
        "Replace the library's version with this one?",
        true,
    )? {
        ctx.say("Nothing was promoted.");
        return Ok(0);
    }
    beskar.promote(&promotion, allow_overwrite)?;
    ctx.say(format!("Promoted '{id}' into the library."));
    if !promotion.exists_in_library {
        // Nothing selects a brand new skill yet, and a profile has to before any repository installs it.
        ctx.say(format!(
            "No profile lists it yet. Add it to one with: beskar profile add <profile> {id}"
        ));
        if !promotion.tracked {
            ctx.say(format!(
                "Beskar did not install the folder in {repo}, so updates leave it as it is."
            ));
        }
    }
    if !promotion.other_repos.is_empty() {
        let n = promotion.other_repos.len();
        ctx.say(format!(
            "{} still {} the old version. Bring {} up to date with 'beskar registry update --all'.",
            count(n, "other repository"),
            if n == 1 { "has" } else { "have" },
            if n == 1 { "it" } else { "them" },
        ));
    }
    Ok(0)
}

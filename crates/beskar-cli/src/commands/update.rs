//! `repo update`, `registry update` and the `update` shortcut.

use std::collections::BTreeSet;
use std::path::PathBuf;

use beskar_core::app::{RepoUpdate, UpdateOptions};
use beskar_core::reconcile::{
    self, Action, ConflictKind, Done, PlanEntry, PolicyResolver, Resolver,
};
use beskar_core::{ConflictPolicy, Error, ProfileName, SkillId, Timestamp};

use super::repo::action_word;
use super::{Failure, Result, emit, open, policy, resolve_path};
use crate::args::Parsed;
use crate::context::{Context, Style, count, names, sanitize};
use crate::json::Json;
use crate::prompt::TerminalResolver;

/// Runs `run` with a resolver that asks a person if the policy is `ask` and a terminal is available,
/// and otherwise applies the policy.
pub fn with_resolver<T>(
    ctx: &mut Context,
    policy: ConflictPolicy,
    run: impl FnOnce(&mut dyn Resolver) -> T,
) -> T {
    if policy == ConflictPolicy::Ask && ctx.interactive {
        let style = ctx.err_style();
        let home = ctx.env.user_home.clone();
        let mut resolver = TerminalResolver::new(&mut *ctx.input, &mut *ctx.err, style, home);
        run(&mut resolver)
    } else {
        run(&mut PolicyResolver(policy))
    }
}

pub fn run(parsed: &Parsed, ctx: &mut Context, all: bool) -> Result {
    let beskar = open(ctx, parsed)?;
    let dry_run = parsed.flag("dry-run");
    let verbose = parsed.flag("verbose");
    let json = parsed.flag("json");
    let chosen = policy(parsed, beskar.config().on_conflict)?;
    let options = UpdateOptions {
        dry_run,
        now: Timestamp::now(),
    };
    // With nobody to ask, 'ask' stops at the first conflict, like 'fail'. A dry run reports that too,
    // so its exit status can be trusted as a preview of the real one.
    let stops_on_conflict =
        chosen == ConflictPolicy::Fail || (chosen == ConflictPolicy::Ask && !ctx.interactive);

    let targets: Vec<PathBuf> = if all {
        if parsed.arg(0).is_some() {
            return Err(Failure::usage(
                parsed,
                "a path cannot be combined with --all",
                Some("use either 'beskar update <path>' or 'beskar update --all'".into()),
            ));
        }
        beskar
            .store()
            .read()?
            .repos()
            .map(|r| r.path.clone())
            .collect()
    } else {
        let registry = beskar.store().read()?;
        let target = parsed.arg(0).map(|p| resolve_path(ctx, p));
        vec![
            beskar
                .find_repo(&registry, target.as_deref(), &ctx.cwd)?
                .path,
        ]
    };
    if all && targets.is_empty() && !json {
        ctx.say(
            "No repositories are registered yet. Register one with 'beskar repo add <folder>'.",
        );
        return Ok(0);
    }

    let multi = all;
    let mut failures = 0;
    let mut summary = Summary::default();
    let mut json_items = Vec::new();
    for path in &targets {
        let result = with_resolver(ctx, chosen, |resolver| {
            beskar.update_repo(path, &options, resolver)
        });
        match result {
            Ok(update) => {
                let blocked = if dry_run && stops_on_conflict {
                    reconcile::undecided(&update.plan)
                } else {
                    None
                };
                summary.record(&update, blocked.is_some());
                let problems = update.plan.problems();
                failures += usize::from(!problems.is_empty() || blocked.is_some());
                failures += update
                    .outcome
                    .as_ref()
                    .map_or(0, |o| usize::from(!o.failures().is_empty()));
                if json {
                    json_items.push(repo_json(&update, blocked.as_ref()));
                } else {
                    let view = View {
                        verbose,
                        multi,
                        blocked: blocked.is_some(),
                    };
                    for line in repo_lines(ctx, &update, &view) {
                        ctx.say(line);
                    }
                    if multi {
                        ctx.say("");
                    }
                }
            }
            Err(error) if !multi => return Err(Failure::Core(error)),
            Err(error) => {
                failures += 1;
                summary.failed += 1;
                if json {
                    json_items.push(error_json(path, &error));
                } else {
                    let style = ctx.style();
                    ctx.say(format!(
                        "{} {}",
                        style.dim("Repository:"),
                        style.bold(&ctx.tilde(path))
                    ));
                    for line in sanitize(error.message()).lines() {
                        ctx.say(format!("  {}", style.red(line)));
                    }
                    if let Some(hint) = error.hint() {
                        let hint = sanitize(hint);
                        ctx.say(format!("  {}", style.dim(&format!("hint: {hint}"))));
                    }
                    ctx.say("");
                }
            }
        }
    }
    if json {
        emit(
            ctx,
            Json::obj([
                ("dry_run", dry_run.into()),
                ("repositories", Json::Arr(json_items)),
            ]),
        );
    } else if multi && !targets.is_empty() {
        ctx.say(summary.line(targets.len(), dry_run));
    }
    Ok(i32::from(failures > 0))
}

/// How the results are shown.
struct View {
    verbose: bool,
    multi: bool,
    /// The real run would stop at this repository's conflicts, because nobody can decide them.
    blocked: bool,
}

#[derive(Default)]
struct Summary {
    updated: usize,
    current: usize,
    attention: usize,
    failed: usize,
}

impl Summary {
    /// Files each repository under one heading. Something that failed comes first, then local changes
    /// that were kept (or, in a dry run, still wait for a decision), then changes made, then nothing.
    /// A conflict that was settled by replacing or promoting counts as a change made.
    fn record(&mut self, update: &RepoUpdate, blocked: bool) {
        let failed = !update.plan.problems().is_empty()
            || blocked
            || update
                .outcome
                .as_ref()
                .is_some_and(|o| !o.failures().is_empty());
        let (undecided, changed) = match &update.outcome {
            Some(outcome) => (
                outcome
                    .applied
                    .iter()
                    .any(|a| matches!(a.done, Done::Kept(_))),
                outcome.applied.iter().any(|a| {
                    matches!(
                        a.done,
                        Done::Added
                            | Done::Updated
                            | Done::Removed
                            | Done::Replaced(_)
                            | Done::Promoted(_)
                    )
                }),
            ),
            None => (
                update.plan.conflicts().count() > 0,
                update.plan.changes().count() > 0,
            ),
        };
        if failed {
            self.failed += 1;
        } else if undecided {
            self.attention += 1;
        } else if changed {
            self.updated += 1;
        } else {
            self.current += 1;
        }
    }

    fn line(&self, total: usize, dry_run: bool) -> String {
        let mut parts = Vec::new();
        if self.updated > 0 {
            parts.push(format!(
                "{} {}",
                self.updated,
                if dry_run { "would change" } else { "updated" }
            ));
        }
        if self.current > 0 {
            parts.push(format!("{} already up to date", self.current));
        }
        if self.attention > 0 {
            parts.push(format!(
                "{} {}",
                self.attention,
                if dry_run {
                    "with local changes to decide"
                } else {
                    "kept local changes"
                }
            ));
        }
        if self.failed > 0 {
            parts.push(format!("{} failed", self.failed));
        }
        format!("{}: {}.", count(total, "repository"), parts.join(", "))
    }
}

#[derive(Clone, Copy)]
enum Tone {
    Green,
    Yellow,
    Red,
    Dim,
}

/// What to say about one skill: a symbol, its color and a short explanation.
struct Note {
    symbol: char,
    tone: Tone,
    detail: String,
    show_via: bool,
}

fn note(symbol: char, tone: Tone, detail: impl Into<String>) -> Note {
    Note {
        symbol,
        tone,
        detail: detail.into(),
        show_via: false,
    }
}

impl Note {
    /// Also names the profiles that ask for the skill (shown only with --verbose).
    fn with_via(mut self) -> Note {
        self.show_via = true;
        self
    }
}

/// Where a line goes in the listing: additions, then changes, then removals, then anything that
/// needs a person's eyes.
fn rank(symbol: char) -> u8 {
    match symbol {
        '+' => 0,
        '~' => 1,
        '-' => 2,
        '^' => 3,
        '!' => 4,
        '?' => 5,
        _ => 6,
    }
}

fn render(
    style: &Style,
    skill: &SkillId,
    note: &Note,
    via: &BTreeSet<ProfileName>,
    verbose: bool,
) -> (u8, String) {
    let symbol = note.symbol.to_string();
    let mark = match note.tone {
        Tone::Green => style.green(&symbol),
        Tone::Yellow => style.yellow(&symbol),
        Tone::Red => style.red(&symbol),
        Tone::Dim => style.dim(&symbol),
    };
    let mut text = format!("{mark} {skill}");
    if !note.detail.is_empty() {
        text.push_str("  ");
        text.push_str(&note.detail);
    }
    if verbose && note.show_via && !via.is_empty() {
        text.push_str("  ");
        text.push_str(&style.dim(&format!("({})", names(via))));
    }
    (rank(note.symbol), text)
}

/// What a dry run would do about one skill. Quiet skills show only when `verbose`.
fn plan_note(entry: &PlanEntry, verbose: bool) -> Option<Note> {
    Some(match entry.action {
        Action::Add => note('+', Tone::Green, "").with_via(),
        Action::Update => note('~', Tone::Yellow, "").with_via(),
        Action::Remove => note('-', Tone::Red, ""),
        Action::Conflict(_) => note(
            '!',
            Tone::Red,
            format!("{}; needs a decision", entry.state().describe()),
        ),
        Action::Unavailable => note('?', Tone::Red, "not in the library"),
        Action::Adopt if verbose => note('=', Tone::Dim, "matches the library; will be tracked"),
        Action::Forget if verbose => {
            note('-', Tone::Dim, "already gone; the record will be dropped")
        }
        Action::Unchanged if verbose => note('=', Tone::Dim, ""),
        Action::Adopt | Action::Forget | Action::Unchanged => return None,
    })
}

/// What happened to one skill. Quiet skills show only when `verbose`.
fn done_note(done: &Done, verbose: bool) -> Option<Note> {
    Some(match done {
        Done::Added => note('+', Tone::Green, "").with_via(),
        Done::Updated => note('~', Tone::Yellow, "").with_via(),
        Done::Removed => note('-', Tone::Red, ""),
        Done::Kept(_) => note('!', Tone::Yellow, "kept your local changes"),
        Done::Replaced(ConflictKind::ModifiedRemoval) => {
            note('-', Tone::Red, "removed; local changes discarded")
        }
        Done::Replaced(_) => note(
            '~',
            Tone::Yellow,
            "replaced local changes with the library version",
        ),
        Done::Promoted(ConflictKind::ModifiedRemoval) => note(
            '^',
            Tone::Green,
            "promoted to the library, then removed here",
        ),
        Done::Promoted(_) => note(
            '^',
            Tone::Green,
            "promoted your local changes to the library",
        ),
        Done::Failed(reason) => note('!', Tone::Red, format!("failed: {}", sanitize(reason))),
        Done::Adopted if verbose => note('=', Tone::Dim, "matches the library; now tracked"),
        Done::Forgotten if verbose => note('-', Tone::Dim, "already gone; record dropped"),
        Done::Unchanged if verbose => note('=', Tone::Dim, ""),
        Done::Adopted | Done::Forgotten | Done::Unchanged => return None,
    })
}

/// The line for one skill's result, without the profile names.
pub fn done_line(skill: &SkillId, done: &Done, verbose: bool, style: &Style) -> Option<String> {
    done_note(done, verbose).map(|n| render(style, skill, &n, &BTreeSet::new(), false).1)
}

fn repo_lines(ctx: &Context, update: &RepoUpdate, view: &View) -> Vec<String> {
    let View {
        verbose,
        multi,
        blocked,
    } = *view;
    let style = ctx.style();
    let header = format!(
        "{} {}",
        style.dim("Repository:"),
        style.bold(&ctx.tilde(&update.repo.path))
    );
    let problems = update.plan.problems();
    let waiting = update.plan.changes().count();
    let conflicts = update.plan.conflicts().count();
    if multi && !verbose && problems.is_empty() && waiting == 0 && conflicts == 0 {
        return vec![format!("{header}  {}", style.dim("already up to date"))];
    }
    let mut lines = vec![header, String::new()];
    let mut rows: Vec<(u8, &str, String)> = Vec::new();
    for entry in &update.plan.entries {
        let found = match &update.outcome {
            Some(outcome) => outcome
                .applied
                .iter()
                .find(|a| a.skill == entry.skill)
                .and_then(|a| done_note(&a.done, verbose)),
            None => plan_note(entry, verbose),
        };
        if let Some(found) = found {
            let (rank, text) = render(&style, &entry.skill, &found, &entry.via, verbose);
            rows.push((rank, entry.skill.as_str(), text));
        }
    }
    rows.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    let mut shown = rows.len();
    lines.extend(rows.into_iter().map(|(_, _, text)| text));
    for problem in &problems {
        lines.push(format!("{} {problem}", style.red("problem:")));
        shown += 1;
    }
    if verbose && !update.plan.unmanaged.is_empty() {
        lines.push(style.dim(&format!(
            "Left alone (not installed by Beskar): {}",
            names(&update.plan.unmanaged)
        )));
        shown += 1;
    }
    if shown > 0 {
        lines.push(String::new());
    }
    if blocked {
        lines.push(
            "A real update would stop at the skills marked !, because nobody can decide for them here. \
Choose in advance with --on-conflict keep or --on-conflict replace, or run it in a terminal to decide one by one."
                .to_string(),
        );
    }
    lines.push(match &update.outcome {
        None if waiting + conflicts == 0 && problems.is_empty() => {
            "Already up to date. No files changed.".to_string()
        }
        None => "No files changed.".to_string(),
        Some(outcome) => outcome_footer(outcome),
    });
    lines
}

fn outcome_footer(outcome: &beskar_core::reconcile::Outcome) -> String {
    let tally =
        |wanted: fn(&Done) -> bool| outcome.applied.iter().filter(|a| wanted(&a.done)).count();
    let removed = tally(|d| {
        matches!(
            d,
            Done::Removed
                | Done::Replaced(ConflictKind::ModifiedRemoval)
                | Done::Promoted(ConflictKind::ModifiedRemoval)
        )
    });
    let parts: Vec<(usize, &str)> = vec![
        (tally(|d| matches!(d, Done::Added)), "added"),
        (tally(|d| matches!(d, Done::Updated)), "updated"),
        (removed, "removed"),
        (
            tally(|d| matches!(d, Done::Replaced(k) if *k != ConflictKind::ModifiedRemoval)),
            "replaced",
        ),
        (
            tally(|d| matches!(d, Done::Promoted(k) if *k != ConflictKind::ModifiedRemoval)),
            "promoted",
        ),
        (tally(|d| matches!(d, Done::Kept(_))), "kept"),
        (tally(|d| matches!(d, Done::Failed(_))), "failed"),
    ];
    let text: Vec<String> = parts
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect();
    if text.is_empty() {
        "Already up to date.".to_string()
    } else {
        format!("Done: {}.", text.join(", "))
    }
}

fn done_word(done: &Done) -> &'static str {
    match done {
        Done::Added => "added",
        Done::Updated => "updated",
        Done::Removed => "removed",
        Done::Forgotten => "forgotten",
        Done::Adopted => "adopted",
        Done::Unchanged => "unchanged",
        Done::Kept(_) => "kept",
        Done::Replaced(_) => "replaced",
        Done::Promoted(_) => "promoted",
        Done::Failed(_) => "failed",
    }
}

fn repo_json(update: &RepoUpdate, blocked: Option<&Error>) -> Json {
    let skills = update.plan.entries.iter().map(|entry| {
        let done = update
            .outcome
            .as_ref()
            .and_then(|o| o.applied.iter().find(|a| a.skill == entry.skill))
            .map(|a| &a.done);
        Json::obj([
            ("skill", entry.skill.to_string().into()),
            ("action", action_word(entry.action).into()),
            ("state", entry.state().id().into()),
            ("via", Json::strings(&entry.via)),
            ("result", done.map(done_word).into()),
            (
                "error",
                match done {
                    Some(Done::Failed(reason)) => reason.clone().into(),
                    _ => Json::Null,
                },
            ),
        ])
    });
    let failed = update
        .outcome
        .as_ref()
        .is_some_and(|o| !o.failures().is_empty());
    let broken = failed || blocked.is_some() || !update.plan.problems().is_empty();
    Json::obj([
        ("path", Json::path(&update.repo.path)),
        ("status", if broken { "error" } else { "ok" }.into()),
        ("error", blocked.map_or(Json::Null, Json::error)),
        ("problems", Json::strings(update.plan.problems().iter())),
        ("up_to_date", update.plan.is_up_to_date().into()),
        ("skills", Json::arr(skills)),
        ("unmanaged", Json::strings(&update.plan.unmanaged)),
    ])
}

fn error_json(path: &std::path::Path, error: &Error) -> Json {
    Json::obj([
        ("path", Json::path(path)),
        ("status", "error".into()),
        ("error", Json::error(error)),
        ("problems", Json::arr([])),
        ("up_to_date", false.into()),
        ("skills", Json::arr([])),
        ("unmanaged", Json::arr([])),
    ])
}

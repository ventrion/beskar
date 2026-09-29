//! Presenting and applying reconciliation plans. Shared by `repo update`,
//! `update --all` and `registry update`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use beskar_core::diff;
use beskar_core::reconcile::{ConflictKind, Outcome, SkillPlan};
use beskar_core::{Beskar, ConflictPolicy, Plan, Resolution, Result, SkillId, State, err};

use crate::args::Args;
use crate::ui::{self, Style, Ui, show};
use crate::{Ctx, Exit};

pub struct Options {
    pub dry_run: bool,
    pub policy: ConflictPolicy,
    pub verbose: bool,
}

impl Options {
    pub fn from_args(args: &Args, b: &Beskar) -> Result<Options> {
        let policy = match args.value("on-conflict") {
            None => b.config.on_conflict,
            Some(v) => ConflictPolicy::parse(v).ok_or_else(|| {
                err!("unknown --on-conflict policy `{v}`")
                    .hint(format!("use one of: {}", ConflictPolicy::NAMES.join(", ")))
            })?,
        };
        Ok(Options { dry_run: args.has("dry-run"), policy, verbose: args.has("verbose") })
    }
}

/// Reconcile each repository in turn.
pub fn run(ctx: &Ctx, b: &mut Beskar, repos: &[PathBuf], opts: &Options) -> Exit {
    let mut worst = Exit::Ok;
    let (mut changed, mut current, mut attention, mut failed) = (0, 0, 0, 0);
    let mut promoted = false;
    for (i, path) in repos.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let r = one(ctx, b, path, opts);
        promoted |= r.promoted;
        match r.exit {
            Exit::Ok if r.changed => changed += 1,
            Exit::Ok => current += 1,
            Exit::Attention => attention += 1,
            _ => failed += 1,
        }
        worst = worst.worst(r.exit);
    }
    if repos.len() > 1 {
        let verb = if opts.dry_run { "would change" } else { "updated" };
        println!(
            "\n{}: {changed} {verb}, {current} up to date, {attention} need attention, {failed} failed",
            ui::plural(repos.len(), "repository", "repositories")
        );
    }
    if promoted && repos.len() > 1 {
        println!("{}", ctx.ui.dim("hint: skills were promoted during this run; run `beskar update --all` again so every workspace gets them"));
    }
    if repos.is_empty() {
        println!("No repositories registered.");
        println!("{}", ctx.ui.dim("hint: `beskar repo add <path>`"));
    }
    worst
}

struct RepoResult {
    exit: Exit,
    changed: bool,
    promoted: bool,
}

fn one(ctx: &Ctx, b: &mut Beskar, path: &Path, opts: &Options) -> RepoResult {
    let ui = &ctx.ui;
    println!("{} {}", ui.bold("Repository:"), show(path));
    let fail = |e: beskar_core::Error| {
        crate::report(ui, &e);
        RepoResult { exit: Exit::Failure, changed: false, promoted: false }
    };
    let Some(entry) = b.registry.get(path) else { return fail(err!("{} is not registered", show(path))) };
    let plan = match b.plan(entry) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let missing = plan.skills.iter().any(|s| s.state == State::Missing);
    let base = if missing { Exit::Attention } else { Exit::Ok };
    println!();
    print_plan(ui, &plan, opts.verbose);
    if plan.is_up_to_date() {
        println!("Up to date.");
        return RepoResult { exit: base, changed: false, promoted: false };
    }
    if opts.dry_run {
        println!("No files changed.");
        let exit = if plan.conflicts().next().is_some() { Exit::Attention } else { base };
        return RepoResult { exit, changed: true, promoted: false };
    }
    let resolutions = match resolve(ui, b, &plan, opts.policy) {
        Some(r) => r,
        None => {
            let n = plan.conflicts().count();
            println!(
                "Stopped: {} a decision. Nothing changed in this repository.",
                ui::plural(n, "conflict needs", "conflicts need")
            );
            println!(
                "{}",
                ui.dim("hint: re-run in a terminal, or choose with --on-conflict keep|replace; see `beskar skill diff <skill>`")
            );
            return RepoResult { exit: Exit::Attention, changed: false, promoted: false };
        }
    };
    let applied = match b.apply(&plan, &resolutions) {
        Ok(a) => a,
        Err(e) => return fail(e),
    };
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut exit = base;
    let mut promoted = false;
    for a in &applied {
        let word = match &a.outcome {
            Outcome::Installed | Outcome::Restored => "installed",
            Outcome::Updated => "updated",
            Outcome::Removed => "removed",
            Outcome::Adopted => "recorded",
            Outcome::Forgotten => "forgotten",
            Outcome::Kept => "kept local",
            Outcome::Replaced => "replaced local",
            Outcome::Promoted => {
                promoted = true;
                println!("{} promoted `{}` to the library", ui.paint("↑", Style::Green), a.id);
                "promoted"
            }
            Outcome::Unresolved => {
                exit = exit.worst(Exit::Attention);
                "unresolved"
            }
            Outcome::Failed(e) => {
                exit = Exit::Failure;
                eprintln!("{} {}: {}", ui.paint("error:", Style::Red), a.id, e.message());
                "failed"
            }
        };
        *counts.entry(word).or_default() += 1;
    }
    let summary: Vec<String> = counts.iter().map(|(w, n)| format!("{n} {w}")).collect();
    println!("Done: {}.", summary.join(", "));
    RepoResult { exit, changed: true, promoted }
}

/// Print one line per skill that matters; `verbose` adds unchanged ones.
pub fn print_plan(ui: &Ui, plan: &Plan, verbose: bool) {
    let mut rows = Vec::new();
    for s in &plan.skills {
        let note = match s.state {
            State::Install | State::Update | State::Remove => String::new(),
            State::Restore => "deleted locally; will be restored".into(),
            State::Adopt => "already matches the library; will be recorded".into(),
            State::Forget => "already gone; will be forgotten".into(),
            State::Modified => "modified locally; library unchanged, kept as is".into(),
            State::Missing => format!("not in the library (wanted by {})", s.profiles.join(", ")),
            State::Conflict(kind) => kind.describe().into(),
            State::Clean | State::Untracked if !verbose => continue,
            State::Clean => ui.dim("clean"),
            State::Untracked => ui.dim("not managed by Beskar"),
        };
        rows.push(vec![ui.symbol(s.state), paint_id(ui, s), note]);
    }
    print!("{}", ui::table(&rows));
    if !rows.is_empty() {
        println!();
    }
}

fn paint_id(ui: &Ui, s: &SkillPlan) -> String {
    match s.state {
        State::Clean | State::Untracked => ui.dim(s.id.as_str()),
        _ => s.id.to_string(),
    }
}

/// Decide every conflict in `plan`. `None` means stop without changes.
fn resolve(ui: &Ui, b: &Beskar, plan: &Plan, policy: ConflictPolicy) -> Option<BTreeMap<SkillId, Resolution>> {
    let conflicts: Vec<&SkillPlan> = plan.conflicts().collect();
    let all = |r: Resolution| conflicts.iter().map(|s| (s.id.clone(), r)).collect();
    match policy {
        _ if conflicts.is_empty() => Some(BTreeMap::new()),
        ConflictPolicy::Keep => Some(all(Resolution::Keep)),
        ConflictPolicy::Replace => Some(all(Resolution::Replace)),
        ConflictPolicy::Fail => None,
        ConflictPolicy::Ask if !ui.interactive() => None,
        ConflictPolicy::Ask => {
            let mut out = BTreeMap::new();
            for s in conflicts {
                out.insert(s.id.clone(), ask(ui, b, plan, s)?);
            }
            Some(out)
        }
    }
}

fn ask(ui: &Ui, b: &Beskar, plan: &Plan, s: &SkillPlan) -> Option<Resolution> {
    let State::Conflict(kind) = s.state else { unreachable!("only conflicts are asked about") };
    let (keep, replace, promote) = match kind {
        ConflictKind::Diverged => {
            ("keep local", "replace with library", "promote to library (overwrites the library's newer version)")
        }
        ConflictKind::Collision => (
            "keep the existing directory (skill stays uninstalled)",
            "replace with library",
            "promote to library (replaces the library's version)",
        ),
        ConflictKind::RemoveModified => (
            "keep local (Beskar stops managing it)",
            "remove it, discarding local changes",
            "promote to library, then remove it here",
        ),
    };
    println!("{} {}", ui.paint("Conflict:", Style::Magenta), ui.bold(s.id.as_str()));
    println!("  {}\n", kind.describe());
    println!("  [k] {keep}\n  [l] {replace}\n  [p] {promote}\n  [d] show diff\n");
    loop {
        let answer = ui.ask("Choice [k/l/p/d]: ")?;
        match answer.to_ascii_lowercase().as_str() {
            "k" => return Some(Resolution::Keep),
            "l" => return Some(Resolution::Replace),
            "p" => return Some(Resolution::Promote),
            "d" => {
                let local = plan.skills_dir.join(s.id.as_str());
                print!("{}", render_diff(ui, &b.library.skill_path(&s.id), &local));
            }
            _ => println!("Please answer k, l, p or d."),
        }
    }
}

/// A diff from the library copy to a workspace copy.
pub fn render_diff(ui: &Ui, library: &Path, workspace: &Path) -> String {
    let changes = match diff::diff_dirs(library, workspace) {
        Ok(c) => c,
        Err(e) => return format!("error: {}\n", e.message()),
    };
    if changes.is_empty() {
        return "No differences.\n".to_string();
    }
    let mut out = String::new();
    for c in &changes {
        match c {
            diff::Change::Added(p) => out.push_str(&ui.paint(&format!("only in workspace: {p}\n"), Style::Green)),
            diff::Change::Removed(p) => out.push_str(&ui.paint(&format!("only in library:   {p}\n"), Style::Red)),
            diff::Change::Modified(_) => {}
        }
    }
    for c in &changes {
        if let diff::Change::Modified(p) = c {
            for line in diff::file_diff(library, workspace, p, "library", "workspace").lines() {
                let styled = match line.chars().next() {
                    Some('+') if !line.starts_with("+++") => ui.paint(line, Style::Green),
                    Some('-') if !line.starts_with("---") => ui.paint(line, Style::Red),
                    Some('@') => ui.paint(line, Style::Cyan),
                    _ => line.to_string(),
                };
                out.push_str(&styled);
                out.push('\n');
            }
        }
    }
    out
}

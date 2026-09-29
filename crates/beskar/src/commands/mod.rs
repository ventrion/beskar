//! Command handlers. Each `<group>` has a module; shared plumbing lives here.

mod library;
mod profile;
mod registry;
mod repo;
mod setup;

use std::path::{Path, PathBuf};

use beskar_core::config::ConflictPolicy;
use beskar_core::fsutil::display_path;
use beskar_core::reconcile::{self, Action, Conflict, Plan, PlanItem, Resolution};
use beskar_core::{Beskar, Error, Home, Result};

use crate::args::{self, Flag, Parsed};
use crate::{help, ui, Outcome};

pub const ALL_FLAGS: &[Flag] = &[
    args::YES,
    args::DRY_RUN,
    args::ALL,
    args::ON_CONFLICT,
    args::REPO,
    args::NAME,
    args::REPLACE,
    args::FORCE,
    args::LIBRARY,
    args::DESCRIPTION,
    args::PURGE,
];

/// Route `beskar <group> ...` to its handler.
pub fn dispatch(home: &Home, group: &str, rest: &[String], argv: &[String]) -> Result<Outcome> {
    let sub = rest.first().map(String::as_str);
    match group {
        "init" => setup::init(home, argv),
        "doctor" => setup::doctor(home, argv),
        "config" => setup::config(home, sub, argv),
        "library" => library::run(home, sub, argv),
        "profile" => profile::run(home, sub, argv),
        "repo" => repo::run(home, sub, argv),
        "registry" => registry::run(home, sub, argv),
        // Convenience aliases from the brief.
        "status" => registry::status(home, argv, 1),
        "update" => {
            let probe = args::parse(argv, ALL_FLAGS).map_err(Error::invalid)?;
            if probe.has(&args::ALL) {
                registry::update(home, argv, 1)
            } else {
                repo::update(home, argv, 1)
            }
        }
        other => crate::usage(&format!("unknown command '{other}'"), help::MAIN),
    }
}

/// Parse a command's arguments, or print usage. `skip` is how many leading
/// words (group and subcommand) to drop from the positionals.
pub fn parse_cmd(
    argv: &[String],
    allowed: &[Flag],
    skip: usize,
    help_text: &'static str,
) -> std::result::Result<Parsed, Outcome> {
    match args::parse(argv, allowed) {
        Ok(mut p) => {
            p.positional.drain(..skip.min(p.positional.len()));
            Ok(p)
        }
        Err(msg) => {
            eprintln!("error: {msg}\n");
            eprint!("{help_text}");
            Err(Outcome::Usage)
        }
    }
}

/// A missing or unknown subcommand.
pub fn unknown_sub(group: &str, sub: Option<&str>, help_text: &str) -> Result<Outcome> {
    match sub {
        None => crate::usage(&format!("'beskar {group}' needs a subcommand"), help_text),
        Some(s) => crate::usage(
            &format!("unknown subcommand 'beskar {group} {s}'"),
            help_text,
        ),
    }
}

/// Find the repository an operation applies to: an explicit path, `--repo`,
/// or the registered repository that contains the current directory.
pub fn resolve_repo(beskar: &Beskar, parsed: &Parsed, explicit: Option<&str>) -> Result<PathBuf> {
    let given = explicit.or(parsed.value(&args::REPO));
    match given {
        Some(p) => {
            let abs = beskar_core::fsutil::absolute(Path::new(p))?;
            match beskar.registry.find_containing(&abs) {
                Some(repo) => Ok(repo.path.clone()),
                None => Err(Error::not_found(format!(
                    "{} is not a registered repository (run `beskar repo add {p}`)",
                    display_path(&abs)
                ))),
            }
        }
        None => {
            let cwd = std::env::current_dir().map_err(|e| Error::io(".", e))?;
            let cwd = beskar_core::fsutil::absolute(&cwd)?;
            beskar
                .registry
                .find_containing(&cwd)
                .map(|r| r.path.clone())
                .ok_or_else(|| {
                    Error::not_found(
                        "the current directory is not inside a registered repository \
                     (run `beskar repo add .` or pass --repo <path>)",
                    )
                })
        }
    }
}

/// Print a plan the way the brief sketches it: one marker line per skill.
pub fn print_plan(plan: &Plan, profiles: &[String], verbose: bool) {
    println!("Repository: {}", display_path(&plan.repo));
    if profiles.is_empty() {
        println!("Profiles:   (none enabled)");
    } else {
        println!("Profiles:   {}", profiles.join(", "));
    }
    for p in &plan.missing_profiles {
        println!("            ! profile '{p}' does not exist in the library");
    }
    println!();
    let mut rows = Vec::new();
    for item in &plan.items {
        if !verbose && matches!(item.action, Action::Unchanged | Action::Unmanaged) {
            continue;
        }
        let via = if item.profiles.is_empty() {
            String::new()
        } else {
            format!("[{}]", item.profiles.join(", "))
        };
        rows.push(vec![
            format!("{} {}", item.action.marker(), item.skill),
            item.action.to_string(),
            via,
        ]);
    }
    if rows.is_empty() {
        let n = plan
            .items
            .iter()
            .filter(|i| i.action == Action::Unchanged)
            .count();
        println!("{n} skill(s) installed, all up to date.");
    } else {
        print!("{}", ui::table(&rows));
    }
    let unmanaged: Vec<&str> = plan
        .items
        .iter()
        .filter(|i| i.action == Action::Unmanaged)
        .map(|i| i.skill.as_str())
        .collect();
    if !verbose && !unmanaged.is_empty() {
        println!(
            "\nNot managed by Beskar (left alone): {}",
            unmanaged.join(", ")
        );
    }
}

/// Interactive conflict prompt, following the brief's menu.
pub fn ask_conflict(
    plan: &Plan,
    library_path: &Path,
    item: &PlanItem,
    kind: Conflict,
) -> Result<Resolution> {
    println!(
        "\nConflict: {}\n\n  The workspace copy is {}.",
        item.skill,
        kind.describe()
    );
    let removing = kind == Conflict::RemoveModified;
    let options: &[(char, &str)] = if removing {
        &[
            ('k', "keep local (becomes unmanaged)"),
            ('l', "delete it"),
            ('p', "promote to library, then delete"),
            ('d', "show diff"),
            ('a', "abort"),
        ]
    } else {
        &[
            ('k', "keep local"),
            ('l', "replace with library"),
            ('p', "promote to library"),
            ('d', "show diff"),
            ('a', "abort"),
        ]
    };
    loop {
        match ui::choose("", options)? {
            'k' => return Ok(Resolution::KeepLocal),
            'l' => return Ok(Resolution::UseLibrary),
            'p' => return Ok(Resolution::Promote),
            'a' => return Ok(Resolution::Abort),
            'd' => {
                let lib = library_path.join(&item.skill);
                let ws = plan.skills_dir.join(&item.skill);
                if lib.is_dir() {
                    print!(
                        "{}",
                        beskar_core::diff::render(&lib, &ws, "library", "workspace")?
                    );
                } else {
                    println!(
                        "(the library no longer has '{}'; the workspace copy is all there is)",
                        item.skill
                    );
                }
            }
            _ => unreachable!(),
        }
    }
}

pub struct UpdateSummary {
    pub changed_files: bool,
    pub promoted: Vec<String>,
}

/// Reconcile one repository: plan, print, resolve conflicts, apply, save.
pub fn update_repo(
    beskar: &mut Beskar,
    repo_path: &Path,
    dry_run: bool,
    policy: ConflictPolicy,
) -> Result<UpdateSummary> {
    let repo = beskar.registry.get(repo_path).cloned().ok_or_else(|| {
        Error::not_found(format!("{} is not registered", display_path(repo_path)))
    })?;
    if !repo.path.is_dir() {
        return Err(Error::not_found(format!(
            "{} does not exist (run `beskar registry prune` to forget it)",
            display_path(&repo.path)
        )));
    }
    let plan = reconcile::plan(&beskar.library, &repo, &beskar.config.skills_dir)?;
    print_plan(&plan, &repo.profiles, false);

    if dry_run {
        println!("\nNo files changed (dry run).");
        return Ok(UpdateSummary {
            changed_files: false,
            promoted: Vec::new(),
        });
    }
    if !plan.has_changes() {
        if !plan.is_clean() {
            println!("\nNothing to do.");
        }
        return Ok(UpdateSummary {
            changed_files: false,
            promoted: Vec::new(),
        });
    }

    let policy =
        if policy == ConflictPolicy::Ask && !plan.conflicts().is_empty() && !ui::interactive() {
            return Err(Error::invalid(format!(
                "{}: {} skill(s) have local modifications and there is no terminal to ask on; \
             rerun with --on-conflict keep|replace|fail or set on_conflict in the config",
                display_path(&plan.repo),
                plan.conflicts().len()
            )));
        } else {
            policy
        };
    let library_path = beskar.library.skills_root();
    let mut ask = |item: &PlanItem, kind: Conflict| ask_conflict(&plan, &library_path, item, kind);
    let resolutions = reconcile::resolve(&plan, policy, &mut ask)?;

    let repo_mut = beskar
        .registry
        .get_mut(repo_path)
        .expect("repo still registered");
    let report = reconcile::apply(&plan, &resolutions, &beskar.library, repo_mut)?;
    beskar.registry.save()?;

    println!();
    for a in &report.applied {
        println!("  {}: {}", a.skill, a.what);
    }
    if report.changed_files() {
        println!("\nDone.");
    } else {
        println!("\nNo files changed.");
    }
    Ok(UpdateSummary {
        changed_files: report.changed_files(),
        promoted: report.promoted,
    })
}

/// The conflict policy for an update: flag, else config.
pub fn policy_from(parsed: &Parsed, beskar: &Beskar) -> Result<ConflictPolicy> {
    match parsed.value(&args::ON_CONFLICT) {
        Some(v) => v.parse(),
        None => Ok(beskar.config.on_conflict),
    }
}

pub fn done() -> Result<Outcome> {
    Ok(Outcome::Ok)
}

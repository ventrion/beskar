//! Human rendering (spec §92, §42, §43, §91).
//!
//! Human output goes to stdout; warnings may use stderr. Prose is not stable
//! API (§130) — the stable identifiers (state ids, blocker kinds) come from
//! the core types so humans and machines read the same vocabulary.

use beskar_core::drift::DriftState;
use beskar_core::ids::ProfileId;
use beskar_core::lifecycle::{InstallationReport, OperationOutcome, UpdateAllOutcome, WhyAnswer};
use beskar_core::plan::{PlanAction, ReconciliationPlan};
use beskar_core::registry::Installation;
use beskar_core::status::InstallationStatus;

use crate::json_out::blocker_kind;

/// One-line installation description used across messages.
fn describe(installation: &Installation) -> String {
    format!(
        "{} ({} @ {})",
        installation.workspace.display(),
        installation.target,
        installation.source_ref
    )
}

/// Renders one add/remove/unregister/ref set/update outcome.
pub fn operation(command: &str, outcome: &OperationOutcome, dry_run: bool) {
    let plan = &outcome.plan;
    let installation = &outcome.installation;

    if plan.is_blocked() {
        println!(
            "error: {command} blocked by {} issue(s) — nothing was written:",
            plan.blockers.len()
        );
        print_blockers(plan, installation);
        return;
    }

    if plan.is_no_op() {
        if command == "unregister" && !dry_run {
            // §55/§54: an already-empty installation still loses its record.
            println!(
                "Removing the registry record for {}.",
                describe(installation)
            );
            println!("Registry entry removed.");
        } else {
            println!(
                "Nothing to do — {} is already up to date.",
                describe(installation)
            );
        }
        return;
    }

    if dry_run {
        println!(
            "Dry run for {} — no changes written.",
            describe(installation)
        );
    } else {
        match command {
            "add" => println!(
                "Attached profile(s); reconciled {}.",
                describe(installation)
            ),
            "remove" => {
                println!(
                    "Detached profile(s); reconciled {}.",
                    describe(installation)
                );
                if installation.profiles.is_empty() {
                    println!(
                        "This installation has no attached profiles left; it stays \
                         registered (empty). Remove its record with \
                         `beskar unregister {}` (§54).",
                        installation.workspace.display()
                    );
                }
            }
            "unregister" => println!("Retired managed skills in {}.", describe(installation)),
            "ref set" => println!(
                "Changed the source ref of {} to {:?}; all attached profiles \
                 moved together (§56).",
                describe(installation),
                installation.source_ref
            ),
            _ => println!("Reconciled {}.", describe(installation)),
        }
    }

    print_plan(plan);

    if !dry_run && !plan.is_blocked() {
        if command == "unregister" {
            println!("Registry entry removed.");
        } else if outcome.executed {
            println!("Registry updated.");
        }
    }
}

/// Plan view shared by dry-runs and applied runs (§89, §91).
fn print_plan(plan: &ReconciliationPlan) {
    if !plan.profile_changes.is_empty() {
        println!("Profiles:");
        for change in &plan.profile_changes {
            let verb = match change.action {
                PlanAction::AttachProfile => "attach",
                PlanAction::DetachProfile => "detach",
                _ => "profile",
            };
            println!("  {verb}  {}", change.profile_name);
        }
    }
    if !plan.skill_actions.is_empty() {
        println!("Skills:");
        for action in &plan.skill_actions {
            let mut line = format!("  {:<18} {}", action_verb(action.action), action.skill);
            if !action.paths.is_empty() {
                line.push_str(&format!("  ({})", action.paths.join(", ")));
            }
            println!("{line}");
        }
    }
    if !plan.state_actions.is_empty() {
        println!("State:");
        for action in &plan.state_actions {
            println!("  {}", action_verb(*action));
        }
    }
}

fn action_verb(action: PlanAction) -> &'static str {
    match action {
        PlanAction::AttachProfile => "attach-profile",
        PlanAction::DetachProfile => "detach-profile",
        PlanAction::InstallSkill => "install",
        PlanAction::UpdateSkill => "update",
        PlanAction::ChangeSkillMembership => "membership",
        PlanAction::RetireSkill => "retire",
        PlanAction::PreserveExtra => "preserve-extra",
        PlanAction::OverwriteModified => "overwrite-modified",
        PlanAction::ReplaceUnmanaged => "replace-unmanaged",
        PlanAction::UpdateRegistry => "update-registry",
        PlanAction::CommitLibraryPaths => "commit-library-paths",
        PlanAction::FastForwardBranch => "fast-forward-branch",
        PlanAction::PushBranch => "push-branch",
    }
}

/// Every blocker in one pass, with stable kind identifiers (§47, §60).
pub fn print_blockers(plan: &ReconciliationPlan, installation: &Installation) {
    for blocker in &plan.blockers {
        let mut line = format!("  {:<20}", blocker_kind(blocker.kind));
        match (&blocker.skill, blocker.profile) {
            (Some(skill), _) => line.push_str(skill.as_str()),
            (None, Some(profile_id)) => {
                let name = installation
                    .profiles
                    .iter()
                    .find(|attachment| attachment.id == profile_id)
                    .map(|attachment| attachment.name.clone())
                    .unwrap_or_else(|| profile_id.to_string());
                line.push_str(&name);
            }
            (None, None) => {}
        }
        if !blocker.paths.is_empty() {
            line.push_str(&format!(": {}", blocker.paths.join(", ")));
        }
        println!("{}", line.trim_end());
    }
}

/// The §42-style status block for one installation.
pub fn status(report: &(Installation, InstallationReport)) {
    let (installation, report) = report;
    match report {
        InstallationReport::Failed(message) => {
            println!("{}", installation.workspace.display());
            println!("{} @ {}", installation.target, installation.source_ref);
            println!();
            println!("  ! status failed: {message}");
        }
        InstallationReport::Ready(status) => render_status(status),
    }
}

fn render_status(status: &InstallationStatus) {
    println!("{}", status.workspace.display());
    println!("{} @ {}", status.target, status.source_ref);
    println!();

    if let Some(state) = status.installation_state {
        println!(
            "  ! {} — {}",
            state.id(),
            installation_state_description(state)
        );
        println!();
    }

    // Profiles with resolution outcomes (§39 missing profiles stay visible).
    println!("Profiles");
    for entry in &status.profiles {
        let count = match &entry.profile {
            Some(profile) => format!("{} skills", profile.skills.len()),
            None => format!("missing from library revision (id {})", entry.attachment.id),
        };
        println!("  {:<24}{}", entry.attachment.name, count);
    }
    println!();

    // Effective set: skills required by at least one resolvable profile.
    let desired = status
        .skills
        .values()
        .filter(|skill| !skill.required_by.is_empty())
        .count();
    println!("Effective set");
    println!("  {desired} unique skills");
    println!();

    // One line per skill: glyph, name, state, owners (§42, §130 ids).
    let width = status
        .skills
        .keys()
        .map(|name| name.as_str().len())
        .max()
        .unwrap_or(7)
        .max(18);
    let owner_names = |ids: &[ProfileId]| -> String {
        if ids.is_empty() {
            return "-".to_owned();
        }
        ids.iter()
            .map(|id| {
                status
                    .profiles
                    .iter()
                    .find(|entry| entry.attachment.id == *id)
                    .map(|entry| entry.attachment.name.clone())
                    .unwrap_or_else(|| id.to_string())
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    for (name, skill) in &status.skills {
        let owners = if skill.required_by.is_empty() {
            owner_names(&skill.last_required_by)
        } else {
            owner_names(&skill.required_by)
        };
        let mut notes = String::new();
        if !skill.extra_files.is_empty() {
            notes.push_str(&format!(" (+{} extra)", skill.extra_files.len()));
        }
        if skill.protected_by_missing_profile {
            notes.push_str(" (protected: missing profile, §39)");
        }
        println!(
            "{} {:<width$} {:<14}{}{}",
            glyph(skill.state),
            name,
            skill.state.id(),
            owners,
            notes,
            width = width
        );
    }

    // Unmanaged / unsafe target content Beskar does not own (§49, §12).
    if !status.unmanaged.is_empty() || !status.unsafe_paths.is_empty() {
        println!();
        println!("Unmanaged");
        for entry in &status.unmanaged {
            println!("  {entry}");
        }
        for entry in &status.unsafe_paths {
            println!("  {entry} (unsafe: symlink or special object)");
        }
    }

    // Summary counts (§42), listing only nonzero non-current states.
    println!();
    println!("{} skills", status.skills.len());
    println!("{} profiles", status.profiles.len());
    for state in [
        DriftState::Outdated,
        DriftState::Modified,
        DriftState::ProfileAdded,
        DriftState::ProfileRemoved,
        DriftState::MembershipChanged,
        DriftState::Gap,
        DriftState::Unstamped,
        DriftState::Foreign,
        DriftState::OrphanedManaged,
    ] {
        let count = status.count(state);
        if count > 0 {
            println!("{count} {}", state.id());
        }
    }
}

/// Glyph column (§42: ✓ current, ↑ outdated, ! modified).
fn glyph(state: DriftState) -> &'static str {
    match state {
        DriftState::Current => "✓",
        DriftState::Outdated => "↑",
        DriftState::Modified => "!",
        DriftState::Extra => "e",
        DriftState::Gap => "?",
        DriftState::Unstamped => "x",
        DriftState::Foreign => "F",
        DriftState::ProfileAdded => "+",
        DriftState::ProfileRemoved => "-",
        DriftState::MembershipChanged => "~",
        DriftState::OrphanedManaged => "o",
        DriftState::MissingTarget
        | DriftState::MissingWorkspace
        | DriftState::MissingRef
        | DriftState::MissingProfile => "!",
    }
}

fn installation_state_description(state: DriftState) -> &'static str {
    match state {
        DriftState::MissingWorkspace => "the registered workspace directory does not exist",
        DriftState::MissingTarget => "the registered target directory does not exist",
        DriftState::MissingRef => "the source ref no longer resolves in the library",
        _ => "installation is in a broken state",
    }
}

/// The §43 why answer block.
pub fn why(answer: &WhyAnswer) {
    let installation = &answer.installation;
    println!(
        "{} is installed in {} because it is required by:",
        answer.skill, installation.target
    );
    println!();
    if answer.required_by.is_empty() {
        for (_, name) in &answer.last_required_by {
            println!("  {name} (last applied)");
        }
    } else {
        for (_, name) in &answer.required_by {
            println!("  {name}");
        }
    }
    println!();
    println!("Source ref:");
    println!("  {}", installation.source_ref);
    println!();
    println!("State:");
    println!("  {}", answer.state.id());
}

/// The update --all report (§48).
pub fn update_all(outcome: &UpdateAllOutcome) {
    if outcome.results.is_empty() {
        println!("No registered installations.");
        return;
    }
    if outcome.refused {
        println!(
            "error: update --all refused — at least one installation is \
             blocked; nothing was written (§48):"
        );
        println!();
    } else if outcome.dry_run {
        println!("Dry run — no changes written.");
        println!();
    } else {
        println!("Updating {} installation(s)...", outcome.results.len());
        println!();
    }
    for update in &outcome.results {
        println!("{}", describe(&update.installation));
        if update.plan.is_blocked() {
            println!(
                "  skipped: blocked by {} issue(s):",
                update.plan.blockers.len()
            );
            print_blockers(&update.plan, &update.installation);
        } else if let Some(err) = &update.error {
            println!("  error: {err}");
        } else if update.skipped {
            println!(
                "  skipped: another installation is blocked; nothing was \
                 written (§48)"
            );
        } else if update.plan.is_no_op() {
            println!("  current — nothing to do");
        } else {
            print_plan(&update.plan);
        }
        println!();
    }
    let applied = outcome.results.iter().filter(|r| r.executed).count();
    let skipped = outcome.results.iter().filter(|r| r.skipped).count();
    println!("{applied} applied, {skipped} skipped");
}

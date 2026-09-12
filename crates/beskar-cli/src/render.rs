//! Human rendering (spec §92, §42, §43, §91).
//!
//! Human output goes to stdout; warnings may use stderr. Prose is not stable
//! API (§130) — the stable identifiers (state ids, blocker kinds) come from
//! the core types so humans and machines read the same vocabulary.

use beskar_core::doctor::{DoctorReport, Severity};
use beskar_core::drift::DriftState;
use beskar_core::editing::{
    BranchDisplay, InstallationRef, LibraryOutcome, ProfileValidation, SkillDetail, SkillListing,
    SkillRemovalOutcome,
};
use beskar_core::ids::ProfileId;
use beskar_core::lifecycle::{InstallationReport, OperationOutcome, UpdateAllOutcome, WhyAnswer};
use beskar_core::plan::{PlanAction, ReconciliationPlan};
use beskar_core::profile::Profile;
use beskar_core::registry::Installation;
use beskar_core::remote::{BranchSyncState, FetchOutcome, PushOutcome, PushState, RefSource};
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

// ---- library editing (spec §69-§83, §132) ------------------------------------

fn describe_op(kind: beskar_core::editing::LibraryOpKind) -> &'static str {
    match kind {
        beskar_core::editing::LibraryOpKind::Write => "write",
        beskar_core::editing::LibraryOpKind::Edit => "edit",
        beskar_core::editing::LibraryOpKind::Replace => "replace",
        beskar_core::editing::LibraryOpKind::Remove => "remove",
        beskar_core::editing::LibraryOpKind::Move => "move",
    }
}

/// Renders one library-editing outcome (plan, execution, commit, warnings).
pub fn library_outcome(command: &str, outcome: &LibraryOutcome, dry_run: bool) {
    let plan = &outcome.plan;
    if plan.is_no_op() {
        println!("Nothing to do — the library already has that state.");
        return;
    }
    if dry_run {
        println!("Dry run — no changes written (§91):");
    } else {
        println!("{command}:");
    }
    for op in &plan.ops {
        let mut line = format!("  {:<8} {}", describe_op(op.kind), op.path);
        if let Some(from) = &op.from {
            line.push_str(&format!("  (from {from})"));
        }
        println!("{line}");
    }
    if dry_run {
        println!("Commit message would be: {}", plan.message);
    } else {
        match &outcome.commit {
            Some(commit) => println!("Committed as {commit}: {}", plan.message),
            None => println!("No commit needed — the working tree already matched."),
        }
    }
    for warning in &outcome.warnings {
        eprintln!("warning: {warning}");
    }
}

/// `beskar skill remove` (§75).
pub fn skill_removal(outcome: &SkillRemovalOutcome, dry_run: bool) {
    if !outcome.referencing_profiles.is_empty() {
        println!(
            "Removed from profiles: {}",
            outcome.referencing_profiles.join(", ")
        );
    }
    library_outcome("skill remove", &outcome.outcome, dry_run);
}

/// `beskar skill list` (§80).
pub fn skill_list(listings: &[SkillListing]) {
    if listings.is_empty() {
        println!("No skills in the library.");
        return;
    }
    let width = listings
        .iter()
        .map(|listing| listing.name.as_str().len())
        .max()
        .unwrap_or(4)
        .max(12);
    for listing in listings {
        let mut line = format!(
            "{:<width$}  {:<22}",
            listing.name.as_str(),
            if listing.bucket.is_empty() {
                "-"
            } else {
                &listing.bucket
            },
            width = width
        );
        if let Some(rank) = listing.rank {
            line.push_str(&format!(" r{rank}"));
        }
        if !listing.tags.is_empty() {
            line.push_str(&format!("  [{}]", listing.tags.join(", ")));
        }
        if !listing.profiles.is_empty() {
            line.push_str(&format!("  {{{}}}", listing.profiles.join(", ")));
        }
        println!("{}  {}", line.trim_end(), listing.description);
    }
    println!();
    println!("{} skills", listings.len());
}

/// `beskar skill show`.
pub fn skill_show(detail: &SkillDetail) {
    let listing = &detail.listing;
    println!("{}", listing.name);
    println!("  path:      {}", listing.path);
    println!(
        "  bucket:    {}",
        if listing.bucket.is_empty() {
            "-"
        } else {
            &listing.bucket
        }
    );
    println!("  description: {}", listing.description);
    if !listing.tags.is_empty() {
        println!("  tags:      {}", listing.tags.join(", "));
    }
    if let Some(rank) = listing.rank {
        println!("  rank:      {rank}");
    }
    if let Some(notes) = &listing.notes {
        println!("  notes:     {notes}");
    }
    if !listing.profiles.is_empty() {
        println!("  profiles:  {}", listing.profiles.join(", "));
    }
    if let Some(commit) = &listing.last_commit {
        println!("  last commit: {} ({})", commit.hash, commit.unix_time);
    }
    println!("  files:");
    for file in &detail.files {
        println!("    {file}");
    }
}

/// `beskar profile list` (§76).
pub fn profile_list(profiles: &[Profile]) {
    if profiles.is_empty() {
        println!("No profiles in the library.");
        return;
    }
    let width = profiles
        .iter()
        .map(|profile| profile.name.len())
        .max()
        .unwrap_or(7)
        .max(8);
    for profile in profiles {
        println!(
            "{:<width$}  {:>2} skills  {}",
            profile.name,
            profile.skills.len(),
            profile.description.as_deref().unwrap_or("-"),
            width = width
        );
    }
}

/// `beskar profile show` (§76) with local attachment info (§98).
pub fn profile_show(profile: &Profile, attached: &[InstallationRef]) {
    println!("{}", profile.name);
    println!("  id:          {}", profile.id);
    println!(
        "  description: {}",
        profile.description.as_deref().unwrap_or("-")
    );
    println!("  skills:");
    if profile.skills.is_empty() {
        println!("    (none)");
    }
    for skill in &profile.skills {
        println!("    {skill}");
    }
    if attached.is_empty() {
        println!("  attached installations: (none)");
    } else {
        println!("  attached installations:");
        for installation in attached {
            println!(
                "    {} ({}) @ {}",
                installation.workspace.display(),
                installation.target,
                installation.source_ref
            );
        }
    }
}

/// `beskar profile validate` (§76).
pub fn profile_validate(reports: &[ProfileValidation]) {
    for report in reports {
        let marker = if report.valid { "ok" } else { "!" };
        println!("{marker} {} ({})", report.profile, report.profile);
        if let Some(id) = &report.id {
            println!("   id: {id}");
        }
        for problem in &report.problems {
            println!("   problem: {problem}");
        }
        for missing in &report.missing_skills {
            println!("   missing skill: {missing}");
        }
    }
    let invalid = reports.iter().filter(|report| !report.valid).count();
    println!();
    if invalid == 0 {
        println!("{} profile(s) valid", reports.len());
    } else {
        println!("{invalid} profile(s) with problems");
    }
}

/// `beskar library status` (§81).
pub fn library_status(report: &beskar_core::editing::LibraryStatusReport) {
    println!("{}", report.path.display());
    println!("  library id:   {}", report.library_id);
    println!(
        "  branch:       {}",
        report.branch.as_deref().unwrap_or("(detached HEAD)")
    );
    println!(
        "  HEAD:         {}",
        report.head.as_deref().unwrap_or("(no commits yet)")
    );
    if let Some(upstream) = &report.upstream {
        println!("  upstream:     {upstream}");
        if let (Some(ahead), Some(behind)) = (report.ahead, report.behind) {
            println!("  ahead/behind: {ahead}/{behind}");
        }
    }
    println!("  default ref:  {}", report.default_ref);
    println!(
        "  remote:       {}",
        report.remote.as_deref().unwrap_or("(none)")
    );
    if report.dirty {
        println!("  dirty: yes");
        for change in &report.staged {
            println!("    staged:   {} {}", change.index_status, change.path);
        }
        for change in &report.unstaged {
            println!("    unstaged: {} {}", change.worktree_status, change.path);
        }
        for path in &report.untracked {
            println!("    untracked: {path}");
        }
    } else {
        println!("  dirty: no");
    }
    if report.installations.is_empty() {
        println!("  installations: (none registered)");
    } else {
        println!("  installations:");
        for installation in &report.installations {
            println!(
                "    {} ({}) @ {}",
                installation.workspace.display(),
                installation.target,
                installation.source_ref
            );
        }
    }
}

/// `beskar library branch` (§82).
pub fn branches(branches: &[BranchDisplay]) {
    if branches.is_empty() {
        println!("No branches.");
        return;
    }
    for branch in branches {
        let marker = if branch.current { "*" } else { " " };
        let mut line = format!("{marker} {}", branch.name);
        if let Some(upstream) = &branch.upstream {
            line.push_str(&format!("  -> {upstream}"));
            if let (Some(ahead), Some(behind)) = (branch.ahead, branch.behind)
                && (ahead != 0 || behind != 0)
            {
                line.push_str(&format!("  (+{ahead}/-{behind})"));
            }
        }
        println!("{line}");
    }
}

/// `beskar fetch` (§62-§64).
pub fn fetch(outcome: &FetchOutcome) {
    if outcome.dry_run {
        println!(
            "Fetch plan for {:?} (dry-run — nothing contacted or written):",
            outcome.remote
        );
    } else {
        println!("Fetched {:?} from {}:", outcome.remote, outcome.remote_url);
    }
    if outcome.branches.is_empty() {
        println!("  No relevant local branches to synchronize.");
    }
    for branch in &outcome.branches {
        let relevance = branch
            .relevance
            .iter()
            .map(describe_ref_source)
            .collect::<Vec<_>>()
            .join(", ");
        let mut line = format!("  {:<24} {}", branch.branch, describe_sync_state(branch));
        if !relevance.is_empty() {
            line.push_str(&format!("  [{relevance}]"));
        }
        println!("{line}");
        if let Some(note) = &branch.note {
            println!("      {note}");
        }
    }
    let diverged = outcome
        .branches
        .iter()
        .filter(|b| b.state == BranchSyncState::Diverged)
        .count();
    let moved = outcome
        .branches
        .iter()
        .filter(|b| b.state == BranchSyncState::FastForwarded)
        .count();
    println!();
    println!(
        "{} relevant branche(s); {moved} fast-forwarded; {diverged} diverged",
        outcome.branches.len()
    );
    if outcome.is_action_required() {
        println!(
            "Action required: diverged branches are never merged — integrate \
             them manually (§63)."
        );
    }
}

/// `beskar push` (§65, §66).
pub fn push(outcome: &PushOutcome) {
    match outcome.state {
        PushState::Pushed => {
            let created = if outcome.created_remote_branch {
                " (new remote branch)"
            } else {
                ""
            };
            println!(
                "Pushed {:?} to {:?}{created}.",
                outcome.branch, outcome.remote
            );
        }
        PushState::Current => {
            println!(
                "{:?} is up to date on {:?} — nothing to push.",
                outcome.branch, outcome.remote
            );
        }
        PushState::Planned => {
            let commits = match outcome.ahead {
                Some(ahead) => format!("({ahead} commit(s)) "),
                None => String::new(),
            };
            println!(
                "Would push {:?} to {:?} {commits}(dry-run — nothing pushed).",
                outcome.branch, outcome.remote
            );
        }
        PushState::Unverified => {
            println!(
                "The remote state of {:?} on {:?} is unknown offline; a real \
                 push will consult the remote.",
                outcome.branch, outcome.remote
            );
        }
    }
    match (&outcome.upstream_before, &outcome.upstream_after) {
        (_, Some(after)) if Some(after) != outcome.upstream_before.as_ref() => {
            println!("Upstream: {after}");
        }
        (Some(before), _) => println!("Upstream: {before}"),
        (None, _) => {}
    }
}

/// Why a branch was relevant (§62.4, §64).
fn describe_ref_source(source: &RefSource) -> String {
    match source {
        RefSource::DefaultRef => "default_ref".to_owned(),
        RefSource::CheckedOut => "checked out".to_owned(),
        RefSource::Installation { workspace, target } => {
            format!("installation {} ({target})", workspace.display())
        }
    }
}

/// The §63 rule outcome for one branch, with the relation data.
fn describe_sync_state(branch: &beskar_core::remote::BranchOutcome) -> String {
    let relation = match (branch.ahead, branch.behind) {
        (Some(ahead), Some(behind)) if ahead != 0 || behind != 0 => {
            format!(" (+{ahead}/-{behind}) ")
        }
        _ => String::new(),
    };
    let short = |head: &Option<String>| {
        head.as_ref()
            .map(|hash| hash.chars().take(7).collect::<String>())
            .unwrap_or_else(|| "?".to_owned())
    };
    let state = match branch.state {
        BranchSyncState::Current => "current".to_owned(),
        BranchSyncState::FastForwarded => {
            format!(
                "fast-forwarded {} -> {}",
                short(&branch.old_head),
                short(&branch.new_head)
            )
        }
        BranchSyncState::Planned => {
            format!(
                "would fast-forward {} -> {}",
                short(&branch.old_head),
                short(&branch.new_head)
            )
        }
        BranchSyncState::Ahead => "ahead; untouched".to_owned(),
        BranchSyncState::Diverged => "diverged; untouched — integrate manually".to_owned(),
        BranchSyncState::DirtyCheckedOut => {
            "behind, but checked out with local changes — not moved".to_owned()
        }
        BranchSyncState::Unpublished => "no remote branch".to_owned(),
        BranchSyncState::Unknown => "remote state unknown".to_owned(),
    };
    format!("{state}{relation}")
}

/// `beskar doctor` (§83).
pub fn doctor(report: &DoctorReport) {
    for diagnostic in &report.checks {
        let marker = match diagnostic.severity {
            Severity::Ok => "ok",
            Severity::Warning => "!",
            Severity::Error => "x",
        };
        println!("{marker} {:<26} {}", diagnostic.check, diagnostic.message);
    }
    println!();
    let errors = report
        .checks
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .count();
    let warnings = report
        .checks
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Warning)
        .count();
    if report.has_errors() {
        println!(
            "{errors} error(s), {warnings} warning(s) — doctor never repairs automatically (§83)"
        );
    } else if warnings > 0 {
        println!("{warnings} warning(s), no errors");
    } else {
        println!("All checks passed.");
    }
}

/// `beskar migrate skm` (§120-§124).
pub fn migration(outcome: &beskar_core::migrate::MigrationOutcome) {
    let plan = &outcome.plan;
    println!("skill-manager home: {}", plan.home.display());
    println!(
        "Library: {} (new id {})",
        plan.home.display(),
        plan.library_id
    );
    println!();
    println!("Profiles");
    for profile in &plan.profiles {
        let count = if profile.skills.len() == 1 {
            "1 skill"
        } else {
            "skills"
        };
        if profile.valid {
            println!("  {:<24} {} {count}", profile.name, profile.skills.len());
        } else {
            let missing: Vec<String> = profile
                .missing_skills
                .iter()
                .map(|skill| skill.to_string())
                .collect();
            println!(
                "  {:<24} {} {count}  invalid (missing: {})",
                profile.name,
                profile.skills.len(),
                missing.join(", ")
            );
        }
    }
    if !plan.targets.is_empty() {
        println!();
        println!("Installations");
        for report in &plan.targets {
            println!(
                "  {} {} {}",
                report.workspace.display(),
                report.target,
                if report.source_ref_resolved {
                    "@ resolved ref"
                } else {
                    "@ UNRESOLVED ref"
                }
            );
            let installation = plan
                .installations
                .iter()
                .find(|installation| installation.id == report.installation_id);
            if let Some(installation) = installation {
                let names: Vec<&str> = installation
                    .profiles
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect();
                println!("    profiles: {}", names.join(", "));
            }
            if !report.converted.is_empty() {
                println!(
                    "    stamps converted: {}",
                    report
                        .converted
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if report.needs_reconciliation {
                let mut reasons: Vec<String> = report
                    .unconverted
                    .iter()
                    .chain(report.missing.iter())
                    .map(|s| s.to_string())
                    .collect();
                reasons.extend(report.unmanaged.iter().cloned());
                println!("    needs reconciliation: {}", reasons.join(", "));
            }
        }
    }
    if !plan.warnings.is_empty() {
        println!();
        println!("Warnings");
        for warning in &plan.warnings {
            println!("  ! {warning}");
        }
    }
    println!();
    if outcome.dry_run {
        println!("No changes written. (dry run)");
    } else {
        match &outcome.library_commit {
            Some(commit) => {
                let short = commit.get(..12).unwrap_or(commit);
                println!("Migration commit: {short}");
            }
            None => println!("No library commit was necessary."),
        }
        if outcome.registry_written {
            println!("Registry written: {}", outcome.registry_path.display());
        }
        if plan
            .targets
            .iter()
            .any(|report| report.needs_reconciliation)
        {
            println!(
                "Some installations are marked for reconciliation — run `beskar update --all`."
            );
        }
    }
}

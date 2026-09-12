//! The service bridge: fulfills [`Effect`]s against the shared core
//! services (spec §105, §112).
//!
//! This is the ONLY place the TUI touches beskar-core/beskar-git beyond
//! pure data. Every call goes through the same UI-agnostic services the
//! CLI uses — `Lifecycle`, `LibraryEditor`, `Remote` — so the TUI can never
//! drift from the normative CLI behavior (§105: all interfaces invoke the
//! same domain APIs).

use std::path::PathBuf;

use beskar_core::config::PlatformDirs;
use beskar_core::editing::{
    IngestRequest, LibraryEditor, LibraryOutcome, SkillFilter, SkillRemovalOutcome,
};
use beskar_core::error::{Error, Result};
use beskar_core::ids::{ProfileId, SkillName};
use beskar_core::lifecycle::{
    AddRequest, DetachRequest, Lifecycle, RefSetRequest, ReorderOutcome, ReorderRequest,
    UnregisterRequest, UpdateAllOutcome, UpdateAllRequest, UpdateOneRequest,
};
use beskar_core::reconcile::ReconcileOptions;
use beskar_core::remote::{PushRequest, Remote};
use beskar_core::status::{self, InstallationStatus};

use crate::app::{
    ActionOutcome, InstallationRow, MembershipView, PendingAction, PlannedChange, SkillView,
    Snapshot,
};
use crate::effect::Effect;
use crate::event::Event;

/// One TUI session: the active Library and the three core services over it.
pub struct Services {
    lifecycle: Lifecycle,
    editor: LibraryEditor,
    remote: Remote,
}

impl Services {
    /// Builds a session from explicit parts (dependency-injected; tests
    /// pass temp directories instead of touching the environment, §86).
    pub fn new(dirs: PlatformDirs, library: beskar_core::library::Library) -> Self {
        Self {
            lifecycle: Lifecycle::new(dirs.clone(), library.clone()),
            editor: LibraryEditor::new(dirs.clone(), library.clone()),
            remote: Remote::new(dirs, library),
        }
    }

    /// Builds a session from the process environment (§86): `BESKAR_HOME` /
    /// `BESKAR_LIBRARY` overrides, walk-up library discovery from the
    /// working directory. Fails typed when there is no Library — the CLI
    /// prints the error and exits non-zero (§94).
    pub fn from_env() -> Result<Self> {
        let dirs = PlatformDirs::from_env();
        let cwd = std::env::current_dir()?;
        let library = beskar_core::library::Library::discover(&cwd)?;
        Ok(Self::new(dirs, library))
    }
}

/// Fulfills one effect against the core services, producing the event the
/// reducer consumes next.
pub fn fulfill(services: &Services, effect: Effect) -> Option<Event> {
    match &effect {
        Effect::Refresh => Some(Event::Loaded(gather(services).map(Box::new))),
        Effect::LoadSkill { skill } => Some(Event::SkillLoaded(
            load_skill(services, skill).map(Box::new),
        )),
        Effect::Membership {
            installation,
            skill,
            path,
        } => Some(Event::MembershipLoaded(
            load_membership(services, *installation, skill, path.as_deref()).map(Box::new),
        )),
        Effect::Plan { action, options } => Some(Event::Planned(
            plan(services, action, *options).map(Box::new),
        )),
        Effect::Execute { action, options } => Some(Event::Executed(
            execute(services, action, *options).map(Box::new),
        )),
        Effect::Validate { profile } => {
            Some(Event::Validated(validate(services, profile.as_deref())))
        }
    }
}

// ---- snapshot (§96-§101 inputs) ---------------------------------------------

/// Gathers the full rendering snapshot from core read APIs (§41: read-only,
/// no network).
pub fn gather(services: &Services) -> Result<Snapshot> {
    let library = services.editor.library_status()?;
    let skills = services.editor.list_skills(SkillFilter::default())?;
    let profiles = services.editor.profile_list()?;
    let installations = services
        .lifecycle
        .status(None, None)?
        .into_iter()
        .map(|(installation, report)| {
            let (status, error) = match report {
                beskar_core::lifecycle::InstallationReport::Ready(status) => (Some(*status), None),
                beskar_core::lifecycle::InstallationReport::Failed(message) => {
                    (None, Some(message))
                }
            };
            InstallationRow {
                installation,
                status,
                error,
            }
        })
        .collect();
    let branches = services.editor.list_branches()?;
    Ok(Snapshot {
        library,
        skills,
        profiles,
        installations,
        branches,
    })
}

/// Loads the §97 preview: skill detail plus the `SKILL.md` bytes from the
/// Library working tree (the editing surface the TUI displays).
fn load_skill(services: &Services, skill: &str) -> Result<SkillView> {
    let detail = services.editor.show_skill(skill)?;
    let skill_md_path = services
        .editor
        .library()
        .root()
        .join(to_native(&detail.listing.path))
        .join("SKILL.md");
    let skill_md = std::fs::read_to_string(skill_md_path).unwrap_or_default();
    Ok(SkillView {
        name: detail.listing.name.to_string(),
        detail,
        skill_md,
    })
}

/// Loads the §100 membership view for one skill in one installation.
fn load_membership(
    services: &Services,
    installation_id: beskar_core::ids::InstallationId,
    skill: &str,
    skill_path: Option<&str>,
) -> Result<MembershipView> {
    use beskar_git::GitBackend;

    let registry = services.lifecycle.load_registry()?;
    let installation = registry
        .installations
        .iter()
        .find(|installation| installation.id == installation_id)
        .ok_or_else(|| {
            Error::registry(format!(
                "installation {installation_id} is no longer registered"
            ))
        })?
        .clone();
    let status: InstallationStatus = status::compute_status(
        services.lifecycle.backend(),
        services.lifecycle.library(),
        &installation,
    )?;

    let entry = SkillName::parse(skill)
        .ok()
        .and_then(|name| status.skills.get(&name));
    let state = entry.map(|entry| entry.state);
    let membership_drift = entry.and_then(|entry| match entry.membership_drift {
        beskar_core::drift::MembershipDrift::Unchanged => None,
        drift => Some(drift),
    });
    let required_by = entry
        .map(|entry| owners(&installation, &entry.required_by))
        .unwrap_or_default();
    let last_required_by = entry
        .map(|entry| owners(&installation, &entry.last_required_by))
        .unwrap_or_default();

    // §35: the skill's most recent commit at or before the resolved ref.
    // Git pathspecs accept `/` separators on every platform.
    let skill_commit = match (&status.resolved_commit, skill_path) {
        (Some(commit), Some(path)) => services
            .lifecycle
            .backend()
            .last_commit_touching(services.lifecycle.library().root(), commit, path)
            .ok()
            .flatten(),
        _ => None,
    };

    Ok(MembershipView {
        installation: installation.id,
        skill: skill.to_owned(),
        required_by,
        last_required_by,
        source_ref: installation.source_ref.clone(),
        library_commit: status.resolved_commit.clone(),
        skill_commit,
        state,
        membership_drift,
    })
}

/// Display names for requiring profile IDs (§93): the attachment's
/// last-known name, or the bare ID when no attachment exists (§28).
fn owners(
    installation: &beskar_core::registry::Installation,
    ids: &[ProfileId],
) -> Vec<(ProfileId, String)> {
    ids.iter()
        .map(|id| {
            let name = installation
                .profiles
                .iter()
                .find(|attachment| attachment.id == *id)
                .map(|attachment| attachment.name.clone())
                .unwrap_or_else(|| id.to_string());
            (*id, name)
        })
        .collect()
}

/// Runs the read-only profile validation (§76).
fn validate(
    services: &Services,
    profile: Option<&str>,
) -> Result<Vec<beskar_core::editing::ProfileValidation>> {
    services.editor.profile_validate(profile)
}

// ---- planning (§89 step 2, §91 dry runs) --------------------------------------

/// Plans a pending action as a dry run — the exact planner real execution
/// uses, with zero writes (§135.39).
pub fn plan(
    services: &Services,
    action: &PendingAction,
    options: ReconcileOptions,
) -> Result<PlannedChange> {
    match action {
        PendingAction::AttachProfile {
            workspace,
            target,
            profile,
        } => Ok(install_change(
            format!("Attach profile {profile}"),
            services.lifecycle.add(AddRequest {
                workspace,
                profile,
                target: target.as_deref(),
                adapter: None,
                source_ref: None,
                options,
                dry_run: true,
            })?,
        )),
        PendingAction::DetachProfile {
            workspace,
            target,
            profile,
        } => Ok(install_change(
            format!("Detach profile {profile}"),
            services.lifecycle.remove(DetachRequest {
                workspace,
                profile,
                target: target.as_deref(),
                force: options.force,
                dry_run: true,
            })?,
        )),
        PendingAction::UpdateInstallation { workspace, target } => Ok(install_change(
            "Update installation".to_owned(),
            services.lifecycle.update_one(UpdateOneRequest {
                workspace,
                target: target.as_deref(),
                options,
                dry_run: true,
            })?,
        )),
        PendingAction::UpdateAll => {
            let outcome = services.lifecycle.update_all(UpdateAllRequest {
                options,
                best_effort: false,
                dry_run: true,
            })?;
            Ok(PlannedChange::Summary {
                title: "Update all installations".to_owned(),
                lines: update_all_lines(&outcome),
            })
        }
        PendingAction::RefSet {
            workspace,
            target,
            new_ref,
        } => Ok(install_change(
            format!("Change source ref to {new_ref}"),
            services.lifecycle.ref_set(RefSetRequest {
                workspace,
                target: target.as_deref(),
                new_ref,
                dry_run: true,
            })?,
        )),
        PendingAction::Unregister {
            workspace,
            target,
            keep_files,
        } => {
            if *keep_files {
                return Ok(PlannedChange::Summary {
                    title: "Unregister installation (keep files)".to_owned(),
                    lines: vec![
                        "Remove the registry record only.".to_owned(),
                        "Target contents are never touched (§55).".to_owned(),
                    ],
                });
            }
            Ok(install_change(
                "Unregister installation (retire managed skills)".to_owned(),
                services.lifecycle.unregister(UnregisterRequest {
                    workspace,
                    target: target.as_deref(),
                    keep_files: false,
                    force: options.force,
                    dry_run: true,
                })?,
            ))
        }
        PendingAction::ReorderProfiles {
            workspace,
            target,
            order,
        } => {
            let outcome = services.lifecycle.reorder_attachments(ReorderRequest {
                workspace,
                target: target.as_deref(),
                order,
                dry_run: true,
            })?;
            Ok(reorder_change(&outcome))
        }
        PendingAction::Ingest { source, bucket } => Ok(library_change(
            services.editor.ingest(IngestRequest {
                source,
                bucket,
                replace: false,
                recursive: false,
                dry_run: true,
            })?,
            Vec::new(),
        )),
        PendingAction::SkillMove { skill, bucket } => Ok(library_change(
            services.editor.skill_move(skill, bucket, true)?,
            Vec::new(),
        )),
        PendingAction::SkillRename { old, new } => Ok(library_change(
            services.editor.skill_rename(old, new, true)?,
            Vec::new(),
        )),
        PendingAction::SkillRemove { skill, cascade } => {
            let outcome: SkillRemovalOutcome =
                services.editor.skill_remove(skill, *cascade, true)?;
            let notes = if outcome.referencing_profiles.is_empty() {
                Vec::new()
            } else {
                vec![format!(
                    "referencing profiles: {}",
                    outcome.referencing_profiles.join(", ")
                )]
            };
            Ok(library_change(outcome.outcome, notes))
        }
        PendingAction::SkillTag { skill, add, remove } => Ok(library_change(
            services.editor.skill_tag(skill, add, remove, true)?,
            Vec::new(),
        )),
        PendingAction::SkillRank { skill, rank } => Ok(library_change(
            services.editor.skill_rank(skill, *rank, true)?,
            Vec::new(),
        )),
        PendingAction::ProfileCreate { name, description } => Ok(library_change(
            services
                .editor
                .profile_create(name, description.as_deref(), true)?,
            Vec::new(),
        )),
        PendingAction::ProfileDelete { name } => Ok(library_change(
            services.editor.profile_delete(name, true)?,
            Vec::new(),
        )),
        PendingAction::ProfileRename { old, new } => Ok(library_change(
            services.editor.profile_rename(old, new, true)?,
            Vec::new(),
        )),
        PendingAction::ProfileAddSkills { profile, skills } => Ok(library_change(
            services.editor.profile_add_skills(profile, skills, true)?,
            Vec::new(),
        )),
        PendingAction::ProfileRemoveSkills { profile, skills } => Ok(library_change(
            services
                .editor
                .profile_remove_skills(profile, skills, true)?,
            Vec::new(),
        )),
        PendingAction::ProfileMoveSkill {
            profile,
            skill,
            pivot,
        } => Ok(library_change(
            services
                .editor
                .profile_move_skill(profile, skill, pivot.clone(), true)?,
            Vec::new(),
        )),
        PendingAction::Fetch { remote } => {
            let outcome = services.remote.fetch(beskar_core::remote::FetchRequest {
                remote,
                dry_run: true,
            })?;
            Ok(PlannedChange::Fetch { outcome })
        }
        PendingAction::Push {
            branch,
            set_upstream,
            allow_dirty,
        } => {
            let outcome = services.remote.push(PushRequest {
                branch: branch.as_deref(),
                remote: None,
                set_upstream: *set_upstream,
                allow_dirty: *allow_dirty,
                dry_run: true,
            })?;
            Ok(PlannedChange::Push { outcome })
        }
        PendingAction::SwitchBranch { name } => Ok(PlannedChange::Summary {
            title: format!("Switch to branch {name}"),
            lines: vec![
                format!("Switch the library working tree to branch {name:?} (§82)."),
                "Uncommitted library changes may block the switch; ordinary \
                 Git semantics apply."
                    .to_owned(),
            ],
        }),
        PendingAction::CreateBranch { name } => Ok(PlannedChange::Summary {
            title: format!("Create branch {name}"),
            lines: vec![format!(
                "Create branch {name:?} at HEAD and switch to it (§82)."
            )],
        }),
    }
}

fn install_change(
    title: String,
    outcome: beskar_core::lifecycle::OperationOutcome,
) -> PlannedChange {
    PlannedChange::Install {
        title,
        plan: outcome.plan,
    }
}

fn library_change(outcome: LibraryOutcome, notes: Vec<String>) -> PlannedChange {
    PlannedChange::Library {
        title: outcome.plan.message.clone(),
        plan: outcome.plan,
        notes,
    }
}

fn reorder_change(outcome: &ReorderOutcome) -> PlannedChange {
    let names: Vec<String> = outcome
        .installation
        .profiles
        .iter()
        .map(|attachment| attachment.name.clone())
        .collect();
    PlannedChange::Summary {
        title: "Reorder attached profiles".to_owned(),
        lines: vec![
            "New attachment order (presentation only, §79):".to_owned(),
            format!("  {}", names.join(", ")),
            "Effective membership is unchanged; no skill files are rewritten.".to_owned(),
        ],
    }
}

fn update_all_lines(outcome: &UpdateAllOutcome) -> Vec<String> {
    let mut lines = vec![format!(
        "{} installation(s); strict all-or-nothing mode (§48)",
        outcome.results.len()
    )];
    for result in &outcome.results {
        let state = if result.skipped {
            "blocked/refused"
        } else if result.plan.is_no_op() {
            "current"
        } else {
            "planned"
        };
        lines.push(format!(
            "  {} ({}): {} skill action(s), {} blocker(s)",
            result.installation.target,
            state,
            result.plan.skill_actions.len(),
            result.plan.blockers.len(),
        ));
    }
    if outcome.refused {
        lines.push("At least one installation is blocked; nothing would be applied.".to_owned());
    }
    lines
}

// ---- execution (§89 step 4-5) ---------------------------------------------------

/// Executes a confirmed mutation for real. All domain sequencing — locks,
/// filesystem reconciliation, registry persistence LAST — lives in the core
/// services (§29, §59, §61, §88).
pub fn execute(
    services: &Services,
    action: &PendingAction,
    options: ReconcileOptions,
) -> Result<ActionOutcome> {
    match action {
        PendingAction::AttachProfile {
            workspace,
            target,
            profile,
        } => operation_outcome(
            format!("Attach profile {profile}"),
            services.lifecycle.add(AddRequest {
                workspace,
                profile,
                target: target.as_deref(),
                adapter: None,
                source_ref: None,
                options,
                dry_run: false,
            })?,
        ),
        PendingAction::DetachProfile {
            workspace,
            target,
            profile,
        } => operation_outcome(
            format!("Detach profile {profile}"),
            services.lifecycle.remove(DetachRequest {
                workspace,
                profile,
                target: target.as_deref(),
                force: options.force,
                dry_run: false,
            })?,
        ),
        PendingAction::UpdateInstallation { workspace, target } => operation_outcome(
            "Update installation".to_owned(),
            services.lifecycle.update_one(UpdateOneRequest {
                workspace,
                target: target.as_deref(),
                options,
                dry_run: false,
            })?,
        ),
        PendingAction::UpdateAll => {
            let outcome = services.lifecycle.update_all(UpdateAllRequest {
                options,
                best_effort: false,
                dry_run: false,
            })?;
            Ok(ActionOutcome {
                title: "Update all installations".to_owned(),
                applied: outcome.results.iter().any(|result| result.executed),
                lines: update_all_lines(&outcome),
                warnings: Vec::new(),
            })
        }
        PendingAction::RefSet {
            workspace,
            target,
            new_ref,
        } => operation_outcome(
            format!("Change source ref to {new_ref}"),
            services.lifecycle.ref_set(RefSetRequest {
                workspace,
                target: target.as_deref(),
                new_ref,
                dry_run: false,
            })?,
        ),
        PendingAction::Unregister {
            workspace,
            target,
            keep_files,
        } => operation_outcome(
            if *keep_files {
                "Unregister installation (keep files)".to_owned()
            } else {
                "Unregister installation (retire managed skills)".to_owned()
            },
            services.lifecycle.unregister(UnregisterRequest {
                workspace,
                target: target.as_deref(),
                keep_files: *keep_files,
                force: options.force,
                dry_run: false,
            })?,
        ),
        PendingAction::ReorderProfiles {
            workspace,
            target,
            order,
        } => {
            let outcome = services.lifecycle.reorder_attachments(ReorderRequest {
                workspace,
                target: target.as_deref(),
                order,
                dry_run: false,
            })?;
            Ok(ActionOutcome {
                title: "Reorder attached profiles".to_owned(),
                applied: outcome.executed,
                lines: vec![format!(
                    "attachment order: {}",
                    outcome
                        .installation
                        .profiles
                        .iter()
                        .map(|attachment| attachment.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )],
                warnings: Vec::new(),
            })
        }
        PendingAction::Ingest { source, bucket } => library_outcome(
            format!("Ingest {}", source.display()),
            services.editor.ingest(IngestRequest {
                source,
                bucket,
                replace: false,
                recursive: false,
                dry_run: false,
            })?,
        ),
        PendingAction::SkillMove { skill, bucket } => library_outcome(
            format!("Move {skill}"),
            services.editor.skill_move(skill, bucket, false)?,
        ),
        PendingAction::SkillRename { old, new } => library_outcome(
            format!("Rename {old} to {new}"),
            services.editor.skill_rename(old, new, false)?,
        ),
        PendingAction::SkillRemove { skill, cascade } => {
            let outcome = services.editor.skill_remove(skill, *cascade, false)?;
            let mut lines = vec![format!(
                "commit: {}",
                outcome.outcome.commit.as_deref().unwrap_or("(none)")
            )];
            if !outcome.referencing_profiles.is_empty() {
                lines.push(format!(
                    "removed from profiles: {}",
                    outcome.referencing_profiles.join(", ")
                ));
            }
            let mut removed = library_outcome(format!("Remove {skill}"), outcome.outcome)?;
            removed.lines = lines;
            Ok(removed)
        }
        PendingAction::SkillTag { skill, add, remove } => library_outcome(
            format!("Tag {skill}"),
            services.editor.skill_tag(skill, add, remove, false)?,
        ),
        PendingAction::SkillRank { skill, rank } => library_outcome(
            format!("Rank {skill}"),
            services.editor.skill_rank(skill, *rank, false)?,
        ),
        PendingAction::ProfileCreate { name, description } => library_outcome(
            format!("Create profile {name}"),
            services
                .editor
                .profile_create(name, description.as_deref(), false)?,
        ),
        PendingAction::ProfileDelete { name } => library_outcome(
            format!("Delete profile {name}"),
            services.editor.profile_delete(name, false)?,
        ),
        PendingAction::ProfileRename { old, new } => library_outcome(
            format!("Rename profile {old} to {new}"),
            services.editor.profile_rename(old, new, false)?,
        ),
        PendingAction::ProfileAddSkills { profile, skills } => library_outcome(
            format!("Add skills to {profile}"),
            services.editor.profile_add_skills(profile, skills, false)?,
        ),
        PendingAction::ProfileRemoveSkills { profile, skills } => library_outcome(
            format!("Remove skills from {profile}"),
            services
                .editor
                .profile_remove_skills(profile, skills, false)?,
        ),
        PendingAction::ProfileMoveSkill {
            profile,
            skill,
            pivot,
        } => library_outcome(
            format!("Reorder {skill} in {profile}"),
            services
                .editor
                .profile_move_skill(profile, skill, pivot.clone(), false)?,
        ),
        PendingAction::Fetch { remote } => {
            let outcome = services.remote.fetch(beskar_core::remote::FetchRequest {
                remote,
                dry_run: false,
            })?;
            Ok(ActionOutcome {
                title: format!("Fetch from {}", outcome.remote),
                applied: outcome.branches.iter().any(|branch| {
                    branch.state == beskar_core::remote::BranchSyncState::FastForwarded
                }),
                lines: crate::view::fetch_lines(&outcome),
                warnings: Vec::new(),
            })
        }
        PendingAction::Push {
            branch,
            set_upstream,
            allow_dirty,
        } => {
            let outcome = services.remote.push(PushRequest {
                branch: branch.as_deref(),
                remote: None,
                set_upstream: *set_upstream,
                allow_dirty: *allow_dirty,
                dry_run: false,
            })?;
            Ok(ActionOutcome {
                title: format!("Push {}", outcome.branch),
                applied: outcome.state == beskar_core::remote::PushState::Pushed,
                lines: crate::view::push_lines(&outcome),
                warnings: Vec::new(),
            })
        }
        PendingAction::SwitchBranch { name } => {
            services.editor.switch_branch(name)?;
            Ok(ActionOutcome {
                title: format!("Switch to {name}"),
                applied: true,
                lines: vec![format!("library is now on branch {name:?}")],
                warnings: Vec::new(),
            })
        }
        PendingAction::CreateBranch { name } => {
            services.editor.create_branch(name)?;
            Ok(ActionOutcome {
                title: format!("Create branch {name}"),
                applied: true,
                lines: vec![format!("created and switched to branch {name:?}")],
                warnings: Vec::new(),
            })
        }
    }
}

/// Builds the activity summary for a lifecycle operation.
fn operation_outcome(
    title: String,
    outcome: beskar_core::lifecycle::OperationOutcome,
) -> Result<ActionOutcome> {
    if outcome.plan.is_blocked() {
        // Blocked plans are data, not errors (§47); execution never ran.
        return Ok(ActionOutcome {
            title,
            applied: false,
            lines: crate::view::plan_lines(&outcome.plan),
            warnings: Vec::new(),
        });
    }
    let mut lines = crate::view::plan_lines(&outcome.plan);
    if outcome.created {
        lines.push("created a new installation record".to_owned());
    }
    if outcome.retired {
        lines.push("reconciled toward an empty attachment set (§55)".to_owned());
    }
    if outcome.plan.is_no_op() {
        lines.push("nothing to apply — installation is up to date".to_owned());
    }
    Ok(ActionOutcome {
        title,
        applied: outcome.executed,
        lines,
        warnings: Vec::new(),
    })
}

/// Builds the activity summary for a library-editing operation.
fn library_outcome(title: String, outcome: LibraryOutcome) -> Result<ActionOutcome> {
    let mut lines = Vec::new();
    if let Some(commit) = &outcome.commit {
        lines.push(format!("commit: {commit}"));
    }
    if outcome.plan.is_no_op() {
        lines.push("no changes to apply".to_owned());
    } else {
        for op in &outcome.plan.ops {
            lines.push(match (&op.kind, &op.from) {
                (beskar_core::editing::LibraryOpKind::Move, Some(from)) => {
                    format!("moved {from} → {}", op.path)
                }
                (beskar_core::editing::LibraryOpKind::Remove, _) => {
                    format!("removed {}", op.path)
                }
                _ => format!("wrote {}", op.path),
            });
        }
    }
    Ok(ActionOutcome {
        title,
        applied: outcome.executed,
        lines,
        warnings: outcome.warnings,
    })
}

/// Converts a `/`-separated Library-relative path to native form at the
/// filesystem boundary (§119).
fn to_native(relative: &str) -> PathBuf {
    relative.split('/').collect::<PathBuf>()
}

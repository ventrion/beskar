//! The service bridge: fulfills [`GuiAction`]s against the shared core
//! services (spec §105, §113).
//!
//! This is the ONLY place the GUI touches beskar-core/beskar-git beyond
//! pure data. Every call goes through the same UI-agnostic services the
//! CLI and TUI use — `Lifecycle`, `LibraryEditor`, `Remote` — so the GUI
//! can never drift from the normative CLI behavior (§105: all interfaces
//! invoke the same domain APIs; business logic is never re-implemented in
//! a shell).
//!
//! v1 runs these calls synchronously on the UI thread (all operations are
//! local Git/filesystem work; §8.8: nothing here ever touches the network
//! except explicit fetch/push, which the user confirms first).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use beskar_core::config::PlatformDirs;
use beskar_core::editing::{
    IngestRequest, LibraryEditor, LibraryOutcome, SkillFilter, SkillRemovalOutcome,
};
use beskar_core::error::{Error, Result};
use beskar_core::ids::{InstallationId, ProfileId, SkillName};
use beskar_core::library::{Library, resolve_revision};
use beskar_core::lifecycle::{
    AddRequest, DetachRequest, Lifecycle, RefSetRequest, ReorderOutcome, ReorderRequest,
    UnregisterRequest, UpdateAllOutcome, UpdateAllRequest, UpdateOneRequest,
};
use beskar_core::reconcile::{ReconcileOptions, build_desired_state};
use beskar_core::registry::{Installation, ProfileAttachment};
use beskar_core::remote::{FetchRequest, PushRequest, Remote};
use beskar_core::status::{self, InstallationStatus};
use beskar_git::GitBackend;
use time::OffsetDateTime;

use crate::action::GuiAction;
use crate::preview::{self, MembershipPreview, PreviewMode};
use crate::state::{
    BlockedStop, Executed, InstallationRow, MembershipView, Planned, SkillView, Snapshot,
    fetch_lines, plan_lines, push_lines,
};

/// One GUI session: the active Library and the three core services over it.
pub struct Services {
    lifecycle: Lifecycle,
    editor: LibraryEditor,
    remote: Remote,
}

impl Services {
    /// Builds a session from explicit parts (dependency-injected; tests
    /// pass temp directories instead of touching the environment, §86).
    pub fn new(dirs: PlatformDirs, library: Library) -> Self {
        Self {
            lifecycle: Lifecycle::new(dirs.clone(), library.clone()),
            editor: LibraryEditor::new(dirs.clone(), library.clone()),
            remote: Remote::new(dirs, library),
        }
    }

    /// Builds a session from the process environment (§86): `BESKAR_HOME` /
    /// `BESKAR_LIBRARY` overrides, walk-up library discovery from the
    /// working directory. Fails typed when there is no Library — the GUI
    /// launches anyway and shows the typed error with guidance (§4).
    pub fn from_env() -> Result<Self> {
        let dirs = PlatformDirs::from_env();
        let cwd = std::env::current_dir()?;
        let library = Library::discover(&cwd)?;
        Ok(Self::new(dirs, library))
    }

    pub fn lifecycle(&self) -> &Lifecycle {
        &self.lifecycle
    }
}

// ---- reads (§41, §96-§101 inputs) ----------------------------------------------

/// Gathers the full rendering snapshot from core read APIs (read-only, no
/// network, §41).
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

/// Loads the skill preview: skill detail plus the `SKILL.md` bytes from the
/// Library working tree (the editing surface the GUI displays).
pub fn load_skill(services: &Services, skill: &str) -> Result<SkillView> {
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
pub fn load_membership(
    services: &Services,
    installation_id: InstallationId,
    skill: &str,
    skill_path: Option<&str>,
) -> Result<MembershipView> {
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

/// Runs the read-only profile validation (§76).
pub fn validate_profiles(
    services: &Services,
    profile: Option<&str>,
) -> Result<Vec<beskar_core::editing::ProfileValidation>> {
    services.editor.profile_validate(profile)
}

/// Display names for requiring profile IDs (§93): the attachment's
/// last-known name, or the bare ID when no attachment exists (§28).
fn owners(installation: &Installation, ids: &[ProfileId]) -> Vec<(ProfileId, String)> {
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

// ---- the §104 attachment preview -------------------------------------------------

/// Computes the §104 membership preview for a staged selection: resolves
/// the installation's source ref ONCE, builds the desired state before and
/// after the selection (read-only, committed state only, §8.1), and hands
/// both to the pure classifier.
pub fn attachment_preview(
    services: &Services,
    workspace: &Path,
    target: Option<&str>,
    mode: PreviewMode,
    selection: &[String],
) -> Result<MembershipPreview> {
    let registry = services.lifecycle.load_registry()?;
    let installation = services
        .lifecycle
        .find_installation(&registry, workspace, target)?
        .ok_or_else(|| {
            Error::registry(format!(
                "no installation is registered for {}{}",
                workspace.display(),
                match target {
                    Some(target) => format!(" at {target}"),
                    None => String::new(),
                }
            ))
        })?;

    let backend = services.lifecycle.backend();
    let library = services.lifecycle.library();
    let resolved_commit = backend.resolve_ref(library.root(), &installation.source_ref)?;
    let resolved = resolve_revision(backend, library, &resolved_commit)?;

    let before = build_desired_state(
        backend,
        library,
        &installation.source_ref,
        &installation.profiles,
    )?;

    let proposed_profiles: Vec<ProfileAttachment> = match mode {
        PreviewMode::Attach => {
            let mut proposed = installation.profiles.clone();
            for name in selection {
                let profile = if let Ok(id) = ProfileId::parse(name) {
                    resolved.profiles.get(&id).cloned().ok_or_else(|| {
                        Error::profile(format!(
                            "profile id {name} does not exist in the revision \
                             of ref {:?}",
                            installation.source_ref
                        ))
                    })?
                } else {
                    let id = resolved.profile_by_name.get(name).ok_or_else(|| {
                        Error::profile(format!(
                            "profile {name:?} does not exist in the revision \
                             of ref {:?}",
                            installation.source_ref
                        ))
                    })?;
                    resolved.profiles[id].clone()
                };
                // §22: attaching an already-attached profile is a no-op for
                // that profile — never a duplicate attachment.
                if proposed.iter().any(|a| a.id == profile.id) {
                    continue;
                }
                proposed.push(ProfileAttachment {
                    id: profile.id,
                    name: profile.name.clone(),
                    attached_at: OffsetDateTime::now_utc(),
                });
            }
            proposed
        }
        PreviewMode::Detach => {
            let mut proposed = installation.profiles.clone();
            for name in selection {
                let position = proposed
                    .iter()
                    .position(|attachment| {
                        ProfileId::parse(name).is_ok_and(|id| attachment.id == id)
                            || attachment.name == *name
                    })
                    .ok_or_else(|| {
                        Error::profile_attachment(format!(
                            "{name} is not attached to this installation"
                        ))
                    })?;
                proposed.remove(position);
            }
            proposed
        }
    };

    let after = build_desired_state(
        backend,
        library,
        &installation.source_ref,
        &proposed_profiles,
    )?;

    // §28/§93: display names prefer the resolved revision, falling back to
    // the attachment's last-known name.
    let mut names: BTreeMap<ProfileId, String> = proposed_profiles
        .iter()
        .map(|a| (a.id, a.name.clone()))
        .collect();
    for (id, profile) in &resolved.profiles {
        names.insert(*id, profile.name.clone());
    }

    Ok(preview::membership_preview(
        &names,
        &before.membership(),
        &after.membership(),
        &after,
    ))
}

// ---- planning (§89 step 2, §91 dry runs) ------------------------------------------

/// Plans a pending action as a dry run — the exact planner real execution
/// uses, with zero writes (§135.39).
pub fn plan(services: &Services, action: &GuiAction, options: ReconcileOptions) -> Result<Planned> {
    match action {
        GuiAction::AttachProfile {
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
        GuiAction::AttachProfiles { .. } | GuiAction::DetachProfiles { .. } => {
            // Staged §104 runs are planned by the workflow preview (per
            // profile, each `add`/`remove` plans internally at execution
            // time); there is no single plan to dry-run here.
            Ok(Planned::Summary {
                title: action.title(),
                lines: vec![
                    "Each profile is planned and reconciled as its own \
                     complete transaction when applied (§89, §60)."
                        .to_owned(),
                ],
            })
        }
        GuiAction::UpdateInstallation { workspace, target } => Ok(install_change(
            "Update installation".to_owned(),
            services.lifecycle.update_one(UpdateOneRequest {
                workspace,
                target: target.as_deref(),
                options,
                dry_run: true,
            })?,
        )),
        GuiAction::UpdateAll => {
            let outcome = services.lifecycle.update_all(UpdateAllRequest {
                options,
                best_effort: false,
                dry_run: true,
            })?;
            Ok(Planned::Summary {
                title: "Update all installations".to_owned(),
                lines: update_all_lines(&outcome),
            })
        }
        GuiAction::RefSet {
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
        GuiAction::Unregister {
            workspace,
            target,
            keep_files,
        } => {
            if *keep_files {
                return Ok(Planned::Summary {
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
        GuiAction::ReorderProfiles {
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

        GuiAction::Ingest { source, bucket } => Ok(library_change(
            services.editor.ingest(IngestRequest {
                source,
                bucket,
                replace: false,
                recursive: false,
                dry_run: true,
            })?,
            Vec::new(),
        )),
        GuiAction::SkillMove { skill, bucket } => Ok(library_change(
            services.editor.skill_move(skill, bucket, true)?,
            Vec::new(),
        )),
        GuiAction::SkillRename { old, new } => Ok(library_change(
            services.editor.skill_rename(old, new, true)?,
            Vec::new(),
        )),
        GuiAction::SkillRemove { skill, cascade } => {
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
        GuiAction::SkillTag { skill, add, remove } => Ok(library_change(
            services.editor.skill_tag(skill, add, remove, true)?,
            Vec::new(),
        )),
        GuiAction::SkillRank { skill, rank } => Ok(library_change(
            services.editor.skill_rank(skill, *rank, true)?,
            Vec::new(),
        )),

        GuiAction::ProfileCreate { name, description } => Ok(library_change(
            services
                .editor
                .profile_create(name, description.as_deref(), true)?,
            Vec::new(),
        )),
        GuiAction::ProfileDelete { name } => Ok(library_change(
            services.editor.profile_delete(name, true)?,
            Vec::new(),
        )),
        GuiAction::ProfileRename { old, new } => Ok(library_change(
            services.editor.profile_rename(old, new, true)?,
            Vec::new(),
        )),
        GuiAction::ProfileAddSkills { profile, skills } => Ok(library_change(
            services.editor.profile_add_skills(profile, skills, true)?,
            Vec::new(),
        )),
        GuiAction::ProfileRemoveSkills { profile, skills } => Ok(library_change(
            services
                .editor
                .profile_remove_skills(profile, skills, true)?,
            Vec::new(),
        )),
        GuiAction::ProfileMoveSkill {
            profile,
            skill,
            pivot,
        } => Ok(library_change(
            services
                .editor
                .profile_move_skill(profile, skill, pivot.clone(), true)?,
            Vec::new(),
        )),

        GuiAction::Fetch { remote } => {
            let outcome = services.remote.fetch(FetchRequest {
                remote,
                dry_run: true,
            })?;
            Ok(Planned::Fetch { outcome })
        }
        GuiAction::Push {
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
            Ok(Planned::Push { outcome })
        }
        GuiAction::SwitchBranch { name } => Ok(Planned::Summary {
            title: format!("Switch to branch {name}"),
            lines: vec![
                format!("Switch the library working tree to branch {name:?} (§82)."),
                "Uncommitted library changes may block the switch; ordinary \
                 Git semantics apply."
                    .to_owned(),
            ],
        }),
        GuiAction::CreateBranch { name } => Ok(Planned::Summary {
            title: format!("Create branch {name}"),
            lines: vec![format!(
                "Create branch {name:?} at HEAD and switch to it (§82)."
            )],
        }),
    }
}

fn install_change(title: String, outcome: beskar_core::lifecycle::OperationOutcome) -> Planned {
    Planned::Install {
        title,
        plan: outcome.plan,
    }
}

fn library_change(outcome: LibraryOutcome, notes: Vec<String>) -> Planned {
    Planned::Library {
        title: outcome.plan.message.clone(),
        plan: outcome.plan,
        notes,
    }
}

fn reorder_change(outcome: &ReorderOutcome) -> Planned {
    let names: Vec<String> = outcome
        .installation
        .profiles
        .iter()
        .map(|attachment| attachment.name.clone())
        .collect();
    Planned::Summary {
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

// ---- execution (§89 steps 4-5) ------------------------------------------------------

/// Executes a confirmed mutation for real. All domain sequencing — locks,
/// filesystem reconciliation, registry persistence LAST — lives in the core
/// services (§29, §59, §61, §88).
pub fn execute(
    services: &Services,
    action: &GuiAction,
    options: ReconcileOptions,
) -> Result<Executed> {
    match action {
        GuiAction::AttachProfile {
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
        GuiAction::AttachProfiles {
            workspace,
            target,
            profiles,
        } => run_staged(
            services,
            "Attach",
            workspace,
            target.as_deref(),
            profiles,
            options,
            |services, workspace, profile, options| {
                services.lifecycle.add(AddRequest {
                    workspace,
                    profile,
                    target: target.as_deref(),
                    adapter: None,
                    source_ref: None,
                    options,
                    dry_run: false,
                })
            },
        ),
        GuiAction::DetachProfiles {
            workspace,
            target,
            profiles,
        } => run_staged(
            services,
            "Detach",
            workspace,
            target.as_deref(),
            profiles,
            options,
            |services, workspace, profile, options| {
                services.lifecycle.remove(DetachRequest {
                    workspace,
                    profile,
                    target: target.as_deref(),
                    force: options.force,
                    dry_run: false,
                })
            },
        ),
        GuiAction::UpdateInstallation { workspace, target } => operation_outcome(
            "Update installation".to_owned(),
            services.lifecycle.update_one(UpdateOneRequest {
                workspace,
                target: target.as_deref(),
                options,
                dry_run: false,
            })?,
        ),
        GuiAction::UpdateAll => {
            let outcome = services.lifecycle.update_all(UpdateAllRequest {
                options,
                best_effort: false,
                dry_run: false,
            })?;
            Ok(Executed {
                title: "Update all installations".to_owned(),
                applied: outcome.results.iter().any(|result| result.executed),
                lines: update_all_lines(&outcome),
                warnings: Vec::new(),
                stop: None,
            })
        }
        GuiAction::RefSet {
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
        GuiAction::Unregister {
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
        GuiAction::ReorderProfiles {
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
            Ok(Executed {
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
                stop: None,
            })
        }

        GuiAction::Ingest { source, bucket } => library_outcome(
            format!("Ingest {}", source.display()),
            services.editor.ingest(IngestRequest {
                source,
                bucket,
                replace: false,
                recursive: false,
                dry_run: false,
            })?,
        ),
        GuiAction::SkillMove { skill, bucket } => library_outcome(
            format!("Move {skill}"),
            services.editor.skill_move(skill, bucket, false)?,
        ),
        GuiAction::SkillRename { old, new } => library_outcome(
            format!("Rename {old} to {new}"),
            services.editor.skill_rename(old, new, false)?,
        ),
        GuiAction::SkillRemove { skill, cascade } => {
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
        GuiAction::SkillTag { skill, add, remove } => library_outcome(
            format!("Tag {skill}"),
            services.editor.skill_tag(skill, add, remove, false)?,
        ),
        GuiAction::SkillRank { skill, rank } => library_outcome(
            format!("Rank {skill}"),
            services.editor.skill_rank(skill, *rank, false)?,
        ),

        GuiAction::ProfileCreate { name, description } => library_outcome(
            format!("Create profile {name}"),
            services
                .editor
                .profile_create(name, description.as_deref(), false)?,
        ),
        GuiAction::ProfileDelete { name } => library_outcome(
            format!("Delete profile {name}"),
            services.editor.profile_delete(name, false)?,
        ),
        GuiAction::ProfileRename { old, new } => library_outcome(
            format!("Rename profile {old} to {new}"),
            services.editor.profile_rename(old, new, false)?,
        ),
        GuiAction::ProfileAddSkills { profile, skills } => library_outcome(
            format!("Add skills to {profile}"),
            services.editor.profile_add_skills(profile, skills, false)?,
        ),
        GuiAction::ProfileRemoveSkills { profile, skills } => library_outcome(
            format!("Remove skills from {profile}"),
            services
                .editor
                .profile_remove_skills(profile, skills, false)?,
        ),
        GuiAction::ProfileMoveSkill {
            profile,
            skill,
            pivot,
        } => library_outcome(
            format!("Reorder {skill} in {profile}"),
            services
                .editor
                .profile_move_skill(profile, skill, pivot.clone(), false)?,
        ),

        GuiAction::Fetch { remote } => {
            let outcome = services.remote.fetch(FetchRequest {
                remote,
                dry_run: false,
            })?;
            Ok(Executed {
                title: format!("Fetch from {}", outcome.remote),
                applied: outcome.branches.iter().any(|branch| {
                    branch.state == beskar_core::remote::BranchSyncState::FastForwarded
                }),
                lines: fetch_lines(&outcome),
                warnings: Vec::new(),
                stop: None,
            })
        }
        GuiAction::Push {
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
            Ok(Executed {
                title: format!("Push {}", outcome.branch),
                applied: outcome.state == beskar_core::remote::PushState::Pushed,
                lines: push_lines(&outcome),
                warnings: Vec::new(),
                stop: None,
            })
        }
        GuiAction::SwitchBranch { name } => {
            services.editor.switch_branch(name)?;
            Ok(Executed {
                title: format!("Switch to {name}"),
                applied: true,
                lines: vec![format!("library is now on branch {name:?}")],
                warnings: Vec::new(),
                stop: None,
            })
        }
        GuiAction::CreateBranch { name } => {
            services.editor.create_branch(name)?;
            Ok(Executed {
                title: format!("Create branch {name}"),
                applied: true,
                lines: vec![format!("created and switched to branch {name:?}")],
                warnings: Vec::new(),
                stop: None,
            })
        }
    }
}

/// Runs a staged §104 attach/detach as sequential core operations (§104
/// step 6): each profile plans, reconciles, and persists as one complete
/// §89 transaction (§22 idempotent). The run stops at the first blocked
/// plan, surfacing its REAL plan plus the continuation action (§47: report
/// every blocker, require explicit consent before proceeding).
#[allow(clippy::too_many_arguments)]
fn run_staged(
    services: &Services,
    verb: &str,
    workspace: &Path,
    target: Option<&str>,
    profiles: &[String],
    options: ReconcileOptions,
    mut run_one: impl FnMut(
        &Services,
        &Path,
        &str,
        ReconcileOptions,
    ) -> Result<beskar_core::lifecycle::OperationOutcome>,
) -> Result<Executed> {
    let mut lines = Vec::new();
    let mut warnings = Vec::new();
    let mut applied_any = false;
    for (index, profile) in profiles.iter().enumerate() {
        let outcome = run_one(services, workspace, profile, options)?;
        if outcome.plan.is_blocked() {
            let remaining: Vec<String> = profiles[index..].to_vec();
            lines.push(plan_lines(&outcome.plan).join("\n"));
            warnings.push(format!(
                "stopped at {profile}: blocked; remaining profiles: {}",
                remaining.join(", ")
            ));
            let continuation = match verb {
                "Attach" => GuiAction::AttachProfiles {
                    workspace: workspace.to_path_buf(),
                    target: target.map(str::to_owned),
                    profiles: remaining,
                },
                _ => GuiAction::DetachProfiles {
                    workspace: workspace.to_path_buf(),
                    target: target.map(str::to_owned),
                    profiles: remaining,
                },
            };
            return Ok(Executed {
                title: format!("{verb} {}", profiles.join(", ")),
                applied: applied_any,
                lines,
                warnings,
                stop: Some(BlockedStop {
                    plan: outcome.plan,
                    continuation,
                }),
            });
        }
        let detail = if outcome.plan.is_no_op() {
            "nothing to do (already current, §22)".to_owned()
        } else {
            format!("{} skill action(s)", outcome.plan.skill_actions.len())
        };
        lines.push(format!("{profile}: {detail}"));
        applied_any |= outcome.executed;
        if outcome.retired {
            warnings.push(
                "the attachment set is now empty; the installation stays \
                 registered (§54)"
                    .to_owned(),
            );
        }
    }
    Ok(Executed {
        title: format!("{verb} {}", profiles.join(", ")),
        applied: applied_any,
        lines,
        warnings,
        stop: None,
    })
}

/// Builds the outcome summary for a lifecycle operation.
fn operation_outcome(
    title: String,
    outcome: beskar_core::lifecycle::OperationOutcome,
) -> Result<Executed> {
    if outcome.plan.is_blocked() {
        // Blocked plans are data, not errors (§47); execution never ran.
        return Ok(Executed {
            title,
            applied: false,
            lines: plan_lines(&outcome.plan),
            warnings: Vec::new(),
            stop: None,
        });
    }
    let mut lines = plan_lines(&outcome.plan);
    if outcome.created {
        lines.push("created a new installation record".to_owned());
    }
    if outcome.retired {
        lines.push("reconciled toward an empty attachment set (§55)".to_owned());
    }
    if outcome.plan.is_no_op() {
        lines.push("nothing to apply — installation is up to date".to_owned());
    }
    Ok(Executed {
        title,
        applied: outcome.executed,
        lines,
        warnings: Vec::new(),
        stop: None,
    })
}

/// Builds the outcome summary for a library-editing operation.
fn library_outcome(title: String, outcome: LibraryOutcome) -> Result<Executed> {
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
    Ok(Executed {
        title,
        applied: outcome.executed,
        lines,
        warnings: outcome.warnings,
        stop: None,
    })
}

/// Converts a `/`-separated Library-relative path to native form at the
/// filesystem boundary (§119).
fn to_native(relative: &str) -> PathBuf {
    relative.split('/').collect::<PathBuf>()
}

#[cfg(test)]
mod tests {
    //! Hermetic end-to-end tests for the GUI service bridge (§86, §125):
    //! temp homes and libraries, local Git only, no GUI, no network. The
    //! §104 attach/detach workflow is the GUI's own responsibility, so it
    //! is exercised here through the REAL core services.

    use std::path::PathBuf;

    use beskar_core::config::PlatformDirs;
    use beskar_core::lifecycle::AddRequest;
    use beskar_core::plan::BlockerKind;
    use beskar_core::reconcile::ReconcileOptions;
    use beskar_test_support::TempRoot;
    use beskar_test_support::fs::write_file;
    use beskar_test_support::git::TestRepo;

    use crate::preview::PreviewMode;
    use crate::state::Planned;

    const DEV_ID: &str = "98f1513d-94fa-4ace-907e-544c66233653";
    const RUST_ID: &str = "0b0e1c3a-9d24-4c5a-8a30-2b7f0a519102";

    struct Env {
        #[allow(dead_code)]
        home: TempRoot,
        repo: TestRepo,
        /// Keeps the workspace's parent directory alive for the test.
        #[allow(dead_code)]
        ws_root: TempRoot,
        workspace: PathBuf,
    }

    fn profile_toml(id: &str, name: &str, skills: &[&str]) -> String {
        let list: Vec<String> = skills.iter().map(|s| format!("  \"{s}\",")).collect();
        format!(
            "schema = 1\nid = \"{id}\"\nname = \"{name}\"\ndescription = \"{name} set\"\n\nskills = [\n{}\n]\n",
            list.join("\n")
        )
    }

    impl Env {
        fn new() -> Self {
            let repo = TestRepo::new();
            let root = repo.path();
            write_file(
                root,
                "beskar.toml",
                "schema = 1\nlibrary_id = \"550e8400-e29b-41d4-a716-446655440000\"\ndefault_ref = \"main\"\n",
            );
            for (bucket, name) in [
                ("engineering", "git-workflow"),
                ("quality", "testing"),
                ("engineering/process", "code-review"),
                ("languages", "rust"),
            ] {
                write_file(
                    root,
                    &format!("skills/{bucket}/{name}/SKILL.md"),
                    &format!("---\nname: {name}\ndescription: The {name} skill.\n---\n\nbody\n"),
                );
            }
            write_file(
                root,
                "profiles/dev-core.toml",
                &profile_toml(
                    DEV_ID,
                    "dev-core",
                    &["git-workflow", "testing", "code-review"],
                ),
            );
            write_file(
                root,
                "profiles/rust-development.toml",
                &profile_toml(RUST_ID, "rust-development", &["rust", "testing"]),
            );
            repo.commit_all("beskar: seed library");
            let ws_root = TempRoot::new();
            let workspace = ws_root.child("workspace");
            Self {
                home: TempRoot::new(),
                repo,
                ws_root,
                workspace,
            }
        }

        fn services(&self) -> super::Services {
            let dirs = PlatformDirs::resolve(Some(self.home.path()));
            let library =
                beskar_core::library::Library::open_at(self.repo.path()).expect("valid library");
            super::Services::new(dirs, library)
        }

        fn registry_bytes(&self) -> Vec<u8> {
            std::fs::read(self.home.path().join("data/registry.json")).unwrap_or_default()
        }

        fn target(&self) -> PathBuf {
            self.workspace.join(".agents").join("skills")
        }

        /// Attaches one profile through the core service directly.
        fn attach(&self, services: &super::Services, profile: &str) {
            services
                .lifecycle()
                .add(AddRequest {
                    workspace: &self.workspace,
                    profile,
                    target: None,
                    adapter: None,
                    source_ref: None,
                    options: ReconcileOptions::default(),
                    dry_run: false,
                })
                .expect("attach succeeds");
        }
    }

    fn names<'a>(rows: impl Iterator<Item = &'a crate::preview::MembershipRow>) -> Vec<String> {
        rows.map(|row| row.skill.clone()).collect()
    }

    #[test]
    fn gather_reports_library_and_installations_read_only() {
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");

        let snapshot = super::gather(&services).expect("gather");
        assert_eq!(snapshot.skills.len(), 4);
        assert_eq!(snapshot.profiles.len(), 2);
        assert_eq!(snapshot.installations.len(), 1);
        assert_eq!(snapshot.branches[0].name, "main");
        let row = &snapshot.installations[0];
        assert!(row.error.is_none());
        let status = row.status.as_ref().expect("status computed");
        assert_eq!(status.profiles.len(), 1);
        assert_eq!(
            status.profiles[0].profile.as_ref().expect("resolved").name,
            "dev-core"
        );
        let metrics = snapshot.dashboard();
        assert_eq!(metrics.installations, 1);
        assert_eq!(metrics.attachments, 1);
    }

    #[test]
    fn attachment_preview_attach_shows_install_retain_and_membership_only() {
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");

        let preview = super::attachment_preview(
            &services,
            &env.workspace,
            None,
            PreviewMode::Attach,
            &["rust-development".to_owned()],
        )
        .expect("preview");

        assert_eq!(
            names(preview.rows_of(crate::preview::MembershipChange::Install)),
            vec!["rust"]
        );
        let mut retained = names(preview.rows_of(crate::preview::MembershipChange::Retain));
        retained.sort();
        assert_eq!(retained, vec!["code-review", "git-workflow"]);
        let membership = preview.rows_of(crate::preview::MembershipChange::MembershipOnly);
        let changed: Vec<&crate::preview::MembershipRow> = membership.collect();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].skill, "testing");
        assert_eq!(changed[0].after, vec!["dev-core", "rust-development"]);
        assert!(preview.gaps.is_empty());
        assert!(preview.missing_profiles.is_empty());
        assert!(!preview.resolved_commit.is_empty());
    }

    #[test]
    fn attachment_preview_detach_final_owner_marks_retirement() {
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        env.attach(&services, "rust-development");

        let preview = super::attachment_preview(
            &services,
            &env.workspace,
            None,
            PreviewMode::Detach,
            &["rust-development".to_owned()],
        )
        .expect("preview");

        let retired = names(preview.rows_of(crate::preview::MembershipChange::Retire));
        assert_eq!(retired, vec!["rust"]);
        let changed: Vec<String> =
            names(preview.rows_of(crate::preview::MembershipChange::MembershipOnly));
        assert_eq!(changed, vec!["testing"]);
    }

    #[test]
    fn attachment_preview_is_read_only() {
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        let registry_before = env.registry_bytes();
        let target_before = std::fs::read_dir(env.target()).unwrap().count();

        let preview = super::attachment_preview(
            &services,
            &env.workspace,
            None,
            PreviewMode::Detach,
            &["dev-core".to_owned()],
        )
        .expect("preview computes a real diff");
        assert!(!preview.is_no_op());

        assert_eq!(registry_before, env.registry_bytes(), "registry untouched");
        assert_eq!(
            target_before,
            std::fs::read_dir(env.target()).unwrap().count(),
            "target untouched"
        );
    }

    #[test]
    fn attachment_preview_rejects_unknown_profile_and_foreign_attachment() {
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");

        let err = super::attachment_preview(
            &services,
            &env.workspace,
            None,
            PreviewMode::Attach,
            &["no-such-profile".to_owned()],
        )
        .expect_err("unknown profile fails typed");
        assert_eq!(err.code(), "profile");

        let err = super::attachment_preview(
            &services,
            &env.workspace,
            None,
            PreviewMode::Detach,
            &["rust-development".to_owned()],
        )
        .expect_err("detaching a non-attached profile fails typed");
        assert_eq!(err.code(), "profile_attachment");
    }

    #[test]
    fn staged_attach_executes_sequentially_and_records_the_union() {
        let env = Env::new();
        let services = env.services();

        let executed = super::execute(
            &services,
            &crate::action::GuiAction::AttachProfiles {
                workspace: env.workspace.clone(),
                target: None,
                profiles: vec!["dev-core".to_owned(), "rust-development".to_owned()],
            },
            ReconcileOptions::default(),
        )
        .expect("staged attach runs");
        assert!(executed.applied);
        assert!(executed.stop.is_none());
        assert_eq!(executed.lines.len(), 2);

        // §7.2: one physical copy; §27: union membership recorded.
        assert!(env.target().join("testing/SKILL.md").exists());
        let registry = services.lifecycle().load_registry().expect("registry");
        assert_eq!(registry.installations.len(), 1);
        let installation = &registry.installations[0];
        assert_eq!(installation.profiles.len(), 2, "both attachments persisted");
        let membership = &installation.last_applied.skill_membership.skill_profiles;
        let testing = beskar_core::ids::SkillName::parse("testing").expect("valid name");
        assert_eq!(membership.get(&testing).map(Vec::len), Some(2));
    }

    #[test]
    fn staged_detach_stops_on_blocked_plan_and_force_continuation_finishes() {
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        env.attach(&services, "rust-development");

        // Local modification of a shared managed file (§47 protection).
        write_file(
            &env.target(),
            "testing/SKILL.md",
            "---\nname: testing\ndescription: locally modified\n---\n",
        );

        let executed = super::execute(
            &services,
            &crate::action::GuiAction::DetachProfiles {
                workspace: env.workspace.clone(),
                target: None,
                profiles: vec!["dev-core".to_owned(), "rust-development".to_owned()],
            },
            ReconcileOptions::default(),
        )
        .expect("staged detach stops instead of overwriting");

        // The shared `testing` skill is locally modified, so the core
        // planner (locked Phase 2 behavior, §47 as implemented) blocks the
        // whole installation: the run stops at the FIRST detach with
        // nothing applied, and the continuation covers BOTH profiles.
        let stop = executed.stop.as_ref().expect("stopped on a blocked plan");
        assert!(!executed.applied, "nothing was written without consent");
        assert!(!stop.plan.skill_actions.is_empty());
        assert!(
            stop.plan
                .blockers
                .iter()
                .all(|b| b.kind == BlockerKind::ModifiedContent)
        );
        assert_eq!(stop.plan.blockers[0].paths, vec!["SKILL.md"]);
        assert_eq!(
            stop.continuation,
            crate::action::GuiAction::DetachProfiles {
                workspace: env.workspace.clone(),
                target: None,
                profiles: vec!["dev-core".to_owned(), "rust-development".to_owned()],
            }
        );
        // The modified file survived the refused run (§8.5).
        assert!(env.target().join("testing/SKILL.md").exists());
        let registry = services.lifecycle().load_registry().expect("registry");
        assert_eq!(registry.installations[0].profiles.len(), 2);

        // Explicit consent (§47) finishes the retirement; extra files in
        // retired skills are preserved (§8.6, §51).
        write_file(&env.target(), "testing/extra.txt", "user data");
        let executed = super::execute(
            &services,
            &stop.continuation.clone(),
            ReconcileOptions {
                force: true,
                replace_unmanaged: false,
            },
        )
        .expect("forced continuation runs");
        assert!(executed.applied);
        assert!(executed.stop.is_none());
        assert!(!env.target().join("testing/SKILL.md").exists());
        assert!(env.target().join("testing/extra.txt").exists());
        let registry = services.lifecycle().load_registry().expect("registry");
        let installation = &registry.installations[0];
        assert!(
            installation.profiles.is_empty(),
            "§54: empty installation stays registered"
        );
    }

    #[test]
    fn plan_produces_dry_runs_without_writes() {
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        let registry_before = env.registry_bytes();

        let planned = super::plan(
            &services,
            &crate::action::GuiAction::UpdateInstallation {
                workspace: env.workspace.clone(),
                target: None,
            },
            ReconcileOptions::default(),
        )
        .expect("plan");
        let Planned::Install { plan, .. } = planned else {
            panic!("expected an installation plan");
        };
        assert!(plan.is_no_op(), "fresh install is current");
        assert_eq!(
            registry_before,
            env.registry_bytes(),
            "§91: dry run writes nothing"
        );

        let planned = super::plan(
            &services,
            &crate::action::GuiAction::AttachProfile {
                workspace: env.workspace.clone(),
                target: None,
                profile: "rust-development".to_owned(),
            },
            ReconcileOptions::default(),
        )
        .expect("plan");
        let Planned::Install { plan, .. } = planned else {
            panic!("expected an installation plan");
        };
        // §90: the shared `testing` skill changes membership even though
        // its files stay put — both actions are in the plan.
        assert_eq!(plan.skill_actions.len(), 2);
        let has = |kind: beskar_core::plan::PlanAction| {
            plan.skill_actions
                .iter()
                .any(|action| action.action == kind)
        };
        assert!(has(beskar_core::plan::PlanAction::InstallSkill));
        assert!(has(beskar_core::plan::PlanAction::ChangeSkillMembership));
        assert_eq!(
            registry_before,
            env.registry_bytes(),
            "§91: dry run writes nothing"
        );
    }

    #[test]
    fn membership_view_explains_a_shared_skill() {
        let env = Env::new();
        let services = env.services();
        env.attach(&services, "dev-core");
        env.attach(&services, "rust-development");

        let registry = services.lifecycle().load_registry().expect("registry");
        let id = registry.installations[0].id;
        let view = super::load_membership(&services, id, "testing", Some("skills/quality/testing"))
            .expect("membership view");
        assert_eq!(view.required_by.len(), 2, "§93: both requirers");
        assert_eq!(view.source_ref, "main");
        assert_eq!(view.state, Some(beskar_core::drift::DriftState::Current));
        assert!(view.library_commit.is_some());
    }
}

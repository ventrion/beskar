//! Graphical rendering (spec §102-§104, §113).
//!
//! Thin egui code over the pure [`GuiState`]: it draws state and forwards
//! user intent to the [`crate::controller`], which runs the shared core
//! services and feeds results back. No domain logic lives here — the UI
//! classifies nothing itself and renders typed core data only (§105, §115).

use egui::{Button, Color32, Context, Grid, RichText, ScrollArea, Ui, Window};

use beskar_core::drift::DriftState;

use crate::action::GuiAction;
use crate::app::BeskarApp;
use crate::controller;
use crate::preview::{MembershipChange, PreviewMode};
use crate::services::Services;
use crate::state::{Dialog, GuiState, Page, drift_glyph, short_display};
use beskar_core::config::PlatformDirs;

pub fn root(app: &mut BeskarApp, ui: &mut Ui) {
    // Settings-page data is extracted up front so the body closure can
    // borrow state and services without aliasing `app`.
    let settings = SettingsContext {
        dirs: app.services.as_ref().map(|s| s.lifecycle().dirs().clone()),
        git_version: app.git_version.clone(),
        beskar_home_override: app.beskar_home_override,
        beskar_library_override: app.beskar_library_override,
        services_error: app.services_error.clone(),
    };
    let state = &mut app.state;
    let services = app.services.as_ref();

    top_bar(state, ui);
    ui.separator();

    if let Some(error) = &settings.services_error {
        banner(ui, error, true);
        ui.separator();
    }
    if let Some(error) = state.load_error.clone() {
        banner(ui, &error, true);
        ui.separator();
    }

    egui::ScrollArea::vertical()
        .id_salt("page-body")
        .show(ui, |ui| match state.page {
            Page::Dashboard => page_dashboard(state, services, ui),
            Page::Skills => page_skills(state, services, ui),
            Page::Profiles => page_profiles(state, services, ui),
            Page::Installations => page_installations(state, services, ui),
            Page::Git => page_git(state, services, ui),
            Page::Settings => page_settings(state, &settings, ui),
            Page::Activity => page_activity(state, ui),
        });

    ui.separator();
    status_bar(state, ui);
    dialogs(state, services, ui.ctx());
}

/// Read-only Settings-page context, extracted from the app shell once per
/// frame (§85, §86).
struct SettingsContext {
    dirs: Option<PlatformDirs>,
    git_version: Option<String>,
    beskar_home_override: bool,
    beskar_library_override: bool,
    services_error: Option<String>,
}

fn top_bar(state: &mut GuiState, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("BESKAR").strong());
        ui.label("Better Skill Arrangement");
        ui.separator();
        for page in Page::ALL {
            let selected = state.page == page;
            if ui.selectable_label(selected, page.title()).clicked() {
                state.goto(page);
            }
        }
    });
}

fn status_bar(state: &mut GuiState, ui: &mut Ui) {
    ui.horizontal(|ui| {
        if let Some(busy) = state.busy.clone() {
            ui.label(RichText::new(format!("⏳ {busy}")).weak());
        }
        if let Some(toast) = state.toast.clone() {
            ui.label(toast);
        }
        let failures = state
            .activity
            .entries
            .iter()
            .filter(|entry| entry.failed)
            .count();
        if failures > 0 {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{failures} failure(s) in Activity"))
                        .color(Color32::LIGHT_RED),
                );
            });
        }
    });
}

fn banner(ui: &mut Ui, text: &str, failed: bool) {
    let color = if failed {
        Color32::LIGHT_RED
    } else {
        Color32::LIGHT_BLUE
    };
    ui.colored_label(color, RichText::new(text));
}

// ---- Dashboard (§96 equivalent) -------------------------------------------------

fn page_dashboard(state: &mut GuiState, services: Option<&Services>, ui: &mut Ui) {
    ui.heading("Dashboard");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if button(ui, services.is_some(), "Refresh") {
            let services = services.expect("enabled");
            controller::refresh(state, Some(services));
        }
        if button(ui, services.is_some(), "Update all installations…") {
            let services = services.expect("enabled");
            controller::plan_action(state, services, GuiAction::UpdateAll);
        }
    });
    ui.add_space(4.0);

    let Some(snapshot) = state.snapshot.clone() else {
        ui.label("No library loaded. Run `beskar init`, or set BESKAR_LIBRARY (§86).");
        return;
    };
    let metrics = snapshot.dashboard();
    let library = &snapshot.library;

    ui.collapsing("Metrics (§96)", |ui| {
        Grid::new("dashboard-metrics")
            .num_columns(2)
            .show(ui, |ui| {
                metric(ui, "Skills", metrics.skills);
                metric(ui, "Profiles", metrics.profiles);
                metric(ui, "Installations", metrics.installations);
                metric(ui, "Profile attachments", metrics.attachments);
                metric_colored(
                    ui,
                    "Outdated installations",
                    metrics.outdated,
                    metrics.outdated > 0,
                    Color32::YELLOW,
                );
                metric_colored(
                    ui,
                    "Modified installations",
                    metrics.modified,
                    metrics.modified > 0,
                    Color32::LIGHT_RED,
                );
                metric_colored(
                    ui,
                    "Broken installations",
                    metrics.broken,
                    metrics.broken > 0,
                    Color32::LIGHT_RED,
                );
                metric_colored(
                    ui,
                    "Missing attached profiles",
                    metrics.missing_profiles,
                    metrics.missing_profiles > 0,
                    Color32::LIGHT_RED,
                );
            });
    });

    ui.collapsing("Library (§81)", |ui| {
        Grid::new("dashboard-library")
            .num_columns(2)
            .show(ui, |ui| {
                kv(ui, "Path", library.path.display().to_string());
                kv(ui, "Library ID", library.library_id.clone());
                kv(
                    ui,
                    "Branch",
                    library
                        .branch
                        .clone()
                        .unwrap_or_else(|| "(detached)".to_owned()),
                );
                kv(
                    ui,
                    "HEAD",
                    library
                        .head
                        .as_deref()
                        .map(short_display)
                        .unwrap_or_else(|| "—".to_owned()),
                );
                kv(ui, "Default ref", library.default_ref.clone());
                kv(
                    ui,
                    "Dirty",
                    if library.dirty {
                        "yes — uncommitted changes are not part of a resolved revision (§8.1)"
                            .to_owned()
                    } else {
                        "no".to_owned()
                    },
                );
                kv(
                    ui,
                    "Remote (sanitized, §67)",
                    library.remote.clone().unwrap_or_else(|| "none".to_owned()),
                );
                kv(
                    ui,
                    "Ahead/behind",
                    match (library.ahead, library.behind) {
                        (Some(a), Some(b)) => format!("{a} ahead, {b} behind"),
                        (Some(a), None) => format!("{a} ahead"),
                        (None, Some(b)) => format!("{b} behind"),
                        (None, None) => "—".to_owned(),
                    },
                );
            });
    });

    let broken: Vec<String> = snapshot
        .installations
        .iter()
        .filter(|row| {
            row.error.is_some()
                || row
                    .status
                    .as_ref()
                    .is_some_and(|s| s.installation_state.is_some())
        })
        .map(|row| match (&row.error, &row.status) {
            (Some(error), _) => format!("{}: {error}", row.label()),
            (None, Some(status)) => format!(
                "{}: {}",
                row.label(),
                status.installation_state_id().unwrap_or("unknown state"),
            ),
            _ => row.label(),
        })
        .collect();
    if !broken.is_empty() {
        ui.collapsing(format!("Attention needed ({})", broken.len()), |ui| {
            for line in broken {
                ui.colored_label(Color32::LIGHT_RED, line);
            }
        });
    }
}

fn metric(ui: &mut Ui, label: &str, value: usize) {
    ui.label(label);
    ui.label(value.to_string());
    ui.end_row();
}

fn metric_colored(ui: &mut Ui, label: &str, value: usize, highlight: bool, color: Color32) {
    ui.label(label);
    if highlight {
        ui.label(RichText::new(value.to_string()).color(color).strong());
    } else {
        ui.label(value.to_string());
    }
    ui.end_row();
}

fn kv(ui: &mut Ui, key: &str, value: String) {
    ui.label(RichText::new(key).weak());
    ui.label(value);
    ui.end_row();
}

// ---- Skills (§97) -----------------------------------------------------------------

fn page_skills(state: &mut GuiState, services: Option<&Services>, ui: &mut Ui) {
    ui.heading("Skills");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if button(ui, services.is_some(), "Ingest…") {
            state.begin_ingest();
        }
    });
    ui.add_space(4.0);

    ui.columns(2, |columns| {
        skills_list(state, &mut columns[0]);
        skills_preview(state, services, &mut columns[1]);
    });
}

fn skills_list(state: &mut GuiState, ui: &mut Ui) {
    ui.label(RichText::new("Library skills").strong());
    ui.text_edit_singleline(&mut state.skills.query);
    ui.horizontal(|ui| {
        let all_selected = state.skills.bucket.is_none();
        if ui.selectable_label(all_selected, "all buckets").clicked() {
            state.skills.bucket = None;
        }
        let buckets = state
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.buckets())
            .unwrap_or_default();
        for bucket in buckets {
            let selected = state.skills.bucket.as_deref() == Some(bucket.as_str());
            if ui.selectable_label(selected, &bucket).clicked() {
                state.skills.bucket = Some(bucket);
            }
        }
    });
    ui.separator();
    let rows: Vec<(String, String)> = state
        .filtered_skills()
        .into_iter()
        .map(|skill| {
            (
                skill.name.to_string(),
                format!("{} — {}", skill.name, skill.description),
            )
        })
        .collect();
    ScrollArea::vertical()
        .id_salt("skills-list")
        .show(ui, |ui| {
            for (name, label) in &rows {
                let selected = state.skills.skill.as_deref() == Some(name.as_str());
                if ui.selectable_label(selected, label).clicked() {
                    state.select_skill(name);
                }
            }
        });
}

fn skills_preview(state: &mut GuiState, services: Option<&Services>, ui: &mut Ui) {
    ui.label(RichText::new("Preview").strong());
    let Some(selected) = state.skills.skill.clone() else {
        ui.label("Select a skill to inspect it.");
        return;
    };
    if state.skills.preview.is_none()
        && state.skills.preview_attempted.as_deref() != Some(selected.as_str())
        && let Some(services) = services
    {
        controller::load_skill_preview(state, services, &selected);
    }
    let Some(view) = state.skills.preview.clone() else {
        ui.label(format!("{selected} — select to load."));
        return;
    };
    let listing = &view.detail.listing;
    ui.heading(listing.name.as_str());
    ui.label(&listing.description);
    Grid::new("skill-meta").num_columns(2).show(ui, |ui| {
        kv(ui, "Bucket", listing.bucket.clone());
        kv(ui, "Path (library-relative)", listing.path.clone());
        kv(ui, "Tags", listing.tags.join(", "));
        kv(
            ui,
            "Rank",
            listing
                .rank
                .map(|r| r.to_string())
                .unwrap_or_else(|| "—".to_owned()),
        );
        kv(
            ui,
            "In profiles",
            if listing.profiles.is_empty() {
                "none".to_owned()
            } else {
                listing.profiles.join(", ")
            },
        );
        kv(
            ui,
            "Last commit (§35)",
            listing
                .last_commit
                .as_ref()
                .map(|c| format!("{} ({})", short_display(&c.hash), c.unix_time))
                .unwrap_or_else(|| "—".to_owned()),
        );
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let enabled = services.is_some();
        if button(ui, enabled, "Move…") {
            state.begin_skill_move(listing.name.as_str());
        }
        if button(ui, enabled, "Rename…") {
            state.begin_skill_rename(listing.name.as_str());
        }
        if button(ui, enabled, "Tag…") {
            state.begin_skill_tag(listing.name.as_str());
        }
        if button(ui, enabled, "Rank…") {
            state.begin_skill_rank(listing.name.as_str());
        }
        if button(ui, enabled, "Remove…") {
            state.begin_skill_remove(listing.name.as_str());
        }
        if button(ui, enabled, "Add to profile…") {
            state.begin_add_to_profile(listing.name.as_str());
        }
    });
    ui.add_space(4.0);

    ui.collapsing(format!("Files ({})", view.detail.files.len()), |ui| {
        for file in &view.detail.files {
            ui.monospace(file);
        }
    });
    ui.collapsing("SKILL.md", |ui| {
        ScrollArea::vertical()
            .id_salt("skill-md")
            .max_height(320.0)
            .show(ui, |ui| {
                ui.monospace(view.skill_md.clone());
            });
    });
}

// ---- Profiles (§98) -----------------------------------------------------------------

fn page_profiles(state: &mut GuiState, services: Option<&Services>, ui: &mut Ui) {
    ui.heading("Profiles");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if button(ui, services.is_some(), "Create…") {
            state.begin_profile_create();
        }
        if button(ui, services.is_some(), "Validate") {
            let services = services.expect("enabled");
            controller::validate_profiles(state, services, None);
        }
    });
    ui.add_space(4.0);

    ui.columns(2, |columns| {
        profiles_list(state, &mut columns[0]);
        profiles_detail(state, services, &mut columns[1]);
    });
}

fn profiles_list(state: &mut GuiState, ui: &mut Ui) {
    ui.label(RichText::new("Library profiles").strong());
    ScrollArea::vertical()
        .id_salt("profiles-list")
        .show(ui, |ui| {
            let Some(snapshot) = state.snapshot.clone() else {
                return;
            };
            for profile in &snapshot.profiles {
                let selected = state.profiles.profile.as_deref() == Some(profile.name.as_str());
                let label = format!(
                    "{} ({} skill{})",
                    profile.name,
                    profile.skills.len(),
                    if profile.skills.len() == 1 { "" } else { "s" },
                );
                if ui.selectable_label(selected, label).clicked() {
                    let name = profile.name.clone();
                    state.select_profile(&name);
                }
            }
        });
}

fn profiles_detail(state: &mut GuiState, services: Option<&Services>, ui: &mut Ui) {
    ui.label(RichText::new("Detail").strong());
    let Some(profile) = state.selected_profile().cloned() else {
        ui.label("Select a profile to inspect it.");
        return;
    };
    ui.heading(&profile.name);
    if let Some(description) = &profile.description {
        ui.label(description);
    }
    Grid::new("profile-meta").num_columns(2).show(ui, |ui| {
        kv(ui, "Immutable ID (§15)", profile.id.to_string());
        kv(ui, "Attached locally by", {
            let attachments = attached_installations(state, &profile.id);
            if attachments.is_empty() {
                "no local installation".to_owned()
            } else {
                attachments.join(", ")
            }
        });
    });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let enabled = services.is_some();
        if button(ui, enabled, "Rename…") {
            state.begin_profile_rename(&profile.name);
        }
        if button(ui, enabled, "Delete") {
            controller::plan_action(
                state,
                services.expect("enabled"),
                GuiAction::ProfileDelete {
                    name: profile.name.clone(),
                },
            );
        }
        if button(ui, enabled, "Add skill…") {
            state.begin_profile_add_skill(&profile.name);
        }
    });

    ui.label(RichText::new("Ordered skills (§16)").strong());
    let count = profile.skills.len();
    ScrollArea::vertical()
        .id_salt("profile-skills")
        .show(ui, |ui| {
            for (index, skill) in profile.skills.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.monospace(format!("{index}"));
                    ui.label(skill.as_str());
                    if button(ui, services.is_some() && index > 0, "↑") {
                        let pivot = beskar_core::editing::SkillPivot::Before(
                            profile.skills[index - 1].to_string(),
                        );
                        controller::plan_action(
                            state,
                            services.expect("enabled"),
                            GuiAction::ProfileMoveSkill {
                                profile: profile.name.clone(),
                                skill: skill.to_string(),
                                pivot,
                            },
                        );
                    }
                    if button(ui, services.is_some() && index + 1 < count, "↓") {
                        let pivot = beskar_core::editing::SkillPivot::After(
                            profile.skills[index + 1].to_string(),
                        );
                        controller::plan_action(
                            state,
                            services.expect("enabled"),
                            GuiAction::ProfileMoveSkill {
                                profile: profile.name.clone(),
                                skill: skill.to_string(),
                                pivot,
                            },
                        );
                    }
                    if button(ui, services.is_some(), "remove") {
                        controller::plan_action(
                            state,
                            services.expect("enabled"),
                            GuiAction::ProfileRemoveSkills {
                                profile: profile.name.clone(),
                                skills: vec![skill.to_string()],
                            },
                        );
                    }
                });
            }
        });

    if let Some(validations) = &state.profiles.validations.clone() {
        ui.collapsing("Validation (§76)", |ui| {
            for validation in validations {
                let mark = if validation.valid { "✓" } else { "✗" };
                ui.label(format!("{mark} {}", validation.profile));
                for problem in &validation.problems {
                    ui.colored_label(Color32::LIGHT_RED, format!("    {problem}"));
                }
                for missing in &validation.missing_skills {
                    ui.colored_label(Color32::YELLOW, format!("    missing skill: {missing}"));
                }
            }
        });
    }
}

/// Local installations attaching the profile, as `target — workspace`
/// labels (§98 "showing which Installations currently attach that Profile").
fn attached_installations(
    state: &GuiState,
    profile_id: &beskar_core::ids::ProfileId,
) -> Vec<String> {
    state
        .snapshot
        .as_ref()
        .map(|snapshot| {
            snapshot
                .installations
                .iter()
                .filter(|row| {
                    row.installation
                        .profiles
                        .iter()
                        .any(|attachment| attachment.id == *profile_id)
                })
                .map(|row| row.label())
                .collect()
        })
        .unwrap_or_default()
}

// ---- Installations (§99, §104) ------------------------------------------------------

fn page_installations(state: &mut GuiState, services: Option<&Services>, ui: &mut Ui) {
    ui.heading("Installations");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if button(ui, services.is_some(), "New installation…") {
            state.begin_new_installation();
        }
        if button(ui, services.is_some(), "Update all…") {
            controller::plan_action(state, services.expect("enabled"), GuiAction::UpdateAll);
        }
    });
    ui.add_space(4.0);

    ui.columns(2, |columns| {
        installations_list(state, &mut columns[0]);
        installations_detail(state, services, &mut columns[1]);
    });
}

fn installations_list(state: &mut GuiState, ui: &mut Ui) {
    ui.label(RichText::new("Registered installations").strong());
    ScrollArea::vertical()
        .id_salt("installations-list")
        .show(ui, |ui| {
            let Some(snapshot) = state.snapshot.clone() else {
                return;
            };
            for row in &snapshot.installations {
                let selected = state.installations.installation == Some(row.installation.id);
                let mut label = row.label();
                if let Some(status) = &row.status {
                    let outdated = status.count(DriftState::Outdated);
                    let modified = status.count(DriftState::Modified);
                    if outdated > 0 {
                        label.push_str(&format!("  ↑{outdated}"));
                    }
                    if modified > 0 {
                        label.push_str(&format!("  !{modified}"));
                    }
                }
                if ui.selectable_label(selected, label).clicked() {
                    let id = row.installation.id;
                    state.select_installation(id);
                }
            }
        });
}

fn installations_detail(state: &mut GuiState, services: Option<&Services>, ui: &mut Ui) {
    ui.label(RichText::new("Detail").strong());
    let Some(row) = state.selected_installation().cloned() else {
        ui.label("Select an installation (§99).");
        return;
    };
    let installation = &row.installation;
    ui.heading(installation.target.clone());
    Grid::new("installation-meta")
        .num_columns(2)
        .show(ui, |ui| {
            kv(
                ui,
                "Workspace",
                installation.workspace.display().to_string(),
            );
            kv(ui, "Target", installation.target.clone());
            kv(ui, "Adapter (§24)", format!("{:?}", installation.adapter));
            kv(ui, "Source ref (§18)", installation.source_ref.clone());
            kv(ui, "Installation ID", installation.id.to_string());
            kv(
                ui,
                "Last applied commit (§27)",
                installation
                    .last_applied
                    .source_commit
                    .as_deref()
                    .map(short_display)
                    .unwrap_or_else(|| "never applied".to_owned()),
            );
        });

    if let Some(error) = &row.error {
        banner(ui, error, true);
    }
    if let Some(state_id) = row.status.as_ref().and_then(|s| s.installation_state_id()) {
        banner(ui, &format!("installation state: {state_id}"), true);
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let enabled = services.is_some();
        if button(ui, enabled, "Add profiles…") {
            state.begin_workflow(PreviewMode::Attach);
        }
        if button(
            ui,
            enabled && !installation.profiles.is_empty(),
            "Detach profiles…",
        ) {
            state.begin_workflow(PreviewMode::Detach);
        }
        if button(ui, enabled, "Update") {
            controller::plan_action(
                state,
                services.expect("enabled"),
                GuiAction::UpdateInstallation {
                    workspace: installation.workspace.clone(),
                    target: Some(installation.target.clone()),
                },
            );
        }
        if button(ui, enabled, "Change ref…") {
            state.begin_ref_set();
        }
        if button(ui, enabled, "Unregister…") {
            state.begin_unregister();
        }
    });

    ui.add_space(4.0);

    // The §104 workflow, when staged.
    if state.installations.workflow.is_some() {
        workflow_section(state, services, ui);
        ui.separator();
    }

    // Attached profiles: multiple simultaneous attachments are the point
    // (§103, §7).
    ui.collapsing(
        format!(
            "Attached profiles ({}) — order is attachment order (§17)",
            installation.profiles.len()
        ),
        |ui| {
            let count = installation.profiles.len();
            for (index, attachment) in installation.profiles.iter().enumerate() {
                ui.horizontal(|ui| {
                    let missing = row.status.as_ref().is_some_and(|status| {
                        status
                            .profiles
                            .iter()
                            .any(|p| p.attachment.id == attachment.id && p.profile.is_none())
                    });
                    let name = if missing {
                        format!("{} (missing profile ⚑)", attachment.name)
                    } else {
                        attachment.name.clone()
                    };
                    ui.label(name);
                    ui.label(RichText::new(attachment.id.to_string()).weak());
                    if button(ui, services.is_some() && index > 0, "↑")
                        && let Some(action) = reorder_action(state, index, true)
                    {
                        controller::plan_action(state, services.expect("enabled"), action);
                    }
                    if button(ui, services.is_some() && index + 1 < count, "↓")
                        && let Some(action) = reorder_action(state, index, false)
                    {
                        controller::plan_action(state, services.expect("enabled"), action);
                    }
                });
            }
        },
    );

    // Effective skills with drift (§41, §99).
    if let Some(status) = &row.status {
        ui.collapsing(
            format!(
                "Effective skills ({}) — unmanaged: {}",
                status.skills.len(),
                status.unmanaged.len()
            ),
            |ui| {
                Grid::new("effective-skills").num_columns(3).show(ui, |ui| {
                    for (name, skill) in &status.skills {
                        let glyph = drift_glyph(skill.state);
                        let label = format!("{glyph} {}", name);
                        let owners = owners_display(installation, &skill.required_by);
                        ui.label(label);
                        ui.label(skill.state.id());
                        let owners_label = ui.label(owners);
                        // §100: selecting a skill opens the membership view.
                        if owners_label.clicked() || ui.selectable_label(false, "why?").clicked() {
                            let skill_name = name.to_string();
                            let id = installation.id;
                            if let Some(services) = services {
                                controller::load_membership(state, services, id, &skill_name);
                            }
                        }
                        ui.end_row();
                    }
                });
                for extra in &status.unmanaged {
                    ui.label(RichText::new(format!("unmanaged: {extra}")).weak());
                }
            },
        );
    }
}

fn owners_display(
    installation: &beskar_core::registry::Installation,
    ids: &[beskar_core::ids::ProfileId],
) -> String {
    if ids.is_empty() {
        return "—".to_owned();
    }
    ids.iter()
        .map(|id| {
            installation
                .profiles
                .iter()
                .find(|attachment| attachment.id == *id)
                .map(|attachment| attachment.name.clone())
                .unwrap_or_else(|| id.to_string())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Builds the §79 presentation-order change for one move.
fn reorder_action(state: &GuiState, index: usize, up: bool) -> Option<GuiAction> {
    let row = state.selected_installation()?;
    let order: Vec<beskar_core::ids::ProfileId> =
        row.installation.profiles.iter().map(|a| a.id).collect();
    let (from, to) = if up {
        (index, index - 1)
    } else {
        (index, index + 1)
    };
    if to >= order.len() {
        return None;
    }
    let mut next = order;
    next.swap(from, to);
    Some(GuiAction::ReorderProfiles {
        workspace: row.installation.workspace.clone(),
        target: Some(row.installation.target.clone()),
        order: next,
    })
}

/// The §104 staged workflow: select profiles → preview → inspect → apply.
fn workflow_section(state: &mut GuiState, services: Option<&Services>, ui: &mut Ui) {
    let Some(workflow) = state.installations.workflow.clone() else {
        return;
    };
    ui.heading(format!(
        "{} profiles — select, preview, then apply (§104)",
        workflow.mode.verb()
    ));
    ui.horizontal(|ui| {
        if button(ui, services.is_some(), "Preview") {
            let services = services.expect("enabled");
            controller::preview_workflow(state, services);
        }
        let apply_ready = services.is_some() && state.workflow_apply_action().is_some();
        if button(ui, apply_ready, "Apply…") {
            // The confirm dialog carries the §104 preview (step 5→6).
            if let Some(confirm) = state.workflow_confirm() {
                state.dialog = Some(Dialog::Confirm(confirm));
            }
        }
        if ui.button("Cancel workflow").clicked() {
            state.close_workflow();
        }
    });

    ui.label(RichText::new("Selection").strong());
    match workflow.mode {
        PreviewMode::Attach => {
            let Some(snapshot) = state.snapshot.clone() else {
                return;
            };
            ScrollArea::vertical()
                .id_salt("workflow-attach")
                .max_height(140.0)
                .show(ui, |ui| {
                    for profile in &snapshot.profiles {
                        let selected = workflow.selected(&profile.name);
                        let already = state.selected_installation().is_some_and(|row| {
                            row.installation
                                .profiles
                                .iter()
                                .any(|a| a.name == profile.name)
                        });
                        let label = if already {
                            format!(
                                "{} (already attached — selecting is a no-op, §22)",
                                profile.name
                            )
                        } else {
                            profile.name.clone()
                        };
                        if ui.selectable_label(selected, label).clicked() {
                            state.toggle_workflow_profile(&profile.name);
                        }
                    }
                });
        }
        PreviewMode::Detach => {
            let Some(row) = state.selected_installation().cloned() else {
                return;
            };
            ScrollArea::vertical()
                .id_salt("workflow-detach")
                .max_height(140.0)
                .show(ui, |ui| {
                    for attachment in &row.installation.profiles {
                        let selected = workflow.selected(&attachment.name);
                        if ui.selectable_label(selected, &attachment.name).clicked() {
                            state.toggle_workflow_profile(&attachment.name);
                        }
                    }
                });
        }
    };

    ui.add_space(4.0);
    if let Some(error) = &workflow.preview_error {
        banner(ui, error, true);
    }
    if let Some(preview) = &workflow.preview {
        ui.label(
            RichText::new(format!(
                "Preview against resolved commit {}",
                short_display(&preview.resolved_commit)
            ))
            .strong(),
        );
        preview_groups(ui, preview);
    } else if workflow.preview_error.is_none() {
        ui.label(
            "Select profiles, then Preview to compute the effective membership change (§104).",
        );
    }
}

fn preview_groups(ui: &mut Ui, preview: &crate::preview::MembershipPreview) {
    let groups: [(MembershipChange, &str, Color32); 4] = [
        (
            MembershipChange::Install,
            "Will be installed",
            Color32::LIGHT_GREEN,
        ),
        (MembershipChange::Retain, "Retained", Color32::LIGHT_GRAY),
        (
            MembershipChange::Retire,
            "Will be retired",
            Color32::LIGHT_RED,
        ),
        (
            MembershipChange::MembershipOnly,
            "Membership-only change (§90)",
            Color32::YELLOW,
        ),
    ];
    Grid::new("membership-preview")
        .num_columns(3)
        .show(ui, |ui| {
            for (change, title, color) in groups {
                for row in preview.rows_of(change) {
                    ui.label(RichText::new(title).color(color));
                    ui.label(&row.skill);
                    let detail = match row.change {
                        MembershipChange::Install => {
                            format!("required by {}", row.after.join(", "))
                        }
                        MembershipChange::Retire => {
                            format!("was required by {}", row.before.join(", "))
                        }
                        MembershipChange::MembershipOnly => {
                            format!("{} → {}", row.before.join(", "), row.after.join(", "))
                        }
                        MembershipChange::Retain => format!("required by {}", row.after.join(", ")),
                    };
                    ui.label(detail);
                    ui.end_row();
                }
            }
        });
    for (skill, owners) in &preview.gaps {
        ui.colored_label(
            Color32::LIGHT_RED,
            format!(
                "gap: {skill} is required by {} but absent from the revision (§58)",
                owners.join(", ")
            ),
        );
    }
    for name in &preview.missing_profiles {
        ui.colored_label(
            Color32::YELLOW,
            format!("protected missing profile: {name} (§39)"),
        );
    }
    if preview.is_no_op() {
        ui.label("Nothing would change (§22 idempotent).");
    }
}

// ---- Git (§101) -----------------------------------------------------------------

fn page_git(state: &mut GuiState, services: Option<&Services>, ui: &mut Ui) {
    ui.heading("Git");
    ui.add_space(4.0);
    if button(ui, services.is_some(), "Refresh") {
        controller::refresh(state, services);
    }
    ui.add_space(4.0);

    ui.label(RichText::new("Branches").strong());
    ScrollArea::vertical()
        .id_salt("git-branches")
        .show(ui, |ui| {
            let Some(snapshot) = state.snapshot.clone() else {
                return;
            };
            for branch in &snapshot.branches {
                let selected = state.git.branch.as_deref() == Some(branch.name.as_str());
                let mut label = branch.name.clone();
                if branch.current {
                    label.push_str("  ● (checked out)");
                }
                if let Some(upstream) = &branch.upstream {
                    label.push_str(&format!("  ← {upstream}"));
                }
                if let (Some(ahead), Some(behind)) = (branch.ahead, branch.behind) {
                    label.push_str(&format!("  ↑{ahead} ↓{behind}"));
                }
                if ui.selectable_label(selected, label).clicked() {
                    state.select_branch(&branch.name);
                }
            }
        });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let enabled = services.is_some() && state.git.branch.is_some();
        if button(ui, enabled, "Switch to selected (§82)") {
            let name = state.git.branch.clone().expect("enabled");
            controller::plan_action(
                state,
                services.expect("enabled"),
                GuiAction::SwitchBranch { name },
            );
        }
        if button(ui, services.is_some(), "Create branch…") {
            state.begin_create_branch();
        }
    });

    ui.add_space(4.0);
    ui.label(RichText::new("Fetch (§62)").strong());
    ui.horizontal(|ui| {
        ui.label("Remote:");
        ui.add(egui::TextEdit::singleline(&mut state.git.remote).desired_width(140.0));
        if button(ui, services.is_some(), "Fetch…") {
            let remote = if state.git.remote.trim().is_empty() {
                "origin".to_owned()
            } else {
                state.git.remote.trim().to_owned()
            };
            controller::plan_action(
                state,
                services.expect("enabled"),
                GuiAction::Fetch { remote },
            );
        }
    });

    ui.add_space(4.0);
    ui.label(RichText::new("Push (§65)").strong());
    ui.horizontal(|ui| {
        ui.label("Branch:");
        ui.add(egui::TextEdit::singleline(&mut state.git.push_branch).desired_width(180.0));
        ui.checkbox(&mut state.git.set_upstream, "set upstream");
        ui.checkbox(&mut state.git.allow_dirty, "allow dirty (§66)");
    });
    ui.horizontal(|ui| {
        if button(ui, services.is_some(), "Push…") {
            let trimmed = state.git.push_branch.trim().to_owned();
            let branch = (!trimmed.is_empty()).then_some(trimmed);
            controller::plan_action(
                state,
                services.expect("enabled"),
                GuiAction::Push {
                    branch,
                    set_upstream: state.git.set_upstream,
                    allow_dirty: state.git.allow_dirty,
                },
            );
        }
    });
    ui.label(
        RichText::new(
            "Push never force-pushes (§135.27); fetch fast-forwards only, never merge/rebase (§63).",
        )
        .weak(),
    );
}

// ---- Settings (§85, §86 — read-only) -------------------------------------------------

fn page_settings(state: &mut GuiState, settings: &SettingsContext, ui: &mut Ui) {
    ui.heading("Settings");
    ui.label(
        RichText::new(
            "Read-only in v1: Beskar follows platform directories (§85) and \
             environment overrides (§86). Set BESKAR_HOME / BESKAR_LIBRARY \
             before launching to redirect Beskar.",
        )
        .weak(),
    );
    ui.add_space(4.0);

    ui.collapsing("Environment overrides (§86)", |ui| {
        Grid::new("settings-env").num_columns(2).show(ui, |ui| {
            kv(
                ui,
                "BESKAR_HOME",
                if settings.beskar_home_override {
                    "set".to_owned()
                } else {
                    "unset".to_owned()
                },
            );
            kv(
                ui,
                "BESKAR_LIBRARY",
                if settings.beskar_library_override {
                    "set".to_owned()
                } else {
                    "unset".to_owned()
                },
            );
        });
    });

    if let Some(error) = &settings.services_error {
        ui.collapsing("Discovery error", |ui| {
            banner(ui, error, true);
        });
    }

    if let Some(dirs) = &settings.dirs {
        ui.collapsing("Platform directories (§85)", |ui| {
            Grid::new("settings-dirs").num_columns(2).show(ui, |ui| {
                kv(ui, "Data (registry)", dirs.data_dir.display().to_string());
                kv(ui, "Config", dirs.config_dir.display().to_string());
                kv(ui, "State (locks)", dirs.state_dir.display().to_string());
                kv(
                    ui,
                    "Registry file (§25)",
                    dirs.registry_file().display().to_string(),
                );
            });
        });
    }

    if let Some(snapshot) = state.snapshot.clone() {
        ui.collapsing("Library (§81)", |ui| {
            Grid::new("settings-library").num_columns(2).show(ui, |ui| {
                kv(ui, "Path", snapshot.library.path.display().to_string());
                kv(ui, "Library ID", snapshot.library.library_id.clone());
                kv(ui, "Default ref", snapshot.library.default_ref.clone());
                kv(ui, "Schema", beskar_core::config::SCHEMA.to_string());
            });
        });
    }

    ui.collapsing("Diagnostics", |ui| {
        Grid::new("settings-diagnostics")
            .num_columns(2)
            .show(ui, |ui| {
                kv(
                    ui,
                    "Git executable",
                    settings
                        .git_version
                        .clone()
                        .unwrap_or_else(|| "not found".to_owned()),
                );
                kv(ui, "GUI schema", beskar_core::config::SCHEMA.to_string());
            });
    });
}

// ---- Activity (§95) -----------------------------------------------------------------

fn page_activity(state: &mut GuiState, ui: &mut Ui) {
    ui.heading("Activity");
    ui.add_space(4.0);
    if ui.button("Clear log").clicked() {
        state.activity.entries.clear();
    }
    ui.add_space(4.0);
    ScrollArea::vertical().id_salt("activity").show(ui, |ui| {
        let entries = state.activity.entries.clone();
        let empty = entries.is_empty();
        for entry in &entries {
            let mark = if entry.failed { "✗" } else { "·" };
            ui.horizontal(|ui| {
                ui.monospace(entry.time.clone());
                ui.label(RichText::new(format!("{mark} {}", entry.title)).strong());
            });
            for line in &entry.lines {
                ui.indent("activity-line", |ui| {
                    ui.monospace(line);
                });
            }
            ui.add_space(2.0);
        }
        if empty {
            ui.label("Nothing yet. Mutations and errors are logged here (§95).");
        }
    });
}

// ---- dialogs -------------------------------------------------------------------------

fn dialogs(state: &mut GuiState, services: Option<&Services>, ctx: &Context) {
    let Some(dialog) = state.dialog.clone() else {
        return;
    };
    match dialog {
        Dialog::Confirm(confirm) => {
            let mut chosen: Option<usize> = None;
            Window::new(format!("Apply — {}", confirm.title))
                .resizable(true)
                .show(ctx, |ui| {
                    ScrollArea::vertical()
                        .id_salt("confirm-body")
                        .max_height(380.0)
                        .show(ui, |ui| {
                            for line in &confirm.lines {
                                ui.monospace(line);
                            }
                        });
                    ui.separator();
                    ui.horizontal(|ui| {
                        for (index, choice) in confirm.choices.iter().enumerate() {
                            if ui.button(&choice.label).clicked() {
                                chosen = Some(index);
                            }
                        }
                    });
                });
            if let Some(index) = chosen
                && let Some(apply) = state.confirm_choice(index)
                && let Some(services) = services
            {
                controller::handle_confirm(state, services, apply);
            }
        }
        Dialog::Input(dialog) => {
            let mut ok = false;
            let mut cancel = false;
            let mut dialog = dialog;
            Window::new(dialog.title.clone())
                .resizable(false)
                .show(ctx, |ui| {
                    Grid::new("input-fields").num_columns(2).show(ui, |ui| {
                        for field in &mut dialog.fields {
                            ui.label(&field.label);
                            ui.text_edit_singleline(&mut field.buffer);
                            ui.end_row();
                        }
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        ok = ui.button("OK").clicked() || ok;
                        cancel = ui.button("Cancel").clicked() || cancel;
                    });
                });
            if cancel {
                state.close_dialog();
            } else {
                // Write the edited buffers back every frame, then handle OK.
                state.dialog = Some(Dialog::Input(dialog));
                if ok
                    && let Some(action) = state.submit_input()
                    && let Some(services) = services
                {
                    controller::plan_action(state, services, action);
                }
            }
        }
        Dialog::Pick(dialog) => {
            let mut chosen: Option<usize> = None;
            let mut cancel = false;
            Window::new(dialog.title.clone())
                .resizable(false)
                .show(ctx, |ui| {
                    ScrollArea::vertical()
                        .id_salt("pick-options")
                        .max_height(300.0)
                        .show(ui, |ui| {
                            for (index, option) in dialog.options.iter().enumerate() {
                                if ui.selectable_label(false, option).clicked() {
                                    chosen = Some(index);
                                }
                            }
                        });
                    ui.separator();
                    ui.horizontal(|ui| {
                        cancel = ui.button("Cancel").clicked() || cancel;
                    });
                });
            if cancel {
                state.close_dialog();
            } else if let Some(index) = chosen
                && let Some(action) = state.choose_pick(index)
                && let Some(services) = services
            {
                controller::plan_action(state, services, action);
            }
        }
        Dialog::Message(message) => {
            let mut close = false;
            Window::new(message.title.clone())
                .resizable(true)
                .show(ctx, |ui| {
                    ScrollArea::vertical()
                        .id_salt("message-body")
                        .max_height(300.0)
                        .show(ui, |ui| {
                            for line in &message.lines {
                                if message.failed {
                                    ui.colored_label(Color32::LIGHT_RED, line);
                                } else {
                                    ui.label(line);
                                }
                            }
                        });
                    ui.horizontal(|ui| {
                        close = ui.button("Close").clicked() || close;
                    });
                });
            if close {
                state.close_dialog();
            }
        }
        Dialog::Membership(view) => {
            let mut close = false;
            Window::new(format!("Why {}? (§100)", view.skill))
                .resizable(true)
                .show(ctx, |ui| {
                    Grid::new("membership-grid").num_columns(2).show(ui, |ui| {
                        kv(ui, "Skill", view.skill.clone());
                        kv(
                            ui,
                            "Required by (§93)",
                            if view.required_by.is_empty() {
                                "nobody".to_owned()
                            } else {
                                view.required_by
                                    .iter()
                                    .map(|(_, name)| name.clone())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            },
                        );
                        kv(
                            ui,
                            "At last apply (§27)",
                            if view.last_required_by.is_empty() {
                                "nobody".to_owned()
                            } else {
                                view.last_required_by
                                    .iter()
                                    .map(|(_, name)| name.clone())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            },
                        );
                        kv(ui, "Source ref", view.source_ref.clone());
                        kv(
                            ui,
                            "Library commit (§19)",
                            view.library_commit
                                .as_deref()
                                .map(short_display)
                                .unwrap_or_else(|| "—".to_owned()),
                        );
                        kv(
                            ui,
                            "Skill commit (§35)",
                            view.skill_commit
                                .as_deref()
                                .map(short_display)
                                .unwrap_or_else(|| "—".to_owned()),
                        );
                        kv(
                            ui,
                            "State",
                            view.state
                                .map(|s| format!("{} {}", drift_glyph(s), s.id()))
                                .unwrap_or_else(|| "—".to_owned()),
                        );
                        if let Some(drift) = view.membership_drift {
                            kv(ui, "Membership drift", format!("{drift:?}"));
                        }
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        close = ui.button("Close").clicked() || close;
                    });
                });
            if close {
                state.close_dialog();
            }
        }
    }
}

// ---- helpers ---------------------------------------------------------------------

fn button(ui: &mut Ui, enabled: bool, label: &str) -> bool {
    ui.add_enabled(enabled, Button::new(label)).clicked()
}

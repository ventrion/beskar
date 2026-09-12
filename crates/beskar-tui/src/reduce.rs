//! The reducer: pure state transitions (spec §112).
//!
//! `reduce(app, event) -> Vec<Effect>` never performs I/O and never touches
//! a terminal; the runtime executes the returned effects and feeds results
//! back as events. All mutation flows follow the §89 model:
//! plan (dry-run) → confirm against the real plan → execute → refresh.

use beskar_core::editing::SkillPivot;
use beskar_core::error::Error;
use beskar_core::ids::InstallationId;
use beskar_core::plan::{BlockerKind, ReconciliationPlan};
use beskar_core::reconcile::ReconcileOptions;

use crate::app::{
    ActivityEntry, App, ConfirmApply, ConfirmChoice, ConfirmDialog, Dialog, InputDialog,
    InputField, InputKind, InstallationRow, MessageDialog, PendingAction, PickDialog, PickKind,
    PlannedChange, Screen, SkillsFocus, step_index,
};
use crate::effect::Effect;
use crate::event::Event;
use crate::input::Key;

/// Processes one event, returning the effects the runtime should fulfill in
/// order.
pub fn reduce(app: &mut App, event: Event) -> Vec<Effect> {
    let mut effects = Vec::new();
    match event {
        Event::Key(key) => reduce_key(app, key),
        Event::Loaded(Ok(snapshot)) => {
            app.load_error = None;
            app.snapshot = Some(*snapshot);
            reselect_after_refresh(app, &mut effects);
        }
        Event::Loaded(Err(err)) => app.load_error = Some(err.to_string()),
        Event::SkillLoaded(Ok(view)) => {
            if app.skills.skill.as_deref() == Some(view.name.as_str()) {
                let changed = app
                    .skills
                    .preview
                    .as_ref()
                    .map(|preview| preview.name != view.name)
                    .unwrap_or(true);
                if changed {
                    app.skills.preview_scroll = 0;
                }
                app.skills.preview = Some(*view);
            }
        }
        Event::SkillLoaded(Err(err)) => {
            app.toast = Some(format!("could not load preview: {err}"));
        }
        Event::MembershipLoaded(Ok(view)) => {
            app.dialog = Some(Dialog::Membership(Box::new(*view)));
        }
        Event::MembershipLoaded(Err(err)) => open_error(app, "Membership", &err),
        Event::Planned(Ok(change)) => {
            let planned = app.planning.take();
            open_confirm(app, *change, planned);
        }
        Event::Planned(Err(err)) => {
            app.planning = None;
            open_error(app, "Planning failed", &err);
        }
        Event::Executed(Ok(outcome)) => {
            let applied = outcome.applied;
            let title = outcome.title.clone();
            log_outcome(app, &outcome);
            app.toast = Some(if applied {
                format!("{title} — applied")
            } else {
                format!("{title} — nothing to apply")
            });
            effects.push(Effect::Refresh);
        }
        Event::Executed(Err(err)) => {
            open_error(app, "Operation failed", &err);
            effects.push(Effect::Refresh);
        }
        Event::Validated(Ok(reports)) => {
            let mut lines = Vec::new();
            for report in &reports {
                let mark = if report.valid { "✓" } else { "✗" };
                lines.push(format!("{mark} {}", report.profile));
                for problem in &report.problems {
                    lines.push(format!("    {problem}"));
                }
                for missing in &report.missing_skills {
                    lines.push(format!("    missing skill: {missing}"));
                }
            }
            if lines.is_empty() {
                lines.push("no profiles defined".to_owned());
            }
            app.dialog = Some(Dialog::Message(MessageDialog {
                title: "Profile validation".to_owned(),
                lines,
                failed: false,
            }));
        }
        Event::Validated(Err(err)) => open_error(app, "Validation failed", &err),
    }
    // Key-handling helpers queue effects via the per-event slot; merge both
    // channels so callers only deal with `reduce`'s return value.
    let queued = PENDING.with(|cell| cell.borrow_mut().drain(..).collect::<Vec<_>>());
    effects.extend(queued);
    effects
}

// ---- keyboard dispatch ---------------------------------------------------

fn reduce_key(app: &mut App, key: Key) {
    if key == Key::Interrupt {
        app.quit = true;
        return;
    }
    if handle_dialog_key(app, key) {
        return;
    }
    if handle_global_key(app, key) {
        return;
    }
    match app.screen {
        Screen::Dashboard => dashboard_key(app, key),
        Screen::Skills => skills_key(app, key),
        Screen::Profiles => profiles_key(app, key),
        Screen::Installations => installations_key(app, key),
        Screen::Git => git_key(app, key),
        Screen::Activity => activity_key(app, key),
    }
}

/// Global keys, active whenever no dialog is open. Returns true when the
/// key was consumed. Query edit mode (§97) takes precedence: while typing
/// a search, every key belongs to the query — even `r` and the digits.
fn handle_global_key(app: &mut App, key: Key) -> bool {
    if app.skills.querying {
        return false;
    }
    match key {
        Key::Char('q') => {
            app.quit = true;
            true
        }
        Key::Tab => {
            app.screen = app.screen.next();
            true
        }
        Key::BackTab => {
            app.screen = app.screen.previous();
            true
        }
        Key::Char('1'..='6') => {
            if let Some(index) = key.char().and_then(|c| c.to_digit(10)) {
                let index = (index as usize).saturating_sub(1);
                if let Some(screen) = Screen::ALL.get(index) {
                    app.screen = *screen;
                }
            }
            true
        }
        Key::Char('r') => {
            app.toast = None;
            PENDING.with(|cell| cell.borrow_mut().push(Effect::Refresh));
            true
        }
        _ => false,
    }
}

fn dashboard_key(app: &mut App, key: Key) {
    if key == Key::Char('u') {
        start_plan(app, PendingAction::UpdateAll, ReconcileOptions::default());
    }
}

/// True when the key was consumed by a dialog.
fn handle_dialog_key(app: &mut App, key: Key) -> bool {
    // Take the dialog out so handlers can mutate `app` freely (starting
    // plans/executions) without aliasing the dialog reference.
    let mut dialog = match app.dialog.take() {
        Some(dialog) => dialog,
        None => return false,
    };
    let keep = match &mut dialog {
        Dialog::Confirm(confirm) => confirm_key(app, confirm, key),
        Dialog::Input(input) => input_key(app, input, key),
        Dialog::Pick(pick) => pick_key(app, pick, key),
        Dialog::Message(_) | Dialog::Membership(_) => {
            // Any interpreted key dismisses read-only popups; only
            // unrecognized keys keep them open.
            key == Key::Other
        }
    };
    if keep {
        app.dialog = Some(dialog);
    }
    true
}

/// Handles one key on a confirm dialog; returns whether it stays open.
fn confirm_key(app: &mut App, confirm: &mut ConfirmDialog, key: Key) -> bool {
    match key {
        Key::Esc | Key::Char('q') => false,
        Key::PageUp => {
            confirm.scroll = confirm.scroll.saturating_sub(5);
            true
        }
        Key::PageDown => {
            confirm.scroll = confirm.scroll.saturating_add(5);
            true
        }
        Key::Up | Key::Char('k') | Key::Left => {
            confirm.selected =
                step_index(Some(confirm.selected), confirm.choices.len(), -1).unwrap_or(0);
            true
        }
        Key::Down | Key::Char('j') | Key::Right => {
            confirm.selected =
                step_index(Some(confirm.selected), confirm.choices.len(), 1).unwrap_or(0);
            true
        }
        Key::Enter | Key::Char(' ') => {
            if let Some(choice) = confirm.choices.get(confirm.selected).cloned() {
                apply_choice(app, choice);
            }
            false
        }
        _ => true,
    }
}

/// Handles one key on a pick dialog; returns whether it stays open.
fn pick_key(app: &mut App, pick: &mut PickDialog, key: Key) -> bool {
    match key {
        Key::Esc | Key::Char('q') => false,
        Key::Up | Key::Char('k') => {
            pick.selected = step_index(Some(pick.selected), pick.values.len(), -1).unwrap_or(0);
            true
        }
        Key::Down | Key::Char('j') => {
            pick.selected = step_index(Some(pick.selected), pick.values.len(), 1).unwrap_or(0);
            true
        }
        Key::Enter => {
            if let Some(value) = pick.values.get(pick.selected).cloned() {
                let kind = pick.kind.clone();
                apply_pick(app, kind, value);
            }
            false
        }
        _ => true,
    }
}

fn apply_choice(app: &mut App, choice: ConfirmChoice) {
    match choice.apply {
        ConfirmApply::Run { action, options } => start_execute(app, *action, options),
        ConfirmApply::Replan { action, options } => start_plan(app, *action, options),
        ConfirmApply::Cancel => {}
    }
}

fn apply_pick(app: &mut App, kind: PickKind, value: String) {
    match kind {
        PickKind::AttachNew => {
            // First install: still needs a workspace (and optional target).
            app.dialog = Some(Dialog::Input(InputDialog {
                title: format!("Attach {value} to a workspace"),
                fields: vec![
                    InputField {
                        label: "workspace path".to_owned(),
                        buffer: String::new(),
                    },
                    InputField {
                        label: "target (optional, default .agents/skills)".to_owned(),
                        buffer: String::new(),
                    },
                ],
                focused: 0,
                kind: InputKind::AttachNew { profile: value },
            }));
        }
        PickKind::AttachProfile { workspace, target } => start_plan(
            app,
            PendingAction::AttachProfile {
                workspace,
                target,
                profile: value,
            },
            ReconcileOptions::default(),
        ),
        PickKind::DetachProfile { workspace, target } => start_plan(
            app,
            PendingAction::DetachProfile {
                workspace,
                target,
                profile: value,
            },
            ReconcileOptions::default(),
        ),
        PickKind::AddToProfile { skill } => start_plan(
            app,
            PendingAction::ProfileAddSkills {
                profile: value,
                skills: vec![skill],
            },
            ReconcileOptions::default(),
        ),
        PickKind::ProfileAddSkill { profile } => start_plan(
            app,
            PendingAction::ProfileAddSkills {
                profile,
                skills: vec![value],
            },
            ReconcileOptions::default(),
        ),
        PickKind::RemoveSkill { skill } => start_plan(
            app,
            PendingAction::SkillRemove {
                skill,
                cascade: value == "true",
            },
            ReconcileOptions::default(),
        ),
        PickKind::Unregister { workspace, target } => start_plan(
            app,
            PendingAction::Unregister {
                workspace,
                target,
                keep_files: value == "true",
            },
            ReconcileOptions::default(),
        ),
    }
}

/// Handles one key on an input dialog; returns whether it stays open.
fn input_key(app: &mut App, input: &mut InputDialog, key: Key) -> bool {
    match key {
        Key::Esc => false,
        Key::Tab | Key::Down => {
            input.focused = step_index(Some(input.focused), input.fields.len(), 1).unwrap_or(0);
            true
        }
        Key::Up => {
            input.focused = step_index(Some(input.focused), input.fields.len(), -1).unwrap_or(0);
            true
        }
        Key::Backspace => {
            if let Some(field) = input.fields.get_mut(input.focused) {
                field.buffer.pop();
            }
            true
        }
        Key::Enter => {
            if input.focused + 1 < input.fields.len() {
                input.focused += 1;
                true
            } else {
                let kind = input.kind.clone();
                let buffers: Vec<String> = input
                    .fields
                    .iter()
                    .map(|field| field.buffer.trim().to_owned())
                    .collect();
                submit_input(app, kind, &buffers);
                false
            }
        }
        Key::Char(c) => {
            if let Some(field) = input.fields.get_mut(input.focused) {
                field.buffer.push(c);
            }
            true
        }
        _ => true,
    }
}

/// Builds the pending action from submitted input fields. Field layouts are
/// documented per [`InputKind`].
fn submit_input(app: &mut App, kind: InputKind, fields: &[String]) {
    let non_empty = |index: usize| fields.get(index).filter(|f| !f.is_empty()).cloned();
    let split_tags = |raw: &str| -> Vec<String> {
        raw.split(',')
            .map(|tag| tag.trim().to_owned())
            .filter(|tag| !tag.is_empty())
            .collect()
    };
    let action = match kind {
        InputKind::Ingest => {
            let (Some(source), Some(bucket)) = (non_empty(0), non_empty(1)) else {
                app.toast = Some("ingest needs a source path and a bucket".to_owned());
                return;
            };
            PendingAction::Ingest {
                source: source.into(),
                bucket,
            }
        }
        InputKind::SkillMove { skill } => {
            let Some(bucket) = non_empty(0) else {
                app.toast = Some("name a destination bucket".to_owned());
                return;
            };
            PendingAction::SkillMove { skill, bucket }
        }
        InputKind::SkillRename { skill } => {
            let Some(new) = non_empty(0) else {
                app.toast = Some("name a new skill name".to_owned());
                return;
            };
            PendingAction::SkillRename { old: skill, new }
        }
        InputKind::SkillTag { skill } => {
            let add = fields.first().map(|f| split_tags(f)).unwrap_or_default();
            let remove = fields.get(1).map(|f| split_tags(f)).unwrap_or_default();
            if add.is_empty() && remove.is_empty() {
                app.toast = Some("name at least one tag to add or remove".to_owned());
                return;
            }
            PendingAction::SkillTag { skill, add, remove }
        }
        InputKind::SkillRank { skill } => {
            let rank = match non_empty(0) {
                None => None,
                Some(raw) => match raw.parse::<i64>() {
                    Ok(rank) => Some(rank),
                    Err(_) => {
                        app.toast =
                            Some("rank must be an integer (leave empty to clear)".to_owned());
                        return;
                    }
                },
            };
            PendingAction::SkillRank { skill, rank }
        }
        InputKind::ProfileCreate => {
            let Some(name) = non_empty(0) else {
                app.toast = Some("name the profile".to_owned());
                return;
            };
            PendingAction::ProfileCreate {
                name,
                description: non_empty(1),
            }
        }
        InputKind::ProfileRename { old } => {
            let Some(new) = non_empty(0) else {
                app.toast = Some("name the new profile name".to_owned());
                return;
            };
            PendingAction::ProfileRename { old, new }
        }
        InputKind::AttachNew { profile } => {
            let Some(workspace) = non_empty(0) else {
                app.toast = Some("name the workspace directory".to_owned());
                return;
            };
            PendingAction::AttachProfile {
                workspace: workspace.into(),
                target: non_empty(1),
                profile,
            }
        }
        InputKind::RefSet { workspace, target } => {
            let Some(new_ref) = non_empty(0) else {
                app.toast = Some("name the new source ref".to_owned());
                return;
            };
            PendingAction::RefSet {
                workspace,
                target,
                new_ref,
            }
        }
        InputKind::CreateBranch => {
            let Some(name) = non_empty(0) else {
                app.toast = Some("name the branch".to_owned());
                return;
            };
            PendingAction::CreateBranch { name }
        }
    };
    start_plan(app, action, ReconcileOptions::default());
}

// ---- screen keys -----------------------------------------------------------

fn skills_key(app: &mut App, key: Key) {
    // Query edit mode captures everything except confirm/cancel (§97).
    if app.skills.querying {
        let skills = &mut app.skills;
        match key {
            Key::Enter => skills.querying = false,
            Key::Esc => {
                skills.query.clear();
                skills.querying = false;
            }
            Key::Backspace => {
                skills.query.pop();
            }
            Key::Char(c) => skills.query.push(c),
            _ => {}
        }
        return;
    }

    match key {
        Key::Char('/') => app.skills.querying = true,
        Key::Left | Key::Char('h') => {
            app.skills.focus = match app.skills.focus {
                SkillsFocus::Buckets => SkillsFocus::Preview,
                SkillsFocus::List => SkillsFocus::Buckets,
                SkillsFocus::Preview => SkillsFocus::List,
            };
        }
        Key::Right | Key::Char('l') => {
            app.skills.focus = match app.skills.focus {
                SkillsFocus::Buckets => SkillsFocus::List,
                SkillsFocus::List => SkillsFocus::Preview,
                SkillsFocus::Preview => SkillsFocus::Buckets,
            };
        }
        Key::Up | Key::Char('k') => move_skills_selection(app, -1),
        Key::Down | Key::Char('j') => move_skills_selection(app, 1),
        Key::PageUp => move_skills_selection(app, -10),
        Key::PageDown => move_skills_selection(app, 10),
        Key::Enter if app.skills.focus == SkillsFocus::List => {
            app.skills.focus = SkillsFocus::Preview;
        }
        Key::Char('i') => {
            app.dialog = Some(Dialog::Input(InputDialog {
                title: "Ingest skill".to_owned(),
                fields: vec![
                    InputField {
                        label: "source path".to_owned(),
                        buffer: String::new(),
                    },
                    InputField {
                        label: "bucket".to_owned(),
                        buffer: String::new(),
                    },
                ],
                focused: 0,
                kind: InputKind::Ingest,
            }));
        }
        Key::Char('m') => open_for_selected_skill(app, |skill| InputDialog {
            title: format!("Move {skill}"),
            fields: vec![InputField {
                label: "destination bucket".to_owned(),
                buffer: String::new(),
            }],
            focused: 0,
            kind: InputKind::SkillMove { skill },
        }),
        Key::Char('N') => open_for_selected_skill(app, |skill| InputDialog {
            title: format!("Rename {skill}"),
            fields: vec![InputField {
                label: "new name".to_owned(),
                buffer: String::new(),
            }],
            focused: 0,
            kind: InputKind::SkillRename { skill },
        }),
        Key::Char('t') => open_for_selected_skill(app, |skill| InputDialog {
            title: format!("Tag {skill}"),
            fields: vec![
                InputField {
                    label: "add tags (comma-separated)".to_owned(),
                    buffer: String::new(),
                },
                InputField {
                    label: "remove tags (comma-separated)".to_owned(),
                    buffer: String::new(),
                },
            ],
            focused: 0,
            kind: InputKind::SkillTag { skill },
        }),
        Key::Char('e') => open_for_selected_skill(app, |skill| InputDialog {
            title: format!("Rank {skill}"),
            fields: vec![InputField {
                label: "rank (integer, empty clears)".to_owned(),
                buffer: String::new(),
            }],
            focused: 0,
            kind: InputKind::SkillRank { skill },
        }),
        Key::Char('x') => {
            let Some(skill) = app.skills.skill.clone() else {
                app.toast = Some("select a skill first".to_owned());
                return;
            };
            // The chosen mode then goes through the normal plan → confirm
            // pipeline, so the real scoped-commit plan is previewed (§75).
            app.dialog = Some(Dialog::Pick(PickDialog {
                title: format!("Remove {skill}"),
                options: vec![
                    "Remove".to_owned(),
                    "Remove + cascade from profiles (§75)".to_owned(),
                ],
                values: vec!["false".to_owned(), "true".to_owned()],
                selected: 0,
                kind: PickKind::RemoveSkill { skill },
            }));
        }
        Key::Char('p') => {
            let Some(skill) = app.skills.skill.clone() else {
                app.toast = Some("select a skill first".to_owned());
                return;
            };
            let options: Vec<String> = profile_names(app);
            if options.is_empty() {
                app.toast = Some("no profiles defined yet".to_owned());
                return;
            }
            let values = options.clone();
            app.dialog = Some(Dialog::Pick(PickDialog {
                title: format!("Add {skill} to profile"),
                options,
                values,
                selected: 0,
                kind: PickKind::AddToProfile { skill },
            }));
        }
        _ => {}
    }
}

/// Runs `build` with the selected skill name, opening the dialog it
/// returns; a no-op without a selection.
fn open_for_selected_skill(app: &mut App, build: impl FnOnce(String) -> InputDialog) {
    let Some(skill) = app.skills.skill.clone() else {
        app.toast = Some("select a skill first".to_owned());
        return;
    };
    app.dialog = Some(Dialog::Input(build(skill)));
}

fn profile_names(app: &App) -> Vec<String> {
    app.snapshot
        .as_ref()
        .map(|snapshot| snapshot.profiles.iter().map(|p| p.name.clone()).collect())
        .unwrap_or_default()
}

fn skill_names(app: &App) -> Vec<String> {
    app.snapshot
        .as_ref()
        .map(|snapshot| snapshot.skills.iter().map(|s| s.name.to_string()).collect())
        .unwrap_or_default()
}

fn move_skills_selection(app: &mut App, delta: i64) {
    match app.skills.focus {
        SkillsFocus::Buckets => {
            let mut rows: Vec<Option<String>> = vec![None];
            if let Some(snapshot) = app.snapshot.as_ref() {
                rows.extend(
                    snapshot
                        .buckets()
                        .into_iter()
                        .map(|(bucket, _)| Some(bucket)),
                );
            }
            let current = rows
                .iter()
                .position(|row| row.as_ref() == app.skills.bucket.as_ref());
            if let Some(next) = step_index(current, rows.len(), delta) {
                app.skills.bucket = rows[next].clone();
            }
        }
        SkillsFocus::List => {
            let filtered = app.filtered_skills();
            let current = filtered
                .iter()
                .position(|skill| Some(skill.name.as_str()) == app.skills.skill.as_deref());
            if let Some(next) = step_index(current, filtered.len(), delta) {
                let name = filtered[next].name.to_string();
                app.skills.skill = Some(name.clone());
                app.skills.preview_scroll = 0;
                PENDING.with(|cell| cell.borrow_mut().push(Effect::LoadSkill { skill: name }));
            }
        }
        SkillsFocus::Preview => {
            app.skills.preview_scroll = app
                .skills
                .preview_scroll
                .saturating_add_signed(delta as i16);
        }
    }
}

fn profiles_key(app: &mut App, key: Key) {
    use crate::app::ProfilesFocus;
    match key {
        Key::Left | Key::Char('h') => app.profiles.focus = ProfilesFocus::List,
        Key::Right | Key::Char('l') => app.profiles.focus = ProfilesFocus::Detail,
        Key::Up | Key::Char('k') => move_profiles_selection(app, -1),
        Key::Down | Key::Char('j') => move_profiles_selection(app, 1),
        Key::Char('c') => {
            app.dialog = Some(Dialog::Input(InputDialog {
                title: "Create profile".to_owned(),
                fields: vec![
                    InputField {
                        label: "name".to_owned(),
                        buffer: String::new(),
                    },
                    InputField {
                        label: "description (optional)".to_owned(),
                        buffer: String::new(),
                    },
                ],
                focused: 0,
                kind: InputKind::ProfileCreate,
            }));
        }
        Key::Char('e') => {
            let Some(profile) = app.profiles.profile.clone() else {
                app.toast = Some("select a profile first".to_owned());
                return;
            };
            app.dialog = Some(Dialog::Input(InputDialog {
                title: format!("Rename profile {profile}"),
                fields: vec![InputField {
                    label: "new name".to_owned(),
                    buffer: String::new(),
                }],
                focused: 0,
                kind: InputKind::ProfileRename { old: profile },
            }));
        }
        Key::Char('d') => {
            let Some(profile) = app.profiles.profile.clone() else {
                app.toast = Some("select a profile first".to_owned());
                return;
            };
            start_plan(
                app,
                PendingAction::ProfileDelete { name: profile },
                ReconcileOptions::default(),
            );
        }
        Key::Char('v') => {
            let profile = app.profiles.profile.clone();
            start_validate(profile);
        }
        Key::Char('a') => {
            let Some(profile) = app.profiles.profile.clone() else {
                app.toast = Some("select a profile first".to_owned());
                return;
            };
            let options = skill_names(app);
            if options.is_empty() {
                app.toast = Some("no skills in the library yet".to_owned());
                return;
            }
            let values = options.clone();
            app.dialog = Some(Dialog::Pick(PickDialog {
                title: format!("Add skill to {profile}"),
                options,
                values,
                selected: 0,
                kind: PickKind::ProfileAddSkill { profile },
            }));
        }
        Key::Char('x') => {
            let (Some(profile), Some(snapshot)) =
                (app.profiles.profile.clone(), app.snapshot.as_ref())
            else {
                app.toast = Some("select a profile first".to_owned());
                return;
            };
            let Some(skill) = snapshot
                .profile(&profile)
                .and_then(|p| p.skills.get(app.profiles.skill_index))
                .map(|s| s.to_string())
            else {
                app.toast = Some("select a skill in the profile first".to_owned());
                return;
            };
            start_plan(
                app,
                PendingAction::ProfileRemoveSkills {
                    profile,
                    skills: vec![skill],
                },
                ReconcileOptions::default(),
            );
        }
        Key::Char('[') | Key::Char(']') => {
            let up = key == Key::Char('[');
            let (Some(profile), Some(snapshot)) =
                (app.profiles.profile.clone(), app.snapshot.as_ref())
            else {
                app.toast = Some("select a profile first".to_owned());
                return;
            };
            let skills: Vec<String> = snapshot
                .profile(&profile)
                .map(|p| p.skills.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default();
            let index = app.profiles.skill_index;
            let moved = skills.get(index).cloned();
            let neighbor = if up {
                index.checked_sub(1).and_then(|i| skills.get(i))
            } else {
                index.checked_add(1).and_then(|i| {
                    if i < skills.len() {
                        skills.get(i)
                    } else {
                        None
                    }
                })
            };
            let (Some(moved), Some(neighbor)) = (moved, neighbor) else {
                app.toast = Some("no room to move in that direction".to_owned());
                return;
            };
            let (moved, neighbor) = (moved.clone(), neighbor.clone());
            let pivot = if up {
                SkillPivot::Before(neighbor)
            } else {
                SkillPivot::After(neighbor)
            };
            start_plan(
                app,
                PendingAction::ProfileMoveSkill {
                    profile,
                    skill: moved,
                    pivot,
                },
                ReconcileOptions::default(),
            );
        }
        _ => {}
    }
}

fn move_profiles_selection(app: &mut App, delta: i64) {
    use crate::app::ProfilesFocus;
    match app.profiles.focus {
        ProfilesFocus::List => {
            let names: Vec<String> = app
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.profiles.iter().map(|p| p.name.clone()).collect())
                .unwrap_or_default();
            let current = names
                .iter()
                .position(|name| Some(name.as_str()) == app.profiles.profile.as_deref());
            if let Some(next) = step_index(current, names.len(), delta) {
                app.profiles.profile = Some(names[next].clone());
                app.profiles.skill_index = 0;
            }
        }
        ProfilesFocus::Detail => {
            let len = app
                .snapshot
                .as_ref()
                .and_then(|snapshot| {
                    let name = app.profiles.profile.as_deref()?;
                    snapshot.profile(name).map(|p| p.skills.len())
                })
                .unwrap_or(0);
            if let Some(next) = step_index(Some(app.profiles.skill_index), len, delta) {
                app.profiles.skill_index = next;
            }
        }
    }
}

fn installations_key(app: &mut App, key: Key) {
    use crate::app::InstallationsFocus;
    match key {
        Key::Left | Key::Char('h') => app.installations.focus = InstallationsFocus::List,
        Key::Right | Key::Char('l') => app.installations.focus = InstallationsFocus::Detail,
        Key::Up | Key::Char('k') => move_installations_selection(app, -1),
        Key::Down | Key::Char('j') => move_installations_selection(app, 1),
        Key::Char('a') => attach_action(app),
        Key::Char('d') => detach_action(app),
        Key::Char('u') => {
            let Some(row) = selected_installation(app) else {
                app.toast = Some("select an installation first".to_owned());
                return;
            };
            start_plan(
                app,
                PendingAction::UpdateInstallation {
                    workspace: row.installation.workspace.clone(),
                    target: Some(row.installation.target.clone()),
                },
                ReconcileOptions::default(),
            );
        }
        Key::Char('R') => {
            let Some(row) = selected_installation(app) else {
                app.toast = Some("select an installation first".to_owned());
                return;
            };
            app.dialog = Some(Dialog::Input(InputDialog {
                title: format!(
                    "Change source ref of {} ({})",
                    row.installation.workspace.display(),
                    row.installation.target
                ),
                fields: vec![InputField {
                    label: format!("new ref (currently {})", row.installation.source_ref),
                    buffer: String::new(),
                }],
                focused: 0,
                kind: InputKind::RefSet {
                    workspace: row.installation.workspace.clone(),
                    target: Some(row.installation.target.clone()),
                },
            }));
        }
        Key::Char('U') => {
            let Some(row) = selected_installation(app) else {
                app.toast = Some("select an installation first".to_owned());
                return;
            };
            let (workspace, target) = (
                row.installation.workspace.clone(),
                Some(row.installation.target.clone()),
            );
            // The chosen mode then goes through the normal plan → confirm
            // pipeline (§55: retire is a real reconciliation, keep-files is
            // a registry-only change).
            app.dialog = Some(Dialog::Pick(PickDialog {
                title: format!(
                    "Unregister {} ({})",
                    workspace.display(),
                    target.clone().unwrap_or_default()
                ),
                options: vec![
                    "Retire managed skills (extra files are preserved)".to_owned(),
                    "Keep all files (remove the registry record only)".to_owned(),
                ],
                values: vec!["false".to_owned(), "true".to_owned()],
                selected: 0,
                kind: PickKind::Unregister { workspace, target },
            }));
        }
        Key::Enter if app.installations.focus == InstallationsFocus::Detail => {
            open_membership(app);
        }
        Key::Char('[') | Key::Char(']') => reorder_attachments(app, key == Key::Char('[')),
        _ => {}
    }
}

fn attach_action(app: &mut App) {
    let options = profile_names(app);
    if options.is_empty() {
        app.toast = Some("no profiles defined yet".to_owned());
        return;
    }
    if let Some(row) = selected_installation(app) {
        let (workspace, target) = (
            row.installation.workspace.clone(),
            Some(row.installation.target.clone()),
        );
        let values = options.clone();
        app.dialog = Some(Dialog::Pick(PickDialog {
            title: format!(
                "Attach profile to {} ({})",
                workspace.display(),
                target.clone().unwrap_or_default()
            ),
            options,
            values,
            selected: 0,
            kind: PickKind::AttachProfile { workspace, target },
        }));
    } else {
        // No registration selected: offer a first install (§21).
        let values = options.clone();
        app.dialog = Some(Dialog::Pick(PickDialog {
            title: "Attach profile to a new workspace".to_owned(),
            options,
            values,
            selected: 0,
            kind: PickKind::AttachNew,
        }));
    }
}

fn detach_action(app: &mut App) {
    let Some(row) = selected_installation(app) else {
        app.toast = Some("select an installation first".to_owned());
        return;
    };
    if row.installation.profiles.is_empty() {
        app.toast = Some("this installation has no attached profiles".to_owned());
        return;
    }
    let (workspace, target) = (
        row.installation.workspace.clone(),
        Some(row.installation.target.clone()),
    );
    let options: Vec<String> = row
        .installation
        .profiles
        .iter()
        .map(|a| a.name.clone())
        .collect();
    let values = options.clone();
    app.dialog = Some(Dialog::Pick(PickDialog {
        title: format!(
            "Detach profile from {} ({})",
            workspace.display(),
            target.clone().unwrap_or_default()
        ),
        options,
        values,
        selected: 0,
        kind: PickKind::DetachProfile { workspace, target },
    }));
}

/// Builds the reorder request from the detail cursor (§79). The cursor
/// addresses attached profiles first, then effective skills.
fn reorder_attachments(app: &mut App, up: bool) {
    use crate::app::InstallationsFocus;
    if app.installations.focus != InstallationsFocus::Detail {
        app.toast = Some("focus the detail pane to reorder".to_owned());
        return;
    }
    let Some(row) = selected_installation(app) else {
        app.toast = Some("select an installation first".to_owned());
        return;
    };
    let cursor = app.installations.cursor.unwrap_or(0);
    let profile_count = row.installation.profiles.len();
    if cursor >= profile_count {
        app.toast = Some("place the cursor on an attached profile to reorder".to_owned());
        return;
    }
    let neighbor = if up {
        cursor.checked_sub(1)
    } else if cursor + 1 < profile_count {
        Some(cursor + 1)
    } else {
        None
    };
    let Some(neighbor) = neighbor else {
        app.toast = Some("no room to move in that direction".to_owned());
        return;
    };
    let mut order: Vec<beskar_core::ids::ProfileId> =
        row.installation.profiles.iter().map(|a| a.id).collect();
    order.swap(cursor, neighbor);
    start_plan(
        app,
        PendingAction::ReorderProfiles {
            workspace: row.installation.workspace.clone(),
            target: Some(row.installation.target.clone()),
            order,
        },
        ReconcileOptions::default(),
    );
}

/// Opens the §100 membership view for the skill under the detail cursor.
fn open_membership(app: &mut App) {
    use crate::app::InstallationsFocus;
    if app.installations.focus != InstallationsFocus::Detail {
        app.toast = Some("focus the detail pane to inspect skills".to_owned());
        return;
    }
    let Some(row) = selected_installation(app) else {
        return;
    };
    let Some(status) = &row.status else {
        app.toast = Some(
            row.error
                .clone()
                .unwrap_or_else(|| "status unavailable".to_owned()),
        );
        return;
    };
    let cursor = app.installations.cursor.unwrap_or(0);
    let profile_count = row.installation.profiles.len();
    if cursor < profile_count {
        app.toast = Some("place the cursor on a skill to inspect membership".to_owned());
        return;
    }
    let Some((name, _)) = status.skills.iter().nth(cursor - profile_count) else {
        return;
    };
    let path = app
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.skill(name.as_str()))
        .map(|listing| listing.path.clone());
    queue_effect(Effect::Membership {
        installation: row.installation.id,
        skill: name.to_string(),
        path,
    });
}

fn move_installations_selection(app: &mut App, delta: i64) {
    use crate::app::InstallationsFocus;
    match app.installations.focus {
        InstallationsFocus::List => {
            let ids: Vec<InstallationId> = app
                .snapshot
                .as_ref()
                .map(|snapshot| {
                    snapshot
                        .installations
                        .iter()
                        .map(|row| row.installation.id)
                        .collect()
                })
                .unwrap_or_default();
            let current = ids
                .iter()
                .position(|id| Some(*id) == app.installations.installation);
            if let Some(next) = step_index(current, ids.len(), delta) {
                app.installations.installation = Some(ids[next]);
                app.installations.cursor = Some(0);
            }
        }
        InstallationsFocus::Detail => {
            let len = selected_installation(app)
                .map(|row| {
                    row.installation.profiles.len()
                        + row
                            .status
                            .as_ref()
                            .map(|status| status.skills.len())
                            .unwrap_or(0)
                })
                .unwrap_or(0);
            if let Some(next) = step_index(app.installations.cursor, len, delta) {
                app.installations.cursor = Some(next);
            }
        }
    }
}

fn selected_installation(app: &App) -> Option<&InstallationRow> {
    let id = app.installations.installation?;
    app.snapshot.as_ref()?.installation(id)
}

fn git_key(app: &mut App, key: Key) {
    let branch_names = |app: &App| -> Vec<String> {
        app.snapshot
            .as_ref()
            .map(|snapshot| snapshot.branches.iter().map(|b| b.name.clone()).collect())
            .unwrap_or_default()
    };
    match key {
        Key::Up | Key::Char('k') | Key::Down | Key::Char('j') => {
            let delta = if key.is_down() { 1 } else { -1 };
            let names = branch_names(app);
            let current = names
                .iter()
                .position(|name| Some(name.as_str()) == app.git.branch.as_deref());
            if let Some(next) = step_index(current, names.len(), delta) {
                app.git.branch = Some(names[next].clone());
            }
        }
        Key::Char('f') => start_plan(
            app,
            PendingAction::Fetch {
                remote: beskar_core::remote::DEFAULT_REMOTE.to_owned(),
            },
            ReconcileOptions::default(),
        ),
        Key::Char('p') => start_plan(
            app,
            PendingAction::Push {
                branch: app.git.branch.clone(),
                set_upstream: false,
                allow_dirty: false,
            },
            ReconcileOptions::default(),
        ),
        Key::Char('s') => {
            let Some(branch) = app.git.branch.clone() else {
                app.toast = Some("select a branch first".to_owned());
                return;
            };
            if app
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.library.branch.as_deref())
                == Some(branch.as_str())
            {
                app.toast = Some("that branch is already checked out".to_owned());
                return;
            }
            start_plan(
                app,
                PendingAction::SwitchBranch { name: branch },
                ReconcileOptions::default(),
            );
        }
        Key::Char('c') => {
            app.dialog = Some(Dialog::Input(InputDialog {
                title: "Create branch".to_owned(),
                fields: vec![InputField {
                    label: "branch name".to_owned(),
                    buffer: String::new(),
                }],
                focused: 0,
                kind: InputKind::CreateBranch,
            }));
        }
        _ => {}
    }
}

fn activity_key(app: &mut App, key: Key) {
    let len = app.activity.entries.len();
    let delta = match key {
        Key::Up | Key::Char('k') | Key::PageUp => -1,
        Key::Down | Key::Char('j') | Key::PageDown => 1,
        _ => return,
    };
    if let Some(next) = step_index(Some(app.activity.scroll), len, delta) {
        app.activity.scroll = next;
    }
}

// ---- effect starters ---------------------------------------------------------

fn start_plan(app: &mut App, action: PendingAction, options: ReconcileOptions) {
    app.planning = Some((action.clone(), options));
    app.toast = None;
    PENDING.with(|cell| {
        cell.borrow_mut().push(Effect::Plan {
            action: Box::new(action),
            options,
        })
    });
}

fn start_execute(app: &mut App, action: PendingAction, options: ReconcileOptions) {
    app.toast = None;
    PENDING.with(|cell| {
        cell.borrow_mut().push(Effect::Execute {
            action: Box::new(action),
            options,
        })
    });
}

fn start_validate(profile: Option<String>) {
    PENDING.with(|cell| {
        cell.borrow_mut().push(Effect::Validate { profile });
    });
}

fn queue_effect(effect: Effect) {
    PENDING.with(|cell| cell.borrow_mut().push(effect));
}

thread_local! {
    /// Effects queued by the reducer for the event just processed. Drained
    /// by [`take_effects`] immediately after each `reduce` call.
    static PENDING: std::cell::RefCell<Vec<Effect>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Drains the effects queued by the most recent [`reduce`] call. Call
/// exactly once per event, right after `reduce`.
pub fn take_effects() -> Vec<Effect> {
    PENDING.with(|cell| cell.borrow_mut().drain(..).collect())
}

// ---- confirm flow (§89 step 3, §47, §91) -------------------------------------

/// Opens the confirmation dialog for a freshly produced plan. Waivable
/// blockers (§47, §49) offer an explicit-consent re-plan that will display
/// the exact destructive operations; non-waivable blockers leave only
/// Cancel (fail closed, §4).
fn open_confirm(
    app: &mut App,
    change: PlannedChange,
    planned: Option<(PendingAction, ReconcileOptions)>,
) {
    let title = change.title().to_owned();
    let (summary, choices) = confirm_choices(&change, planned.as_ref());
    if choices.is_empty() {
        // No-op plan: nothing to confirm or apply (§22, §91).
        app.toast = Some(format!("{title} — nothing to do"));
        return;
    }
    app.dialog = Some(Dialog::Confirm(ConfirmDialog {
        title,
        change,
        summary,
        choices,
        selected: 0,
        scroll: 0,
    }));
}

/// Builds the context lines and choices for a planned change.
fn confirm_choices(
    change: &PlannedChange,
    planned: Option<&(PendingAction, ReconcileOptions)>,
) -> (Vec<String>, Vec<ConfirmChoice>) {
    let run = |options: ReconcileOptions| match planned {
        Some((action, _)) => ConfirmApply::Run {
            action: Box::new(action.clone()),
            options,
        },
        None => ConfirmApply::Cancel,
    };
    let replan = |options: ReconcileOptions| match planned {
        Some((action, _)) => ConfirmApply::Replan {
            action: Box::new(action.clone()),
            options,
        },
        None => ConfirmApply::Cancel,
    };
    match change {
        PlannedChange::Install { plan, .. } => {
            let summary = crate::view::plan_lines(plan);
            if plan.is_no_op() {
                return (summary, Vec::new());
            }
            if plan.is_blocked() {
                let waivers = waivable_options(plan);
                let mut lines = summary;
                lines.push(String::new());
                lines.push(
                    "Blocked: nothing will be written without explicit \
                     consent (§47, §49)."
                        .to_owned(),
                );
                let choices = if waivers.force || waivers.replace_unmanaged {
                    vec![
                        ConfirmChoice {
                            label: "Force: discard the modified content listed above (§47)"
                                .to_owned(),
                            apply: replan(waivers),
                        },
                        ConfirmChoice {
                            label: "Cancel".to_owned(),
                            apply: ConfirmApply::Cancel,
                        },
                    ]
                } else {
                    vec![ConfirmChoice {
                        label: "Cancel".to_owned(),
                        apply: ConfirmApply::Cancel,
                    }]
                };
                return (lines, choices);
            }
            let options = planned.map(|(_, options)| *options).unwrap_or_default();
            (
                summary,
                vec![
                    ConfirmChoice {
                        label: "Apply".to_owned(),
                        apply: run(options),
                    },
                    ConfirmChoice {
                        label: "Cancel".to_owned(),
                        apply: ConfirmApply::Cancel,
                    },
                ],
            )
        }
        PlannedChange::Library { plan, notes, .. } => {
            let mut summary = notes.clone();
            summary.push(format!("commit message: {}", plan.message));
            for op in &plan.ops {
                summary.push(op_line(op));
            }
            if plan.is_no_op() {
                return (summary, Vec::new());
            }
            (
                summary,
                vec![
                    ConfirmChoice {
                        label: "Apply".to_owned(),
                        apply: run(ReconcileOptions::default()),
                    },
                    ConfirmChoice {
                        label: "Cancel".to_owned(),
                        apply: ConfirmApply::Cancel,
                    },
                ],
            )
        }
        PlannedChange::Summary { lines, .. } => (
            lines.clone(),
            vec![
                ConfirmChoice {
                    label: "Apply".to_owned(),
                    apply: run(ReconcileOptions::default()),
                },
                ConfirmChoice {
                    label: "Cancel".to_owned(),
                    apply: ConfirmApply::Cancel,
                },
            ],
        ),
        PlannedChange::Fetch { outcome } => (
            crate::view::fetch_lines(outcome),
            vec![
                ConfirmChoice {
                    label: "Fetch now".to_owned(),
                    apply: ConfirmApply::Run {
                        action: Box::new(PendingAction::Fetch {
                            remote: outcome.remote.clone(),
                        }),
                        options: ReconcileOptions::default(),
                    },
                },
                ConfirmChoice {
                    label: "Cancel".to_owned(),
                    apply: ConfirmApply::Cancel,
                },
            ],
        ),
        PlannedChange::Push { outcome } => (
            crate::view::push_lines(outcome),
            vec![
                ConfirmChoice {
                    label: "Push now".to_owned(),
                    apply: ConfirmApply::Run {
                        action: Box::new(PendingAction::Push {
                            branch: Some(outcome.branch.clone()),
                            set_upstream: outcome.upstream_before.is_none()
                                && outcome.upstream_after.is_some(),
                            allow_dirty: false,
                        }),
                        options: ReconcileOptions::default(),
                    },
                },
                ConfirmChoice {
                    label: "Cancel".to_owned(),
                    apply: ConfirmApply::Cancel,
                },
            ],
        ),
    }
}

/// One line describing a planned library edit.
fn op_line(op: &beskar_core::editing::LibraryOp) -> String {
    use beskar_core::editing::LibraryOpKind;
    match op.kind {
        LibraryOpKind::Write => format!("  write {}", op.path),
        LibraryOpKind::Edit => format!("  edit {}", op.path),
        LibraryOpKind::Replace => format!("  replace {}", op.path),
        LibraryOpKind::Remove => format!("  remove {}", op.path),
        LibraryOpKind::Move => match &op.from {
            Some(from) => format!("  move {from} → {}", op.path),
            None => format!("  move → {}", op.path),
        },
    }
}

/// Whether every blocker can be waived by force/replace consent (§47, §49).
/// Foreign stamps, library mismatches, missing refs, missing profiles,
/// missing skills and diverged state can never be (§4).
fn waivable_options(plan: &ReconciliationPlan) -> ReconcileOptions {
    if plan.blockers.is_empty() {
        return ReconcileOptions::default();
    }
    let mut options = ReconcileOptions::default();
    for blocker in &plan.blockers {
        match blocker.kind {
            BlockerKind::ModifiedContent => options.force = true,
            BlockerKind::UnmanagedCollision => options.replace_unmanaged = true,
            _ => return ReconcileOptions::default(),
        }
    }
    options
}

// ---- activity + errors --------------------------------------------------------

fn open_error(app: &mut App, title: &str, err: &Error) {
    app.activity.push(ActivityEntry {
        time: crate::view::timestamp(),
        title: title.to_owned(),
        lines: vec![format!("error ({}): {}", err.code(), err)],
        failed: true,
    });
    app.dialog = Some(Dialog::Message(MessageDialog {
        title: title.to_owned(),
        lines: vec![
            format!("error ({}): {}", err.code(), err),
            String::new(),
            "See the Activity log and `beskar doctor` for diagnostics.".to_owned(),
        ],
        failed: true,
    }));
}

fn log_outcome(app: &mut App, outcome: &crate::app::ActionOutcome) {
    let mut lines = outcome.lines.clone();
    lines.extend(outcome.warnings.iter().map(|w| format!("warning: {w}")));
    if lines.is_empty() {
        lines.push("completed".to_owned());
    }
    app.activity.push(ActivityEntry {
        time: crate::view::timestamp(),
        title: outcome.title.clone(),
        lines,
        failed: false,
    });
}

/// Re-establishes selections against a fresh snapshot and reloads the
/// skill preview when needed.
fn reselect_after_refresh(app: &mut App, effects: &mut Vec<Effect>) {
    let Some(snapshot) = app.snapshot.as_ref() else {
        return;
    };

    // Skills: keep the selected skill when it still exists.
    let skills: Vec<String> = snapshot.skills.iter().map(|s| s.name.to_string()).collect();
    if app
        .skills
        .skill
        .as_ref()
        .is_some_and(|name| !skills.contains(name))
    {
        app.skills.skill = None;
        app.skills.preview = None;
    }
    if app.skills.skill.is_none() {
        app.skills.skill = skills.first().cloned();
    }
    if let Some(skill) = app.skills.skill.clone() {
        let needs_load = app
            .skills
            .preview
            .as_ref()
            .map(|preview| preview.name != skill)
            .unwrap_or(true);
        if needs_load {
            effects.push(Effect::LoadSkill { skill });
        }
    }

    // Profiles: keep the selected profile when it still exists.
    let profiles: Vec<String> = snapshot.profiles.iter().map(|p| p.name.clone()).collect();
    if app
        .profiles
        .profile
        .as_ref()
        .is_some_and(|name| !profiles.contains(name))
    {
        app.profiles.profile = None;
        app.profiles.skill_index = 0;
    }
    if app.profiles.profile.is_none() {
        app.profiles.profile = profiles.first().cloned();
    }

    // Installations.
    let ids: Vec<InstallationId> = snapshot
        .installations
        .iter()
        .map(|row| row.installation.id)
        .collect();
    if app
        .installations
        .installation
        .is_none_or(|id| !ids.contains(&id))
    {
        app.installations.installation = ids.first().copied();
        app.installations.cursor = Some(0);
    }

    // Git: default to the checked-out branch.
    if app.git.branch.is_none() {
        app.git.branch = snapshot.library.branch.clone();
    }
}

#[cfg(test)]
mod tests {
    use beskar_core::plan::{Blocker, BlockerKind, PlanAction, SkillAction};
    use beskar_core::reconcile::ReconcileOptions;

    use crate::app::{
        ConfirmApply, Dialog, MessageDialog, PendingAction, PlannedChange, SkillsFocus,
    };
    use crate::effect::Effect;
    use crate::event::Event;
    use crate::input::Key;
    use crate::testkit;

    use super::{reduce, take_effects};

    fn app_with_snapshot() -> crate::app::App {
        let mut app = crate::app::App::new();
        reduce(&mut app, Event::Loaded(Ok(Box::new(testkit::snapshot()))));
        take_effects();
        app
    }

    fn keys(app: &mut crate::app::App, keys: &[Key]) -> Vec<Effect> {
        let mut effects = Vec::new();
        for key in keys {
            effects.extend(reduce(app, Event::Key(*key)));
        }
        effects
    }

    fn plan(plan: beskar_core::plan::ReconciliationPlan) -> PlannedChange {
        PlannedChange::Install {
            title: "Test mutation".to_owned(),
            plan,
        }
    }

    fn empty_plan() -> beskar_core::plan::ReconciliationPlan {
        beskar_core::plan::ReconciliationPlan {
            installation_id: testkit::installation_id(),
            resolved_commit: Some("abc123".to_owned()),
            profile_changes: vec![],
            skill_actions: vec![],
            state_actions: vec![],
            blockers: vec![],
        }
    }

    fn action_plan(action: PlanAction, skill: &str) -> beskar_core::plan::ReconciliationPlan {
        beskar_core::plan::ReconciliationPlan {
            installation_id: testkit::installation_id(),
            resolved_commit: Some("abc123".to_owned()),
            profile_changes: vec![],
            skill_actions: vec![SkillAction {
                action,
                skill: beskar_core::ids::SkillName::parse(skill).expect("valid skill"),
                resulting_state: beskar_core::drift::DriftState::ProfileAdded,
                paths: vec![],
            }],
            state_actions: vec![],
            blockers: vec![],
        }
    }

    fn dialog_of(app: &crate::app::App) -> &Dialog {
        app.dialog.as_ref().expect("a dialog is open")
    }

    // ---- navigation -------------------------------------------------------

    #[test]
    fn tab_cycles_screens_and_numbers_jump_directly() {
        let mut app = crate::app::App::new();
        assert_eq!(app.screen, crate::app::Screen::Dashboard);
        keys(&mut app, &[Key::Tab]);
        assert_eq!(app.screen, crate::app::Screen::Skills);
        keys(&mut app, &[Key::BackTab]);
        assert_eq!(app.screen, crate::app::Screen::Dashboard);
        keys(&mut app, &[Key::Char('4')]);
        assert_eq!(app.screen, crate::app::Screen::Installations);
        keys(&mut app, &[Key::Char('9')]);
        assert_eq!(
            app.screen,
            crate::app::Screen::Installations,
            "out of range is ignored"
        );
    }

    #[test]
    fn q_quits_but_other_keys_do_not() {
        let mut app = crate::app::App::new();
        keys(&mut app, &[Key::Char('x')]);
        assert!(!app.quit);
        keys(&mut app, &[Key::Char('q')]);
        assert!(app.quit);
    }

    #[test]
    fn refresh_key_queues_effect_refresh() {
        let mut app = app_with_snapshot();
        let effects = keys(&mut app, &[Key::Char('r')]);
        assert_eq!(effects.len(), 1);
        assert!(matches!(effects[0], Effect::Refresh));
    }

    // ---- snapshot load + selection ----------------------------------------

    #[test]
    fn first_snapshot_selects_defaults_and_requests_preview() {
        let mut app = crate::app::App::new();
        let effects = reduce(&mut app, Event::Loaded(Ok(Box::new(testkit::snapshot()))));
        let snapshot = app.snapshot.as_ref().expect("loaded");
        assert_eq!(snapshot.skills.len(), 4);
        assert_eq!(app.skills.skill.as_deref(), Some("code-review"));
        assert_eq!(app.profiles.profile.as_deref(), Some("dev-core"));
        assert_eq!(
            app.installations.installation,
            Some(testkit::installation_id())
        );
        assert_eq!(app.git.branch.as_deref(), Some("main"));
        assert!(
            effects.iter().any(
                |effect| matches!(effect, Effect::LoadSkill { skill } if skill == "code-review")
            )
        );
    }

    #[test]
    fn refreshed_snapshot_keeps_selection_and_reloads_changed_preview() {
        let mut app = app_with_snapshot();
        app.skills.skill = Some("testing".to_owned());
        app.skills.preview = Some(crate::app::SkillView {
            name: "stale".to_owned(),
            detail: preview_detail("stale"),
            skill_md: String::new(),
        });
        let effects = reduce(&mut app, Event::Loaded(Ok(Box::new(testkit::snapshot()))));
        assert_eq!(app.skills.skill.as_deref(), Some("testing"));
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::LoadSkill { skill } if skill == "testing")),
            "a stale preview must be reloaded"
        );
    }

    fn preview_detail(name: &str) -> beskar_core::editing::SkillDetail {
        beskar_core::editing::SkillDetail {
            listing: testkit::skill_listing(name, "quality", "desc", &[]),
            files: vec!["SKILL.md".to_owned()],
        }
    }

    #[test]
    fn skill_list_movement_loads_new_preview() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('2'), Key::Right]); // skills screen, list pane
        assert_eq!(app.skills.focus, SkillsFocus::List);
        let effects = keys(&mut app, &[Key::Down]);
        assert_eq!(app.skills.skill.as_deref(), Some("git"));
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::LoadSkill { skill } if skill == "git"))
        );
    }

    #[test]
    fn bucket_selection_filters_and_query_searches() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('2')]);
        // Bucket rows: (all), engineering, engineering/process, languages,
        // quality — four Downs land on quality.
        keys(&mut app, &[Key::Down, Key::Down, Key::Down, Key::Down]);
        assert_eq!(app.skills.bucket.as_deref(), Some("quality"));
        assert_eq!(app.filtered_skills().len(), 1);

        // Query mode: '/' then type, Enter to apply.
        keys(&mut app, &[Key::Char('/')]);
        assert!(app.skills.querying);
        keys(
            &mut app,
            &[
                Key::Char('r'),
                Key::Char('u'),
                Key::Char('s'),
                Key::Char('t'),
            ],
        );
        keys(&mut app, &[Key::Enter]);
        assert!(!app.skills.querying);
        assert_eq!(app.skills.query, "rust");
        // Bucket filter (quality) still applies: no rust skill in quality.
        assert_eq!(app.filtered_skills().len(), 0);
        // Cancel editing via '/' + Esc: canceling clears the query.
        keys(&mut app, &[Key::Char('/'), Key::Esc]);
        assert_eq!(app.skills.query, "");
        // Back to "(all)" — four Ups from quality; with the query cleared,
        // every skill is visible again.
        keys(&mut app, &[Key::Up, Key::Up, Key::Up, Key::Up]);
        assert_eq!(app.skills.bucket, None);
        assert_eq!(app.filtered_skills().len(), 4);
        // And a fresh query narrows to the single rust skill.
        keys(
            &mut app,
            &[
                Key::Char('/'),
                Key::Char('r'),
                Key::Char('u'),
                Key::Char('s'),
                Key::Char('t'),
                Key::Enter,
            ],
        );
        assert_eq!(app.filtered_skills().len(), 1, "rust matches the query");
    }

    #[test]
    fn esc_in_query_clears_the_query() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('/'), Key::Char('x'), Key::Esc]);
        assert!(!app.skills.querying);
        assert_eq!(app.skills.query, "");
    }

    // ---- plan → confirm → execute flow --------------------------------------

    #[test]
    fn attach_flow_plans_confirms_executes_and_refreshes() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('4')]);
        // Attach: 'a' opens the profile picker for the selected installation.
        keys(&mut app, &[Key::Char('a')]);
        let Dialog::Pick(pick) = dialog_of(&app) else {
            panic!("expected pick dialog");
        };
        assert_eq!(pick.values.len(), 2);
        // Choose "rust-development" (second option).
        keys(&mut app, &[Key::Down, Key::Enter]);
        assert!(app.dialog.is_none(), "picker closes after choose");
        assert!(matches!(
            app.planning.as_ref().map(|(action, _)| action),
            Some(PendingAction::AttachProfile { profile, .. }) if profile == "rust-development"
        ));
        reduce(
            &mut app,
            Event::Planned(Ok(Box::new(plan(action_plan(
                PlanAction::InstallSkill,
                "testing",
            ))))),
        );
        take_effects();
        let Dialog::Confirm(confirm) = dialog_of(&app) else {
            panic!("expected confirm dialog");
        };
        assert_eq!(
            confirm.choices.len(),
            2,
            "apply + cancel for an unblocked plan"
        );
        // Confirm "Apply".
        let effects = keys(&mut app, &[Key::Enter]);
        assert_eq!(effects.len(), 1);
        let Effect::Execute { action, options } = &effects[0] else {
            panic!("expected execute effect");
        };
        assert!(matches!(
            action.as_ref(),
            PendingAction::AttachProfile { .. }
        ));
        assert_eq!(*options, ReconcileOptions::default());
        // The runtime executes and reports:
        let effects = reduce(
            &mut app,
            Event::Executed(Ok(Box::new(crate::app::ActionOutcome {
                title: "Attach profile".to_owned(),
                applied: true,
                lines: vec!["install testing".to_owned()],
                warnings: vec![],
            }))),
        );
        take_effects();
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::Refresh))
        );
        assert_eq!(app.activity.entries.len(), 1);
        assert_eq!(app.activity.entries[0].title, "Attach profile");
        assert!(app.toast.as_deref().is_some_and(|t| t.contains("applied")));
    }

    #[test]
    fn planning_failure_opens_error_dialog_with_stable_code() {
        let mut app = app_with_snapshot();
        app.planning = Some((PendingAction::UpdateAll, ReconcileOptions::default()));
        let err = beskar_core::Error::validation("profile missing");
        reduce(&mut app, Event::Planned(Err(err)));
        take_effects();
        let Dialog::Message(message) = dialog_of(&app) else {
            panic!("expected message dialog");
        };
        assert!(message.failed);
        assert!(message.lines[0].starts_with("error (validation):"));
        assert!(app.planning.is_none(), "stale planning state is cleared");
    }

    #[test]
    fn blocked_plan_offers_force_replan_for_waivable_blockers_only() {
        let mut app = app_with_snapshot();
        app.planning = Some((PendingAction::UpdateAll, ReconcileOptions::default()));
        let mut blocked = action_plan(PlanAction::UpdateSkill, "testing");
        blocked.blockers = vec![Blocker {
            kind: BlockerKind::ModifiedContent,
            skill: Some(beskar_core::ids::SkillName::parse("testing").expect("valid")),
            profile: None,
            paths: vec!["testing/SKILL.md".to_owned()],
        }];
        reduce(&mut app, Event::Planned(Ok(Box::new(plan(blocked)))));
        take_effects();
        let Dialog::Confirm(confirm) = dialog_of(&app) else {
            panic!("expected confirm dialog");
        };
        assert_eq!(confirm.choices.len(), 2);
        assert!(confirm.choices[0].label.contains("Force"));
        // Confirming force re-plans with explicit consent (§47: the re-plan
        // will display the exact destructive operations).
        let effects = keys(&mut app, &[Key::Enter]);
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::Plan { options, .. } if options.force))
        );

        // A non-waivable blocker leaves only Cancel (fail closed, §4).
        app.planning = Some((PendingAction::UpdateAll, ReconcileOptions::default()));
        let mut hopeless = empty_plan();
        hopeless.blockers = vec![Blocker {
            kind: BlockerKind::MissingProfile,
            skill: None,
            profile: Some(testkit::rust_id()),
            paths: vec![],
        }];
        reduce(&mut app, Event::Planned(Ok(Box::new(plan(hopeless)))));
        take_effects();
        let Dialog::Confirm(confirm) = dialog_of(&app) else {
            panic!("expected confirm dialog");
        };
        assert_eq!(confirm.choices.len(), 1);
        assert_eq!(confirm.choices[0].label, "Cancel");
    }

    #[test]
    fn force_replan_then_execute_carries_force_options() {
        let mut app = app_with_snapshot();
        app.planning = Some((PendingAction::UpdateAll, ReconcileOptions::default()));
        let mut blocked = action_plan(PlanAction::UpdateSkill, "testing");
        blocked.blockers = vec![Blocker {
            kind: BlockerKind::ModifiedContent,
            skill: None,
            profile: None,
            paths: vec![],
        }];
        reduce(&mut app, Event::Planned(Ok(Box::new(plan(blocked)))));
        take_effects();
        let effects = keys(&mut app, &[Key::Enter]); // choose Force
        let Effect::Plan { action, options } = &effects[0] else {
            panic!("expected plan effect");
        };
        assert!(options.force);
        // The re-planned (now unblocked) plan is confirmed:
        reduce(
            &mut app,
            Event::Planned(Ok(Box::new(plan(action_plan(
                PlanAction::UpdateSkill,
                "testing",
            ))))),
        );
        take_effects();
        let effects = keys(&mut app, &[Key::Enter]); // choose Apply
        let Effect::Execute { options, .. } = &effects[0] else {
            panic!("expected execute effect");
        };
        assert!(
            options.force,
            "execution must carry the same consent the plan was built with"
        );
        assert!(matches!(action.as_ref(), PendingAction::UpdateAll));
    }

    #[test]
    fn no_op_plan_shows_a_toast_instead_of_a_dialog() {
        let mut app = app_with_snapshot();
        app.planning = Some((PendingAction::UpdateAll, ReconcileOptions::default()));
        reduce(&mut app, Event::Planned(Ok(Box::new(plan(empty_plan())))));
        take_effects();
        assert!(app.dialog.is_none());
        assert!(
            app.toast
                .as_deref()
                .is_some_and(|t| t.contains("nothing to do"))
        );
    }

    // ---- drift + protection visibility (§38, §39) ----------------------------

    #[test]
    fn dashboard_counts_drift_and_missing_profiles_from_the_snapshot() {
        // §38: per-skill drift states surface as dashboard counts.
        let mut app = crate::app::App::new();
        reduce(
            &mut app,
            Event::Loaded(Ok(Box::new(testkit::drifted_snapshot()))),
        );
        take_effects();
        let metrics = app.snapshot.as_ref().expect("snapshot").dashboard();
        assert_eq!(metrics.outdated, 1, "one outdated installation");
        assert_eq!(metrics.modified, 1, "one modified installation");
        assert_eq!(metrics.installations, 1);

        // §39: a missing attached profile is a protected, broken state —
        // never an empty profile.
        let mut app = crate::app::App::new();
        reduce(
            &mut app,
            Event::Loaded(Ok(Box::new(testkit::snapshot_with_missing_profile()))),
        );
        take_effects();
        let snapshot = app.snapshot.as_ref().expect("snapshot");
        let metrics = snapshot.dashboard();
        assert_eq!(metrics.missing_profiles, 1);
        assert_eq!(metrics.broken, 1);
        let status = snapshot.installations[0].status.as_ref().expect("status");
        assert!(status.profiles[1].profile.is_none(), "§39: unresolvable");
        assert!(status.profiles[0].profile.is_some());
    }

    #[test]
    fn the_membership_popup_for_a_shared_skill_loads_all_owners() {
        // §100/§37: the shared skill `testing` is listed once and the §100
        // view resolves ALL requiring profiles for it.
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('4'), Key::Right]);
        // Detail rows: profiles 0-1, then skills sorted: git (2), rust (3),
        // testing (4).
        keys(&mut app, &[Key::Down, Key::Down, Key::Down, Key::Down]);
        assert_eq!(app.installations.cursor, Some(4));
        let effects = keys(&mut app, &[Key::Enter]);
        assert!(matches!(
            &effects[0],
            Effect::Membership { skill, .. } if skill == "testing"
        ));
        reduce(
            &mut app,
            Event::MembershipLoaded(Ok(Box::new(crate::app::MembershipView {
                installation: testkit::installation_id(),
                skill: "testing".to_owned(),
                required_by: vec![
                    (testkit::dev_id(), "dev-core".to_owned()),
                    (testkit::rust_id(), "rust-development".to_owned()),
                ],
                last_required_by: vec![],
                source_ref: "main".to_owned(),
                library_commit: Some("def456".to_owned()),
                skill_commit: Some("abc123".to_owned()),
                state: Some(beskar_core::drift::DriftState::Current),
                membership_drift: None,
            }))),
        );
        take_effects();
        let Dialog::Membership(view) = dialog_of(&app) else {
            panic!("expected the §100 membership popup");
        };
        assert_eq!(view.required_by.len(), 2, "both owners are visible");
    }

    #[test]
    fn detach_picker_offers_the_missing_profile_by_last_known_name() {
        // §40: an unresolvable attachment is still explicitly detachable by
        // its last-known Registry name.
        let mut app = crate::app::App::new();
        reduce(
            &mut app,
            Event::Loaded(Ok(Box::new(testkit::snapshot_with_missing_profile()))),
        );
        take_effects();
        keys(&mut app, &[Key::Char('4'), Key::Char('d')]);
        let Dialog::Pick(pick) = dialog_of(&app) else {
            panic!("expected the detach picker");
        };
        assert_eq!(
            pick.values,
            vec!["dev-core".to_owned(), "rust-development".to_owned()],
            "the missing profile is offered under its last-known name"
        );
        keys(&mut app, &[Key::Down, Key::Enter]);
        assert!(matches!(
            app.planning.as_ref().map(|(action, _)| action),
            Some(PendingAction::DetachProfile { profile, .. }) if profile == "rust-development"
        ));
    }

    #[test]
    fn ref_set_input_plans_the_change_before_applying() {
        // §56: changing the source ref is planned (implications computed)
        // before it can be applied.
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('4')]);
        keys(&mut app, &[Key::Char('R')]);
        let Dialog::Input(input) = dialog_of(&app) else {
            panic!("expected the ref-set input dialog");
        };
        assert!(matches!(input.kind, crate::app::InputKind::RefSet { .. }));
        for c in "next".chars() {
            keys(&mut app, &[Key::Char(c)]);
        }
        let effects = keys(&mut app, &[Key::Enter]);
        let Effect::Plan { action, options } = &effects[0] else {
            panic!("expected a plan effect");
        };
        let PendingAction::RefSet { new_ref, .. } = action.as_ref() else {
            panic!("expected a ref-set action");
        };
        assert_eq!(new_ref, "next");
        assert_eq!(*options, ReconcileOptions::default());
    }

    #[test]
    fn blocked_plan_lists_every_blocker_with_exact_paths() {
        // §47: ALL blockers surface in one planning pass, each with the
        // exact managed paths, before any force consent is offered.
        let mut app = app_with_snapshot();
        app.planning = Some((
            PendingAction::UpdateInstallation {
                workspace: std::path::PathBuf::from("/fixtures/workspace"),
                target: None,
            },
            ReconcileOptions::default(),
        ));
        let sid = |name: &str| beskar_core::ids::SkillName::parse(name).expect("valid");
        let mut blocked = empty_plan();
        blocked.blockers = vec![
            Blocker {
                kind: BlockerKind::ModifiedContent,
                skill: Some(sid("git")),
                profile: None,
                paths: vec!["git/SKILL.md".to_owned()],
            },
            Blocker {
                kind: BlockerKind::ModifiedContent,
                skill: Some(sid("testing")),
                profile: None,
                paths: vec!["testing/SKILL.md".to_owned()],
            },
        ];
        reduce(&mut app, Event::Planned(Ok(Box::new(plan(blocked)))));
        take_effects();
        let Dialog::Confirm(confirm) = dialog_of(&app) else {
            panic!("expected the confirm dialog");
        };
        for path in ["git/SKILL.md", "testing/SKILL.md"] {
            assert!(
                confirm.summary.iter().any(|line| line.contains(path)),
                "blocker for {path} must be listed: {:?}",
                confirm.summary
            );
        }
        assert_eq!(confirm.choices.len(), 2);
        assert!(confirm.choices[0].label.contains("Force"));
    }

    #[test]
    fn detach_flow_targets_the_selected_installation() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('4')]);
        keys(&mut app, &[Key::Char('d')]);
        let Dialog::Pick(pick) = dialog_of(&app) else {
            panic!("expected pick dialog");
        };
        assert_eq!(pick.values, vec!["dev-core", "rust-development"]);
        keys(&mut app, &[Key::Enter]);
        let Some((action, _)) = app.planning.clone() else {
            panic!("expected planning state");
        };
        let PendingAction::DetachProfile {
            profile,
            target,
            workspace,
        } = action
        else {
            panic!("expected detach action");
        };
        assert_eq!(profile, "dev-core");
        assert_eq!(target.as_deref(), Some(".agents/skills"));
        assert_eq!(
            workspace,
            app.snapshot.as_ref().expect("snapshot").installations[0]
                .installation
                .workspace
        );
    }

    #[test]
    fn unregister_offers_retire_and_keep_files_choices() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('4')]);
        keys(&mut app, &[Key::Char('U')]);
        let Dialog::Pick(pick) = dialog_of(&app) else {
            panic!("expected unregister picker");
        };
        assert_eq!(pick.options.len(), 2);
        // Choose "Keep all files": the mode goes through the normal plan
        // pipeline (§55), so a dry-run plan is requested first.
        let effects = keys(&mut app, &[Key::Down, Key::Enter]);
        let Effect::Plan { action, .. } = &effects[0] else {
            panic!("expected plan effect");
        };
        let PendingAction::Unregister { keep_files, .. } = action.as_ref() else {
            panic!("expected unregister action");
        };
        assert!(keep_files);
        // The keep-files plan is a textual preview; confirming executes.
        reduce(
            &mut app,
            Event::Planned(Ok(Box::new(PlannedChange::Summary {
                title: "Unregister installation (keep files)".to_owned(),
                lines: vec!["Remove the registry record only.".to_owned()],
            }))),
        );
        take_effects();
        let effects = keys(&mut app, &[Key::Enter]);
        let Effect::Execute { action, .. } = &effects[0] else {
            panic!("expected execute effect");
        };
        let PendingAction::Unregister { keep_files, .. } = action.as_ref() else {
            panic!("expected unregister action");
        };
        assert!(keep_files);
    }

    #[test]
    fn reorder_builds_a_permutation_of_attachment_ids() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('4')]);
        keys(&mut app, &[Key::Right]); // focus detail
        assert_eq!(app.installations.cursor, Some(0));
        keys(&mut app, &[Key::Char(']')]); // move first attachment down
        let Some((PendingAction::ReorderProfiles { order, .. }, _)) = app.planning.clone() else {
            panic!("expected reorder planning");
        };
        assert_eq!(order, vec![testkit::rust_id(), testkit::dev_id()]);
    }

    #[test]
    fn membership_popup_opens_from_a_skill_row() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('4')]);
        keys(&mut app, &[Key::Right]); // focus detail: cursor on profile row 0
        keys(&mut app, &[Key::Down, Key::Down]); // onto effective skills
        assert_eq!(app.installations.cursor, Some(2));
        let effects = keys(&mut app, &[Key::Enter]);
        assert!(matches!(
            &effects[0],
            Effect::Membership { skill, .. } if skill == "git"
        ));
        // The runtime resolves and returns the §100 view:
        reduce(
            &mut app,
            Event::MembershipLoaded(Ok(Box::new(crate::app::MembershipView {
                installation: testkit::installation_id(),
                skill: "git".to_owned(),
                required_by: vec![(testkit::dev_id(), "dev-core".to_owned())],
                last_required_by: vec![],
                source_ref: "main".to_owned(),
                library_commit: Some("def456".to_owned()),
                skill_commit: Some("abc123".to_owned()),
                state: Some(beskar_core::drift::DriftState::Current),
                membership_drift: None,
            }))),
        );
        take_effects();
        assert!(matches!(dialog_of(&app), Dialog::Membership(_)));
        keys(&mut app, &[Key::Enter]);
        assert!(app.dialog.is_none(), "any key dismisses the popup");
    }

    // ---- library editing ------------------------------------------------------

    #[test]
    fn ingest_input_builds_an_ingest_plan() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('2')]);
        keys(&mut app, &[Key::Char('i')]);
        for c in "/tmp/my-skill".chars() {
            keys(&mut app, &[Key::Char(c)]);
        }
        keys(&mut app, &[Key::Tab]);
        for c in "engineering".chars() {
            keys(&mut app, &[Key::Char(c)]);
        }
        keys(&mut app, &[Key::Enter]);
        let Some((PendingAction::Ingest { source, bucket }, _)) = app.planning.clone() else {
            panic!("expected ingest planning");
        };
        assert_eq!(source, std::path::PathBuf::from("/tmp/my-skill"));
        assert_eq!(bucket, "engineering");
    }

    #[test]
    fn empty_ingest_input_is_rejected_with_a_toast() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('2')]);
        // Two fields: Enter moves to the second, second Enter submits.
        keys(&mut app, &[Key::Char('i'), Key::Enter, Key::Enter]);
        assert!(app.planning.is_none());
        assert!(
            app.toast
                .as_deref()
                .is_some_and(|t| t.contains("source path"))
        );
    }

    #[test]
    fn skill_remove_offers_cascade_through_the_plan_pipeline() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('2')]);
        keys(&mut app, &[Key::Right]); // focus list
        keys(&mut app, &[Key::Char('x')]);
        let Dialog::Pick(pick) = dialog_of(&app) else {
            panic!("expected removal picker");
        };
        assert_eq!(pick.options.len(), 2);
        // Choose cascade: the removal is PLANNED first (§75 preview), never
        // executed straight from the menu.
        let effects = keys(&mut app, &[Key::Down, Key::Enter]);
        let Effect::Plan { action, .. } = &effects[0] else {
            panic!("expected plan effect");
        };
        let PendingAction::SkillRemove { cascade, .. } = action.as_ref() else {
            panic!("expected skill remove action");
        };
        assert!(cascade);
        // The real scoped-commit plan then gets its own confirm dialog.
        reduce(
            &mut app,
            Event::Planned(Ok(Box::new(PlannedChange::Library {
                title: "beskar: remove skill testing".to_owned(),
                plan: crate::testkit::removal_plan(),
                notes: vec!["referencing profiles: dev-core".to_owned()],
            }))),
        );
        take_effects();
        let Dialog::Confirm(confirm) = dialog_of(&app) else {
            panic!("expected confirm dialog");
        };
        assert!(confirm.summary.iter().any(|line| line.contains("dev-core")));
        let effects = keys(&mut app, &[Key::Enter]);
        let Effect::Execute { action, .. } = &effects[0] else {
            panic!("expected execute effect");
        };
        let PendingAction::SkillRemove { cascade, .. } = action.as_ref() else {
            panic!("expected skill remove action");
        };
        assert!(cascade);
    }

    #[test]
    fn rank_input_rejects_non_integers_and_accepts_clear() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('2'), Key::Right]);
        keys(&mut app, &[Key::Char('e')]);
        for c in "not-a-number".chars() {
            keys(&mut app, &[Key::Char(c)]);
        }
        keys(&mut app, &[Key::Enter]);
        assert!(app.planning.is_none());
        assert!(app.toast.as_deref().is_some_and(|t| t.contains("integer")));
        // Again, leaving the field empty clears the rank.
        keys(&mut app, &[Key::Char('e'), Key::Enter]);
        let Some((PendingAction::SkillRank { rank, .. }, _)) = app.planning.clone() else {
            panic!("expected rank planning");
        };
        assert_eq!(rank, None);
    }

    #[test]
    fn profile_reorder_uses_neighbor_pivots() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('3')]);
        keys(&mut app, &[Key::Right]); // focus detail
        // dev-core's ordered skills: git, testing — move to the second one.
        keys(&mut app, &[Key::Down]);
        keys(&mut app, &[Key::Char('[')]); // move "testing" before "git"
        let Some((PendingAction::ProfileMoveSkill { skill, pivot, .. }, _)) = app.planning.clone()
        else {
            panic!("expected profile move planning");
        };
        assert_eq!(skill, "testing");
        assert_eq!(
            pivot,
            beskar_core::editing::SkillPivot::Before("git".to_owned())
        );
    }

    // ---- errors and activity ---------------------------------------------------

    #[test]
    fn execution_failure_logs_and_opens_the_error_dialog() {
        let mut app = app_with_snapshot();
        let err = beskar_core::Error::Lock("registry held by pid 42".to_owned());
        let effects = reduce(&mut app, Event::Executed(Err(err)));
        take_effects();
        let Dialog::Message(message) = dialog_of(&app) else {
            panic!("expected message dialog");
        };
        assert!(message.failed);
        assert!(message.lines[0].starts_with("error (locked):"));
        assert_eq!(app.activity.entries.len(), 1);
        assert!(app.activity.entries[0].failed);
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::Refresh)),
            "a failed execution still refreshes read-only state"
        );
    }

    #[test]
    fn validation_results_open_a_read_only_report() {
        let mut app = app_with_snapshot();
        reduce(
            &mut app,
            Event::Validated(Ok(vec![beskar_core::editing::ProfileValidation {
                profile: "dev-core".to_owned(),
                id: Some(testkit::dev_id()),
                valid: true,
                problems: vec![],
                missing_skills: vec!["ghost".to_owned()],
            }])),
        );
        take_effects();
        let Dialog::Message(message) = dialog_of(&app) else {
            panic!("expected message dialog");
        };
        assert!(!message.failed);
        assert!(message.lines.iter().any(|line| line.contains("dev-core")));
        assert!(message.lines.iter().any(|line| line.contains("ghost")));
    }

    #[test]
    fn dialogs_capture_keys_so_screens_do_not_scroll_underneath() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('4'), Key::Char('a')]); // pick dialog
        let before = app.installations.installation;
        keys(&mut app, &[Key::Char('j')]); // movement stays in the dialog
        assert!(matches!(dialog_of(&app), Dialog::Pick(_)));
        let Dialog::Pick(pick) = dialog_of(&app) else {
            unreachable!()
        };
        assert_eq!(pick.selected, 1, "j moves the dialog selection");
        keys(&mut app, &[Key::Esc]);
        assert!(app.dialog.is_none());
        assert_eq!(app.installations.installation, before);
    }

    #[test]
    fn planning_state_survives_until_the_plan_arrives() {
        let mut app = app_with_snapshot();
        keys(&mut app, &[Key::Char('4'), Key::Char('u')]);
        assert!(app.planning.is_some());
        // Cancel the confirm dialog: planning was already cleared on arrival.
        reduce(
            &mut app,
            Event::Planned(Ok(Box::new(plan(action_plan(
                PlanAction::UpdateSkill,
                "git",
            ))))),
        );
        take_effects();
        assert!(app.planning.is_none());
        keys(&mut app, &[Key::Esc]);
        assert!(app.dialog.is_none());
    }

    #[test]
    fn confirm_apply_variant_roundtrips_through_the_choice() {
        let apply = ConfirmApply::Run {
            action: Box::new(PendingAction::UpdateAll),
            options: ReconcileOptions::default(),
        };
        assert!(matches!(apply, ConfirmApply::Run { .. }));
    }

    #[test]
    fn message_dialog_is_dismissed_by_any_interpreted_key() {
        let mut app = crate::app::App::new();
        app.dialog = Some(Dialog::Message(MessageDialog {
            title: "hello".to_owned(),
            lines: vec!["world".to_owned()],
            failed: false,
        }));
        keys(&mut app, &[Key::Char('q')]);
        assert!(
            app.dialog.is_none(),
            "q closes the dialog instead of quitting"
        );
        assert!(!app.quit);
    }
}

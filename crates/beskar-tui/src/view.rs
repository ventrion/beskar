//! Rendering (spec §95-§101, §112).
//!
//! Draws the [`App`] onto a ratatui [`Frame`]. Pure with respect to app
//! state: no I/O, no input handling. The line-building helpers
//! ([`plan_lines`], [`fetch_lines`], [`push_lines`]) are also reused by the
//! reducer and service to build confirm dialogs and activity entries.
//!
//! State glyphs mirror the stable §130 drift identifiers; the UI classifies
//! nothing itself — it renders typed core data (§115).

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};

use beskar_core::drift::DriftState;
use beskar_core::plan::{Blocker, BlockerKind, PlanAction, ReconciliationPlan};
use beskar_core::remote::{BranchSyncState, FetchOutcome, PushOutcome, PushState};

use crate::app::{App, Dialog, PlannedChange, Screen, SkillsFocus};

// ---- shared line builders (also used by reduce.rs / service.rs) -------------

/// Current UTC wall-clock time as `HH:MM:SS` for Activity entries.
pub fn timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!("{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second())
}

/// Short display form of a commit hash.
fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

/// The stable §130-style identifier of a fetch/push state.
fn sync_state_id(state: BranchSyncState) -> &'static str {
    match state {
        BranchSyncState::Current => "current",
        BranchSyncState::FastForwarded => "fast_forwarded",
        BranchSyncState::Planned => "planned",
        BranchSyncState::Ahead => "ahead",
        BranchSyncState::Diverged => "diverged",
        BranchSyncState::DirtyCheckedOut => "dirty_checked_out",
        BranchSyncState::Unpublished => "unpublished",
        BranchSyncState::Unknown => "unknown",
    }
}

/// Summarizes a reconciliation plan as plain lines (§89, §91 output shape).
pub fn plan_lines(plan: &ReconciliationPlan) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(commit) = &plan.resolved_commit {
        lines.push(format!("resolved library commit: {}", short(commit)));
    }
    for change in &plan.profile_changes {
        let verb = match change.action {
            PlanAction::AttachProfile => "attach",
            PlanAction::DetachProfile => "detach",
            _ => "change",
        };
        lines.push(format!("  {verb} profile {}", change.profile_name));
    }
    for action in &plan.skill_actions {
        let name = action.skill.as_str();
        match action.action {
            PlanAction::InstallSkill => lines.push(format!("  install {name}")),
            PlanAction::UpdateSkill => lines.push(format!("  update {name}")),
            PlanAction::RetireSkill => lines.push(format!("  retire {name}")),
            PlanAction::ChangeSkillMembership => {
                lines.push(format!("  membership change: {name}"));
            }
            PlanAction::OverwriteModified => lines.push(format!(
                "  overwrite modified files of {name}: {}",
                action.paths.join(", ")
            )),
            PlanAction::ReplaceUnmanaged => {
                lines.push(format!("  replace unmanaged directory {name}"));
            }
            PlanAction::PreserveExtra => lines.push(format!(
                "  preserve extra files in {name}: {}",
                action.paths.join(", ")
            )),
            other => lines.push(format!("  {}: {name}", action_label(other))),
        }
    }
    for action in &plan.state_actions {
        lines.push(format!("  {}", action_label(*action)));
    }
    for blocker in &plan.blockers {
        let mut line = format!("  BLOCKED {}", blocker_label(blocker));
        if let Some(skill) = &blocker.skill {
            line.push_str(&format!(" [{skill}]"));
        }
        if !blocker.paths.is_empty() {
            line.push_str(&format!(": {}", blocker.paths.join(", ")));
        }
        lines.push(line);
    }
    if plan.is_no_op() {
        lines.push("  nothing to do — already up to date".to_owned());
    }
    lines
}

fn action_label(action: PlanAction) -> &'static str {
    match action {
        PlanAction::AttachProfile => "attach profile",
        PlanAction::DetachProfile => "detach profile",
        PlanAction::InstallSkill => "install",
        PlanAction::UpdateSkill => "update",
        PlanAction::ChangeSkillMembership => "membership change",
        PlanAction::RetireSkill => "retire",
        PlanAction::PreserveExtra => "preserve extra",
        PlanAction::OverwriteModified => "overwrite modified",
        PlanAction::ReplaceUnmanaged => "replace unmanaged",
        PlanAction::UpdateRegistry => "update registry",
        PlanAction::CommitLibraryPaths => "commit library paths",
        PlanAction::FastForwardBranch => "fast-forward branch",
        PlanAction::PushBranch => "push branch",
    }
}

fn blocker_label(blocker: &Blocker) -> &'static str {
    match blocker.kind {
        BlockerKind::ModifiedContent => "modified content (§47)",
        BlockerKind::UnmanagedCollision => "unmanaged collision (§49)",
        BlockerKind::ForeignStamp => "foreign stamp",
        BlockerKind::LibraryMismatch => "library mismatch",
        BlockerKind::MissingRef => "missing ref",
        BlockerKind::MissingProfile => "missing profile (§39)",
        BlockerKind::MissingWorkspace => "missing workspace",
        BlockerKind::MissingSkill => "missing skill (§58)",
        BlockerKind::InvalidSkill => "invalid skill",
        BlockerKind::Diverged => "diverged (§63)",
    }
}

/// Summarizes a fetch outcome/plan as plain lines (§62, §91).
pub fn fetch_lines(outcome: &FetchOutcome) -> Vec<String> {
    let mut lines = vec![format!(
        "remote {} ({}){}",
        outcome.remote,
        outcome.remote_url,
        if outcome.dry_run {
            " — dry run, no network"
        } else {
            ""
        }
    )];
    for branch in &outcome.branches {
        let mut line = format!("  {}: {}", branch.branch, sync_state_id(branch.state));
        if let (Some(ahead), Some(behind)) = (branch.ahead, branch.behind) {
            line.push_str(&format!(" (ahead {ahead}, behind {behind})"));
        } else if let Some(behind) = branch.behind {
            line.push_str(&format!(" (behind {behind})"));
        } else if let Some(ahead) = branch.ahead {
            line.push_str(&format!(" (ahead {ahead})"));
        }
        if let Some(note) = &branch.note {
            line.push_str(&format!(" — {note}"));
        }
        lines.push(line);
    }
    for action in &outcome.plan.actions {
        lines.push(format!(
            "  {} {} → {}",
            action_label(action.action),
            action.branch,
            short(action.to.as_deref().unwrap_or("?"))
        ));
    }
    lines
}

/// Summarizes a push outcome/plan as plain lines (§65, §91).
pub fn push_lines(outcome: &PushOutcome) -> Vec<String> {
    let state = match outcome.state {
        PushState::Current => "current",
        PushState::Pushed => "pushed",
        PushState::Planned => "planned",
        PushState::Unverified => "unverified",
    };
    let mut lines = vec![format!(
        "{} → {} ({}): {}{}",
        outcome.branch,
        outcome.remote,
        outcome.remote_url,
        state,
        if outcome.dry_run {
            " — dry run, no network"
        } else {
            ""
        }
    )];
    if let Some(ahead) = outcome.ahead {
        lines.push(format!("  {ahead} local commit(s) not on the remote"));
    }
    if outcome.created_remote_branch {
        lines.push("  the remote branch does not exist yet and will be created".to_owned());
    }
    match (
        outcome.upstream_before.as_deref(),
        outcome.upstream_after.as_deref(),
    ) {
        (before, Some(after)) if before != Some(after) => {
            lines.push(format!("  upstream will be set to {after}"));
        }
        (Some(before), _) => lines.push(format!("  upstream: {before}")),
        (None, Some(after)) => lines.push(format!("  upstream: {after}")),
        (None, None) => {}
    }
    lines
}

// ---- styles -----------------------------------------------------------------

fn state_color(state: DriftState) -> Color {
    match state {
        DriftState::Current => Color::Green,
        DriftState::Outdated
        | DriftState::Extra
        | DriftState::ProfileAdded
        | DriftState::ProfileRemoved
        | DriftState::MembershipChanged => Color::Yellow,
        DriftState::Modified
        | DriftState::Foreign
        | DriftState::OrphanedManaged
        | DriftState::MissingTarget
        | DriftState::MissingWorkspace
        | DriftState::MissingRef => Color::Red,
        DriftState::Gap | DriftState::Unstamped | DriftState::MissingProfile => Color::Cyan,
    }
}

/// A one-character state glyph for list rows (§42-style markers).
pub fn state_glyph(state: DriftState) -> &'static str {
    match state {
        DriftState::Current => "✓",
        DriftState::Outdated => "↑",
        DriftState::Modified => "!",
        DriftState::Extra => "+",
        DriftState::Gap | DriftState::Unstamped => "?",
        DriftState::Foreign | DriftState::OrphanedManaged => "×",
        DriftState::ProfileAdded => "+",
        DriftState::ProfileRemoved => "−",
        DriftState::MembershipChanged => "~",
        DriftState::MissingTarget | DriftState::MissingWorkspace | DriftState::MissingRef => "!",
        DriftState::MissingProfile => "?",
    }
}

fn selected_style() -> Style {
    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn accent() -> Style {
    Style::default().fg(Color::Cyan)
}

fn block(title: impl Into<String>) -> Block<'static> {
    Block::default().borders(Borders::ALL).title(Span::styled(
        title.into(),
        Style::default().add_modifier(Modifier::BOLD),
    ))
}

fn lines_of(lines: Vec<String>) -> Vec<Line<'static>> {
    lines.into_iter().map(Line::from).collect()
}

// ---- top-level render ---------------------------------------------------------

/// Draws the whole application frame.
pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);
    let (header, body, footer) = (layout[0], layout[1], layout[2]);

    render_header(app, frame, header);
    match app.screen {
        Screen::Dashboard => render_dashboard(app, frame, body),
        Screen::Skills => render_skills(app, frame, body),
        Screen::Profiles => render_profiles(app, frame, body),
        Screen::Installations => render_installations(app, frame, body),
        Screen::Git => render_git(app, frame, body),
        Screen::Activity => render_activity(app, frame, body),
    }
    render_footer(app, frame, footer);
    render_dialog(app, frame);
}

fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Tabs::new(Screen::ALL.iter().map(|screen| screen.title()))
            .select(
                Screen::ALL
                    .iter()
                    .position(|screen| *screen == app.screen)
                    .unwrap_or(0),
            )
            .highlight_style(selected_style()),
        area,
    );
    if let Some(snapshot) = &app.snapshot {
        let label = format!(
            "{} ",
            snapshot
                .library
                .path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default()
        );
        let width = label.len() as u16;
        if width + 20 < area.width {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(label, dim()))),
                Rect {
                    x: area.width - width,
                    y: area.y,
                    width,
                    height: 1,
                },
            );
        }
    }
}

fn render_footer(app: &App, frame: &mut Frame, area: Rect) {
    let hints = match app.screen {
        Screen::Dashboard => "1-6/Tab screens · r refresh · u update all · q quit",
        Screen::Skills => {
            "/ search · h/l panes · j/k move · i ingest · m move · N rename · x remove · t tag · e rank · p add-to-profile · r refresh · q quit"
        }
        Screen::Profiles => {
            "h/l panes · j/k move · c create · e rename · d delete · a add skill · x remove skill · [ ] reorder · v validate · q quit"
        }
        Screen::Installations => {
            "h/l panes · j/k move · a attach · d detach · u update · R ref · U unregister · [ ] order · Enter membership · q quit"
        }
        Screen::Git => {
            "j/k select · f fetch · p push · s switch · c create branch · r refresh · q quit"
        }
        Screen::Activity => "j/k scroll · q quit",
    };
    let mut spans = vec![Span::styled(format!(" {hints} "), dim())];
    if let Some(busy) = &app.busy {
        spans.push(Span::styled(format!(" {busy}"), accent()));
    }
    if let Some(toast) = &app.toast {
        spans.push(Span::styled(format!("  {toast}"), Color::Yellow));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---- dashboard (§96) ---------------------------------------------------------

fn render_dashboard(app: &App, frame: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    match &app.snapshot {
        None => {
            lines.push(Line::from(Span::styled(
                match &app.load_error {
                    Some(error) => error.clone(),
                    None => "loading…".to_owned(),
                },
                Color::Red,
            )));
        }
        Some(snapshot) => {
            let metrics = snapshot.dashboard();
            let library = &snapshot.library;
            lines.push(Line::from(vec![
                Span::styled("Library  ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(library.path.display().to_string()),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Branch   ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(
                    library
                        .branch
                        .clone()
                        .unwrap_or_else(|| "(detached HEAD)".to_owned()),
                ),
                Span::styled(
                    if library.dirty {
                        "  (uncommitted changes)"
                    } else {
                        ""
                    },
                    Color::Yellow,
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("HEAD     ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(
                    library
                        .head
                        .as_deref()
                        .map(short)
                        .unwrap_or("(none)")
                        .to_owned(),
                ),
                Span::styled(
                    match (library.ahead, library.behind) {
                        (Some(ahead), Some(behind)) => format!("  ahead {ahead}, behind {behind}"),
                        (Some(ahead), None) => format!("  ahead {ahead}"),
                        (None, Some(behind)) => format!("  behind {behind}"),
                        (None, None) => String::new(),
                    },
                    accent(),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Remote   ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(
                    library
                        .remote
                        .clone()
                        .unwrap_or_else(|| "(none)".to_owned()),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Ref      ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!("default_ref = {}", library.default_ref)),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Overview",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(metric_line("Skills", metrics.skills));
            lines.push(metric_line("Profiles", metrics.profiles));
            lines.push(metric_line("Installations", metrics.installations));
            lines.push(metric_line("Profile attachments", metrics.attachments));
            lines.push(metric_line("Outdated installations", metrics.outdated));
            lines.push(metric_line("Modified installations", metrics.modified));
            lines.push(metric_line("Broken installations", metrics.broken));
            if metrics.missing_profiles > 0 {
                lines.push(Line::from(Span::styled(
                    format!(
                        "  {} missing attached profile(s) — protected until explicitly detached (§39)",
                        metrics.missing_profiles
                    ),
                    Color::Cyan,
                )));
            }
            lines.push(Line::from(""));
            if snapshot.installations.is_empty() {
                lines.push(Line::from(Span::styled(
                    "No registered installations yet — press 3 (Installations) and 'a' to attach a profile to a workspace.",
                    dim(),
                )));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(block("Dashboard"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn metric_line(label: &str, value: usize) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!("  {label}: ")),
        Span::styled(
            value.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ])
}

// ---- skills (§97) -----------------------------------------------------------

fn render_skills(app: &App, frame: &mut Frame, area: Rect) {
    let layout = Layout::horizontal([
        Constraint::Percentage(24),
        Constraint::Percentage(34),
        Constraint::Percentage(42),
    ])
    .split(area);
    let (buckets_area, list_area, preview_area) = (layout[0], layout[1], layout[2]);

    // Pane 1: bucket tree.
    let mut bucket_items: Vec<ListItem> =
        vec![ListItem::new(Line::from(if app.skills.bucket.is_none() {
            Span::styled(
                "(all buckets)",
                Style::default().add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("(all buckets)")
        }))];
    if let Some(snapshot) = &app.snapshot {
        for (bucket, depth) in snapshot.buckets() {
            let leaf = bucket.rsplit('/').next().unwrap_or(&bucket);
            let indent = "  ".repeat(depth.saturating_sub(1));
            let selected = app.skills.bucket.as_deref() == Some(bucket.as_str());
            let span = if selected {
                Span::styled(
                    format!("{indent}{leaf}"),
                    Style::default().add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw(format!("{indent}{leaf}"))
            };
            bucket_items.push(ListItem::new(Line::from(span)));
        }
    }
    let rows = bucket_items.len();
    let bucket_selected = match app.skills.focus {
        SkillsFocus::Buckets => {
            let index = app
                .skills
                .bucket
                .as_ref()
                .map(|bucket| {
                    1 + app
                        .snapshot
                        .as_ref()
                        .map(|snapshot| {
                            snapshot
                                .buckets()
                                .iter()
                                .position(|(candidate, _)| candidate == bucket)
                                .unwrap_or(0)
                        })
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            ListState::default().with_selected(Some(index.min(rows.saturating_sub(1))))
        }
        _ => ListState::default(),
    };
    frame.render_stateful_widget(
        List::new(bucket_items)
            .block(block("Buckets"))
            .highlight_style(selected_style()),
        buckets_area,
        &mut bucket_selected.clone(),
    );

    // Pane 2: searchable skill list.
    let filtered = app.filtered_skills();
    let items: Vec<ListItem> = filtered
        .iter()
        .map(|skill| {
            let mut spans = vec![Span::raw(skill.name.to_string())];
            if !skill.tags.is_empty() {
                spans.push(Span::styled(
                    format!("  [{}]", skill.tags.join(", ")),
                    dim(),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let selected_index = filtered
        .iter()
        .position(|skill| Some(skill.name.as_str()) == app.skills.skill.as_deref());
    let mut list_state = ListState::default().with_selected(selected_index);
    let focused = app.skills.focus == SkillsFocus::List;
    let list_title = if app.skills.query.is_empty() {
        if app.skills.querying {
            "Skills — search: (type, Enter to apply)".to_owned()
        } else {
            "Skills — / to search".to_owned()
        }
    } else if app.skills.querying {
        format!("Skills — search: {}▏", app.skills.query)
    } else {
        format!("Skills — search: {}", app.skills.query)
    };
    frame.render_stateful_widget(
        List::new(items)
            .block(block(list_title))
            .highlight_style(if focused {
                selected_style()
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            }),
        list_area,
        &mut list_state,
    );

    // Pane 3: metadata + SKILL.md preview.
    let mut lines: Vec<Line> = Vec::new();
    match &app.skills.preview {
        None => lines.push(Line::from(Span::styled("select a skill", dim()))),
        Some(preview) => {
            let listing = &preview.detail.listing;
            lines.push(Line::from(Span::styled(
                listing.name.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(vec![
                Span::styled("bucket   ", dim()),
                Span::raw(if listing.bucket.is_empty() {
                    "(top level)".to_owned()
                } else {
                    listing.bucket.clone()
                }),
            ]));
            lines.push(Line::from(Span::raw(listing.description.clone())));
            if !listing.tags.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("tags     ", dim()),
                    Span::raw(listing.tags.join(", ")),
                ]));
            }
            if let Some(rank) = listing.rank {
                lines.push(Line::from(vec![
                    Span::styled("rank     ", dim()),
                    Span::raw(rank.to_string()),
                ]));
            }
            if !listing.profiles.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("profiles ", dim()),
                    Span::raw(listing.profiles.join(", ")),
                ]));
            }
            if let Some(commit) = &listing.last_commit {
                lines.push(Line::from(vec![
                    Span::styled("commit   ", dim()),
                    Span::raw(format!("{} ({})", short(&commit.hash), commit.unix_time)),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled("files    ", dim()),
                Span::raw(preview.detail.files.join(", ")),
            ]));
            lines.push(Line::from(""));
            for text_line in preview.skill_md.lines() {
                lines.push(Line::from(text_line.to_owned()));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(block("Preview"))
            .wrap(Wrap { trim: false })
            .scroll((app.skills.preview_scroll, 0)),
        preview_area,
    );
}

// ---- profiles (§98) -----------------------------------------------------------

fn render_profiles(app: &App, frame: &mut Frame, area: Rect) {
    use crate::app::ProfilesFocus;
    let layout =
        Layout::horizontal([Constraint::Percentage(34), Constraint::Percentage(66)]).split(area);
    let (list_area, detail_area) = (layout[0], layout[1]);

    let Some(snapshot) = &app.snapshot else {
        frame.render_widget(Paragraph::new("loading…").block(block("Profiles")), area);
        return;
    };

    let items: Vec<ListItem> = snapshot
        .profiles
        .iter()
        .map(|profile| {
            ListItem::new(Line::from(vec![
                Span::raw(profile.name.clone()),
                Span::styled(format!("  {} skills", profile.skills.len()), dim()),
            ]))
        })
        .collect();
    let selected_index = snapshot
        .profiles
        .iter()
        .position(|profile| Some(profile.name.as_str()) == app.profiles.profile.as_deref());
    let mut list_state = ListState::default().with_selected(selected_index);
    let focused = app.profiles.focus == ProfilesFocus::List;
    frame.render_stateful_widget(
        List::new(items)
            .block(block("Profiles"))
            .highlight_style(if focused {
                selected_style()
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            }),
        list_area,
        &mut list_state,
    );

    let mut lines: Vec<Line> = Vec::new();
    let profile = app
        .profiles
        .profile
        .as_deref()
        .and_then(|name| snapshot.profile(name));
    match profile {
        None => lines.push(Line::from(Span::styled("no profile selected", dim()))),
        Some(profile) => {
            lines.push(Line::from(Span::styled(
                profile.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            if let Some(description) = &profile.description {
                lines.push(Line::from(Span::raw(description.clone())));
            }
            lines.push(Line::from(vec![
                Span::styled("id       ", dim()),
                Span::raw(profile.id.to_string()),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Skills (ordered, §16)",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            if profile.skills.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  (empty — press 'a' to add skills)",
                    dim(),
                )));
            }
            for (index, skill) in profile.skills.iter().enumerate() {
                let marker = if focused && index == app.profiles.skill_index {
                    "▸ "
                } else {
                    "  "
                };
                lines.push(Line::from(format!("{marker}{index}. {skill}")));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Attached by (local installations)",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            let attaching = app.attaching_installations(&profile.id);
            if attaching.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  (no local installations attach this profile)",
                    dim(),
                )));
            }
            for (workspace, target) in attaching {
                lines.push(Line::from(format!("  {target} — {}", workspace.display())));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(block("Profile detail"))
            .wrap(Wrap { trim: false }),
        detail_area,
    );
}

// ---- installations (§99, §100) --------------------------------------------------

fn render_installations(app: &App, frame: &mut Frame, area: Rect) {
    use crate::app::InstallationsFocus;
    let layout =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(area);
    let (list_area, detail_area) = (layout[0], layout[1]);

    let Some(snapshot) = &app.snapshot else {
        frame.render_widget(
            Paragraph::new("loading…").block(block("Installations")),
            area,
        );
        return;
    };

    let items: Vec<ListItem> = snapshot
        .installations
        .iter()
        .map(|row| {
            let mut spans = vec![Span::raw(format!(
                "{} — {}",
                row.installation.target,
                row.installation.workspace.display()
            ))];
            if let Some(status) = &row.status {
                if let Some(state) = status.installation_state {
                    spans.push(Span::styled(
                        format!("  {} {}", state_glyph(state), state.id()),
                        state_color(state),
                    ));
                }
                let outdated = status.count(DriftState::Outdated);
                let modified = status.count(DriftState::Modified);
                if outdated > 0 {
                    spans.push(Span::styled(format!("  ↑{outdated}"), Color::Yellow));
                }
                if modified > 0 {
                    spans.push(Span::styled(format!("  !{modified}"), Color::Red));
                }
            } else if row.error.is_some() {
                spans.push(Span::styled("  broken", Color::Red));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let selected_index = snapshot
        .installations
        .iter()
        .position(|row| Some(row.installation.id) == app.installations.installation);
    let mut list_state = ListState::default().with_selected(selected_index);
    let focused = app.installations.focus == InstallationsFocus::List;
    frame.render_stateful_widget(
        List::new(items)
            .block(block("Installations"))
            .highlight_style(if focused {
                selected_style()
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            }),
        list_area,
        &mut list_state,
    );

    let mut lines: Vec<Line> = Vec::new();
    let row = app
        .installations
        .installation
        .and_then(|id| snapshot.installation(id));
    match row {
        None => {
            lines.push(Line::from(Span::styled(
                "no installation selected — press 'a' to attach a profile (§21)",
                dim(),
            )));
        }
        Some(row) => {
            let installation = &row.installation;
            lines.push(Line::from(vec![
                Span::styled("Workspace  ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(installation.workspace.display().to_string()),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Target     ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!(
                    "{} (adapter {})",
                    installation.target,
                    adapter_label(installation.adapter)
                )),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Source ref ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(installation.source_ref.clone()),
            ]));
            if let Some(status) = &row.status {
                if let Some(commit) = &status.resolved_commit {
                    lines.push(Line::from(vec![
                        Span::styled("Commit     ", dim()),
                        Span::raw(short(commit)),
                    ]));
                }
            } else if let Some(error) = &row.error {
                lines.push(Line::from(Span::styled(
                    format!("status unavailable: {error}"),
                    Color::Red,
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Attached Profiles (attachment order, §17)",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            if installation.profiles.is_empty() {
                lines.push(Line::from(Span::styled("  (none)", dim())));
            }
            for (index, attachment) in installation.profiles.iter().enumerate() {
                let marker = if !focused
                    && app.installations.focus == InstallationsFocus::Detail
                    && app.installations.cursor.unwrap_or(0) == index
                {
                    "▸ "
                } else {
                    "  "
                };
                let mut spans = vec![Span::raw(format!("{marker}{}", attachment.name))];
                if let Some(status) = &row.status {
                    let profile_status = status
                        .profiles
                        .iter()
                        .find(|p| p.attachment.id == attachment.id);
                    if let Some(profile_status) = profile_status
                        && profile_status.profile.is_none()
                    {
                        spans.push(Span::styled("  ? missing (protected, §39)", Color::Cyan));
                    }
                }
                lines.push(Line::from(spans));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Effective Skills",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            match &row.status {
                None => lines.push(Line::from(Span::styled("  (status unavailable)", dim()))),
                Some(status) => {
                    if status.skills.is_empty() {
                        lines.push(Line::from(Span::styled("  (none)", dim())));
                    }
                    let profile_count = installation.profiles.len();
                    for (index, (name, skill)) in status.skills.iter().enumerate() {
                        let cursor_index = profile_count + index;
                        let marker = if app.installations.focus == InstallationsFocus::Detail
                            && app.installations.cursor.unwrap_or(0) == cursor_index
                        {
                            "▸ "
                        } else {
                            "  "
                        };
                        lines.push(Line::from(vec![
                            Span::raw(marker),
                            Span::styled(
                                format!("{} {name}", state_glyph(skill.state)),
                                state_color(skill.state),
                            ),
                            Span::styled(format!("  {}", required_by_label(status, skill)), dim()),
                        ]));
                    }
                    if !status.unmanaged.is_empty() {
                        lines.push(Line::from(""));
                        lines.push(Line::from(Span::styled(
                            format!("Unmanaged content: {}", status.unmanaged.join(", ")),
                            dim(),
                        )));
                    }
                }
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(block("Installation detail"))
            .wrap(Wrap { trim: false }),
        detail_area,
    );
}

fn adapter_label(adapter: beskar_core::registry::Adapter) -> &'static str {
    match adapter {
        beskar_core::registry::Adapter::Agents => "agents",
        beskar_core::registry::Adapter::Claude => "claude",
        beskar_core::registry::Adapter::Custom => "custom",
    }
}

fn required_by_label(
    status: &beskar_core::status::InstallationStatus,
    skill: &beskar_core::status::SkillStatus,
) -> String {
    let names: Vec<String> = skill
        .required_by
        .iter()
        .map(|id| {
            status
                .profiles
                .iter()
                .find(|profile| profile.attachment.id == *id)
                .map(|profile| profile.attachment.name.clone())
                .unwrap_or_else(|| id.to_string())
        })
        .collect();
    names.join(", ")
}

// ---- git (§101) -----------------------------------------------------------------

fn render_git(app: &App, frame: &mut Frame, area: Rect) {
    let layout = Layout::vertical([Constraint::Length(8), Constraint::Min(1)]).split(area);
    let (summary_area, branches_area) = (layout[0], layout[1]);

    let Some(snapshot) = &app.snapshot else {
        frame.render_widget(Paragraph::new("loading…").block(block("Git")), area);
        return;
    };
    let library = &snapshot.library;
    let summary = lines_of(vec![
        format!(
            "branch: {}{}",
            library
                .branch
                .clone()
                .unwrap_or_else(|| "(detached)".to_owned()),
            if library.dirty { "  (dirty)" } else { "" }
        ),
        format!(
            "head: {}",
            library.head.as_deref().map(short).unwrap_or("-")
        ),
        format!(
            "remote: {}",
            library
                .remote
                .clone()
                .unwrap_or_else(|| "(none)".to_owned())
        ),
        format!("default ref: {}", library.default_ref),
        format!(
            "upstream: {} (ahead {:?}, behind {:?})",
            library
                .upstream
                .clone()
                .unwrap_or_else(|| "(none)".to_owned()),
            library.ahead,
            library.behind
        ),
    ]);
    frame.render_widget(
        Paragraph::new(summary).block(block("Library")),
        summary_area,
    );

    let items: Vec<ListItem> = snapshot
        .branches
        .iter()
        .map(|branch| {
            let mut spans = vec![Span::raw(if branch.current { "* " } else { "  " })];
            if branch.current {
                spans.push(Span::styled(
                    branch.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::raw(branch.name.clone()));
            }
            if let Some(upstream) = &branch.upstream {
                spans.push(Span::styled(format!("  → {upstream}"), dim()));
            }
            if let (Some(ahead), Some(behind)) = (branch.ahead, branch.behind) {
                spans.push(Span::styled(format!("  ↑{ahead} ↓{behind}"), accent()));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let selected_index = snapshot
        .branches
        .iter()
        .position(|branch| Some(branch.name.as_str()) == app.git.branch.as_deref())
        .or(if snapshot.branches.is_empty() {
            None
        } else {
            Some(0)
        });
    let mut list_state = ListState::default().with_selected(selected_index);
    frame.render_stateful_widget(
        List::new(items)
            .block(block(
                "Branches — j/k select, f fetch, p push, s switch, c create",
            ))
            .highlight_style(selected_style()),
        branches_area,
        &mut list_state,
    );
}

// ---- activity ---------------------------------------------------------------------

fn render_activity(app: &App, frame: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if app.activity.entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "no activity yet — mutations appear here after they run",
            dim(),
        )));
    }
    for entry in app.activity.entries.iter().skip(app.activity.scroll) {
        let color = if entry.failed {
            Color::Red
        } else {
            Color::Green
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", entry.time), dim()),
            Span::styled(
                entry.title.clone(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]));
        for line in &entry.lines {
            lines.push(Line::from(Span::styled(format!("    {line}"), dim())));
        }
        lines.push(Line::from(""));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(block(format!(
                "Activity ({} entries)",
                app.activity.entries.len()
            )))
            .wrap(Wrap { trim: false }),
        area,
    );
}

// ---- dialogs -----------------------------------------------------------------------

fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let popup = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);
    let horizontal = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup[1]);
    horizontal[1]
}

fn render_dialog(app: &App, frame: &mut Frame) {
    let Some(dialog) = &app.dialog else {
        return;
    };
    match dialog {
        Dialog::Confirm(confirm) => {
            let area = centered(frame.area(), 72, 78);
            frame.render_widget(Clear, area);
            let choices_height = confirm.choices.len() as u16 + 1;
            let layout = Layout::vertical([Constraint::Min(1), Constraint::Length(choices_height)])
                .split(area);
            let mut lines = vec![Line::from(Span::styled(
                confirm.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ))];
            lines.push(Line::from(""));
            lines.extend(change_lines(&confirm.change).into_iter().map(|line| {
                if line.starts_with("  BLOCKED") {
                    Line::from(Span::styled(line, Color::Red))
                } else if line.starts_with("  overwrite") || line.starts_with("  replace") {
                    Line::from(Span::styled(line, Color::Yellow))
                } else {
                    Line::from(line)
                }
            }));
            for note in &confirm.summary {
                lines.push(Line::from(Span::styled(note.clone(), dim())));
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .block(block("Confirm"))
                    .wrap(Wrap { trim: false })
                    .scroll((confirm.scroll, 0)),
                layout[0],
            );
            let items: Vec<ListItem> = confirm
                .choices
                .iter()
                .enumerate()
                .map(|(index, choice)| {
                    let marker = if index == confirm.selected {
                        "▸ "
                    } else {
                        "  "
                    };
                    ListItem::new(Line::from(format!("{marker}{}", choice.label)))
                })
                .collect();
            let mut state = ListState::default().with_selected(Some(confirm.selected));
            frame.render_stateful_widget(
                List::new(items).highlight_style(selected_style()),
                layout[1],
                &mut state,
            );
        }
        Dialog::Input(input) => {
            let height = input.fields.len() as u16 * 2 + 3;
            let area = centered(
                frame.area(),
                64,
                (height * 100 / frame.area().height.max(1)).clamp(20, 60),
            );
            frame.render_widget(Clear, area);
            let mut lines: Vec<Line> = Vec::new();
            for (index, field) in input.fields.iter().enumerate() {
                let marker = if index == input.focused { "▸ " } else { "  " };
                let value = if index == input.focused {
                    format!("{}▏", field.buffer)
                } else {
                    field.buffer.clone()
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{marker}{}: ", field.label), accent()),
                    Span::raw(value),
                ]));
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                "Enter submit · Tab next field · Esc cancel",
                dim(),
            )));
            frame.render_widget(
                Paragraph::new(lines)
                    .block(block(&input.title))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        Dialog::Pick(pick) => {
            let area = centered(frame.area(), 60, 60);
            frame.render_widget(Clear, area);
            let items: Vec<ListItem> = pick
                .options
                .iter()
                .map(|option| ListItem::new(Line::from(option.clone())))
                .collect();
            let mut state = ListState::default().with_selected(Some(pick.selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(block(&pick.title))
                    .highlight_style(selected_style()),
                area,
                &mut state,
            );
        }
        Dialog::Message(message) => {
            let area = centered(frame.area(), 64, 50);
            frame.render_widget(Clear, area);
            let color = if message.failed {
                Color::Red
            } else {
                Color::Green
            };
            let mut lines = vec![Line::from(Span::styled(
                message.title.clone(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ))];
            lines.push(Line::from(""));
            lines.extend(lines_of(message.lines.clone()));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("any key closes", dim())));
            frame.render_widget(
                Paragraph::new(lines)
                    .block(Block::default().borders(Borders::ALL))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        Dialog::Membership(view) => {
            let area = centered(frame.area(), 60, 66);
            frame.render_widget(Clear, area);
            let mut lines = vec![Line::from(Span::styled(
                view.skill.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ))];
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Required by:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            if view.required_by.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  (not currently required)",
                    dim(),
                )));
            }
            for (_, name) in &view.required_by {
                lines.push(Line::from(format!("  {name}")));
            }
            if !view.last_required_by.is_empty() {
                let names: Vec<String> = view
                    .last_required_by
                    .iter()
                    .map(|(_, name)| name.clone())
                    .collect();
                lines.push(Line::from(Span::styled(
                    format!("previously required by: {}", names.join(", ")),
                    dim(),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Source:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(format!("  library/{}", view.source_ref)));
            lines.push(Line::from(format!(
                "  library commit {}",
                view.library_commit.as_deref().map(short).unwrap_or("?")
            )));
            lines.push(Line::from(format!(
                "  skill commit {}",
                view.skill_commit
                    .as_deref()
                    .map(short)
                    .unwrap_or("(unavailable)")
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "State:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            match view.state {
                Some(state) => {
                    lines.push(Line::from(Span::styled(
                        format!("  {} {}", state_glyph(state), state.id()),
                        state_color(state),
                    )));
                }
                None => lines.push(Line::from(Span::styled(
                    "  not part of this installation's membership",
                    dim(),
                ))),
            }
            if let Some(drift) = view.membership_drift {
                lines.push(Line::from(Span::styled(
                    format!("  membership: {}", drift_id(drift)),
                    Color::Yellow,
                )));
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .block(Block::default().borders(Borders::ALL).title(Span::styled(
                        "Skill membership (§100)",
                        Style::default().add_modifier(Modifier::BOLD),
                    )))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
    }
}

fn drift_id(drift: beskar_core::drift::MembershipDrift) -> &'static str {
    match drift {
        beskar_core::drift::MembershipDrift::Unchanged => "unchanged",
        beskar_core::drift::MembershipDrift::Added => "profile_added",
        beskar_core::drift::MembershipDrift::Removed => "profile_removed",
        beskar_core::drift::MembershipDrift::Changed => "membership_changed",
    }
}

/// The body lines of a planned change inside the confirm dialog.
fn change_lines(change: &PlannedChange) -> Vec<String> {
    match change {
        PlannedChange::Install { plan, .. } => plan_lines(plan),
        PlannedChange::Library { plan, .. } => {
            let mut lines = vec![format!("commit message: {}", plan.message)];
            for op in &plan.ops {
                let kind = match op.kind {
                    beskar_core::editing::LibraryOpKind::Write => "write",
                    beskar_core::editing::LibraryOpKind::Edit => "edit",
                    beskar_core::editing::LibraryOpKind::Replace => "replace",
                    beskar_core::editing::LibraryOpKind::Remove => "remove",
                    beskar_core::editing::LibraryOpKind::Move => "move",
                };
                match &op.from {
                    Some(from) => lines.push(format!("  {kind} {from} → {}", op.path)),
                    None => lines.push(format!("  {kind} {}", op.path)),
                }
            }
            lines
        }
        PlannedChange::Summary { lines, .. } => lines.clone(),
        PlannedChange::Fetch { outcome } => fetch_lines(outcome),
        PlannedChange::Push { outcome } => push_lines(outcome),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::event::Event;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Renders the app's current state into an in-memory backend and
    /// returns the screen contents (render smoke tests need no terminal).
    fn draw(app: &App) -> String {
        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal.draw(|frame| render(app, frame)).expect("draws");
        terminal.backend().to_string()
    }

    fn app_with_snapshot() -> App {
        let mut app = App::new();
        crate::reduce::reduce(
            &mut app,
            Event::Loaded(Ok(Box::new(crate::testkit::snapshot()))),
        );
        crate::reduce::take_effects();
        app
    }

    #[test]
    fn every_screen_renders_without_panicking() {
        let mut app = app_with_snapshot();
        let screens = [
            (
                Screen::Dashboard,
                vec!["Library", "Skills:", "Profiles:", "Installations:"],
            ),
            (Screen::Skills, vec!["Buckets", "code-review", "Preview"]),
            (
                Screen::Profiles,
                vec!["Profiles", "dev-core", "Attached by"],
            ),
            (
                Screen::Installations,
                vec!["Workspace", "Attached Profiles", "Effective Skills"],
            ),
            (Screen::Git, vec!["branch:", "Branches"]),
            (Screen::Activity, vec!["Activity"]),
        ];
        for (screen, expected) in screens {
            app.screen = screen;
            let drawn = draw(&app);
            for needle in expected {
                assert!(
                    drawn.contains(needle),
                    "{screen:?} should show {needle:?}:\n{drawn}"
                );
            }
        }
    }

    #[test]
    fn skill_state_glyphs_appear_in_the_installation_detail() {
        let mut app = app_with_snapshot();
        app.screen = Screen::Installations;
        let drawn = draw(&app);
        assert!(
            drawn.contains('✓'),
            "current skills show the check glyph:\n{drawn}"
        );
        assert!(
            drawn.contains("testing"),
            "effective skills are listed:\n{drawn}"
        );
    }

    #[test]
    fn dialogs_render_over_the_screen() {
        use crate::app::{ConfirmDialog, Dialog, MessageDialog, PlannedChange};
        let mut app = app_with_snapshot();
        app.dialog = Some(Dialog::Confirm(ConfirmDialog {
            title: "Test mutation".to_owned(),
            change: PlannedChange::Install {
                title: "Test mutation".to_owned(),
                plan: crate::testkit::plan_fixture(),
            },
            summary: vec!["context line".to_owned()],
            choices: vec![
                crate::app::ConfirmChoice {
                    label: "Apply".to_owned(),
                    apply: crate::app::ConfirmApply::Cancel,
                },
                crate::app::ConfirmChoice {
                    label: "Cancel".to_owned(),
                    apply: crate::app::ConfirmApply::Cancel,
                },
            ],
            selected: 0,
            scroll: 0,
        }));
        let drawn = draw(&app);
        assert!(drawn.contains("Test mutation"));
        assert!(drawn.contains("Apply"));
        assert!(drawn.contains("Cancel"));

        app.dialog = Some(Dialog::Message(MessageDialog {
            title: "oops".to_owned(),
            lines: vec!["error (locked): held".to_owned()],
            failed: true,
        }));
        let drawn = draw(&app);
        assert!(drawn.contains("oops"));
        assert!(drawn.contains("locked"));
    }

    #[test]
    fn the_membership_view_matches_the_100_shape() {
        use crate::app::Dialog;
        let mut app = app_with_snapshot();
        app.screen = Screen::Installations;
        app.dialog = Some(Dialog::Membership(Box::new(crate::app::MembershipView {
            installation: crate::testkit::installation_id(),
            skill: "testing".to_owned(),
            required_by: vec![
                (crate::testkit::dev_id(), "dev-core".to_owned()),
                (crate::testkit::rust_id(), "rust-development".to_owned()),
            ],
            last_required_by: vec![],
            source_ref: "main".to_owned(),
            library_commit: Some("def456789".to_owned()),
            skill_commit: Some("abc123456".to_owned()),
            state: Some(beskar_core::drift::DriftState::Current),
            membership_drift: None,
        })));
        let drawn = draw(&app);
        for needle in [
            "testing",
            "Required by:",
            "dev-core",
            "rust-development",
            "library/main",
            "State:",
            "current",
        ] {
            assert!(
                drawn.contains(needle),
                "membership popup should show {needle:?}:\n{drawn}"
            );
        }
    }

    #[test]
    fn loading_and_error_states_render() {
        let mut app = App::new();
        let drawn = draw(&app);
        assert!(
            drawn.contains("loading"),
            "pre-refresh shows a loading state:\n{drawn}"
        );
        app.load_error = Some("no Beskar Library found".to_owned());
        let drawn = draw(&app);
        assert!(drawn.contains("no Beskar Library found"));
    }

    #[test]
    fn timestamps_are_clock_shaped() {
        let stamp = timestamp();
        let parts: Vec<&str> = stamp.split(':').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts[0].len() == 2 && parts[1].len() == 2 && parts[2].len() == 2);
    }

    #[test]
    fn glyphs_cover_every_drift_state() {
        for state in [
            DriftState::Current,
            DriftState::Outdated,
            DriftState::Modified,
            DriftState::Extra,
            DriftState::Gap,
            DriftState::Unstamped,
            DriftState::Foreign,
            DriftState::ProfileAdded,
            DriftState::ProfileRemoved,
            DriftState::MembershipChanged,
            DriftState::OrphanedManaged,
            DriftState::MissingTarget,
            DriftState::MissingWorkspace,
            DriftState::MissingRef,
            DriftState::MissingProfile,
        ] {
            assert!(!state_glyph(state).is_empty());
        }
    }

    #[test]
    fn plan_lines_describe_actions_and_blockers() {
        let plan = ReconciliationPlan {
            installation_id: beskar_core::ids::InstallationId::generate(),
            resolved_commit: Some("abc1234567890".to_owned()),
            profile_changes: vec![],
            skill_actions: vec![beskar_core::plan::SkillAction {
                action: PlanAction::InstallSkill,
                skill: beskar_core::ids::SkillName::parse("testing").expect("valid"),
                resulting_state: DriftState::ProfileAdded,
                paths: vec![],
            }],
            state_actions: vec![],
            blockers: vec![Blocker {
                kind: BlockerKind::ModifiedContent,
                skill: Some(beskar_core::ids::SkillName::parse("testing").expect("valid")),
                profile: None,
                paths: vec!["testing/SKILL.md".to_owned()],
            }],
        };
        let lines = plan_lines(&plan);
        assert!(lines.iter().any(|line| line.contains("install testing")));
        assert!(lines.iter().any(|line| line.contains("abc1234")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("BLOCKED") && line.contains("testing/SKILL.md"))
        );
    }
}

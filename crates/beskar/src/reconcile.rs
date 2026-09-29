//! Reconciliation: compute the gap between desired and actual state, then
//! close it — without ever silently destroying local work.
//!
//! `plan` is a pure function over three inputs and is unit-tested
//! exhaustively; `apply` performs the plan against the filesystem and the
//! registry, resolving conflicts according to policy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::ConflictPolicy;
use crate::error::Result;
use crate::fingerprint;
use crate::library::Library;
use crate::registry::{InstalledSkill, RepoRecord};
use crate::ui::Ui;
use crate::util;

/// One planned change for one skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Wanted, not installed, nothing in the way: copy from library.
    Install { skill: String },
    /// On disk but unregistered, and byte-identical to the library:
    /// register it instead of recopying.
    Adopt { skill: String },
    /// Library moved on and the workspace copy is untouched: replace it.
    Update { skill: String },
    /// No longer wanted. `already_gone` when the directory is missing.
    Remove { skill: String, already_gone: bool },
    /// Wanted, installed, nothing to do. `drifted` marks local edits
    /// (reported, never overwritten here since library and source agree).
    Keep { skill: String, drifted: bool },
    /// Updating or removing would destroy local work.
    Conflict { skill: String, reason: ConflictReason },
    /// Wanted by a profile but missing from the library.
    Warn { skill: String, msg: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictReason {
    /// Library version changed and the workspace copy was modified too.
    BothChanged,
    /// Skill is no longer wanted but the workspace copy was modified.
    RemoveDrift,
    /// Directory exists, is not registered, and differs from the library.
    UntrackedModified,
}

impl ConflictReason {
    pub fn describe(&self) -> &'static str {
        match self {
            ConflictReason::BothChanged => "library changed AND local modifications",
            ConflictReason::RemoveDrift => "local modifications, no longer wanted by any profile",
            ConflictReason::UntrackedModified => "untracked copy differs from library",
        }
    }
}

/// Everything known about one repo's materialized skills directory.
#[derive(Debug, Default, Clone)]
pub struct Workspace {
    /// Fingerprint per skill directory currently present.
    pub fingerprints: BTreeMap<String, String>,
}

impl Workspace {
    /// Fingerprint every directory inside `skills_dir` (which may not exist).
    pub fn scan(skills_dir: &Path) -> Result<Workspace> {
        let mut fingerprints = BTreeMap::new();
        if let Ok(rd) = std::fs::read_dir(skills_dir) {
            let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let id = e.file_name().to_string_lossy().into_owned();
                    fingerprints.insert(id, fingerprint::fingerprint_dir(&e.path())?);
                }
            }
        }
        Ok(Workspace { fingerprints })
    }
}

/// Compute the plan for one repository.
///
/// - `desired`: (skill id, current library fingerprint), deduplicated, any order.
/// - `installed`: the repo's registry records.
/// - `workspace`: what is actually on disk.
pub fn plan(
    desired: &[(String, String)],
    installed: &[InstalledSkill],
    workspace: &Workspace,
) -> Vec<Action> {
    let mut actions: Vec<Action> = Vec::new();
    let mut wanted: Vec<&str> = Vec::new();

    for (id, lib_fp) in desired {
        if wanted.contains(&id.as_str()) {
            continue; // two profiles bring the same skill: one action
        }
        wanted.push(id);
        // Sentinel: an empty library fingerprint means the profile wants a
        // skill the library does not have. Warn; never invent one.
        if lib_fp.is_empty() {
            actions.push(Action::Warn {
                skill: id.clone(),
                msg: "wanted by enabled profiles but missing in the library".to_string(),
            });
            continue;
        }
        let ws_fp = workspace.fingerprints.get(id);
        match installed.iter().find(|s| s.id == *id) {
            None => match ws_fp {
                None => actions.push(Action::Install { skill: id.clone() }),
                Some(fp) if fp == lib_fp => actions.push(Action::Adopt { skill: id.clone() }),
                Some(_) => actions.push(Action::Conflict {
                    skill: id.clone(),
                    reason: ConflictReason::UntrackedModified,
                }),
            },
            Some(rec) => {
                let source_changed = rec.source_fingerprint != *lib_fp;
                match ws_fp {
                    None => {
                        // Registered but vanished from the workspace: put it back.
                        actions.push(Action::Install { skill: id.clone() });
                    }
                    Some(ws) if ws == &rec.installed_fingerprint => {
                        if source_changed {
                            actions.push(Action::Update { skill: id.clone() });
                        } else {
                            actions.push(Action::Keep { skill: id.clone(), drifted: false });
                        }
                    }
                    Some(_) => {
                        if source_changed {
                            actions.push(Action::Conflict {
                                skill: id.clone(),
                                reason: ConflictReason::BothChanged,
                            });
                        } else {
                            actions.push(Action::Keep { skill: id.clone(), drifted: true });
                        }
                    }
                }
            }
        }
    }

    // Installed but no longer wanted by any profile.
    for rec in installed {
        if wanted.contains(&rec.id.as_str()) {
            continue;
        }
        match workspace.fingerprints.get(&rec.id) {
            // Already gone from disk: just drop the record.
            None => actions.push(Action::Remove { skill: rec.id.clone(), already_gone: true }),
            // Untouched copy: safe to remove.
            Some(ws) if ws == &rec.installed_fingerprint => {
                actions.push(Action::Remove { skill: rec.id.clone(), already_gone: false })
            }
            // Someone's local work: never guess destructively.
            Some(_) => actions.push(Action::Conflict {
                skill: rec.id.clone(),
                reason: ConflictReason::RemoveDrift,
            }),
        }
    }

    actions.sort_by_key(|a| skill_name(a).to_string());
    actions
}

fn skill_name(a: &Action) -> &str {
    match a {
        Action::Install { skill }
        | Action::Adopt { skill }
        | Action::Update { skill }
        | Action::Remove { skill, .. }
        | Action::Keep { skill, .. }
        | Action::Conflict { skill, .. }
        | Action::Warn { skill, .. } => skill,
    }
}

/// Outcome counters for one apply run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub installed: usize,
    pub updated: usize,
    pub removed: usize,
    pub adopted: usize,
    pub kept: usize,
    pub conflicts: usize,
    pub warnings: usize,
    pub failed: bool,
}

impl Summary {
    pub fn quiet(&self) -> bool {
        self.installed + self.updated + self.removed + self.adopted + self.conflicts == 0
    }
}

/// Resolve the effective policy: explicit flag > config > interactivity.
/// A non-interactive session with no explicit policy skips conflicts
/// (safe, announced) — it never guesses destructively.
pub fn resolve_policy(flag: Option<ConflictPolicy>, config: Option<ConflictPolicy>, interactive: bool) -> (ConflictPolicy, bool) {
    if let Some(p) = flag {
        if p == ConflictPolicy::Ask && !interactive {
            return (ConflictPolicy::Skip, true);
        }
        return (p, false);
    }
    if let Some(p) = config {
        if p == ConflictPolicy::Ask && !interactive {
            return (ConflictPolicy::Skip, true);
        }
        return (p, false);
    }
    if interactive {
        (ConflictPolicy::Ask, false)
    } else {
        (ConflictPolicy::Skip, true)
    }
}

/// Apply (or print) a plan against one repo.
pub struct Applier<'a> {
    pub library: &'a Library,
    pub skills_dir: PathBuf,
    pub ui: &'a Ui,
    pub policy: ConflictPolicy,
    pub dry_run: bool,
    /// Promotions performed during this run: skill id -> new library fingerprint.
    pub promoted: BTreeMap<String, String>,
}

impl<'a> Applier<'a> {
    /// Walk the plan, printing each step and performing the filesystem work
    /// (unless `dry_run`). Returns the repo's new installed records and a
    /// summary; the caller persists them into the registry. On abort the
    /// records cover only what was applied before the stop.
    pub fn apply(
        &mut self,
        actions: &[Action],
        repo: &RepoRecord,
    ) -> Result<(Vec<InstalledSkill>, Summary)> {
        let mut summary = Summary::default();
        let mut records: Vec<InstalledSkill> = Vec::new();

        for action in actions {
            match action {
                Action::Install { skill } => {
                    self.line("+", skill, "install")?;
                    summary.installed += 1;
                    if !self.dry_run {
                        records.push(self.install_record(skill)?);
                    }
                }
                Action::Adopt { skill } => {
                    self.line("+", skill, "adopt (matches library)")?;
                    summary.adopted += 1;
                    if !self.dry_run {
                        let fp = self.effective_library_fp(skill)?;
                        records.push(self.record(skill, fp.clone(), fp));
                    }
                }
                Action::Update { skill } => {
                    self.line("~", skill, "update (library changed)")?;
                    summary.updated += 1;
                    if !self.dry_run {
                        records.push(self.install_record(skill)?);
                    }
                }
                Action::Remove { skill, already_gone } => {
                    if *already_gone {
                        self.line("-", skill, "remove (already missing on disk)")?;
                    } else {
                        self.line("-", skill, "remove")?;
                    }
                    summary.removed += 1;
                    if !self.dry_run && !already_gone {
                        let dst = self.skills_dir.join(skill);
                        util::remove_tree(&dst)?;
                    }
                }
                Action::Keep { skill, drifted } => {
                    if *drifted {
                        self.line("=", skill, "keep (local modifications, library unchanged)")?;
                    } else {
                        self.line("=", skill, "up to date")?;
                    }
                    summary.kept += 1;
                    if !self.dry_run {
                        if let Some(rec) = repo.installed(skill) {
                            let mut r = rec.clone();
                            r.status = if *drifted { "local-drift" } else { "clean" }.into();
                            records.push(r);
                        }
                    }
                }
                Action::Conflict { skill, reason } => {
                    let outcome = self.resolve_conflict(skill, *reason)?;
                    match outcome {
                        ConflictOutcome::ReplacedWithLibrary => {
                            self.line("~", skill, "replaced with library version")?;
                            summary.updated += 1;
                            if !self.dry_run {
                                records.push(self.install_record(skill)?);
                            }
                        }
                        ConflictOutcome::Promoted => {
                            self.line("~", skill, "promoted local version to library, then updated")?;
                            summary.updated += 1;
                            if !self.dry_run {
                                records.push(self.install_record(skill)?);
                            }
                        }
                        ConflictOutcome::KeptLocal => {
                            self.line("!", skill, "kept local modifications (not updated)")?;
                            summary.conflicts += 1;
                            if !self.dry_run {
                                if let Some(rec) = repo.installed(skill) {
                                    let mut r = rec.clone();
                                    r.status = "local-drift".into();
                                    records.push(r);
                                } else {
                                    // Untracked and kept: leave it untracked
                                    // (no registry record, nothing deleted).
                                }
                            }
                        }
                        ConflictOutcome::Aborted => {
                            self.line("!", skill, "aborted — remaining actions skipped")?;
                            summary.conflicts += 1;
                            summary.failed = true;
                            return Ok((records, summary));
                        }
                    }
                }
                Action::Warn { skill, msg } => {
                    self.line("!", skill, msg)?;
                    summary.warnings += 1;
                }
            }
        }
        Ok((records, summary))
    }

    fn line(&self, symbol: &str, skill: &str, note: &str) -> Result<()> {
        self.ui.action_line(symbol, skill, note, self.dry_run);
        Ok(())
    }

    /// Copy the library version into the workspace (replacing it), then
    /// build the registry record from what is actually on disk.
    fn install_record(&self, skill: &str) -> Result<InstalledSkill> {
        let src = self.library.skill_path(skill);
        let dst = self.skills_dir.join(skill);
        util::remove_tree(&dst)?;
        util::copy_tree(&src, &dst)?;
        let now_fp = fingerprint::fingerprint_dir(&dst)?;
        let source_fp = self.effective_library_fp(skill)?;
        Ok(self.record(skill, source_fp, now_fp))
    }

    /// Library fingerprint, honoring promotions that happened this run.
    fn effective_library_fp(&self, skill: &str) -> Result<String> {
        if let Some(fp) = self.promoted.get(skill) {
            return Ok(fp.clone());
        }
        self.library.fingerprint(skill)
    }

    fn record(&self, skill: &str, source_fp: String, installed_fp: String) -> InstalledSkill {
        InstalledSkill {
            id: skill.to_string(),
            source_fingerprint: source_fp,
            installed_fingerprint: installed_fp,
            installed_at: util::now_iso(),
            status: "clean".to_string(),
        }
    }

    fn resolve_conflict(
        &mut self,
        skill: &str,
        reason: ConflictReason,
    ) -> Result<ConflictOutcome> {
        // Announce once per conflict, regardless of policy.
        let ws_dir = self.skills_dir.join(skill);
        self.ui.conflict_header(skill, reason.describe(), &ws_dir);

        let mut policy = self.policy;
        if policy == ConflictPolicy::Ask {
            // Interactive menu.
            loop {
                match self.ui.conflict_menu(skill)? {
                    'd' => {
                        let lib = self.library.skill_path(skill);
                        let text = crate::diff::diff_dirs(&lib, &ws_dir);
                        print!("{text}");
                    }
                    'k' => {
                        policy = ConflictPolicy::Skip;
                        break;
                    }
                    'l' => {
                        policy = ConflictPolicy::Replace;
                        break;
                    }
                    'p' => {
                        policy = ConflictPolicy::Promote;
                        break;
                    }
                    'a' => return Ok(ConflictOutcome::Aborted),
                    _ => {}
                }
            }
        }

        match policy {
            ConflictPolicy::Skip => Ok(ConflictOutcome::KeptLocal),
            // Abort resolves nothing: stop here, skill untouched.
            ConflictPolicy::Abort => Ok(ConflictOutcome::Aborted),
            ConflictPolicy::Replace => {
                if !self.dry_run {
                    self.apply_now(skill)?;
                }
                Ok(ConflictOutcome::ReplacedWithLibrary)
            }
            ConflictPolicy::Promote => {
                if !self.dry_run {
                    let new_fp = self.promote(skill)?;
                    self.promoted.insert(skill.to_string(), new_fp);
                }
                Ok(ConflictOutcome::Promoted)
            }
            ConflictPolicy::Ask => unreachable!("resolved above"),
        }
    }

    /// Physically replace the workspace copy with the library version.
    fn apply_now(&self, skill: &str) -> Result<()> {
        let src = self.library.skill_path(skill);
        let dst = self.skills_dir.join(skill);
        util::remove_tree(&dst)?;
        util::copy_tree(&src, &dst)?;
        Ok(())
    }

    /// Copy the workspace version over the library version, return new fp.
    fn promote(&mut self, skill: &str) -> Result<String> {
        let ws = self.skills_dir.join(skill);
        let lib = self.library.skill_path(skill);
        util::remove_tree(&lib)?;
        util::copy_tree(&ws, &lib)?;
        let fp = fingerprint::fingerprint_dir(&lib)?;
        self.ui.note(&format!("promoted {} to library ({})", skill, fp));
        Ok(fp)
    }
}

enum ConflictOutcome {
    ReplacedWithLibrary,
    Promoted,
    KeptLocal,
    Aborted,
}

/// Current per-skill state for `status` views (not a plan — a report).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillStatus {
    /// Installed, workspace untouched, library unchanged.
    Clean,
    /// Workspace untouched but the library has a newer version.
    LibraryChanged,
    /// Workspace copy differs from what beskar installed.
    LocalDrift,
    /// Library changed and the workspace was modified.
    BothChanged,
    /// In the registry but the workspace directory is gone.
    MissingOnDisk,
    /// In the registry but no longer in the library.
    MissingLibrary,
    /// Directory present, never installed by beskar.
    Untracked,
}

impl SkillStatus {
    pub fn label(&self) -> &'static str {
        match self {
            SkillStatus::Clean => "clean",
            SkillStatus::LibraryChanged => "library has a newer version",
            SkillStatus::LocalDrift => "local modifications",
            SkillStatus::BothChanged => "conflict",
            SkillStatus::MissingOnDisk => "missing on disk",
            SkillStatus::MissingLibrary => "no longer in the library",
            SkillStatus::Untracked => "untracked",
        }
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            SkillStatus::Clean => "=",
            SkillStatus::LibraryChanged => "~",
            SkillStatus::LocalDrift => "!",
            SkillStatus::BothChanged => "!!",
            SkillStatus::MissingOnDisk => "!",
            SkillStatus::MissingLibrary => "!",
            SkillStatus::Untracked => "?",
        }
    }
}

/// Classify one skill for reporting. Only meaningful for ids that are
/// installed, on disk, or desired — anything else reads as `Clean`
/// ("nothing to report") by convention.
pub fn classify(
    rec: Option<&InstalledSkill>,
    lib_fp: Option<String>,
    ws_fp: Option<String>,
) -> SkillStatus {
    match (rec, lib_fp, ws_fp) {
        (Some(_r), None, _) => SkillStatus::MissingLibrary,
        (Some(_), Some(_), None) => SkillStatus::MissingOnDisk,
        (Some(r), Some(lib), Some(ws)) => {
            let ws_changed = ws != r.installed_fingerprint;
            let lib_changed = r.source_fingerprint != lib;
            match (ws_changed, lib_changed) {
                (false, false) => SkillStatus::Clean,
                (false, true) => SkillStatus::LibraryChanged,
                (true, false) => SkillStatus::LocalDrift,
                (true, true) => SkillStatus::BothChanged,
            }
        }
        (None, _, Some(_)) => SkillStatus::Untracked,
        (None, _, None) => SkillStatus::Clean,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(n: u8) -> String {
        format!("sha256:{n:016x}")
    }

    fn rec(id: &str, source: u8, installed: u8) -> InstalledSkill {
        InstalledSkill {
            id: id.into(),
            source_fingerprint: fp(source),
            installed_fingerprint: fp(installed),
            installed_at: "2026-01-01T00:00:00Z".into(),
            status: "clean".into(),
        }
    }

    fn ws(pairs: &[(&str, u8)]) -> Workspace {
        let mut fingerprints = BTreeMap::new();
        for (id, n) in pairs {
            fingerprints.insert((*id).to_string(), fp(*n));
        }
        Workspace { fingerprints }
    }

    fn desired(pairs: &[(&str, u8)]) -> Vec<(String, String)> {
        pairs.iter().map(|(s, n)| ((*s).to_string(), fp(*n))).collect()
    }

    fn names(actions: &[Action]) -> Vec<(&str, &'static str)> {
        actions
            .iter()
            .map(|a| {
                let k = match a {
                    Action::Install { .. } => "install",
                    Action::Adopt { .. } => "adopt",
                    Action::Update { .. } => "update",
                    Action::Remove { .. } => "remove",
                    Action::Keep { .. } => "keep",
                    Action::Conflict { .. } => "conflict",
                    Action::Warn { .. } => "warn",
                };
                (skill_name(a), k)
            })
            .collect()
    }

    #[test]
    fn fresh_install() {
        let plan = plan(&desired(&[("a", 1), ("b", 2)]), &[], &ws(&[]));
        assert_eq!(names(&plan), vec![("a", "install"), ("b", "install")]);
    }

    #[test]
    fn the_brief_example() {
        // Workspace has git, testing, pdf. Profiles now want git, testing,
        // playwright. pdf's registry copy is clean.
        let installed = vec![rec("git", 1, 1), rec("testing", 1, 1), rec("pdf", 1, 1)];
        let workspace = ws(&[("git", 1), ("testing", 1), ("pdf", 1)]);
        let want = desired(&[("git", 1), ("testing", 1), ("playwright", 2)]);
        let plan = plan(&want, &installed, &workspace);
        assert_eq!(
            names(&plan),
            vec![
                ("git", "keep"),
                ("pdf", "remove"),
                ("playwright", "install"),
                ("testing", "keep"),
            ]
        );
    }

    #[test]
    fn library_update_when_clean() {
        let installed = vec![rec("a", 1, 1)];
        let workspace = ws(&[("a", 1)]);
        let plan = plan(&desired(&[("a", 2)]), &installed, &workspace);
        assert_eq!(names(&plan), vec![("a", "update")]);
    }

    #[test]
    fn drift_detection_matrix() {
        // Local drift, library unchanged: keep (never overwrite silently).
        let installed = vec![rec("a", 1, 1)];
        let p = plan(&desired(&[("a", 1)]), &installed, &ws(&[("a", 9)]));
        assert_eq!(p, vec![Action::Keep { skill: "a".into(), drifted: true }]);

        // Local drift AND library changed: conflict.
        let p = plan(&desired(&[("a", 2)]), &installed, &ws(&[("a", 9)]));
        assert_eq!(
            p,
            vec![Action::Conflict { skill: "a".into(), reason: ConflictReason::BothChanged }]
        );

        // Removal of a drifted skill: conflict.
        let p = plan(&[], &installed, &ws(&[("a", 9)]));
        assert_eq!(
            p,
            vec![Action::Conflict { skill: "a".into(), reason: ConflictReason::RemoveDrift }]
        );

        // Removal of a clean skill: remove.
        let p = plan(&[], &installed, &ws(&[("a", 1)]));
        assert_eq!(p, vec![Action::Remove { skill: "a".into(), already_gone: false }]);

        // Removal when already gone: remove, already_gone.
        let p = plan(&[], &installed, &ws(&[]));
        assert_eq!(p, vec![Action::Remove { skill: "a".into(), already_gone: true }]);
    }

    #[test]
    fn untracked_dirs() {
        // Identical to library: adopt (no recopy).
        let p = plan(&desired(&[("a", 1)]), &[], &ws(&[("a", 1)]));
        assert_eq!(p, vec![Action::Adopt { skill: "a".into() }]);

        // Different from library: untracked conflict.
        let p = plan(&desired(&[("a", 1)]), &[], &ws(&[("a", 7)]));
        assert_eq!(
            p,
            vec![Action::Conflict { skill: "a".into(), reason: ConflictReason::UntrackedModified }]
        );

        // Untracked and unwanted: not our business in the plan (reported
        // separately by status).
        let p = plan(&[], &[], &ws(&[("a", 1)]));
        assert!(p.is_empty());
    }

    #[test]
    fn missing_and_reinstall() {
        // Desired but not in the library at all (empty-fp sentinel): warn.
        let p = plan(&[("ghost".to_string(), String::new())], &[], &ws(&[]));
        assert!(matches!(p[0], Action::Warn { .. }));

        // Registered, wanted, but vanished from disk: reinstall.
        let installed = vec![rec("a", 1, 1)];
        let p = plan(&desired(&[("a", 1)]), &installed, &ws(&[]));
        assert_eq!(p, vec![Action::Install { skill: "a".into() }]);

        // Registered, not wanted, vanished: quiet removal.
        let p = plan(&[], &installed, &ws(&[]));
        assert_eq!(p, vec![Action::Remove { skill: "a".into(), already_gone: true }]);
    }

    #[test]
    fn duplicate_desired_skills_collapse() {
        // Union of two profiles both containing "a": one action only.
        let mut want = desired(&[("a", 1)]);
        want.extend(desired(&[("a", 1)]));
        let plan = plan(&want, &[], &ws(&[]));
        assert_eq!(plan.len(), 1);
    }

    #[test]
    fn dry_run_never_touches_conflicted_workspace() {
        // --dry-run --conflict replace must print the plan without
        // physically replacing the drifted workspace copy.
        let base = std::env::temp_dir().join(format!("beskar-apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let lib = Library::new(&base.join("library"));
        lib.init_dirs().unwrap();
        std::fs::create_dir_all(lib.skill_path("a")).unwrap();
        std::fs::write(lib.skill_path("a").join("SKILL.md"), "library version\n").unwrap();

        let skills_dir = base.join("ws").join(".agents").join("skills");
        std::fs::create_dir_all(skills_dir.join("a")).unwrap();
        std::fs::write(skills_dir.join("a").join("SKILL.md"), "local work\n").unwrap();

        let ui = crate::ui::Ui { color: false };
        let repo = crate::registry::RepoRecord {
            installed: vec![rec("a", 1, 1)],
            ..Default::default()
        };
        let actions =
            vec![Action::Conflict { skill: "a".into(), reason: ConflictReason::BothChanged }];

        let mut dry = Applier {
            library: &lib,
            skills_dir: skills_dir.clone(),
            ui: &ui,
            policy: ConflictPolicy::Replace,
            dry_run: true,
            promoted: BTreeMap::new(),
        };
        let (records, summary) = dry.apply(&actions, &repo).unwrap();
        assert_eq!(summary.updated, 1);
        assert!(records.is_empty(), "dry run records nothing");
        assert_eq!(
            std::fs::read_to_string(skills_dir.join("a").join("SKILL.md")).unwrap(),
            "local work\n",
            "dry run must leave the local file alone"
        );

        // A real run with the same policy replaces the file.
        let mut real = Applier {
            library: &lib,
            skills_dir: skills_dir.clone(),
            ui: &ui,
            policy: ConflictPolicy::Replace,
            dry_run: false,
            promoted: BTreeMap::new(),
        };
        let (records, _) = real.apply(&actions, &repo).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            std::fs::read_to_string(skills_dir.join("a").join("SKILL.md")).unwrap(),
            "library version\n"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn policy_resolution() {
        use ConflictPolicy::*;
        // Explicit flag wins.
        assert_eq!(resolve_policy(Some(Replace), Some(Skip), false), (Replace, false));
        // Ask degrades to skip outside a terminal, with a notice.
        assert_eq!(resolve_policy(Some(Ask), None, false), (Skip, true));
        assert_eq!(resolve_policy(None, Some(Ask), false), (Skip, true));
        // Interactive default is ask.
        assert_eq!(resolve_policy(None, None, true), (Ask, false));
        // Non-interactive default is skip.
        assert_eq!(resolve_policy(None, None, false), (Skip, true));
    }

    #[test]
    fn classify_matrix() {
        let r = rec("a", 1, 1);
        let lib = || Some(fp(1));
        let ws_ok = || Some(fp(1));
        assert_eq!(classify(Some(&r), lib(), ws_ok()), SkillStatus::Clean);
        assert_eq!(classify(Some(&r), Some(fp(2)), ws_ok()), SkillStatus::LibraryChanged);
        assert_eq!(classify(Some(&r), lib(), Some(fp(9))), SkillStatus::LocalDrift);
        assert_eq!(classify(Some(&r), Some(fp(2)), Some(fp(9))), SkillStatus::BothChanged);
        assert_eq!(classify(Some(&r), lib(), None), SkillStatus::MissingOnDisk);
        assert_eq!(classify(Some(&r), None, ws_ok()), SkillStatus::MissingLibrary);
        assert_eq!(classify(None, lib(), Some(fp(3))), SkillStatus::Untracked);
    }
}

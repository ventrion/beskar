//! Health checks over configuration, library, registry and workspaces.

use std::fmt;

use crate::config::{Config, Home};
use crate::error::Result;
use crate::fsutil;
use crate::library::Library;
use crate::reconcile::{self, Action};
use crate::registry::Registry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    pub message: String,
}

fn finding(level: Level, message: impl Into<String>) -> Finding {
    Finding {
        level,
        message: message.into(),
    }
}

/// Run every check. Never fails itself; problems become findings.
pub fn run(home: &Home) -> Vec<Finding> {
    let mut out = Vec::new();
    let config = match Config::load(home) {
        Ok(c) => {
            out.push(finding(
                Level::Ok,
                format!("config {}", fsutil::display_path(&c.path)),
            ));
            c
        }
        Err(e) => {
            out.push(finding(Level::Fail, format!("config: {e}")));
            return out;
        }
    };

    let library = match Library::open(&config.library) {
        Ok(l) => {
            out.push(finding(
                Level::Ok,
                format!("library {}", fsutil::display_path(&l.root)),
            ));
            Some(l)
        }
        Err(e) => {
            out.push(finding(Level::Fail, format!("library: {e}")));
            None
        }
    };

    if let Some(lib) = &library {
        match lib.skills() {
            Ok(skills) => {
                out.push(finding(
                    Level::Ok,
                    format!("{} skill(s) in the library", skills.len()),
                ));
                for s in skills.iter().filter(|s| !s.has_skill_file()) {
                    out.push(finding(
                        Level::Warn,
                        format!("skill '{}' has no SKILL.md", s.name),
                    ));
                }
            }
            Err(e) => out.push(finding(Level::Fail, format!("skills: {e}"))),
        }
        match lib.profiles() {
            Ok(profiles) => {
                out.push(finding(
                    Level::Ok,
                    format!("{} profile(s) parse", profiles.len()),
                ));
                for p in &profiles {
                    for s in p.skills().iter().filter(|s| !lib.has_skill(s)) {
                        out.push(finding(
                            Level::Warn,
                            format!("profile '{}' lists unknown skill '{s}'", p.name),
                        ));
                    }
                }
            }
            Err(e) => out.push(finding(Level::Fail, format!("profiles: {e}"))),
        }
    }

    let registry = match Registry::load(&config.registry) {
        Ok(r) => {
            out.push(finding(
                Level::Ok,
                format!(
                    "registry {} ({} repositor{})",
                    fsutil::display_path(&r.path),
                    r.repos().len(),
                    if r.repos().len() == 1 { "y" } else { "ies" }
                ),
            ));
            r
        }
        Err(e) => {
            out.push(finding(Level::Fail, format!("registry: {e}")));
            return out;
        }
    };

    for repo in registry.repos() {
        let shown = fsutil::display_path(&repo.path);
        if !repo.path.is_dir() {
            out.push(finding(
                Level::Warn,
                format!("{shown}: directory is missing (run `beskar registry prune`)"),
            ));
            continue;
        }
        let Some(lib) = &library else { continue };
        match reconcile::plan(lib, repo, &config.skills_dir) {
            Ok(plan) => summarise_plan(&plan, &shown, &mut out),
            Err(e) => out.push(finding(Level::Fail, format!("{shown}: {e}"))),
        }
    }
    out
}

fn summarise_plan(plan: &reconcile::Plan, shown: &str, out: &mut Vec<Finding>) {
    for p in &plan.missing_profiles {
        out.push(finding(
            Level::Warn,
            format!("{shown}: enabled profile '{p}' does not exist"),
        ));
    }
    let mut pending = 0;
    for item in &plan.items {
        match &item.action {
            Action::Unchanged | Action::Unmanaged => {}
            Action::Conflict(c) => out.push(finding(
                Level::Warn,
                format!("{shown}: {}: {}", item.skill, c.describe()),
            )),
            Action::Modified => out.push(finding(
                Level::Warn,
                format!("{shown}: {}: modified locally", item.skill),
            )),
            Action::MissingInLibrary => out.push(finding(
                Level::Warn,
                format!(
                    "{shown}: {}: wanted by {} but not in the library",
                    item.skill,
                    item.profiles.join(", ")
                ),
            )),
            _ => pending += 1,
        }
    }
    if pending > 0 {
        out.push(finding(
            Level::Warn,
            format!("{shown}: {pending} change(s) pending (run `beskar repo update`)"),
        ));
    } else if plan.is_clean() {
        out.push(finding(Level::Ok, format!("{shown}: in sync")));
    }
}

/// Convenience: run and tell whether anything failed.
pub fn run_all(home: &Home) -> Result<(Vec<Finding>, bool)> {
    let findings = run(home);
    let failed = findings.iter().any(|f| f.level == Level::Fail);
    Ok((findings, failed))
}

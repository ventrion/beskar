//! Test fixtures for pure state-machine tests (no terminal, no I/O).
//!
//! Builds small synthetic [`Snapshot`]s shaped exactly like the ones the
//! service layer gathers from core read APIs, so reducer transitions are
//! exercised against realistic data.

#![cfg(test)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use beskar_core::editing::{BranchDisplay, LibraryStatusReport, SkillListing};
use beskar_core::ids::{InstallationId, LibraryId, ProfileId, SkillName};
use beskar_core::registry::{Adapter, Installation, LastAppliedState, ProfileAttachment};
use beskar_core::status::{InstallationStatus, ProfileStatus, SkillStatus};
use time::OffsetDateTime;

use crate::app::{InstallationRow, Snapshot};

/// The dev-core profile ID (§6 fixture).
pub const DEV_ID: &str = "98f1513d-94fa-4ace-907e-544c66233653";
/// The rust-development profile ID (§6 fixture).
pub const RUST_ID: &str = "0b0e1c3a-9d24-4c5a-8a30-2b7f0a519102";
/// The single installation ID.
pub const INSTALLATION_ID: &str = "11111111-2222-3333-4444-555555555555";

/// Parses a fixed UUID for stable fixture identities.
pub fn library_id() -> LibraryId {
    LibraryId::parse("550e8400-e29b-41d4-a716-446655440000").expect("valid fixture uuid")
}

pub fn dev_id() -> ProfileId {
    ProfileId::parse(DEV_ID).expect("valid fixture profile id")
}

pub fn rust_id() -> ProfileId {
    ProfileId::parse(RUST_ID).expect("valid fixture profile id")
}

pub fn installation_id() -> InstallationId {
    InstallationId::parse(INSTALLATION_ID).expect("valid fixture installation id")
}

/// Unique per-call path so tests can assert reselection behavior.
fn workspace_path() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(1);
    PathBuf::from(format!(
        "/fixtures/workspace-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

pub fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("fixed timestamp")
}

pub fn skill_listing(name: &str, bucket: &str, description: &str, tags: &[&str]) -> SkillListing {
    SkillListing {
        name: SkillName::parse(name).expect("valid fixture skill name"),
        path: if bucket.is_empty() {
            format!("skills/{name}")
        } else {
            format!("skills/{bucket}/{name}")
        },
        bucket: bucket.to_owned(),
        description: description.to_owned(),
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        rank: None,
        notes: None,
        profiles: Vec::new(),
        last_commit: None,
    }
}

pub fn profile(id: &str, name: &str, skills: &[&str]) -> beskar_core::profile::Profile {
    beskar_core::profile::Profile {
        schema: 1,
        id: ProfileId::parse(id).expect("valid fixture profile id"),
        name: name.to_owned(),
        description: Some(format!("{name} capability set")),
        skills: skills
            .iter()
            .map(|skill| SkillName::parse(skill).expect("valid fixture skill"))
            .collect(),
    }
}

pub fn attachment(id: &str, name: &str) -> ProfileAttachment {
    ProfileAttachment {
        id: ProfileId::parse(id).expect("valid fixture profile id"),
        name: name.to_owned(),
        attached_at: now(),
    }
}

pub fn library_report() -> LibraryStatusReport {
    LibraryStatusReport {
        path: PathBuf::from("/fixtures/library"),
        library_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        branch: Some("main".to_owned()),
        head: Some("abc1234567890def".to_owned()),
        dirty: false,
        staged: Vec::new(),
        unstaged: Vec::new(),
        untracked: Vec::new(),
        remote: Some("git@example.com:skills.git".to_owned()),
        default_ref: "main".to_owned(),
        upstream: Some("origin/main".to_owned()),
        ahead: Some(0),
        behind: Some(2),
        installations: Vec::new(),
    }
}

pub fn installation(profiles: Vec<ProfileAttachment>) -> Installation {
    Installation {
        id: installation_id(),
        library_id: library_id(),
        workspace: workspace_path(),
        target: ".agents/skills".to_owned(),
        adapter: Adapter::Agents,
        source_ref: "main".to_owned(),
        profiles,
        last_applied: LastAppliedState::default(),
        workspace_info: None,
        installed_at: now(),
        updated_at: now(),
    }
}

pub fn skill_status(
    name: &str,
    state: beskar_core::drift::DriftState,
    required_by: Vec<ProfileId>,
) -> (SkillName, SkillStatus) {
    (
        SkillName::parse(name).expect("valid fixture skill"),
        SkillStatus {
            state,
            membership_drift: beskar_core::drift::MembershipDrift::Unchanged,
            required_by,
            last_required_by: Vec::new(),
            extra_files: Vec::new(),
            protected_by_missing_profile: false,
        },
    )
}

/// A clean installation status: everything current, both profiles resolved.
pub fn status_fixture() -> InstallationStatus {
    InstallationStatus {
        installation_id: installation_id(),
        library_id: library_id(),
        workspace: workspace_path(),
        target: ".agents/skills".to_owned(),
        source_ref: "main".to_owned(),
        installation_state: None,
        resolved_commit: Some("def4567890abcdef".to_owned()),
        profiles: vec![
            ProfileStatus {
                attachment: attachment(DEV_ID, "dev-core"),
                profile: Some(profile(DEV_ID, "dev-core", &["git", "testing"])),
            },
            ProfileStatus {
                attachment: attachment(RUST_ID, "rust-development"),
                profile: Some(profile(RUST_ID, "rust-development", &["rust", "testing"])),
            },
        ],
        skills: [
            skill_status(
                "git",
                beskar_core::drift::DriftState::Current,
                vec![dev_id()],
            ),
            skill_status(
                "rust",
                beskar_core::drift::DriftState::Current,
                vec![rust_id()],
            ),
            skill_status(
                "testing",
                beskar_core::drift::DriftState::Current,
                vec![dev_id(), rust_id()],
            ),
        ]
        .into_iter()
        .collect(),
        unmanaged: Vec::new(),
        unsafe_paths: Vec::new(),
    }
}

/// The standard two-profile fixture (§6): dev-core and rust-development
/// overlap on `testing`, with one installation attaching both and a clean
/// status. Skills are sorted by name (`code-review`, `git`, `rust`,
/// `testing`).
pub fn snapshot() -> Snapshot {
    let dev = profile(DEV_ID, "dev-core", &["git", "testing"]);
    let rust = profile(RUST_ID, "rust-development", &["rust", "testing"]);
    let profiles = vec![dev, rust];
    let mut skills = vec![
        skill_listing(
            "code-review",
            "engineering/process",
            "Review code carefully.",
            &["review"],
        ),
        skill_listing("git", "engineering", "Branch hygiene.", &["git"]),
        skill_listing("rust", "languages", "Write Rust.", &[]),
        skill_listing("testing", "quality", "Test discipline.", &["quality"]),
    ];
    let installation = installation(vec![
        attachment(DEV_ID, "dev-core"),
        attachment(RUST_ID, "rust-development"),
    ]);
    let mut status = status_fixture();
    // Keep the workspace consistent across the row and the status.
    status.workspace = installation.workspace.clone();
    for skill in &mut skills {
        skill.profiles = profiles
            .iter()
            .filter(|candidate| candidate.skills.iter().any(|s| s == &skill.name))
            .map(|candidate| candidate.name.clone())
            .collect();
    }
    Snapshot {
        library: library_report(),
        skills,
        profiles,
        installations: vec![InstallationRow {
            installation,
            status: Some(status),
            error: None,
        }],
        branches: vec![
            BranchDisplay {
                name: "main".to_owned(),
                current: true,
                upstream: Some("origin/main".to_owned()),
                ahead: Some(0),
                behind: Some(2),
            },
            BranchDisplay {
                name: "feature".to_owned(),
                current: false,
                upstream: None,
                ahead: None,
                behind: None,
            },
        ],
    }
}

/// A minimal `beskar: remove skill` library plan for confirm-dialog tests.
pub fn removal_plan() -> beskar_core::editing::LibraryPlan {
    beskar_core::editing::LibraryPlan {
        message: "beskar: remove skill testing".to_owned(),
        ops: vec![beskar_core::editing::LibraryOp {
            kind: beskar_core::editing::LibraryOpKind::Remove,
            path: "skills/quality/testing".to_owned(),
            from: None,
            source: None,
            content: None,
        }],
    }
}

/// A minimal unblocked reconciliation plan for confirm-dialog render tests.
pub fn plan_fixture() -> beskar_core::plan::ReconciliationPlan {
    beskar_core::plan::ReconciliationPlan {
        installation_id: installation_id(),
        resolved_commit: Some("abc1234567890".to_owned()),
        profile_changes: vec![],
        skill_actions: vec![beskar_core::plan::SkillAction {
            action: beskar_core::plan::PlanAction::InstallSkill,
            skill: SkillName::parse("testing").expect("valid fixture skill"),
            resulting_state: beskar_core::drift::DriftState::ProfileAdded,
            paths: vec![],
        }],
        state_actions: vec![],
        blockers: vec![],
    }
}

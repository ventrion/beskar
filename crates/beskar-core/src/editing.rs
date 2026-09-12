//! Library editing services (spec §69-§82, §132, §136 Phase 4).
//!
//! UI-agnostic operations over the active Library working tree — ingest,
//! skill move/rename/remove/tag/rank/list/show, the full profile command
//! set, catalog edits, library status, and branch helpers — shared verbatim
//! by the CLI, TUI, and GUI (§105).
//!
//! Every mutation follows the §89 discipline scoped to the Library:
//!
//! 1. plan against the validated working tree (ALL refusals before any
//!    write, §47 style);
//! 2. create one scoped Git commit (§72): unrelated staged files are
//!    refused up front, unrelated unstaged files are left untouched, and
//!    exactly the operation-owned paths are staged;
//! 3. the complete result re-validates (§74: full [`crate::library::Library::scan`])
//!    before the commit is created;
//! 4. mutations never push (§135.34) and take the advisory library lock
//!    (§88); dry-runs perform no step after 1 (§91, §135.39).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use beskar_git::GitBackend;
use serde::{Deserialize, Serialize};

use crate::config::PlatformDirs;
use crate::error::{Error, Result};
use crate::ids::SkillName;
use crate::library::{Library, LibraryScan};
use crate::profile::{Profile, validate_profile_name};
use crate::registry::RegistryStore;
use crate::skill::{SKILL_FILE, Skill, SkillFrontmatter};

/// Files stripped from incoming skills during ingest (§70: `.beskar.json`
/// removal; §124: recognized legacy installation stamps).
const STRIPPED_INGEST_FILES: [&str; 2] = [".beskar.json", ".skm.json"];

// ---- plans and outcomes (§89: plans are serializable) -------------------

/// Kind of one planned library edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryOpKind {
    /// Copy a file from an absolute source into the Library (ingest).
    Write,
    /// Write managed content to a Library path (SKILL.md name edits,
    /// catalog and profile rewrites).
    Edit,
    /// Remove an existing skill directory and copy the incoming tree
    /// (ingest `--replace`).
    Replace,
    /// Remove a path from the Library (skill removal).
    Remove,
    /// Move a skill directory (bucket change, rename leaf).
    Move,
}

/// One planned library edit (§89). Paths are Library-relative and always
/// `/`-separated (§119); `source` paths are absolute filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryOp {
    pub kind: LibraryOpKind,
    /// Primary Library-relative path: destination for writes and moves.
    pub path: String,
    /// Origin Library-relative path for moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Absolute filesystem source for [`LibraryOpKind::Write`] and
    /// [`LibraryOpKind::Replace`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Inline content for [`LibraryOpKind::Edit`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// The serializable plan for one library mutation (§89, §72): the scoped
/// commit message plus the exact operation-owned edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryPlan {
    /// The §72 commit message, e.g. `beskar: ingest code-review`.
    pub message: String,
    pub ops: Vec<LibraryOp>,
}

impl LibraryPlan {
    /// Whether the plan would change nothing.
    pub fn is_no_op(&self) -> bool {
        self.ops.is_empty()
    }

    /// Every path this plan owns, for §72 commit scoping.
    pub fn owned_paths(&self) -> Vec<String> {
        let mut owned = Vec::new();
        for op in &self.ops {
            match op.kind {
                LibraryOpKind::Move => {
                    if let Some(from) = &op.from {
                        owned.push(from.clone());
                    }
                    owned.push(op.path.clone());
                }
                _ => owned.push(op.path.clone()),
            }
        }
        owned.sort();
        owned.dedup();
        owned
    }
}

/// The outcome of one library editing operation (§89 step 5).
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryOutcome {
    pub plan: LibraryPlan,
    /// Whether the plan was applied to the working tree (false for dry-runs
    /// and no-ops).
    pub executed: bool,
    /// The created commit hash, when one was made (§72; `None` for
    /// dry-runs, no-ops, and empty diffs).
    pub commit: Option<String>,
    /// Non-fatal advisories (e.g. §75 external-installation warning).
    pub warnings: Vec<String>,
}

/// The outcome of `beskar skill remove` (§75): the references that made the
/// removal significant are part of the result.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillRemovalOutcome {
    pub outcome: LibraryOutcome,
    /// Library profiles that referenced the removed skill.
    pub referencing_profiles: Vec<String>,
}

// ---- requests -----------------------------------------------------------

/// `beskar ingest` arguments (§70, §71).
#[derive(Debug, Clone)]
pub struct IngestRequest<'a> {
    pub source: &'a Path,
    pub bucket: &'a str,
    pub replace: bool,
    pub recursive: bool,
    pub dry_run: bool,
}

/// Skill-listing filters and sort (§80).
#[derive(Debug, Clone, Default)]
pub struct SkillFilter<'a> {
    pub bucket: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub profile: Option<&'a str>,
    pub query: Option<&'a str>,
    pub sort: Option<SkillSort>,
}

/// Skill-list sort modes (§80).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillSort {
    #[default]
    Name,
    Bucket,
    Rank,
    Recent,
}

/// Pivot for `beskar profile move` (§76).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillPivot {
    Before(String),
    After(String),
}

// ---- listings -----------------------------------------------------------

/// One skill row for `beskar skill list` (§80).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillListing {
    pub name: SkillName,
    /// Library-relative skill root path, `/`-separated (§119).
    pub path: String,
    /// Bucket beneath the skills directory (§13); empty for top-level.
    pub bucket: String,
    pub description: String,
    /// Catalog metadata (§14).
    pub tags: Vec<String>,
    pub rank: Option<i64>,
    pub notes: Option<String>,
    /// Names of library profiles requiring this skill.
    pub profiles: Vec<String>,
    /// Most recent commit touching the skill at HEAD (§35), when HEAD
    /// resolves.
    pub last_commit: Option<beskar_git::CommitInfo>,
}

/// Full detail for `beskar skill show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDetail {
    pub listing: SkillListing,
    /// Every file in the skill directory, `/`-separated, sorted (§119).
    pub files: Vec<String>,
}

/// Branch listing row for `beskar library branch` (§82).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchDisplay {
    pub name: String,
    pub current: bool,
    pub upstream: Option<String>,
    /// Commits only in this branch relative to its upstream (§81).
    pub ahead: Option<usize>,
    /// Commits only in the upstream relative to this branch (§81).
    pub behind: Option<usize>,
}

/// One registered installation's ref usage, reported by library status
/// (§81: "Source refs currently used by registered Installations").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationRef {
    pub workspace: PathBuf,
    pub target: String,
    pub source_ref: String,
}

/// The `beskar library status` report (§81). No network access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryStatusReport {
    pub path: PathBuf,
    pub library_id: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub dirty: bool,
    pub staged: Vec<beskar_git::FileChange>,
    pub unstaged: Vec<beskar_git::FileChange>,
    pub untracked: Vec<String>,
    /// Sanitized origin URL (§30, §67), when configured.
    pub remote: Option<String>,
    pub default_ref: String,
    pub upstream: Option<String>,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub installations: Vec<InstallationRef>,
}

/// One profile's validation report (§76 `profile validate`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileValidation {
    pub profile: String,
    /// Immutable ID when the file parsed (§15).
    pub id: Option<crate::ids::ProfileId>,
    pub valid: bool,
    pub problems: Vec<String>,
    /// Skills the profile names that the Library does not contain (§83).
    pub missing_skills: Vec<String>,
}

// ---- the service ---------------------------------------------------------

/// A library-editing session: active Library, machine-local Registry store
/// (read-only diagnostics), platform dirs (advisory locks), and the Git
/// backend (§107).
pub struct LibraryEditor {
    dirs: PlatformDirs,
    library: Library,
    store: RegistryStore,
    backend: beskar_git::SystemGitBackend,
}

impl LibraryEditor {
    /// Builds a session from explicit parts (dependency-injected; tests
    /// pass temp directories instead of touching the environment, §86).
    pub fn new(dirs: PlatformDirs, library: Library) -> Self {
        let store = RegistryStore::from_dirs(&dirs);
        Self {
            dirs,
            library,
            store,
            backend: beskar_git::SystemGitBackend,
        }
    }

    /// Builds a session from process environment and the working directory
    /// (§86: `BESKAR_HOME`/`BESKAR_LIBRARY` overrides, walk-up discovery).
    pub fn from_env() -> Result<Self> {
        let dirs = PlatformDirs::from_env();
        let cwd = std::env::current_dir()?;
        let library = Library::discover(&cwd)?;
        Ok(Self::new(dirs, library))
    }

    /// The active Library.
    pub fn library(&self) -> &Library {
        &self.library
    }

    /// The Registry store (read-only diagnostics, §81/§83).
    pub fn store(&self) -> &RegistryStore {
        &self.store
    }

    /// The Git backend.
    pub fn backend(&self) -> &beskar_git::SystemGitBackend {
        &self.backend
    }

    // ---- ingest (§70, §71) ------------------------------------------------

    /// Ingests skills from `source` into `bucket` (§70, §71). Validation
    /// happens entirely before any write: the incoming batch is validated,
    /// duplicate incoming names are fatal, existing name collisions require
    /// `--replace` (and are refused across buckets — moving identity is a
    /// separate operation), and `.beskar.json`/legacy stamps are stripped.
    /// One batch results in one scoped commit (§71, §72).
    pub fn ingest(&self, request: IngestRequest<'_>) -> Result<LibraryOutcome> {
        crate::paths::validate_bucket(request.bucket)?;
        let skills_dir = self.library.config().skills_dir.clone();
        let roots = discover_skill_roots(request.source, request.recursive)?;
        let mut incoming = Vec::new();
        for root in roots {
            incoming.push(inspect_incoming_skill(root)?);
        }
        // Duplicate incoming names are fatal (§71).
        let mut seen: BTreeMap<SkillName, &Path> = BTreeMap::new();
        for skill in &incoming {
            if let Some(existing) = seen.insert(skill.name.clone(), skill.root.as_path()) {
                return Err(Error::validation(format!(
                    "duplicate incoming skill name {:?} (from {} and {}); \
                     names are globally unique within one Library revision (§8.2)",
                    skill.name.as_str(),
                    existing.display(),
                    skill.root.display()
                )));
            }
        }

        // The Library must be valid before editing (§4 safer behavior).
        let scan = self.scan()?;
        let mut ops = Vec::new();
        for skill in &incoming {
            let dest = format!("{skills_dir}/{}/{name}", request.bucket, name = skill.name);
            let dest_path = self
                .library
                .root()
                .join(crate::paths::to_native_path(&dest));
            if let Some(existing) = scan.skills.iter().find(|s| s.name == skill.name) {
                if existing.path != dest {
                    return Err(Error::validation(format!(
                        "skill {:?} already exists at {:?}; ingesting into \
                         bucket {:?} would duplicate its name — move or remove \
                         the existing skill first (§8.2, §71)",
                        skill.name.as_str(),
                        existing.path,
                        request.bucket
                    )));
                }
                if !request.replace {
                    return Err(Error::validation(format!(
                        "skill {:?} already exists at {:?}; pass --replace to \
                         replace it (§71)",
                        skill.name.as_str(),
                        existing.path
                    )));
                }
            } else if dest_path.symlink_metadata().is_ok() && !request.replace {
                // A non-skill directory (or leftover content) occupies the
                // destination; replacing it needs explicit consent (§4).
                return Err(Error::validation(format!(
                    "destination {dest:?} already exists and is not a managed \
                     skill; pass --replace to overwrite it"
                )));
            }
            if scan.skills.iter().any(|s| s.path == dest) {
                ops.push(LibraryOp {
                    kind: LibraryOpKind::Replace,
                    path: dest,
                    from: None,
                    source: Some(skill.root.display().to_string()),
                    content: None,
                });
            } else {
                for (relative, absolute) in &skill.files {
                    ops.push(LibraryOp {
                        kind: LibraryOpKind::Write,
                        path: format!("{dest}/{relative}"),
                        from: None,
                        source: Some(absolute.display().to_string()),
                        content: None,
                    });
                }
            }
        }
        let names: Vec<&str> = incoming.iter().map(|s| s.name.as_str()).collect();
        let plan = LibraryPlan {
            message: format!("beskar: ingest {}", names.join(", ")),
            ops,
        };
        self.commit_plan(plan, request.dry_run)
    }

    // ---- skill operations (§73-§75, §80) ----------------------------------

    /// Moves a skill to another bucket (§73). Buckets never affect
    /// identity: `SKILL.md`, profile membership, and installations are
    /// untouched.
    pub fn skill_move(&self, skill: &str, bucket: &str, dry_run: bool) -> Result<LibraryOutcome> {
        crate::paths::validate_bucket(bucket)?;
        let name = SkillName::parse(skill)?;
        let scan = self.scan()?;
        let found = find_skill(&scan, &name)?;
        let leaf = crate::paths::split_leaf(&found.path).1;
        let dest = format!("{}/{bucket}/{leaf}", self.library.config().skills_dir);
        if dest == found.path {
            return Ok(LibraryOutcome {
                plan: LibraryPlan {
                    message: format!("beskar: move {leaf}"),
                    ops: vec![],
                },
                executed: false,
                commit: None,
                warnings: vec![],
            });
        }
        let dest_native = self
            .library
            .root()
            .join(crate::paths::to_native_path(&dest));
        if dest_native.symlink_metadata().is_ok() {
            return Err(Error::validation(format!(
                "destination {dest:?} already exists (§73 refuses to merge \
                 skill directories)"
            )));
        }
        let plan = LibraryPlan {
            message: format!("beskar: move {leaf}"),
            ops: vec![LibraryOp {
                kind: LibraryOpKind::Move,
                path: dest,
                from: Some(found.path.clone()),
                source: None,
                content: None,
            }],
        };
        self.commit_plan(plan, dry_run)
    }

    /// Renames a skill (§74): an atomic identity migration updating the
    /// directory leaf, `name:` in `SKILL.md` (a precise line edit, never
    /// regex-mangling), the catalog key, and every profile referencing the
    /// old name. The complete result re-validates before the commit (§74).
    pub fn skill_rename(&self, old: &str, new: &str, dry_run: bool) -> Result<LibraryOutcome> {
        let old_name = SkillName::parse(old)?;
        let new_name = SkillName::parse(new)?;
        if old_name == new_name {
            return Err(Error::validation(
                "the new skill name equals the old one; nothing to rename",
            ));
        }
        let scan = self.scan()?;
        let found = find_skill(&scan, &old_name)?;
        if scan.skills.iter().any(|s| s.name == new_name) {
            return Err(Error::validation(format!(
                "a skill named {new:?} already exists (§8.2)"
            )));
        }
        let (parent, _) = crate::paths::split_leaf(&found.path);
        let new_path = format!("{parent}/{}", new_name.as_str());

        // §74: edit `name:` properly, preserving all other bytes.
        let skill_md_relative = format!("{}/{}", found.path, SKILL_FILE);
        let raw = std::fs::read_to_string(
            self.library
                .root()
                .join(crate::paths::to_native_path(&skill_md_relative)),
        )?;
        let updated = edit_frontmatter_name(&raw, new_name.as_str())?;
        let verified = SkillFrontmatter::parse(&updated)?;
        if verified.name != new_name {
            return Err(Error::unsupported_state(
                "the edited SKILL.md does not carry the new name",
            ));
        }

        // Catalog key migration (§74); other entries are preserved.
        let mut catalog = scan.catalog.clone();
        let mut catalog_edited = false;
        if let Some(metadata) = catalog.skills.remove(old_name.as_str()) {
            catalog.skills.insert(new_name.to_string(), metadata);
            catalog_edited = true;
        }

        // Every profile referencing the old name (§74), preserving order.
        let mut profile_edits: Vec<(PathBuf, String)> = Vec::new();
        for profile in &scan.profiles {
            if !profile.skills.contains(&old_name) {
                continue;
            }
            let mut updated_profile = profile.clone();
            updated_profile.skills = profile
                .skills
                .iter()
                .map(|s| {
                    if *s == old_name {
                        new_name.clone()
                    } else {
                        s.clone()
                    }
                })
                .collect();
            let path = self.profile_file_path(&profile.name)?;
            let content = updated_profile.to_toml()?;
            profile_edits.push((path, content));
        }

        let mut ops = vec![LibraryOp {
            kind: LibraryOpKind::Move,
            path: new_path.clone(),
            from: Some(found.path.clone()),
            source: None,
            content: None,
        }];
        ops.push(LibraryOp {
            kind: LibraryOpKind::Edit,
            path: format!("{new_path}/{SKILL_FILE}"),
            from: None,
            source: None,
            content: Some(updated),
        });
        if catalog_edited {
            ops.push(LibraryOp {
                kind: LibraryOpKind::Edit,
                path: self.library.config().catalog_file.clone(),
                from: None,
                source: None,
                content: Some(catalog.to_toml()?),
            });
        }
        for (path, content) in profile_edits {
            ops.push(LibraryOp {
                kind: LibraryOpKind::Edit,
                path: self.library_relative(&path)?,
                from: None,
                source: None,
                content: Some(content),
            });
        }
        let plan = LibraryPlan {
            message: format!(
                "beskar: rename {} to {}",
                old_name.as_str(),
                new_name.as_str()
            ),
            ops,
        };
        self.commit_plan(plan, dry_run)
    }

    /// Removes a skill from the Library (§75). Default refuses while any
    /// library profile references the skill and lists them; `--cascade`
    /// removes the skill from all such profiles in the same commit. The
    /// result always warns that synchronizing may alter external
    /// installations (§75: Beskar cannot know remote Registry state).
    pub fn skill_remove(
        &self,
        skill: &str,
        cascade: bool,
        dry_run: bool,
    ) -> Result<SkillRemovalOutcome> {
        let name = SkillName::parse(skill)?;
        let scan = self.scan()?;
        let found = find_skill(&scan, &name)?;
        let referencing: Vec<String> = scan
            .profiles
            .iter()
            .filter(|profile| profile.skills.contains(&name))
            .map(|profile| profile.name.clone())
            .collect();
        if !referencing.is_empty() && !cascade {
            return Err(Error::validation(format!(
                "skill {:?} is referenced by profiles: {} — detach it there \
                 first or pass --cascade to remove it from all of them in the \
                 same commit (§75)",
                name.as_str(),
                referencing.join(", ")
            )));
        }
        let mut ops = vec![LibraryOp {
            kind: LibraryOpKind::Remove,
            path: found.path.clone(),
            from: None,
            source: None,
            content: None,
        }];
        for profile_name in &referencing {
            let profile = scan
                .profiles
                .iter()
                .find(|p| &p.name == profile_name)
                .expect("listed profile");
            let mut updated = profile.clone();
            updated.skills.retain(|s| *s != name);
            let path = self.profile_file_path(profile_name)?;
            ops.push(LibraryOp {
                kind: LibraryOpKind::Edit,
                path: self.library_relative(&path)?,
                from: None,
                source: None,
                content: Some(updated.to_toml()?),
            });
        }
        let leaf = crate::paths::split_leaf(&found.path).1;
        let plan = LibraryPlan {
            message: format!("beskar: remove skill {leaf}"),
            ops,
        };
        let warnings = vec![
            "synchronizing this removal may alter external installations that \
             installed this skill (§75: Beskar cannot know remote machines' \
             registry state)"
                .to_owned(),
        ];
        let outcome = self.finish(plan, dry_run, warnings)?;
        Ok(SkillRemovalOutcome {
            outcome,
            referencing_profiles: referencing,
        })
    }

    /// Adds and/or removes catalog tags for one skill (§14, §80).
    pub fn skill_tag(
        &self,
        skill: &str,
        add: &[String],
        remove: &[String],
        dry_run: bool,
    ) -> Result<LibraryOutcome> {
        for tag in add.iter().chain(remove.iter()) {
            validate_tag(tag)?;
        }
        let name = SkillName::parse(skill)?;
        let scan = self.scan()?;
        find_skill(&scan, &name)?;
        let mut catalog = scan.catalog.clone();
        {
            let entry = catalog.skills.entry(name.to_string()).or_default();
            for tag in add {
                if !entry.tags.contains(tag) {
                    entry.tags.push(tag.clone());
                }
            }
            if !remove.is_empty() {
                entry.tags.retain(|tag| !remove.contains(tag));
            }
        }
        let plan = self.catalog_plan(&name, catalog, &scan, "tag")?;
        self.commit_plan(plan, dry_run)
    }

    /// Sets or clears the catalog rank of one skill (§14, §80).
    pub fn skill_rank(
        &self,
        skill: &str,
        rank: Option<i64>,
        dry_run: bool,
    ) -> Result<LibraryOutcome> {
        let name = SkillName::parse(skill)?;
        let scan = self.scan()?;
        find_skill(&scan, &name)?;
        let mut catalog = scan.catalog.clone();
        let entry = catalog.skills.entry(name.to_string()).or_default();
        entry.rank = rank;
        let plan = self.catalog_plan(&name, catalog, &scan, "rank")?;
        self.commit_plan(plan, dry_run)
    }

    /// Lists skills with catalog metadata and profile references (§80).
    pub fn list_skills(&self, filter: SkillFilter<'_>) -> Result<Vec<SkillListing>> {
        let scan = self.scan()?;
        let mut listings = Vec::new();
        for skill in &scan.skills {
            if let Some(bucket) = filter.bucket
                && !bucket_of(&skill.path, &self.library.config().skills_dir).starts_with(bucket)
            {
                continue;
            }
            let metadata = scan.catalog.skills.get(skill.name.as_str());
            if let Some(tag) = filter.tag
                && !metadata.is_some_and(|m| m.tags.iter().any(|t| t == tag))
            {
                continue;
            }
            let profiles: Vec<String> = scan
                .profiles
                .iter()
                .filter(|profile| profile.skills.iter().any(|s| s == &skill.name))
                .map(|profile| profile.name.clone())
                .collect();
            if let Some(profile) = filter.profile
                && !profiles.iter().any(|p| p == profile)
            {
                continue;
            }
            let description = self.skill_description(&skill.path)?;
            if let Some(query) = filter.query
                && !description.to_lowercase().contains(&query.to_lowercase())
                && !skill.name.as_str().contains(&query.to_lowercase())
            {
                continue;
            }
            listings.push(SkillListing {
                name: skill.name.clone(),
                path: skill.path.clone(),
                bucket: bucket_of(&skill.path, &self.library.config().skills_dir),
                description,
                tags: metadata.map(|m| m.tags.clone()).unwrap_or_default(),
                rank: metadata.and_then(|m| m.rank),
                notes: metadata.and_then(|m| m.notes.clone()),
                profiles,
                last_commit: None,
            });
        }
        match filter.sort.unwrap_or_default() {
            SkillSort::Name => listings.sort_by(|a, b| a.name.cmp(&b.name)),
            SkillSort::Bucket => {
                listings.sort_by(|a, b| a.bucket.cmp(&b.bucket).then_with(|| a.name.cmp(&b.name)))
            }
            SkillSort::Rank => listings.sort_by(|a, b| {
                a.rank
                    .unwrap_or(i64::MAX)
                    .cmp(&b.rank.unwrap_or(i64::MAX))
                    .then_with(|| a.name.cmp(&b.name))
            }),
            // `recent` needs per-skill commit times from Git history (§80);
            // when HEAD is unborn the listing falls back to name order.
            SkillSort::Recent => {
                if let Ok(head) = self.backend.resolve_ref(self.library.root(), "HEAD") {
                    for listing in &mut listings {
                        listing.last_commit = self
                            .backend
                            .last_commit_info(self.library.root(), &head, &listing.path)
                            .ok()
                            .flatten();
                    }
                    listings.sort_by(|a, b| {
                        let key = |l: &SkillListing| {
                            l.last_commit
                                .as_ref()
                                .map(|info| (info.unix_time, info.hash.clone()))
                        };
                        // Reverse: most recent first.
                        key(b).cmp(&key(a)).then_with(|| a.name.cmp(&b.name))
                    });
                }
            }
        }
        Ok(listings)
    }

    /// Full detail for one skill (§69 `skill show`).
    pub fn show_skill(&self, skill: &str) -> Result<SkillDetail> {
        let name = SkillName::parse(skill)?;
        let scan = self.scan()?;
        let found = find_skill(&scan, &name)?;
        let mut listing = SkillListing {
            name: found.name.clone(),
            path: found.path.clone(),
            bucket: bucket_of(&found.path, &self.library.config().skills_dir),
            description: self.skill_description(&found.path)?,
            tags: vec![],
            rank: None,
            notes: None,
            profiles: scan
                .profiles
                .iter()
                .filter(|profile| profile.skills.iter().any(|s| s == &found.name))
                .map(|profile| profile.name.clone())
                .collect(),
            last_commit: None,
        };
        if let Some(metadata) = scan.catalog.skills.get(name.as_str()) {
            listing.tags = metadata.tags.clone();
            listing.rank = metadata.rank;
            listing.notes = metadata.notes.clone();
        }
        if let Ok(head) = self.backend.resolve_ref(self.library.root(), "HEAD") {
            listing.last_commit = self
                .backend
                .last_commit_info(self.library.root(), &head, &found.path)
                .ok()
                .flatten();
        }
        // File inventory from the working tree, `/`-separated (§119).
        let mut files = Vec::new();
        let native_root = self
            .library
            .root()
            .join(crate::paths::to_native_path(&found.path));
        for entry in walkdir::WalkDir::new(&native_root).follow_links(false) {
            let entry = entry.map_err(|e| Error::library(format!("walk failed: {e}")))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&native_root)
                .map_err(|_| Error::path_safety("skill file escaped its root"))?
                .to_str()
                .ok_or_else(|| Error::path_safety("non-UTF-8 skill file path"))?;
            files.push(relative.replace(std::path::MAIN_SEPARATOR, "/"));
        }
        files.sort();
        Ok(SkillDetail { listing, files })
    }

    // ---- profile operations (§76, §77) -------------------------------------

    /// Lists every profile in the working tree (§76).
    pub fn profile_list(&self) -> Result<Vec<Profile>> {
        Ok(self.scan()?.profiles)
    }

    /// Shows one profile plus the local installations currently attached to
    /// it (§76, §98).
    pub fn profile_show(&self, name: &str) -> Result<(Profile, Vec<InstallationRef>)> {
        let scan = self.scan()?;
        let profile = find_profile(&scan, name)?;
        let attached = self.store.load()?.installations;
        let refs = attached
            .iter()
            .filter(|installation| {
                installation
                    .profiles
                    .iter()
                    .any(|attachment| attachment.id == profile.id)
            })
            .map(|installation| InstallationRef {
                workspace: installation.workspace.clone(),
                target: installation.target.clone(),
                source_ref: installation.source_ref.clone(),
            })
            .collect();
        Ok((profile.clone(), refs))
    }

    /// Creates an empty profile (§76). The UUID is generated once and is
    /// immutable thereafter (§135.4).
    pub fn profile_create(
        &self,
        name: &str,
        description: Option<&str>,
        dry_run: bool,
    ) -> Result<LibraryOutcome> {
        validate_profile_name(name)?;
        let scan = self.scan()?;
        ensure_profile_name_free(&scan, name)?;
        let mut profile = Profile::new(name, vec![]);
        profile.description = description.map(str::to_owned);
        let relative = format!("{}/{}.toml", self.library.config().profiles_dir, name);
        let plan = LibraryPlan {
            message: format!("beskar: create profile {name}"),
            ops: vec![LibraryOp {
                kind: LibraryOpKind::Edit,
                path: relative,
                from: None,
                source: None,
                content: Some(profile.to_toml()?),
            }],
        };
        self.commit_plan(plan, dry_run)
    }

    /// Deletes a profile from the Library — distinct from detaching it from
    /// an installation (§77). Attached installations later classify the
    /// profile as missing-profile and protect its skills (§39), so the
    /// result warns when local installations still attach it.
    pub fn profile_delete(&self, name: &str, dry_run: bool) -> Result<LibraryOutcome> {
        let scan = self.scan()?;
        let profile = find_profile(&scan, name)?;
        let path = self.profile_file_path(&profile.name)?;
        let mut warnings = Vec::new();
        let attached: Vec<String> = self
            .store
            .load()?
            .installations
            .iter()
            .filter(|installation| {
                installation
                    .profiles
                    .iter()
                    .any(|attachment| attachment.id == profile.id)
            })
            .map(|installation| {
                format!(
                    "{} ({})",
                    installation.workspace.display(),
                    installation.target
                )
            })
            .collect();
        if !attached.is_empty() {
            warnings.push(format!(
                "local installation(s) still attach this profile: {} — they \
                 will classify it as missing_profile and protect its skills \
                 until explicitly detached (§77, §39)",
                attached.join(", ")
            ));
        }
        let plan = LibraryPlan {
            message: format!("beskar: delete profile {}", profile.name),
            ops: vec![LibraryOp {
                kind: LibraryOpKind::Remove,
                path: self.library_relative(&path)?,
                from: None,
                source: None,
                content: None,
            }],
        };
        self.finish(plan, dry_run, warnings)
    }

    /// Renames a profile, keeping its immutable UUID (§76, §135.23): the
    /// `name` field and the file name both change; attachments follow the
    /// ID and need no migration (§28).
    pub fn profile_rename(&self, old: &str, new: &str, dry_run: bool) -> Result<LibraryOutcome> {
        validate_profile_name(new)?;
        let scan = self.scan()?;
        let profile = find_profile(&scan, old)?;
        if profile.name == new {
            return Err(Error::validation(
                "the new profile name equals the old one; nothing to rename",
            ));
        }
        ensure_profile_name_free(&scan, new)?;
        let mut updated = profile.clone();
        updated.name = new.to_owned();
        let old_path = self.profile_file_path(&profile.name)?;
        let new_relative = format!("{}/{}.toml", self.library.config().profiles_dir, new);
        let plan = LibraryPlan {
            message: format!("beskar: rename profile {} to {new}", profile.name),
            ops: vec![
                LibraryOp {
                    kind: LibraryOpKind::Edit,
                    path: new_relative,
                    from: None,
                    source: None,
                    content: Some(updated.to_toml()?),
                },
                LibraryOp {
                    kind: LibraryOpKind::Remove,
                    path: self.library_relative(&old_path)?,
                    from: None,
                    source: None,
                    content: None,
                },
            ],
        };
        self.commit_plan(plan, dry_run)
    }

    /// Appends skills to a profile in the given order (§76, §16). Each
    /// skill must exist in the Library and not already be listed (§15).
    pub fn profile_add_skills(
        &self,
        profile: &str,
        skills: &[String],
        dry_run: bool,
    ) -> Result<LibraryOutcome> {
        if skills.is_empty() {
            return Err(Error::validation("name at least one skill to add"));
        }
        let scan = self.scan()?;
        let mut updated = find_profile(&scan, profile)?.clone();
        for skill in skills {
            let name = SkillName::parse(skill)?;
            find_skill(&scan, &name)?;
            if updated.skills.contains(&name) {
                return Err(Error::validation(format!(
                    "profile {:?} already lists skill {:?} (§15: duplicates are \
                     invalid)",
                    updated.name, skill
                )));
            }
            updated.skills.push(name);
        }
        self.profile_update_plan(&updated, "update", dry_run)
    }

    /// Removes skills from a profile (§76). Each named skill must be
    /// listed.
    pub fn profile_remove_skills(
        &self,
        profile: &str,
        skills: &[String],
        dry_run: bool,
    ) -> Result<LibraryOutcome> {
        if skills.is_empty() {
            return Err(Error::validation("name at least one skill to remove"));
        }
        let scan = self.scan()?;
        let mut updated = find_profile(&scan, profile)?.clone();
        for skill in skills {
            let name = SkillName::parse(skill)?;
            if !updated.skills.contains(&name) {
                return Err(Error::validation(format!(
                    "profile {:?} does not list skill {:?}",
                    updated.name, skill
                )));
            }
            updated.skills.retain(|s| *s != name);
        }
        self.profile_update_plan(&updated, "update", dry_run)
    }

    /// Moves one skill before/after another inside the profile's ordered
    /// list (§76, §16: order is meaningful for editing and presentation).
    pub fn profile_move_skill(
        &self,
        profile: &str,
        skill: &str,
        pivot: SkillPivot,
        dry_run: bool,
    ) -> Result<LibraryOutcome> {
        let scan = self.scan()?;
        let mut updated = find_profile(&scan, profile)?.clone();
        let name = SkillName::parse(skill)?;
        let position = updated
            .skills
            .iter()
            .position(|s| *s == name)
            .ok_or_else(|| {
                Error::validation(format!(
                    "profile {:?} does not list skill {:?}",
                    updated.name, skill
                ))
            })?;
        let (pivot_name, insert_at_left) = match &pivot {
            SkillPivot::Before(pivot) => (SkillName::parse(pivot)?, true),
            SkillPivot::After(pivot) => (SkillName::parse(pivot)?, false),
        };
        if pivot_name == name {
            return Err(Error::validation(
                "the pivot skill must differ from the moved skill",
            ));
        }
        if !updated.skills.contains(&pivot_name) {
            return Err(Error::validation(format!(
                "profile {:?} does not list pivot skill {:?}",
                updated.name, pivot_name
            )));
        }
        updated.skills.remove(position);
        // Recompute the pivot index after removal (shifts by one when the
        // moved skill sat before the pivot).
        let final_pivot = updated
            .skills
            .iter()
            .position(|s| *s == pivot_name)
            .expect("pivot unaffected by other-skill removal");
        let target = if insert_at_left {
            final_pivot
        } else {
            final_pivot + 1
        };
        updated.skills.insert(target, name);
        self.profile_update_plan(&updated, "reorder", dry_run)
    }

    /// Validates one profile or all of them (§76 `profile validate`). This
    /// is deliberately tolerant: invalid profiles are reported as problems,
    /// never as hard command failures of unrelated profiles.
    pub fn profile_validate(&self, name: Option<&str>) -> Result<Vec<ProfileValidation>> {
        let profiles = collect_profiles_tolerant(&self.library.profiles_dir())?;
        if let Some(name) = name {
            let chosen: Vec<_> = profiles
                .into_iter()
                .filter(|(parsed, _)| parsed.as_ref().is_ok_and(|p| p.name == name))
                .collect();
            if chosen.is_empty() {
                return Err(Error::profile(format!(
                    "profile {name:?} does not exist in the Library working tree"
                )));
            }
            return self.validate_profiles(chosen);
        }
        self.validate_profiles(profiles)
    }

    // ---- library status (§81) and branch helpers (§82) ----------------------

    /// The read-only `beskar library status` report (§81). Resolves
    /// everything from local refs — never the network (§8.8).
    pub fn library_status(&self) -> Result<LibraryStatusReport> {
        let status = self.backend.status(self.library.root())?;
        let dirty = status.is_dirty();
        let staged = status.staged.clone();
        let unstaged = status.unstaged.clone();
        let untracked = status.untracked.clone();
        let head = self.backend.resolve_ref(self.library.root(), "HEAD").ok();
        let branches = self.backend.list_branches(self.library.root())?;
        let current = status.branch.clone();
        let upstream = current.as_ref().and_then(|name| {
            branches
                .iter()
                .find(|branch| &branch.name == name)
                .and_then(|branch| branch.upstream.clone())
        });
        let (ahead, behind) = match (&current, &upstream) {
            (Some(branch), Some(upstream)) => {
                match self
                    .backend
                    .ahead_behind(self.library.root(), branch, upstream)
                {
                    Ok((ahead, behind)) => (Some(ahead), Some(behind)),
                    // The upstream may not exist locally until a fetch; that is
                    // informational (§81), not a failure.
                    Err(_) => (None, None),
                }
            }
            _ => (None, None),
        };
        let installations = self
            .store
            .load()?
            .installations
            .iter()
            .map(|installation| InstallationRef {
                workspace: installation.workspace.clone(),
                target: installation.target.clone(),
                source_ref: installation.source_ref.clone(),
            })
            .collect();
        Ok(LibraryStatusReport {
            path: self.library.root().to_path_buf(),
            library_id: self.library.config().library_id.to_string(),
            branch: status.branch,
            head,
            dirty,
            staged,
            unstaged,
            untracked,
            remote: self
                .backend
                .remote_url(self.library.root(), "origin")
                .unwrap_or(None),
            default_ref: self.library.config().default_ref.clone(),
            upstream,
            ahead,
            behind,
            installations,
        })
    }

    /// Lists local branches with ahead/behind against their upstreams
    /// (§82, §81).
    pub fn list_branches(&self) -> Result<Vec<BranchDisplay>> {
        let status = self.backend.status(self.library.root())?;
        let current = status.branch;
        let mut display = Vec::new();
        for branch in self.backend.list_branches(self.library.root())? {
            let (ahead, behind) = match (&branch.upstream, &current) {
                (Some(upstream), Some(name)) if &branch.name == name => self
                    .backend
                    .ahead_behind(self.library.root(), name, upstream)
                    .ok()
                    .map_or((None, None), |(a, b)| (Some(a), Some(b))),
                (Some(upstream), _) => self
                    .backend
                    .ahead_behind(self.library.root(), &branch.name, upstream)
                    .map_or((None, None), |(a, b)| (Some(a), Some(b))),
                (None, _) => (None, None),
            };
            let current_branch = current.as_ref().is_some_and(|name| *name == branch.name);
            display.push(BranchDisplay {
                name: branch.name,
                current: current_branch,
                upstream: branch.upstream,
                ahead,
                behind,
            });
        }
        display.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(display)
    }

    /// Creates a new branch at HEAD and switches to it (§82).
    pub fn create_branch(&self, name: &str) -> Result<()> {
        let _lock = self.library_lock()?;
        self.backend
            .create_branch(self.library.root(), name)
            .map_err(Error::from)
    }

    /// Switches the working tree to an existing branch (§82).
    pub fn switch_branch(&self, name: &str) -> Result<()> {
        let _lock = self.library_lock()?;
        self.backend
            .switch_branch(self.library.root(), name)
            .map_err(Error::from)
    }

    // ---- shared machinery ---------------------------------------------------

    /// Validated working-tree scan (§11, §12, §15).
    fn scan(&self) -> Result<LibraryScan> {
        self.library.scan()
    }

    /// The advisory library-mutation lock (§88). Dry-runs never lock.
    fn library_lock(&self) -> Result<std::fs::File> {
        crate::lock::lock_file_exclusive(&self.dirs.state_dir.join("library.lock"), "library")
    }

    /// The path of the profile file defining `name` (§76: file stem is the
    /// name in practice, but the actual file is located by content so
    /// hand-renamed files keep working).
    fn profile_file_path(&self, name: &str) -> Result<PathBuf> {
        for entry in std::fs::read_dir(self.library.profiles_dir())? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "toml") {
                continue;
            }
            let raw = std::fs::read_to_string(&path)?;
            if let Ok(profile) = Profile::parse_toml(&raw)
                && profile.name == name
            {
                return Ok(path);
            }
        }
        Err(Error::profile(format!(
            "no profile file defines {name:?} under {}",
            self.library.profiles_dir().display()
        )))
    }

    /// Converts an absolute path inside the Library to its Library-relative
    /// `/`-separated form (§119).
    fn library_relative(&self, absolute: &Path) -> Result<String> {
        let relative = absolute
            .strip_prefix(self.library.root())
            .map_err(|_| Error::path_safety("path escapes the library root"))?;
        let text = relative
            .to_str()
            .ok_or_else(|| Error::path_safety("non-UTF-8 library path"))?;
        validate_committed_path(text)?;
        Ok(text.replace(std::path::MAIN_SEPARATOR, "/"))
    }

    /// The frontmatter description of a working-tree skill (§11).
    fn skill_description(&self, skill_path: &str) -> Result<String> {
        let native = self
            .library
            .root()
            .join(crate::paths::to_native_path(&format!(
                "{skill_path}/{SKILL_FILE}"
            )));
        let raw = std::fs::read_to_string(native)?;
        Ok(SkillFrontmatter::parse(&raw)?.description)
    }

    /// Builds the single-edit catalog plan for tag/rank changes after
    /// verifying the skill exists (§14).
    fn catalog_plan(
        &self,
        name: &SkillName,
        mut catalog: crate::catalog::Catalog,
        scan: &LibraryScan,
        verb: &str,
    ) -> Result<LibraryPlan> {
        let _ = find_skill(scan, name)?;
        // An entry with no metadata left is dropped entirely.
        if let Some(entry) = catalog.skills.get(name.as_str())
            && entry.tags.is_empty()
            && entry.rank.is_none()
            && entry.notes.is_none()
        {
            catalog.skills.remove(name.as_str());
        }
        Ok(LibraryPlan {
            message: format!("beskar: {verb} {}", name.as_str()),
            ops: vec![LibraryOp {
                kind: LibraryOpKind::Edit,
                path: self.library.config().catalog_file.clone(),
                from: None,
                source: None,
                content: Some(catalog.to_toml()?),
            }],
        })
    }

    /// Builds the single-edit plan for a profile content change (§76).
    fn profile_update_plan(
        &self,
        profile: &Profile,
        verb: &str,
        dry_run: bool,
    ) -> Result<LibraryOutcome> {
        let relative = format!(
            "{}/{}.toml",
            self.library.config().profiles_dir,
            profile.name
        );
        let plan = LibraryPlan {
            message: format!("beskar: {verb} profile {}", profile.name),
            ops: vec![LibraryOp {
                kind: LibraryOpKind::Edit,
                path: relative,
                from: None,
                source: None,
                content: Some(profile.to_toml()?),
            }],
        };
        self.commit_plan(plan, dry_run)
    }

    fn validate_profiles(
        &self,
        profiles: Vec<(std::result::Result<Profile, String>, String)>,
    ) -> Result<Vec<ProfileValidation>> {
        let (skill_names, skill_problems) =
            collect_skill_names_tolerant(&self.library.skills_dir())?;
        let mut reports = Vec::new();
        for (parsed, file) in profiles {
            match parsed {
                Ok(profile) => {
                    let mut problems = Vec::new();
                    if let Err(err) = profile.validate() {
                        problems.push(err.to_string());
                    }
                    let missing: Vec<String> = profile
                        .skills
                        .iter()
                        .filter(|skill| !skill_names.contains(skill))
                        .map(|skill| skill.to_string())
                        .collect();
                    let mut problem_list = problems;
                    for problem in &skill_problems {
                        problem_list.push(format!("library: {problem}"));
                    }
                    reports.push(ProfileValidation {
                        profile: profile.name.clone(),
                        id: Some(profile.id),
                        valid: problem_list.is_empty() && missing.is_empty(),
                        problems: problem_list,
                        missing_skills: missing,
                    });
                }
                Err(message) => reports.push(ProfileValidation {
                    profile: file,
                    id: None,
                    valid: false,
                    problems: vec![message],
                    missing_skills: vec![],
                }),
            }
        }
        reports.sort_by(|a, b| a.profile.cmp(&b.profile));
        Ok(reports)
    }

    /// Plans-then-commits with the standard warnings-free path.
    fn commit_plan(&self, plan: LibraryPlan, dry_run: bool) -> Result<LibraryOutcome> {
        self.finish(plan, dry_run, vec![])
    }

    /// The shared §89 sequence for library mutations: plan (already built
    /// and validated), preflight the index (§72), then — unless this is a
    /// dry-run or no-op — apply the edits, re-validate the whole Library
    /// (§74), and create the scoped commit. Dry-runs perform no step after
    /// planning (§91).
    fn finish(
        &self,
        plan: LibraryPlan,
        dry_run: bool,
        warnings: Vec<String>,
    ) -> Result<LibraryOutcome> {
        if plan.is_no_op() {
            return Ok(LibraryOutcome {
                plan,
                executed: false,
                commit: None,
                warnings,
            });
        }
        let owned = plan.owned_paths();
        if dry_run {
            return Ok(LibraryOutcome {
                plan,
                executed: false,
                commit: None,
                warnings,
            });
        }
        let lock = self.library_lock()?;
        // §72: refuse early when unrelated staged files exist, before any
        // working-tree mutation. `commit_paths` re-checks at commit time.
        self.ensure_index_compatible(&owned)?;
        apply_ops(self.library.root(), &plan.ops)?;
        // §74: the complete result MUST validate before commit.
        if let Err(err) = self.scan() {
            return Err(Error::library(format!(
                "the library is invalid after applying the plan; the working \
                 tree holds uncommitted changes — inspect and revert them \
                 manually ({err})"
            )));
        }
        let commit = self
            .backend
            .commit_paths(self.library.root(), &plan.message, &owned)?;
        drop(lock);
        Ok(LibraryOutcome {
            plan,
            executed: true,
            commit,
            warnings,
        })
    }

    /// §72 preflight: every staged file must belong to the operation's
    /// owned paths.
    fn ensure_index_compatible(&self, owned: &[String]) -> Result<()> {
        let status = self.backend.status(self.library.root())?;
        for change in &status.staged {
            let related = owned.iter().any(|path| {
                change.path == *path
                    || change.path.starts_with(&format!("{path}/"))
                    || path.starts_with(&format!("{}/", change.path))
            });
            if !related {
                return Err(Error::git(format!(
                    "refusing to mutate the library: {:?} is staged but \
                     unrelated to this operation — commit or unstage it first \
                     (§72)",
                    change.path
                )));
            }
        }
        Ok(())
    }
}

// ---- incoming skill inspection (§70, §12, §11) ----------------------------

/// One validated incoming skill ready to be copied.
struct IncomingSkill {
    name: SkillName,
    /// Absolute source root.
    root: PathBuf,
    /// Skill-relative `/` paths of regular files to copy, with absolute
    /// sources. Stamp files are already stripped (§70).
    files: Vec<(String, PathBuf)>,
}

/// Finds skill roots under `source`: the directory itself when it directly
/// contains `SKILL.md`, or — with `recursive` — every directory directly
/// containing one (§71). Nested roots are ambiguous and rejected.
fn discover_skill_roots(source: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    if !source.is_dir() {
        return Err(Error::validation(format!(
            "ingest source {} is not a directory",
            source.display()
        )));
    }
    let direct = source.join(SKILL_FILE);
    if direct.is_file() {
        return Ok(vec![source.to_path_buf()]);
    }
    if !recursive {
        return Err(Error::validation(format!(
            "no {SKILL_FILE} found directly in {} — pass --recursive to \
             discover skill roots beneath it (§70, §71)",
            source.display()
        )));
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(source).follow_links(false) {
        let entry = entry.map_err(|e| Error::library(format!("walk failed: {e}")))?;
        if entry.file_type().is_file() && entry.file_name() == std::ffi::OsStr::new(SKILL_FILE) {
            let parent = entry
                .path()
                .parent()
                .ok_or_else(|| Error::path_safety("SKILL.md without a parent"))?;
            roots.push(parent.to_path_buf());
        }
    }
    roots.sort();
    for (index, root) in roots.iter().enumerate() {
        for other in &roots[index + 1..] {
            if other.starts_with(root) {
                return Err(Error::validation(format!(
                    "nested skill directories are ambiguous: {} lies inside {}",
                    other.display(),
                    root.display()
                )));
            }
        }
    }
    Ok(roots)
}

/// Validates one incoming skill root before anything is written (§70):
/// frontmatter parses, only regular files/directories appear (§12), and
/// stamp files are stripped from the copy set.
fn inspect_incoming_skill(root: PathBuf) -> Result<IncomingSkill> {
    let raw = std::fs::read_to_string(root.join(SKILL_FILE)).map_err(|e| {
        Error::validation(format!(
            "cannot read {}: {e}",
            root.join(SKILL_FILE).display()
        ))
    })?;
    let frontmatter = SkillFrontmatter::parse(&raw)?;
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(&root).follow_links(false) {
        let entry = entry.map_err(|e| Error::path_safety(format!("walk failed: {e}")))?;
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            return Err(Error::path_safety(format!(
                "symlinks are not allowed in ingested skills (§12): {}",
                entry.path().display()
            )));
        }
        if !file_type.is_file() && !file_type.is_dir() {
            return Err(Error::path_safety(format!(
                "special filesystem object rejected (§12): {}",
                entry.path().display()
            )));
        }
        if !file_type.is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&root)
            .map_err(|_| Error::path_safety("ingested file escaped its skill root"))?
            .to_str()
            .ok_or_else(|| Error::path_safety("non-UTF-8 ingested path"))?
            .replace(std::path::MAIN_SEPARATOR, "/");
        validate_committed_path(&relative)?;
        if STRIPPED_INGEST_FILES.contains(&relative.as_str()) {
            continue;
        }
        files.push((relative, entry.path().to_path_buf()));
    }
    files.sort();
    Ok(IncomingSkill {
        name: frontmatter.name,
        root,
        files,
    })
}

/// Validates a path destined for the Library index: `/`-separated relative
/// form with no traversal or Windows-hostile segments (§12, §119).
fn validate_committed_path(path: &str) -> Result<()> {
    crate::paths::validate_relative_path(path)
}

/// Applies planned edits to the Library working tree, in order.
fn apply_ops(root: &Path, ops: &[LibraryOp]) -> Result<()> {
    for op in ops {
        let native = root.join(crate::paths::to_native_path(&op.path));
        match op.kind {
            LibraryOpKind::Write => {
                let source = op
                    .source
                    .as_deref()
                    .ok_or_else(|| Error::unsupported_state("write op without a source"))?;
                copy_file_preserving_exec(Path::new(source), &native)?;
            }
            LibraryOpKind::Edit => {
                let content = op
                    .content
                    .as_deref()
                    .ok_or_else(|| Error::unsupported_state("edit op without content"))?;
                if let Some(parent) = native.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&native, content)?;
            }
            LibraryOpKind::Replace => {
                let source = op
                    .source
                    .as_deref()
                    .ok_or_else(|| Error::unsupported_state("replace op without a source"))?;
                if native.symlink_metadata().is_ok() {
                    std::fs::remove_dir_all(&native)?;
                }
                copy_tree_preserving_exec(Path::new(source), &native)?;
            }
            LibraryOpKind::Remove => {
                if native.is_dir() {
                    std::fs::remove_dir_all(&native)?;
                } else if native.symlink_metadata().is_ok() {
                    std::fs::remove_file(&native)?;
                }
            }
            LibraryOpKind::Move => {
                let from = op
                    .from
                    .as_deref()
                    .ok_or_else(|| Error::unsupported_state("move op without an origin"))?;
                let from_native = root.join(crate::paths::to_native_path(from));
                if let Some(parent) = native.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::rename(&from_native, &native)?;
            }
        }
    }
    Ok(())
}

/// Copies one file, preserving the executable bit where the platform
/// supports it (§70, §34; `fs::copy` copies permission bits on POSIX).
fn copy_file_preserving_exec(source: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, dest)?;
    #[cfg(unix)]
    {
        let metadata = std::fs::metadata(source)?;
        let mut permissions = std::fs::metadata(dest)?.permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(metadata.permissions().mode());
        std::fs::set_permissions(dest, permissions)?;
    }
    Ok(())
}

/// Copies a whole directory tree, preserving executable bits, skipping the
/// recognized stamp files (§70, §124).
fn copy_tree_preserving_exec(source: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in walkdir::WalkDir::new(source).follow_links(false) {
        let entry = entry.map_err(|e| Error::path_safety(format!("walk failed: {e}")))?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|_| Error::path_safety("copy escaped its root"))?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            let name = entry.file_name().to_str().unwrap_or_default();
            if STRIPPED_INGEST_FILES.contains(&name) {
                continue;
            }
            copy_file_preserving_exec(entry.path(), &target)?;
        } else {
            return Err(Error::path_safety(format!(
                "special filesystem object rejected (§12): {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

// ---- SKILL.md frontmatter editing (§74) ------------------------------------

/// Rewrites ONLY the top-level `name:` value inside the YAML frontmatter,
/// preserving every other byte of the file (§74: no regex-only YAML
/// mutation that could corrupt formatting; unknown fields are preserved
/// because the rest of the file is untouched, §11).
///
/// Fails closed when the frontmatter has no unambiguous top-level `name:`
/// line (e.g. flow-style mappings) instead of guessing.
fn edit_frontmatter_name(raw: &str, new_name: &str) -> Result<String> {
    let mut lines: Vec<String> = raw.split_inclusive('\n').map(str::to_owned).collect();
    let first = lines
        .first()
        .map(String::as_str)
        .map(str::trim_end)
        .ok_or_else(|| Error::validation("SKILL.md is empty"))?;
    if first != "---" {
        return Err(Error::validation(
            "SKILL.md does not start with a `---` frontmatter marker",
        ));
    }
    let close = lines[1..]
        .iter()
        .position(|line| line.trim_end() == "---")
        .map(|position| position + 1)
        .ok_or_else(|| Error::validation("SKILL.md frontmatter has no closing `---` marker"))?;
    let mut matches: Vec<usize> = lines[1..close]
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            line.starts_with("name")
                && line["name".len()..]
                    .trim_start_matches([' ', '\t'])
                    .starts_with(':')
        })
        .map(|(index, _)| index + 1)
        .collect();
    if matches.len() != 1 {
        return Err(Error::validation(format!(
            "SKILL.md frontmatter must contain exactly one top-level `name:` \
             line to be safely edited (found {}); edit it manually to rename",
            matches.len()
        )));
    }
    let index = matches.remove(0);
    let original = &lines[index];
    // Preserve the original line terminator, replace only the value.
    let line_end = original.trim_end_matches(['\n', '\r']);
    let newline = &original[line_end.len()..];
    lines[index] = format!("name: {new_name}{newline}");
    let updated: String = lines.concat();
    // The edited file must parse to the new name before it is used (§74).
    let verified = SkillFrontmatter::parse(&updated)?;
    if verified.name.as_str() != new_name {
        return Err(Error::unsupported_state(
            "the edited SKILL.md does not carry the new name",
        ));
    }
    Ok(updated)
}

// ---- tolerant collection for validate/doctor -------------------------------

/// Parses every `.toml` under the profiles dir, tolerating failures:
/// returns (result, file path) pairs preserving parse errors as messages.
pub(crate) fn collect_profiles_tolerant(
    profiles_dir: &Path,
) -> Result<Vec<(std::result::Result<Profile, String>, String)>> {
    if !profiles_dir.is_dir() {
        return Err(Error::library(format!(
            "profiles directory {} does not exist",
            profiles_dir.display()
        )));
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(profiles_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|e| e == "toml") {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files
        .into_iter()
        .map(|path| {
            let display = path.display().to_string();
            match std::fs::read_to_string(&path) {
                Ok(raw) => (
                    Profile::parse_toml(&raw).map_err(|e| e.to_string()),
                    display,
                ),
                Err(e) => (Err(e.to_string()), display),
            }
        })
        .collect())
}

/// Collects the set of valid skill names from the working tree, tolerating
/// invalid layouts (for `profile validate` and `doctor`, which report
/// problems instead of failing fast).
pub(crate) fn collect_skill_names_tolerant(
    skills_dir: &Path,
) -> Result<(BTreeSet<SkillName>, Vec<String>)> {
    if !skills_dir.is_dir() {
        return Err(Error::library(format!(
            "skills directory {} does not exist",
            skills_dir.display()
        )));
    }
    let mut names = BTreeSet::new();
    let mut problems = Vec::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(skills_dir).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                problems.push(format!("walk failed: {e}"));
                continue;
            }
        };
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            problems.push(format!(
                "symlink is not allowed in the library (§12): {}",
                entry.path().display()
            ));
            continue;
        }
        if !file_type.is_file() && !file_type.is_dir() {
            problems.push(format!(
                "special filesystem object rejected (§12): {}",
                entry.path().display()
            ));
            continue;
        }
        if file_type.is_file() && entry.file_name() == std::ffi::OsStr::new(SKILL_FILE) {
            roots.push(
                entry
                    .path()
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| entry.path().to_path_buf()),
            );
        }
    }
    for root in &roots {
        let raw = match std::fs::read_to_string(root.join(SKILL_FILE)) {
            Ok(raw) => raw,
            Err(e) => {
                problems.push(format!(
                    "cannot read {}: {e}",
                    root.join(SKILL_FILE).display()
                ));
                continue;
            }
        };
        match SkillFrontmatter::parse(&raw) {
            Ok(frontmatter) => {
                let leaf = root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                if leaf != frontmatter.name.as_str() {
                    problems.push(format!(
                        "directory leaf {leaf:?} does not equal skill name {:?}",
                        frontmatter.name.as_str()
                    ));
                }
                if !names.insert(frontmatter.name.clone()) {
                    problems.push(format!(
                        "duplicate skill name {:?} (§8.2)",
                        frontmatter.name.as_str()
                    ));
                }
            }
            Err(err) => problems.push(format!("{}: {err}", root.join(SKILL_FILE).display())),
        }
    }
    Ok((names, problems))
}

// ---- lookups ---------------------------------------------------------------

fn find_skill<'a>(scan: &'a LibraryScan, name: &SkillName) -> Result<&'a Skill> {
    scan.skills
        .iter()
        .find(|skill| &skill.name == name)
        .ok_or_else(|| {
            Error::validation(format!(
                "no skill named {:?} exists in the library working tree",
                name.as_str()
            ))
        })
}

fn find_profile<'a>(scan: &'a LibraryScan, name: &str) -> Result<&'a Profile> {
    scan.profiles
        .iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| {
            Error::profile(format!(
                "no profile named {name:?} exists in the library working tree"
            ))
        })
}

fn ensure_profile_name_free(scan: &LibraryScan, name: &str) -> Result<()> {
    if scan.profiles.iter().any(|profile| profile.name == name) {
        return Err(Error::profile(format!(
            "a profile named {name:?} already exists (§15: profile names are \
             unique within one Library revision)"
        )));
    }
    let folded = name.to_lowercase();
    if scan
        .profiles
        .iter()
        .any(|profile| profile.name.to_lowercase() == folded)
    {
        return Err(Error::profile(format!(
            "a profile named {name:?} collides case-insensitively with an \
             existing profile; case-insensitive filesystems cannot hold both"
        )));
    }
    Ok(())
}

/// The bucket portion of a library-relative skill root (§13).
fn bucket_of(skill_path: &str, skills_dir: &str) -> String {
    let (parent, _) = crate::paths::split_leaf(skill_path);
    parent
        .strip_prefix(&format!("{skills_dir}/"))
        .unwrap_or_default()
        .to_owned()
}

/// Validates a catalog tag string: non-empty, printable, no separators.
fn validate_tag(tag: &str) -> Result<()> {
    if tag.is_empty() {
        return Err(Error::validation("tag must not be empty"));
    }
    if tag.chars().any(char::is_control) {
        return Err(Error::validation("tag must not contain control characters"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_name_edit_preserves_every_other_byte() {
        // Quoted value, unusual spacing, CRLF body, unknown fields: only the
        // name value may change (§74: no regex-mangling; §11: unknown fields
        // preserved).
        let raw = "---\nname: \"old-name\"  \ndescription: D\nlicense: MIT\n---\r\n\r\n# Body\r\ncode: `x`\r\n";
        let updated = edit_frontmatter_name(raw, "new-name").expect("edit");
        assert!(updated.starts_with("---\nname: new-name\ndescription: D"));
        assert!(updated.contains("license: MIT"));
        assert!(updated.contains("# Body\r\ncode: `x`\r\n"));
        assert!(updated.ends_with("---\r\n\r\n# Body\r\ncode: `x`\r\n"));
        assert_eq!(
            SkillFrontmatter::parse(&updated)
                .expect("valid")
                .name
                .as_str(),
            "new-name"
        );
    }

    #[test]
    fn frontmatter_name_edit_handles_file_without_trailing_newline() {
        let raw = "---\ndescription: D\nname: old\n---";
        let updated = edit_frontmatter_name(raw, "renamed").expect("edit");
        assert_eq!(updated, "---\ndescription: D\nname: renamed\n---");
    }

    #[test]
    fn frontmatter_name_edit_fails_closed_on_ambiguous_layouts() {
        // No top-level name line (flow style).
        assert!(edit_frontmatter_name("---\n{name: a, description: b}\n---\n", "c").is_err());
        // No frontmatter at all.
        assert!(edit_frontmatter_name("just text", "c").is_err());
        // Unclosed frontmatter.
        assert!(edit_frontmatter_name("---\nname: a\n", "c").is_err());
        // Ambiguous duplicate keys are never guessed at.
        assert!(edit_frontmatter_name("---\nname: a\nname: b\n---\n", "c").is_err());
        // Indented (nested) name lines are not top-level keys.
        assert!(edit_frontmatter_name("---\nmetadata:\n  name: a\n---\n", "c").is_err());
    }

    #[test]
    fn ingest_destination_paths_come_from_frontmatter_not_source_leaf() {
        // The source directory name never constrains identity (§5: the
        // frontmatter name is canonical; the destination leaf follows it).
        let root = beskar_test_support::TempRoot::new();
        let source = root.child("random-dir-name");
        std::fs::create_dir_all(&source).expect("source");
        std::fs::write(
            source.join(SKILL_FILE),
            "---\nname: proper-name\ndescription: D\n---\n",
        )
        .expect("write");
        assert_eq!(
            inspect_incoming_skill(source).expect("valid").name.as_str(),
            "proper-name"
        );
    }
}

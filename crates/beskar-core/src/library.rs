//! Library discovery, structure validation, and revision resolution
//! (spec §5, §8-§11, §86, §107).
//!
//! A Library is the user-level Git repository containing skills, profiles,
//! and catalog metadata. Discovery walks up from a start directory to the
//! nearest `beskar.toml`; `BESKAR_LIBRARY` overrides the active Library
//! path entirely (§86). Committed state is authoritative for operations
//! (§8.1): [`resolve_revision`] reads exact committed bytes through the
//! [`GitBackend`] abstraction, never the working tree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use beskar_git::{GitBackend, TreeEntryKind};

use crate::catalog::Catalog;
use crate::config::LibraryConfig;
use crate::ids::{ProfileId, SkillName};
use crate::paths::{split_leaf, validate_bucket, validate_relative_path};
use crate::profile::Profile;
use crate::skill::{SKILL_FILE, Skill, SkillFrontmatter};

/// The Library configuration file name (spec §9, §10).
pub const CONFIG_FILE: &str = "beskar.toml";

/// A discovered Beskar Library: its root path plus parsed `beskar.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    root: PathBuf,
    config: LibraryConfig,
}

impl Library {
    /// Opens the Library rooted exactly at `root`, failing closed on a
    /// missing or unsupported `beskar.toml` (§10, §129).
    pub fn open_at(root: impl Into<PathBuf>) -> crate::Result<Self> {
        let root = root.into();
        let config_path = root.join(CONFIG_FILE);
        let raw = std::fs::read_to_string(&config_path).map_err(|e| {
            crate::Error::config(format!("cannot read {}: {e}", config_path.display()))
        })?;
        let config = LibraryConfig::parse_toml(&raw)?;
        Ok(Self { root, config })
    }

    /// Discovers the active Library (spec §86): `BESKAR_LIBRARY` wins when
    /// set; otherwise the nearest `beskar.toml` at-or-above `start` is used.
    pub fn discover(start: &Path) -> crate::Result<Self> {
        Self::discover_from(
            start,
            std::env::var_os("BESKAR_LIBRARY").as_deref().map(Path::new),
        )
    }

    /// Dependency-injected variant of [`Library::discover`]; tests pass the
    /// override explicitly instead of mutating process environment (§86).
    pub fn discover_from(start: &Path, library_override: Option<&Path>) -> crate::Result<Self> {
        if let Some(path) = library_override {
            // An explicit override must point at a valid Library — no silent
            // fallback (§86: environment takes precedence).
            return Self::open_at(path);
        }
        let start = start
            .canonicalize()
            .map_err(|e| crate::Error::config(format!("cannot access {}: {e}", start.display())))?;
        let mut current: Option<&Path> = Some(&start);
        while let Some(dir) = current {
            if dir.join(CONFIG_FILE).is_file() {
                return Self::open_at(dir);
            }
            current = dir.parent();
        }
        Err(crate::Error::config(format!(
            "no Beskar Library (beskar.toml) found at or above {}",
            start.display()
        )))
    }

    /// The Library root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The parsed `beskar.toml` (§10).
    pub fn config(&self) -> &LibraryConfig {
        &self.config
    }

    /// Absolute path of the skills directory (§9).
    pub fn skills_dir(&self) -> PathBuf {
        self.root.join(&self.config.skills_dir)
    }

    /// Absolute path of the profiles directory (§9, §15).
    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join(&self.config.profiles_dir)
    }

    /// Absolute path of the catalog file (§9, §14).
    pub fn catalog_path(&self) -> PathBuf {
        self.root.join(&self.config.catalog_file)
    }

    /// Scans the Library working tree (spec §107 "Library scanning").
    ///
    /// Validates every skill (frontmatter, naming, leaf equality, global
    /// uniqueness — §11) and every profile (schema, unique names and IDs,
    /// §15). Symlinks and special filesystem objects are rejected (§12);
    /// bucket segments must be portable (§13). The filesystem remains
    /// authoritative for bucket placement; catalog metadata is never
    /// consulted for identity (§14).
    pub fn scan(&self) -> crate::Result<LibraryScan> {
        let skills = scan_skills_dir(&self.skills_dir(), &self.config.skills_dir)?;
        let profiles = scan_profiles_dir(&self.profiles_dir())?;
        let catalog = match std::fs::read_to_string(self.catalog_path()) {
            Ok(raw) => Catalog::parse_toml(&raw)?,
            // Catalog metadata is optional (§14).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Catalog::new(),
            Err(e) => return Err(crate::Error::Io(e)),
        };
        Ok(LibraryScan {
            skills,
            profiles,
            catalog,
        })
    }
}

/// The result of scanning one Library revision or working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryScan {
    /// Validated skills, ordered by canonical name (§11: names are globally
    /// unique; buckets never appear in identity).
    pub skills: Vec<Skill>,
    /// Validated profiles, ordered by name (§15).
    pub profiles: Vec<Profile>,
    /// Parsed catalog metadata (§14); empty when the file is absent.
    pub catalog: Catalog,
}

/// One skill inside a resolved Library revision (spec §5, §35).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSkill {
    /// Canonical identity (§5).
    pub name: SkillName,
    /// Library-relative skill root path, `/`-separated (§119).
    pub path: String,
    /// Frontmatter description (§11).
    pub description: String,
    /// Most recent commit at-or-before the resolved ref touching this skill
    /// directory; MAY differ from the library commit (§35).
    pub skill_commit: String,
    /// Every committed file of the skill, keyed by skill-relative
    /// `/`-separated path (§119). `SKILL.md` included.
    pub files: BTreeMap<String, ResolvedFile>,
}

/// One committed file of a resolved skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFile {
    /// Library-relative path of the file, `/`-separated (§119).
    pub path: String,
    /// POSIX executable mode as committed (§34).
    pub executable: bool,
}

/// One Library revision resolved from exact committed Git objects
/// (spec §8.1, §19): a consistent snapshot of skills, profiles, and catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLibrary {
    /// The exact commit this snapshot was read from (§19, §35).
    pub commit: String,
    /// Skills keyed by canonical name (globally unique per revision, §11).
    pub skills: BTreeMap<SkillName, ResolvedSkill>,
    /// Profiles keyed by immutable ID (§15).
    pub profiles: BTreeMap<ProfileId, Profile>,
    /// Profiles indexed by (case-sensitive) name for name-based lookups.
    pub profile_by_name: BTreeMap<String, ProfileId>,
    /// Catalog metadata; empty when the file is absent in the revision (§14).
    pub catalog: Catalog,
}

/// Resolves one Library revision from exact committed Git objects
/// (spec §8.1, §19, §107).
///
/// All content is read through the [`GitBackend`] abstraction at the exact
/// `commit`; the working tree is never consulted. Fails closed on invalid
/// structure: duplicate skill names (§8.2), leaf/name mismatch (§5), nested
/// skills, unsafe bucket segments (§13), duplicate profile names/IDs (§15).
pub fn resolve_revision(
    backend: &dyn GitBackend,
    library: &Library,
    commit: &str,
) -> crate::Result<ResolvedLibrary> {
    let config = library.config();

    // Verify the revision's structure before reading content (§9): the
    // declared skills and profiles directories must exist as trees.
    let root_tree = backend.tree(library.root(), commit, "")?;
    for required in [&config.skills_dir, &config.profiles_dir] {
        match root_tree.iter().find(|entry| entry.path == *required) {
            Some(entry) if entry.kind == TreeEntryKind::Tree => {}
            Some(entry) => {
                return Err(crate::Error::library(format!(
                    "{} is not a directory in revision {commit}",
                    entry.path
                )));
            }
            None => {
                return Err(crate::Error::library(format!(
                    "revision {commit} has no {:?} directory",
                    required
                )));
            }
        }
    }

    let skills = resolve_skills(backend, library.root(), commit, &config.skills_dir)?;

    let profiles_dir = &config.profiles_dir;
    let profile_entries = backend.tree(library.root(), commit, profiles_dir)?;
    let mut profiles: BTreeMap<ProfileId, Profile> = BTreeMap::new();
    let mut profile_by_name = BTreeMap::new();
    let mut profiles_by_name_folded: BTreeMap<String, ProfileId> = BTreeMap::new();
    for entry in profile_entries {
        if entry.kind != TreeEntryKind::Blob || !entry.path.ends_with(".toml") {
            continue;
        }
        let raw = backend.blob(library.root(), commit, &entry.path)?;
        let raw = std::str::from_utf8(&raw)
            .map_err(|e| crate::Error::profile(format!("{} is not UTF-8: {e}", entry.path)))?;
        let profile = Profile::parse_toml(raw)?;
        if let Some(existing) = profiles.get(&profile.id) {
            return Err(crate::Error::profile(format!(
                "profile id {} is used by both {:?} and {:?}",
                profile.id, existing.name, profile.name
            )));
        }
        let folded = profile.name.to_lowercase();
        if let Some(existing) = profiles_by_name_folded.get(&folded) {
            return Err(crate::Error::profile(format!(
                "profile names {:?} and {:?} collide (case-insensitive filesystems \
                 cannot hold both)",
                profiles.get(existing).expect("recorded").name,
                profile.name
            )));
        }
        profiles_by_name_folded.insert(folded, profile.id);
        profile_by_name.insert(profile.name.clone(), profile.id);
        profiles.insert(profile.id, profile);
    }

    // Catalog is optional (§14): present → parse, absent → empty.
    let catalog = match root_tree
        .iter()
        .find(|entry| entry.kind == TreeEntryKind::Blob && entry.path == config.catalog_file)
    {
        Some(_) => {
            let raw = backend.blob(library.root(), commit, &config.catalog_file)?;
            let raw = std::str::from_utf8(&raw).map_err(|e| {
                crate::Error::schema(format!("{} is not UTF-8: {e}", config.catalog_file))
            })?;
            Catalog::parse_toml(raw)?
        }
        None => Catalog::new(),
    };

    Ok(ResolvedLibrary {
        commit: commit.to_owned(),
        skills,
        profiles,
        profile_by_name,
        catalog,
    })
}

/// Resolves and validates every skill in a revision under `skills_dir`.
fn resolve_skills(
    backend: &dyn GitBackend,
    repo: &Path,
    commit: &str,
    skills_dir: &str,
) -> crate::Result<BTreeMap<SkillName, ResolvedSkill>> {
    let entries = backend
        .tree_recursive(repo, commit, skills_dir)
        .map_err(|e| {
            crate::Error::library(format!(
                "revision {commit} has no readable skills directory {skills_dir:?}: {e}"
            ))
        })?;
    for entry in &entries {
        if entry.kind == TreeEntryKind::Gitlink {
            return Err(crate::Error::path_safety(format!(
                "submodule {} is not supported inside the skills directory (§12)",
                entry.path
            )));
        }
    }

    // Locate skill roots: directories owning a directly-contained SKILL.md.
    let mut roots: Vec<String> = Vec::new();
    for entry in &entries {
        if entry.kind != TreeEntryKind::Blob || !entry.path.ends_with(SKILL_FILE) {
            continue;
        }
        let root = match entry.path.strip_suffix(SKILL_FILE) {
            Some(r) => r.trim_end_matches('/').to_owned(),
            None => continue,
        };
        if root == skills_dir || !root.starts_with(&format!("{skills_dir}/")) {
            return Err(crate::Error::validation(format!(
                "SKILL.md directly inside the skills directory is not a valid \
                 skill layout: {}",
                entry.path
            )));
        }
        roots.push(root);
    }
    reject_nested_roots(&roots)?;

    let mut skills = BTreeMap::new();
    for root in &roots {
        let (parent, leaf) = split_leaf(root);
        let bucket = parent
            .strip_prefix(&format!("{skills_dir}/"))
            .unwrap_or_default();
        // Buckets are organizational only, but committed bucket segments
        // must still be portable (§13).
        validate_bucket(bucket)?;

        let skill_file = format!("{root}/{SKILL_FILE}");
        let bytes = backend.blob(repo, commit, &skill_file)?;
        let frontmatter = SkillFrontmatter::parse_bytes(&bytes)?;
        if leaf != frontmatter.name.as_str() {
            return Err(crate::Error::validation(format!(
                "directory leaf {leaf:?} does not equal skill name {:?}",
                frontmatter.name.as_str()
            )));
        }

        let skill_commit = backend
            .last_commit_touching(repo, commit, root)?
            .unwrap_or_else(|| commit.to_owned());

        let mut files = BTreeMap::new();
        let root_prefix = format!("{root}/");
        for entry in entries
            .iter()
            .filter(|e| e.kind == TreeEntryKind::Blob && e.path.starts_with(&root_prefix))
        {
            let relative = entry.path[root_prefix.len()..].to_owned();
            validate_relative_path(&relative)?;
            files.insert(
                relative,
                ResolvedFile {
                    path: entry.path.clone(),
                    executable: entry.is_executable(),
                },
            );
        }

        let resolved = ResolvedSkill {
            name: frontmatter.name.clone(),
            path: root.clone(),
            description: frontmatter.description,
            skill_commit,
            files,
        };
        if skills.insert(frontmatter.name, resolved).is_some() {
            return Err(crate::Error::validation(format!(
                "duplicate skill name {leaf:?}: skill names are globally unique \
                 within one Library revision regardless of bucket (§8.2)"
            )));
        }
    }
    Ok(skills)
}

/// Fails when one skill root is nested inside another (ambiguous layout).
fn reject_nested_roots(roots: &[String]) -> crate::Result<()> {
    let mut sorted: Vec<&str> = roots.iter().map(String::as_str).collect();
    sorted.sort();
    for (i, root) in sorted.iter().enumerate() {
        for other in &sorted[i + 1..] {
            if other.starts_with(&format!("{root}/")) {
                return Err(crate::Error::validation(format!(
                    "nested skill directories are ambiguous: {other} lies inside {root}"
                )));
            }
        }
    }
    Ok(())
}

/// Filesystem working-tree scan of the skills directory (§12, §13, §11).
fn scan_skills_dir(skills_path: &Path, skills_dir_name: &str) -> crate::Result<Vec<Skill>> {
    if !skills_path.is_dir() {
        return Err(crate::Error::library(format!(
            "skills directory {} does not exist",
            skills_path.display()
        )));
    }
    let mut roots: Vec<String> = Vec::new();
    for entry in walkdir::WalkDir::new(skills_path).follow_links(false) {
        let entry = entry.map_err(|e| crate::Error::library(format!("scan failed: {e}")))?;
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            return Err(crate::Error::path_safety(format!(
                "symlinks are not allowed inside the Library: {}",
                entry.path().display()
            )));
        }
        // Regular directories are walked; anything that is neither a regular
        // file nor a directory (socket, fifo, device) is rejected (§12).
        if !file_type.is_file() && !file_type.is_dir() {
            return Err(crate::Error::path_safety(format!(
                "special filesystem object is not allowed: {}",
                entry.path().display()
            )));
        }
        if !file_type.is_file() || entry.file_name() != std::ffi::OsStr::new(SKILL_FILE) {
            continue;
        }
        let parent = entry
            .path()
            .parent()
            .ok_or_else(|| crate::Error::path_safety("SKILL.md without a parent"))?;
        if parent == skills_path {
            return Err(crate::Error::validation(format!(
                "SKILL.md directly inside the skills directory is not a valid \
                 skill layout: {}",
                entry.path().display()
            )));
        }
        let relative = crate::paths::to_slash_path(
            parent
                .strip_prefix(skills_path)
                .map_err(|_| crate::Error::path_safety("skill root escaped skills dir"))?,
        )?;
        roots.push(format!("{skills_dir_name}/{relative}"));
    }
    reject_nested_roots(&roots)?;

    let mut skills = Vec::new();
    let mut seen: BTreeMap<SkillName, String> = BTreeMap::new();
    for root in &roots {
        let (parent, leaf) = split_leaf(root);
        let bucket_start = format!("{skills_dir_name}/");
        let bucket = parent
            .strip_prefix(&bucket_start)
            .unwrap_or_default()
            .to_owned();
        validate_bucket(&bucket)?;

        let skill_path = skills_path
            .join(crate::paths::to_native_path(&root[bucket_start.len()..]))
            .join(SKILL_FILE);
        let bytes = std::fs::read(&skill_path)?;
        let frontmatter = SkillFrontmatter::parse_bytes(&bytes)?;
        if leaf != frontmatter.name.as_str() {
            return Err(crate::Error::validation(format!(
                "directory leaf {leaf:?} does not equal skill name {:?}",
                frontmatter.name.as_str()
            )));
        }
        let skill = Skill {
            name: frontmatter.name.clone(),
            path: root.clone(),
        };
        if let Some(existing) = seen.get(&skill.name) {
            return Err(crate::Error::validation(format!(
                "duplicate skill name {:?} at {existing:?} and {:?} (§8.2)",
                skill.name.as_str(),
                skill.path
            )));
        }
        seen.insert(skill.name.clone(), skill.path.clone());
        skills.push(skill);
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

/// Filesystem working-tree scan of the profiles directory (§15).
fn scan_profiles_dir(profiles_path: &Path) -> crate::Result<Vec<Profile>> {
    if !profiles_path.is_dir() {
        return Err(crate::Error::library(format!(
            "profiles directory {} does not exist",
            profiles_path.display()
        )));
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(profiles_path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(crate::Error::path_safety(format!(
                "symlinks are not allowed inside the Library: {}",
                entry.path().display()
            )));
        }
        if file_type.is_file() && entry.path().extension().is_some_and(|e| e == "toml") {
            files.push(entry.path());
        }
    }
    files.sort();

    let mut profiles = Vec::new();
    let mut ids: BTreeMap<ProfileId, String> = BTreeMap::new();
    let mut names_folded: BTreeMap<String, String> = BTreeMap::new();
    for path in files {
        let raw = std::fs::read_to_string(&path)?;
        let profile = Profile::parse_toml(&raw)?;
        if let Some(existing) = ids.get(&profile.id) {
            return Err(crate::Error::profile(format!(
                "profile id {} is used by both {existing:?} and {:?}",
                profile.id, profile.name
            )));
        }
        let folded = profile.name.to_lowercase();
        if let Some(existing) = names_folded.get(&folded) {
            return Err(crate::Error::profile(format!(
                "profile names {existing:?} and {:?} collide (case-insensitive \
                 filesystems cannot hold both)",
                profile.name
            )));
        }
        names_folded.insert(folded, profile.name.clone());
        ids.insert(profile.id, profile.name.clone());
        profiles.push(profile);
    }
    Ok(profiles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use beskar_git::SystemGitBackend;
    use beskar_test_support::TempRoot;
    use beskar_test_support::fs::write_file;
    use beskar_test_support::git::{TestRepo, git_ok};

    fn config_toml(library_id: &str) -> String {
        format!("schema = 1\nlibrary_id = {library_id:?}\n")
    }

    /// Writes a complete valid Library layout into `root` (uncommitted).
    fn write_library(root: &Path, library_id: &str) {
        write_file(root, "beskar.toml", &config_toml(library_id));
        write_file(
            root,
            "skills/engineering/process/code-review/SKILL.md",
            "---\nname: code-review\ndescription: Reviews code carefully.\n---\n",
        );
        write_file(
            root,
            "skills/languages/rust/rust-dev/SKILL.md",
            "---\nname: rust-dev\ndescription: Rust development.\n---\n",
        );
        write_file(
            root,
            "profiles/dev-core.toml",
            "schema = 1\nid = \"98f1513d-94fa-4ace-907e-544c66233653\"\n\
             name = \"dev-core\"\nskills = [\"code-review\"]\n",
        );
        write_file(
            root,
            "catalog.toml",
            "schema = 1\n[skills.code-review]\ntags = [\"git\"]\nrank = 100\n",
        );
    }

    #[test]
    fn discovery_finds_library_above_start_directory() {
        let root = TempRoot::new();
        write_library(root.path(), "550e8400-e29b-41d4-a716-446655440000");
        let nested = root.child("skills/engineering/process");
        let library = Library::discover_from(&nested, None).expect("discover");
        assert_eq!(library.root(), root.path().canonicalize().expect("exists"));
        assert_eq!(library.config().default_ref, "main");
        assert_eq!(
            library.config().library_id.to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn discovery_fails_without_any_beskar_toml() {
        let root = TempRoot::new();
        let nested = root.child("deeply/nested");
        assert!(matches!(
            Library::discover_from(&nested, None),
            Err(crate::Error::Config(_))
        ));
    }

    #[test]
    fn explicit_library_override_takes_precedence() {
        // §86: BESKAR_LIBRARY overrides discovery; it must be a valid Library.
        let root = TempRoot::new();
        let library_dir = root.child("lib");
        write_library(&library_dir, "550e8400-e29b-41d4-a716-446655440000");
        let elsewhere = root.child("elsewhere");
        let library =
            Library::discover_from(&elsewhere, Some(&library_dir)).expect("override used");
        assert_eq!(library.root(), library_dir);

        // An override pointing nowhere fails loudly instead of falling back.
        let missing = root.path().join("missing");
        assert!(matches!(
            Library::discover_from(&elsewhere, Some(&missing)),
            Err(crate::Error::Config(_))
        ));
    }

    #[test]
    fn open_at_fails_closed_on_unsupported_schema() {
        let root = TempRoot::new();
        write_file(
            root.path(),
            "beskar.toml",
            "schema = 99\nlibrary_id = \"550e8400-e29b-41d4-a716-446655440000\"\n",
        );
        assert!(matches!(
            Library::open_at(root.path()),
            Err(crate::Error::Schema(_))
        ));
    }

    #[test]
    fn library_id_is_stable_across_clones() {
        // §10: library_id is generated once and unchanged across clones.
        let repo = TestRepo::new();
        write_library(repo.path(), "550e8400-e29b-41d4-a716-446655440000");
        repo.commit_all("beskar: seed library");

        let clone_dir = TempRoot::new();
        let clone_path = clone_dir.path().join("clone");
        git_ok(
            clone_dir.path(),
            &[
                "clone",
                &repo.path().to_string_lossy(),
                &clone_path.to_string_lossy(),
            ],
        );
        let original = Library::open_at(repo.path()).expect("open original");
        let clone = Library::open_at(&clone_path).expect("open clone");
        assert_eq!(original.config().library_id, clone.config().library_id);
        assert_ne!(original.root(), clone.root());
    }

    #[test]
    fn scan_validates_working_tree_layout() {
        let root = TempRoot::new();
        write_library(root.path(), "550e8400-e29b-41d4-a716-446655440000");
        let library = Library::open_at(root.path()).expect("open");
        let scan = library.scan().expect("scan");
        let names: Vec<&str> = scan.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["code-review", "rust-dev"]);
        assert_eq!(
            scan.skills[0].path,
            "skills/engineering/process/code-review"
        );
        assert_eq!(scan.profiles.len(), 1);
        assert_eq!(scan.profiles[0].name, "dev-core");
        assert_eq!(scan.catalog.skills["code-review"].rank, Some(100));
    }

    #[test]
    fn scan_rejects_duplicate_skill_names_across_buckets() {
        // §8.2: same name in two buckets invalidates the revision.
        let root = TempRoot::new();
        write_library(root.path(), "550e8400-e29b-41d4-a716-446655440000");
        write_file(
            root.path(),
            "skills/other/place/code-review/SKILL.md",
            "---\nname: code-review\ndescription: A second copy.\n---\n",
        );
        let library = Library::open_at(root.path()).expect("open");
        assert!(matches!(library.scan(), Err(crate::Error::Validation(_))));
    }

    #[test]
    fn scan_rejects_leaf_name_mismatch() {
        let root = TempRoot::new();
        write_library(root.path(), "550e8400-e29b-41d4-a716-446655440000");
        write_file(
            root.path(),
            "skills/wrong-leaf/SKILL.md",
            "---\nname: right-name\ndescription: d\n---\n",
        );
        let library = Library::open_at(root.path()).expect("open");
        assert!(matches!(library.scan(), Err(crate::Error::Validation(_))));
    }

    #[test]
    fn scan_rejects_nested_skill_directories() {
        let root = TempRoot::new();
        write_library(root.path(), "550e8400-e29b-41d4-a716-446655440000");
        write_file(
            root.path(),
            "skills/outer/inner/SKILL.md",
            "---\nname: inner\ndescription: d\n---\n",
        );
        write_file(
            root.path(),
            "skills/outer/SKILL.md",
            "---\nname: outer\ndescription: d\n---\n",
        );
        let library = Library::open_at(root.path()).expect("open");
        assert!(matches!(library.scan(), Err(crate::Error::Validation(_))));
    }

    #[test]
    fn scan_rejects_bad_bucket_segments() {
        let root = TempRoot::new();
        write_library(root.path(), "550e8400-e29b-41d4-a716-446655440000");
        write_file(
            root.path(),
            "skills/BadBucket/thing/SKILL.md",
            "---\nname: thing\ndescription: d\n---\n",
        );
        let library = Library::open_at(root.path()).expect("open");
        assert!(matches!(library.scan(), Err(crate::Error::PathSafety(_))));
    }

    #[cfg(unix)]
    #[test]
    fn scan_rejects_symlinks() {
        // §12: symlinks are rejected during Library scans.
        let root = TempRoot::new();
        write_library(root.path(), "550e8400-e29b-41d4-a716-446655440000");
        std::os::unix::fs::symlink(
            root.path().join("skills/engineering"),
            root.path().join("skills/link"),
        )
        .expect("symlink");
        let library = Library::open_at(root.path()).expect("open");
        assert!(matches!(library.scan(), Err(crate::Error::PathSafety(_))));
    }

    #[test]
    fn scan_requires_profiles_dir() {
        let root = TempRoot::new();
        write_library(root.path(), "550e8400-e29b-41d4-a716-446655440000");
        std::fs::remove_dir_all(root.path().join("profiles")).expect("remove profiles");
        let library = Library::open_at(root.path()).expect("open");
        assert!(matches!(library.scan(), Err(crate::Error::Library(_))));
    }

    #[test]
    fn scan_rejects_duplicate_profile_names() {
        let root = TempRoot::new();
        write_library(root.path(), "550e8400-e29b-41d4-a716-446655440000");
        write_file(
            root.path(),
            "profiles/other.toml",
            "schema = 1\nid = \"11111111-1111-4111-8111-111111111111\"\n\
             name = \"dev-core\"\nskills = []\n",
        );
        let library = Library::open_at(root.path()).expect("open");
        assert!(matches!(library.scan(), Err(crate::Error::Profile(_))));
    }

    #[test]
    fn scan_tolerates_missing_catalog() {
        let root = TempRoot::new();
        write_library(root.path(), "550e8400-e29b-41d4-a716-446655440000");
        std::fs::remove_file(root.path().join("catalog.toml")).expect("remove catalog");
        let library = Library::open_at(root.path()).expect("open");
        let scan = library.scan().expect("scan");
        assert!(scan.catalog.skills.is_empty());
    }

    // ---- revision resolution (real temp Git repositories, §125) ----

    #[test]
    fn resolve_revision_reads_committed_state_exactly() {
        let repo = TestRepo::new();
        write_library(repo.path(), "550e8400-e29b-41d4-a716-446655440000");
        let commit = repo.commit_all("beskar: seed library");

        // Uncommitted working-tree changes MUST NOT appear (§8.1).
        write_file(
            repo.path(),
            "skills/engineering/process/code-review/SKILL.md",
            "---\nname: code-review\ndescription: UNCOMMITTED.\n---\n",
        );

        let library = Library::open_at(repo.path()).expect("open");
        let resolved = resolve_revision(&SystemGitBackend, &library, &commit).expect("resolve");
        assert_eq!(resolved.commit, commit);
        assert_eq!(resolved.skills.len(), 2);

        let review = &resolved.skills[&SkillName::parse("code-review").expect("valid")];
        assert_eq!(review.path, "skills/engineering/process/code-review");
        assert_eq!(review.description, "Reviews code carefully.");
        assert_eq!(review.skill_commit, commit);
        let file_paths: Vec<&str> = review.files.keys().map(String::as_str).collect();
        assert_eq!(file_paths, ["SKILL.md"]);
        assert_eq!(
            review.files["SKILL.md"].path,
            "skills/engineering/process/code-review/SKILL.md"
        );

        assert_eq!(resolved.profiles.len(), 1);
        assert_eq!(
            resolved.profile_by_name["dev-core"],
            resolved.profiles.values().next().expect("one profile").id
        );
        assert_eq!(resolved.catalog.skills["code-review"].rank, Some(100));
    }

    #[test]
    fn resolve_revision_tracks_per_skill_commits() {
        // §35: skill_commit is the last commit touching the skill directory
        // and MAY differ from the resolved library commit.
        let repo = TestRepo::new();
        write_library(repo.path(), "550e8400-e29b-41d4-a716-446655440000");
        let first = repo.commit_all("beskar: seed library");

        write_file(
            repo.path(),
            "skills/engineering/process/code-review/review.md",
            "guidance",
        );
        let second = repo.commit_all("beskar: update code-review");

        let library = Library::open_at(repo.path()).expect("open");
        let resolved = resolve_revision(&SystemGitBackend, &library, &second).expect("resolve");
        let review = &resolved.skills[&SkillName::parse("code-review").expect("valid")];
        let rust = &resolved.skills[&SkillName::parse("rust-dev").expect("valid")];
        assert_eq!(review.skill_commit, second);
        assert_eq!(rust.skill_commit, first);
        assert_eq!(review.files.len(), 2); // SKILL.md + review.md
    }

    #[test]
    fn resolve_revision_resolves_branch_refs() {
        let repo = TestRepo::new();
        write_library(repo.path(), "550e8400-e29b-41d4-a716-446655440000");
        repo.commit_all("beskar: seed");
        write_file(
            repo.path(),
            "skills/writing/unslop/SKILL.md",
            "---\nname: unslop\ndescription: Sharpens prose.\n---\n",
        );
        repo.commit_all("beskar: ingest unslop");
        git_ok(repo.path(), &["switch", "-c", "feature-branch"]);
        write_file(
            repo.path(),
            "skills/feature/SKILL.md",
            "---\nname: feature\ndescription: Branch only.\n---\n",
        );
        repo.commit_all("beskar: branch skill");

        let library = Library::open_at(repo.path()).expect("open");
        let on_feature = resolve_revision(&SystemGitBackend, &library, "feature-branch")
            .expect("resolve branch");
        assert!(
            on_feature
                .skills
                .contains_key(&SkillName::parse("feature").expect("valid"))
        );
        let on_main = resolve_revision(&SystemGitBackend, &library, "main").expect("resolve main");
        assert!(
            !on_main
                .skills
                .contains_key(&SkillName::parse("feature").expect("valid"))
        );
    }

    #[test]
    fn resolve_revision_rejects_duplicate_names_across_buckets() {
        let repo = TestRepo::new();
        write_library(repo.path(), "550e8400-e29b-41d4-a716-446655440000");
        write_file(
            repo.path(),
            "skills/elsewhere/code-review/SKILL.md",
            "---\nname: code-review\ndescription: dup\n---\n",
        );
        repo.commit_all("seed");
        let library = Library::open_at(repo.path()).expect("open");
        let head = repo.head();
        assert!(matches!(
            resolve_revision(&SystemGitBackend, &library, &head),
            Err(crate::Error::Validation(_))
        ));
    }

    #[test]
    fn resolve_revision_requires_skills_dir() {
        let repo = TestRepo::new();
        write_file(
            repo.path(),
            "beskar.toml",
            &config_toml("550e8400-e29b-41d4-a716-446655440000"),
        );
        repo.commit_all("config only");
        let library = Library::open_at(repo.path()).expect("open");
        let head = repo.head();
        assert!(matches!(
            resolve_revision(&SystemGitBackend, &library, &head),
            Err(crate::Error::Library(_))
        ));
    }
}

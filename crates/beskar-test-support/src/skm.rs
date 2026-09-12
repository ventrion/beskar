//! Legacy `skill-manager` Home fixtures for migration tests (spec §120-§124,
//! §125 "Migration tests").
//!
//! Generates the exact fixture format `beskar_core::migrate` documents: a
//! Home that is itself the legacy skills Git repository, with
//! `skills/<bucket...>/<name>/SKILL.md`, `profiles.json`, `registry.json`,
//! and optional installed skill directories carrying `.skm.json` stamps.
//! Everything is hermetic: tempfile directories, local-only Git, isolated
//! config — the real user home and network are never touched.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::TempRoot;
use crate::git::git_ok;

/// A disposable legacy skill-manager Home.
pub struct SkmHome {
    root: TempRoot,
}

impl SkmHome {
    /// Creates the Home: a Git repository (branch `main`) with an empty
    /// committed `skills/` tree and a local hermetic identity.
    pub fn new() -> Self {
        let root = TempRoot::new();
        git_ok(root.path(), &["init", "--initial-branch=main"]);
        git_ok(root.path(), &["config", "user.name", "Skm Legacy"]);
        git_ok(
            root.path(),
            &["config", "user.email", "skm@example.invalid"],
        );
        std::fs::create_dir_all(root.path().join("skills")).expect("create skills dir");
        Self { root }
    }

    /// The Home directory (this whole directory becomes the Beskar Library).
    pub fn path(&self) -> &Path {
        self.root.path()
    }

    /// Adds a skill and commits it; returns the new HEAD.
    pub fn add_skill(&self, bucket_and_name: &str, description: &str) -> String {
        crate::fs::write_skill(
            self.root.path(),
            &format!("skills/{bucket_and_name}"),
            description,
        );
        self.commit(&format!("skm: add {bucket_and_name}"))
    }

    /// Writes a regular file inside a skill directory (uncommitted).
    pub fn write_skill_file(
        &self,
        bucket_and_name: &str,
        relative: &str,
        content: &str,
    ) -> PathBuf {
        crate::fs::write_file(
            self.root.path(),
            &format!("skills/{bucket_and_name}/{relative}"),
            content,
        )
    }

    /// Writes an executable file inside a skill directory (uncommitted; the
    /// executable bit only applies on POSIX).
    pub fn write_executable_skill_file(
        &self,
        bucket_and_name: &str,
        relative: &str,
        content: &str,
    ) {
        let path = self.write_skill_file(bucket_and_name, relative, content);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path).expect("stat").permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).expect("chmod");
        }
        #[cfg(not(unix))]
        drop(path);
    }

    /// Commits everything in the Home; returns the new HEAD.
    pub fn commit(&self, message: &str) -> String {
        git_ok(self.root.path(), &["add", "-A"]);
        git_ok(self.root.path(), &["commit", "-m", message]);
        self.head()
    }

    /// The Home repository's HEAD commit.
    pub fn head(&self) -> String {
        git_ok(self.root.path(), &["rev-parse", "HEAD"])
    }

    /// Appends one legacy profile definition to `profiles.json`.
    pub fn add_profile(&self, name: &str, skills: &[&str]) {
        self.add_profile_full(name, None, skills);
    }

    /// Appends one legacy profile definition with a description.
    pub fn add_profile_full(&self, name: &str, description: Option<&str>, skills: &[&str]) {
        let path = self.root.path().join("profiles.json");
        let mut document = read_json_or(&path, || json!({"schema": 1, "profiles": []}));
        let mut list = document["profiles"]
            .as_array()
            .cloned()
            .expect("profiles array");
        list.push(json!({
            "name": name,
            "description": description,
            "skills": skills,
        }));
        document["profiles"] = Value::Array(list);
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&document).expect("json"),
        )
        .expect("write profiles.json");
    }

    /// Appends one legacy registry entry (ONE profile per installation, §121)
    /// to `<home>/registry.json`.
    pub fn add_installation(&self, workspace: &Path, target: &str, profile: &str) {
        self.add_installation_full(workspace, target, profile, Some("main"), None, None);
    }

    /// Appends a legacy registry entry with all optional fields.
    pub fn add_installation_full(
        &self,
        workspace: &Path,
        target: &str,
        profile: &str,
        source_ref: Option<&str>,
        installed_at: Option<&str>,
        library: Option<&str>,
    ) {
        let path = self.root.path().join("registry.json");
        let mut document = read_json_or(&path, || json!({"schema": 1, "installations": []}));
        let mut list = document["installations"]
            .as_array()
            .cloned()
            .expect("installations array");
        list.push(json!({
            "workspace": workspace.to_string_lossy(),
            "target": target,
            "profile": profile,
            "source_ref": source_ref,
            "commit": self.head(),
            "installed_at": installed_at,
            "library": library,
        }));
        document["installations"] = Value::Array(list);
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&document).expect("json"),
        )
        .expect("write registry.json");
    }

    /// Simulates a legacy installation of one skill: copies the committed
    /// working-tree content of `skills/<bucket_and_name>` into the
    /// workspace target and writes a faithful `.skm.json` stamp beside it.
    /// Returns the installed skill directory.
    pub fn install_skill(&self, workspace: &Path, target: &str, bucket_and_name: &str) -> PathBuf {
        let source = self.root.path().join("skills").join(bucket_and_name);
        let leaf = bucket_and_name.rsplit('/').next().expect("leaf");
        let destination = workspace.join(target).join(leaf);
        copy_tree(&source, &destination);
        write_legacy_stamp(self.head(), leaf, &destination);
        destination
    }
}

impl Default for SkmHome {
    fn default() -> Self {
        Self::new()
    }
}

/// Writes a `.skm.json` manifest with the actual on-disk hashes of the
/// installed skill directory (§124 fixture format).
fn write_legacy_stamp(head: String, skill_leaf: &str, destination: &Path) {
    let mut files = serde_json::Map::new();
    for entry in walkdir::WalkDir::new(destination).follow_links(false) {
        let entry = entry.expect("walk installed skill");
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(destination)
            .expect("relative")
            .to_str()
            .expect("utf-8")
            .replace(std::path::MAIN_SEPARATOR, "/");
        let bytes = std::fs::read(entry.path()).expect("read installed file");
        files.insert(
            relative,
            json!({
                "sha256": beskar_core::stamp::sha256_hex(&bytes),
                "executable": is_executable(entry.path()),
            }),
        );
    }
    let stamp = json!({
        "schema": 1,
        "manager": "skm",
        "skill": skill_leaf,
        "source_ref": "main",
        "commit": head,
        "files": Value::Object(files),
    });
    std::fs::write(
        destination.join(".skm.json"),
        serde_json::to_string_pretty(&stamp).expect("json"),
    )
    .expect("write .skm.json");
}

/// Whether the path has any executable bit (POSIX; false elsewhere, §34).
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// Copies a directory tree of regular files (fixture helper; no special
/// objects ever enter fixtures).
fn copy_tree(source: &Path, destination: &Path) {
    for entry in walkdir::WalkDir::new(source).follow_links(false) {
        let entry = entry.expect("walk source");
        let relative = entry.path().strip_prefix(source).expect("relative");
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).expect("create dir");
        } else if entry.file_type().is_file() {
            std::fs::copy(entry.path(), &target).expect("copy file");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = std::fs::metadata(entry.path()) {
                    let mut permissions = std::fs::metadata(&target).expect("stat").permissions();
                    permissions.set_mode(metadata.permissions().mode());
                    std::fs::set_permissions(&target, permissions).expect("chmod");
                }
            }
        }
    }
}

fn read_json_or(path: &Path, fallback: impl FnOnce() -> Value) -> Value {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|_| fallback()),
        Err(_) => fallback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_roundtrips_profiles_and_installations() {
        let home = SkmHome::new();
        home.add_skill("engineering/process/code-review", "Reviews code.");
        home.add_profile("dev-core", &["code-review"]);
        let workspace = home.path().join("unused");
        home.add_installation(&workspace, ".agents/skills", "dev-core");
        let profiles: Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join("profiles.json")).expect("read"),
        )
        .expect("json");
        assert_eq!(profiles["profiles"][0]["name"], "dev-core");
        let registry: Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join("registry.json")).expect("read"),
        )
        .expect("json");
        assert_eq!(registry["installations"][0]["profile"], "dev-core");
        assert_eq!(home.head().len(), 40);
    }

    #[test]
    fn installed_skill_carries_a_faithful_legacy_stamp() {
        let home = SkmHome::new();
        home.add_skill("engineering/process/code-review", "Reviews code.");
        home.write_skill_file("engineering/process/code-review", "notes.md", "guidance");
        home.commit("skm: add notes");
        let workspace_root = TempRoot::new();
        let target = workspace_root.path().join(".agents/skills");
        let installed = home.install_skill(
            workspace_root.path(),
            ".agents/skills",
            "engineering/process/code-review",
        );
        let stamp: Value = serde_json::from_str(
            &std::fs::read_to_string(installed.join(".skm.json")).expect("read stamp"),
        )
        .expect("json");
        assert_eq!(stamp["skill"], "code-review");
        assert_eq!(stamp["files"]["SKILL.md"]["executable"], false);
        let bytes = std::fs::read(target.join("code-review/notes.md")).expect("read");
        assert_eq!(
            stamp["files"]["notes.md"]["sha256"],
            beskar_core::stamp::sha256_hex(&bytes)
        );
    }
}

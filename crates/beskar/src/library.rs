//! The Library: the user's curated global collection.
//!
//! Layout:
//!
//! ```text
//! <library>/skills/<skill-id>/...     canonical skill directories
//! <library>/profiles/<name>.bsk       profile definitions
//! ```
//!
//! The library is the source of truth for skill content. It is not itself
//! an agent skills directory — nothing scans it automatically.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::fingerprint;
use crate::profile::Profile;
use crate::skillmd;
use crate::util;

#[derive(Debug, Clone)]
pub struct Library {
    pub root: PathBuf,
}

/// A skill known to the library.
#[derive(Debug, Clone)]
pub struct SkillEntry {
    /// Directory name of the skill; used as its id everywhere.
    pub id: String,
    pub path: PathBuf,
    /// `name` from SKILL.md frontmatter, when present. Informational only;
    /// the id is always the directory name.
    pub meta_name: Option<String>,
    pub description: Option<String>,
    pub has_skill_md: bool,
}

impl SkillEntry {
    /// Display name: frontmatter name in parens when it differs from the id.
    pub fn display(&self) -> String {
        match &self.meta_name {
            Some(n) if n != &self.id => format!("{} ({})", self.id, n),
            _ => self.id.clone(),
        }
    }
}

impl Library {
    pub fn new(root: &Path) -> Library {
        Library { root: root.to_path_buf() }
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join("profiles")
    }

    pub fn skill_path(&self, id: &str) -> PathBuf {
        self.skills_dir().join(id)
    }

    pub fn profile_path(&self, name: &str) -> PathBuf {
        self.profiles_dir().join(format!("{name}.bsk"))
    }

    /// A skill id doubles as a path component, so ids that could climb
    /// out of the skills directory (`..`, absolute paths) are not ids.
    pub fn valid_id(id: &str) -> bool {
        crate::profile::validate_name(id).is_ok()
    }

    pub fn has_skill(&self, id: &str) -> bool {
        Self::valid_id(id) && self.skill_path(id).is_dir()
    }

    /// Create the library skeleton. Idempotent.
    pub fn init_dirs(&self) -> Result<()> {
        fs::create_dir_all(self.skills_dir()).map_err(|e| Error::io(self.skills_dir(), &e))?;
        fs::create_dir_all(self.profiles_dir()).map_err(|e| Error::io(self.profiles_dir(), &e))?;
        Ok(())
    }

    /// List skills (subdirectories of `skills/`), sorted by id.
    pub fn list_skills(&self) -> Result<Vec<SkillEntry>> {
        let rd = match fs::read_dir(self.skills_dir()) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::io(self.skills_dir(), &e)),
        };
        let mut out = Vec::new();
        for e in rd {
            let e = e.map_err(|er| Error::io(self.skills_dir(), &er))?;
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue; // stray files are not skills
            }
            let id = e.file_name().to_string_lossy().into_owned();
            let meta = skillmd::from_dir(&e.path());
            out.push(SkillEntry {
                has_skill_md: e.path().join("SKILL.md").is_file(),
                meta_name: meta.name,
                description: meta.description,
                id,
                path: e.path(),
            });
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Current fingerprint of a library skill.
    pub fn fingerprint(&self, id: &str) -> Result<String> {
        Self::ensure_valid_id(id)?;
        let p = self.skill_path(id);
        if !p.is_dir() {
            return Err(Error::msg(format!("skill `{id}` is not in the library")));
        }
        fingerprint::fingerprint_dir(&p)
    }

    /// List profiles, sorted by name.
    pub fn list_profiles(&self) -> Result<Vec<Profile>> {
        let rd = match fs::read_dir(self.profiles_dir()) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::io(self.profiles_dir(), &e)),
        };
        let mut out = Vec::new();
        for e in rd {
            let p = e.map_err(|er| Error::io(self.profiles_dir(), &er))?.path();
            if p.extension().map(|x| x == "bsk").unwrap_or(false) {
                out.push(Profile::load(&p)?);
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn load_profile(&self, name: &str) -> Result<Profile> {
        crate::profile::validate_name(name)?;
        let path = self.profile_path(name);
        if !path.is_file() {
            return Err(Error::msg(format!(
                "no profile `{name}` in {} — see `beskar profile create`",
                util::display_path(&self.profiles_dir())
            )));
        }
        Profile::load(&path)
    }

    /// Import a directory as a skill named `id`. Fails if the target
    /// already exists unless `force`. Returns (files copied, fingerprint).
    pub fn import_skill(&self, source: &Path, id: &str, force: bool) -> Result<(usize, String)> {
        if !source.is_dir() {
            return Err(Error::msg(format!("{} is not a directory", source.display())));
        }
        crate::profile::validate_name(id)?; // same charset rules as profiles
        let dst = self.skill_path(id);
        if dst.exists() {
            // Importing a library skill onto itself is a no-op, not a
            // self-deletion.
            if same_dir(source, &dst) {
                let items = crate::util::walk_sorted(&dst)?;
                let n = items.iter().filter(|i| matches!(i, crate::util::Item::File { .. })).count();
                let fp = fingerprint::fingerprint_dir(&dst)?;
                return Ok((n, fp));
            }
            if !force {
                return Err(Error::msg(format!(
                    "skill `{id}` already exists in the library — use --force to overwrite"
                )));
            }
            util::remove_tree(&dst)?;
        }
        let n = util::copy_tree(source, &dst)?;
        let fp = fingerprint::fingerprint_dir(&dst)?;
        Ok((n, fp))
    }

    /// Remove a skill directory. Refuses implicitly by callers checking usage.
    pub fn remove_skill(&self, id: &str) -> Result<()> {
        Self::ensure_valid_id(id)?;
        let p = self.skill_path(id);
        if !p.is_dir() {
            return Err(Error::msg(format!("skill `{id}` is not in the library")));
        }
        util::remove_tree(&p)
    }

    fn ensure_valid_id(id: &str) -> Result<()> {
        crate::profile::validate_name(id).map_err(|e| {
            Error::msg(format!("invalid skill id `{id}`: {}", e))
        })
    }
}

/// True when both paths resolve to the same existing directory.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

/// Does `dir` look like a skill? The only heuristic: it contains a
/// `SKILL.md`. Everything else is just a directory.
pub fn looks_like_skill(dir: &Path) -> bool {
    dir.join("SKILL.md").is_file()
}

/// Discover skill directories under `root`: either `root` itself, or its
/// immediate subdirectories containing `SKILL.md`.
pub fn discover_skills(root: &Path) -> Result<Vec<SkillEntry>> {
    if looks_like_skill(root) {
        let id = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "skill".to_string());
        let meta = skillmd::from_dir(root);
        return Ok(vec![SkillEntry {
            has_skill_md: true,
            meta_name: meta.name,
            description: meta.description,
            id,
            path: root.to_path_buf(),
        }]);
    }
    let rd = fs::read_dir(root).map_err(|e| Error::io(root, &e))?;
    let mut dirs: Vec<_> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    dirs.sort_by_key(|e| e.file_name());
    let mut out = Vec::new();
    for d in dirs {
        if looks_like_skill(&d.path()) {
            let meta = skillmd::from_dir(&d.path());
            out.push(SkillEntry {
                has_skill_md: true,
                meta_name: meta.name,
                description: meta.description,
                id: d.file_name().to_string_lossy().into_owned(),
                path: d.path(),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmplib() -> (PathBuf, Library) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "beskar-lib-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        let lib = Library::new(&dir.join("library"));
        lib.init_dirs().unwrap();
        (dir, lib)
    }

    #[test]
    fn import_onto_self_is_a_no_op() {
        let (dir, lib) = tmplib();
        let src = dir.join("src-skill");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), "hi").unwrap();
        lib.import_skill(&src, "s", false).unwrap();

        // Re-importing the library's own copy (scan rediscovery, --force)
        // must not delete the skill.
        let (n, fp) = lib
            .import_skill(&lib.skill_path("s"), "s", true)
            .expect("self-import must not fail");
        assert!(n >= 1);
        assert!(fp.starts_with("sha256:"));
        assert!(
            lib.skill_path("s").join("SKILL.md").is_file(),
            "self-import must not delete the skill"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_list_remove() {
        let (_dir, lib) = tmplib();
        let src_parent = lib.root.parent().unwrap().join("staging");
        fs::create_dir_all(src_parent.join("myskill")).unwrap();
        fs::write(src_parent.join("myskill/SKILL.md"), "---\nname: My Skill\ndescription: does things\n---\nhi").unwrap();

        let found = discover_skills(&src_parent).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "myskill");
        assert_eq!(found[0].meta_name.as_deref(), Some("My Skill"));

        let (n, fp) = lib.import_skill(&src_parent.join("myskill"), "myskill", false).unwrap();
        assert_eq!(n, 1);
        assert!(fp.starts_with("sha256:"));
        assert!(lib.has_skill("myskill"));

        // Duplicate import refused without force.
        assert!(lib.import_skill(&src_parent.join("myskill"), "myskill", false).is_err());
        lib.import_skill(&src_parent.join("myskill"), "myskill", true).unwrap();

        let skills = lib.list_skills().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].display(), "myskill (My Skill)");

        lib.remove_skill("myskill").unwrap();
        assert!(!lib.has_skill("myskill"));
        let _ = fs::remove_dir_all(&lib.root);
    }

    #[test]
    fn traversal_profile_names_are_rejected() {
        let (_dir, lib) = tmplib();
        // load_profile guards every command that reads a profile by name.
        for bad in ["../../evil", "..", "a/b", ".hidden"] {
            assert!(lib.load_profile(bad).is_err(), "`{bad}` must not load");
        }
        let _ = fs::remove_dir_all(&lib.root);
    }

    #[test]
    fn traversal_ids_are_rejected() {
        // Skill ids become path components; ids like `..` used to resolve
        // outside the skills directory (remove_skill("..") once pointed at
        // the library root itself).
        let (_dir, lib) = tmplib();
        for bad in ["..", ".", "/tmp/evil", "a/b", ".hidden", ""] {
            assert!(!lib.has_skill(bad), "`{bad}` must not look like a skill");
            assert!(lib.fingerprint(bad).is_err(), "`{bad}` must not fingerprint");
            assert!(lib.remove_skill(bad).is_err(), "`{bad}` must not remove");
        }
        // The attempts above must not have touched anything.
        assert!(lib.skills_dir().is_dir(), "skills directory must survive");
        let _ = fs::remove_dir_all(&lib.root);
    }

    #[test]
    fn discover_single_dir_and_empty() {
        let (_dir, lib) = tmplib();
        // Discovering the library skills dir itself finds nothing (no SKILL.md).
        assert!(discover_skills(&lib.skills_dir()).unwrap().is_empty());
        let _ = fs::remove_dir_all(&lib.root);
    }
}

//! Finding skills in an arbitrary directory tree, for `beskar library scan`
//! and `beskar library add`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::fingerprint::Fingerprint;
use crate::library::Library;
use crate::names::{self, SkillId};
use crate::skill::{SkillMeta, is_skill_dir};
use crate::{Error, Result};

/// How deep below the scanned directory to look.
pub const MAX_DEPTH: usize = 8;

/// Directories that never hold skills worth importing and can be huge.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "venv",
    ".cache",
    ".npm",
    ".cargo",
    ".rustup",
];

/// Where a candidate's library name comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Naming {
    /// The `name` field of its front matter.
    FrontMatter,
    /// Its directory name.
    Directory,
    /// Its directory name, adjusted to fit the naming rules.
    Adjusted { from: String },
    /// Neither the front matter nor the directory gives a usable name.
    Unusable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not in the library yet.
    New,
    /// The library has exactly this content.
    Identical,
    /// The library has a different version.
    Differs,
    /// An earlier candidate in the same scan has this name.
    Duplicate { of: PathBuf },
    /// No usable name.
    Unnamed,
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: PathBuf,
    pub id: Option<SkillId>,
    pub naming: Naming,
    pub status: Status,
    pub meta: SkillMeta,
}

/// The library name for the skill at `dir`: the front matter `name` if it
/// is valid, else the directory name, adjusted if necessary.
pub fn name_for(dir: &Path, meta: &SkillMeta) -> (Option<SkillId>, Naming) {
    if let Some(name) = &meta.name
        && let Ok(id) = SkillId::new(name)
    {
        return (Some(id), Naming::FrontMatter);
    }
    let dir_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Ok(id) = SkillId::new(&dir_name) {
        return (Some(id), Naming::Directory);
    }
    match names::suggest(&dir_name).and_then(|name| SkillId::new(&name).ok()) {
        Some(id) => (Some(id), Naming::Adjusted { from: dir_name }),
        None => (None, Naming::Unusable),
    }
}

/// Directories under `root` that hold a `SKILL.md`, sorted. The search does
/// not descend into a skill once found, skips version control and build
/// directories, and does not follow symlinked directories except to pick
/// up a symlinked skill. `exclude` directories are skipped (the library's
/// own skills, for example).
pub fn discover(root: &Path, exclude: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let root = fs::canonicalize(root)
        .map_err(|err| Error::io(&err, format_args!("read {}", root.display())))?;
    if !root.is_dir() {
        return Err(Error::invalid(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    let exclude: Vec<PathBuf> = exclude
        .iter()
        .filter_map(|p| fs::canonicalize(p).ok())
        .collect();
    let mut found = Vec::new();
    visit(&root, 0, &exclude, &mut found);
    found.sort();
    Ok(found)
}

fn visit(dir: &Path, depth: usize, exclude: &[PathBuf], found: &mut Vec<PathBuf>) {
    if is_skill_dir(dir) {
        found.push(dir.to_path_buf());
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if SKIP_DIRS.contains(&name.as_str())
            || name.starts_with(crate::fsx::TEMP_PREFIX)
            || name == crate::fsx::WORK_DIR
            || exclude.contains(&path)
        {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            visit(&path, depth + 1, exclude, found);
        } else if file_type.is_symlink() && path.is_dir() && is_skill_dir(&path) {
            found.push(path);
        }
    }
}

/// Find skills under `root` and compare each with the library.
pub fn scan(library: &Library, root: &Path) -> Result<Vec<Candidate>> {
    let mut seen: BTreeMap<SkillId, PathBuf> = BTreeMap::new();
    let mut candidates = Vec::new();
    for path in discover(root, &[library.skills_dir()])? {
        let meta = SkillMeta::read(&path, None);
        let (id, naming) = name_for(&path, &meta);
        let status = match &id {
            None => Status::Unnamed,
            Some(id) if seen.contains_key(id) => Status::Duplicate {
                of: seen[id].clone(),
            },
            Some(id) => {
                seen.insert(id.clone(), path.clone());
                compare_with_library(library, &path, id)?
            }
        };
        candidates.push(Candidate {
            path,
            id,
            naming,
            status,
            meta,
        });
    }
    Ok(candidates)
}

/// Whether the library already has the skill at `path` under `id`.
pub fn compare_with_library(library: &Library, path: &Path, id: &SkillId) -> Result<Status> {
    let Some(existing) = library.fingerprint(id)? else {
        return Ok(Status::New);
    };
    let incoming = Fingerprint::of(
        &fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        library.ignore(),
    )
    .map_err(|err| Error::io(&err, format_args!("read {}", path.display())))?;
    Ok(if incoming == existing {
        Status::Identical
    } else {
        Status::Differs
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ignore::Ignore;
    use crate::testutil::TempDir;

    #[test]
    fn discovers_skills_but_not_inside_them() {
        let tmp = TempDir::new();
        tmp.write("src/pdf/SKILL.md", "---\nname: pdf\n---\n");
        tmp.write("src/pdf/examples/nested/SKILL.md", "not a separate skill");
        tmp.write("src/group/git/SKILL.md", "x");
        tmp.write("src/.claude/skills/review/SKILL.md", "x");
        tmp.write("src/node_modules/junk/SKILL.md", "x");
        tmp.write("src/notes/readme.md", "x");
        let found = discover(&tmp.path().join("src"), &[]).unwrap();
        let rel: Vec<String> = found
            .iter()
            .map(|p| {
                p.strip_prefix(tmp.path().join("src"))
                    .unwrap()
                    .display()
                    .to_string()
            })
            .collect();
        assert_eq!(rel, [".claude/skills/review", "group/git", "pdf"]);
    }

    #[test]
    fn the_root_can_be_a_skill() {
        let tmp = TempDir::new();
        tmp.write("pdf/SKILL.md", "x");
        assert_eq!(
            discover(&tmp.path().join("pdf"), &[]).unwrap(),
            [tmp.path().join("pdf")]
        );
    }

    #[test]
    fn naming_prefers_valid_front_matter_then_the_directory() {
        let tmp = TempDir::new();
        tmp.write("PDF Tools/SKILL.md", "---\nname: pdf\n---\n");
        tmp.write("Code_Review/SKILL.md", "---\nname: Code Review\n---\n");
        tmp.write("git/SKILL.md", "no front matter");
        tmp.write("!!!/SKILL.md", "x");
        let name = |dir: &str| {
            let path = tmp.path().join(dir);
            let (id, naming) = name_for(&path, &SkillMeta::read(&path, None));
            (id.map(|id| id.to_string()), naming)
        };
        assert_eq!(name("PDF Tools"), (Some("pdf".into()), Naming::FrontMatter));
        assert_eq!(
            name("Code_Review"),
            (
                Some("code-review".into()),
                Naming::Adjusted {
                    from: "Code_Review".into()
                }
            )
        );
        assert_eq!(name("git"), (Some("git".into()), Naming::Directory));
        assert_eq!(name("!!!"), (None, Naming::Unusable));
    }

    #[test]
    fn scan_compares_with_the_library() {
        let tmp = TempDir::new();
        let library = Library::new(tmp.path().join("lib"), Ignore::default());
        library.create().unwrap();
        tmp.write("lib/skills/same/SKILL.md", "same");
        tmp.write("lib/skills/changed/SKILL.md", "old");
        tmp.write("src/a/same/SKILL.md", "same");
        tmp.write("src/a/changed/SKILL.md", "new");
        tmp.write("src/a/fresh/SKILL.md", "fresh");
        tmp.write("src/b/fresh/SKILL.md", "another fresh");
        let statuses: Vec<(String, Status)> = scan(&library, &tmp.path().join("src"))
            .unwrap()
            .into_iter()
            .map(|c| {
                (
                    c.path
                        .strip_prefix(tmp.path().join("src"))
                        .unwrap()
                        .display()
                        .to_string(),
                    c.status,
                )
            })
            .collect();
        assert_eq!(
            statuses,
            [
                ("a/changed".to_string(), Status::Differs),
                ("a/fresh".to_string(), Status::New),
                ("a/same".to_string(), Status::Identical),
                (
                    "b/fresh".to_string(),
                    Status::Duplicate {
                        of: tmp.path().join("src/a/fresh")
                    }
                ),
            ]
        );
    }
}

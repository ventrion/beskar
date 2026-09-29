//! Discovering skills in an external directory tree for import.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::fingerprint;
use crate::library::Library;
use crate::skill::{SKILL_FILE, SkillId, SkillMeta};

/// Directories never worth descending into.
const SKIP: &[&str] = &[".git", ".hg", ".svn", "node_modules", "target", ".venv", "__pycache__"];
const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateStatus {
    /// Not in the library yet.
    New,
    /// Already in the library with identical content.
    Identical,
    /// In the library with different content.
    Differs,
    /// The directory name is not a valid skill id.
    InvalidName(String),
    /// Another candidate earlier in the scan has the same name.
    Duplicate(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub name: String,
    pub path: PathBuf,
    pub description: Option<String>,
    pub status: CandidateStatus,
}

impl Candidate {
    pub fn id(&self) -> Option<SkillId> {
        SkillId::new(&self.name).ok()
    }
}

/// Every directory under `root` that contains a `SKILL.md`. A skill's own
/// subdirectories are not searched.
pub fn find_skill_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    visit(root, 0, &mut out);
    out.sort();
    out
}

fn visit(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if dir.join(SKILL_FILE).is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let name = entry.file_name();
        if SKIP.iter().any(|s| name == *s) {
            continue;
        }
        let path = entry.path();
        // Never follow symlinks: they can loop or lead far away.
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            visit(&path, depth + 1, out);
        }
    }
}

/// Skills under `root` and how each relates to the library.
pub fn scan(library: &Library, root: &Path) -> Result<Vec<Candidate>> {
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut out = Vec::new();
    for path in find_skill_dirs(root) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let description = SkillMeta::read(&path).description().map(str::to_string);
        let status = match SkillId::new(&name) {
            Err(e) => CandidateStatus::InvalidName(e.message().to_string()),
            Ok(_) if seen.contains_key(&name) => CandidateStatus::Duplicate(seen[&name].clone()),
            Ok(id) => {
                seen.insert(name.clone(), path.clone());
                match library.fingerprint(&id)? {
                    None => CandidateStatus::New,
                    Some(fp) if fp == fingerprint::of_dir(&path)? => CandidateStatus::Identical,
                    Some(_) => CandidateStatus::Differs,
                }
            }
        };
        out.push(Candidate { name, path, description, status });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn finds_nested_skills_and_classifies_them() {
        let t = TempDir::new();
        let lib_root = t.path().join("lib");
        Library::init(&lib_root).unwrap();
        let lib = Library::open(&lib_root).unwrap();
        t.write_tree(
            "dl",
            &[
                ("a/pdf/SKILL.md", "pdf"),
                ("a/pdf/sub/SKILL.md", "nested inside a skill: ignored"),
                ("b/pdf/SKILL.md", "other pdf"),
                ("b/git/SKILL.md", "---\ndescription: Git helper\n---\n"),
                ("b/Bad Name/SKILL.md", ""),
                ("node_modules/x/SKILL.md", ""),
                ("notes.txt", ""),
            ],
        );
        lib.import(&t.path().join("dl/b/git"), &SkillId::new("git").unwrap(), false).unwrap();
        let found = scan(&lib, &t.path().join("dl")).unwrap();
        let summary: Vec<(&str, &CandidateStatus)> = found.iter().map(|c| (c.name.as_str(), &c.status)).collect();
        assert_eq!(summary.len(), 4, "{summary:?}");
        assert_eq!(summary[0], ("pdf", &CandidateStatus::New));
        assert!(matches!(summary[1].1, CandidateStatus::InvalidName(_)));
        assert_eq!(summary[2], ("git", &CandidateStatus::Identical));
        assert!(matches!(summary[3], ("pdf", CandidateStatus::Duplicate(_))));
        assert_eq!(found[2].description.as_deref(), Some("Git helper"));
    }
}

//! A skill is a directory. `SKILL.md` is the conventional entry point and, when
//! it has a YAML-style front matter block, Beskar reads `name` and
//! `description` from it. Nothing else about the layout is enforced.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{IoContext, Result};
use crate::fingerprint::Fingerprint;

pub const SKILL_FILE: &str = "SKILL.md";

/// Optional metadata extracted from `SKILL.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillMetadata {
    pub name: Option<String>,
    pub description: Option<String>,
}

/// A skill directory, canonically inside the library.
#[derive(Debug, Clone)]
pub struct Skill {
    /// The directory name: the identity profiles and the registry refer to.
    pub name: String,
    pub path: PathBuf,
    pub metadata: SkillMetadata,
}

impl Skill {
    pub fn load(name: &str, path: &Path) -> Skill {
        Skill {
            name: name.to_string(),
            path: path.to_path_buf(),
            metadata: read_metadata(path),
        }
    }

    pub fn has_skill_file(&self) -> bool {
        self.path.join(SKILL_FILE).is_file()
    }

    pub fn fingerprint(&self) -> Result<Fingerprint> {
        Fingerprint::of_dir(&self.path)
    }

    /// The description from `SKILL.md`, or an empty string.
    pub fn description(&self) -> &str {
        self.metadata.description.as_deref().unwrap_or("")
    }
}

/// Read `name:` and `description:` from a `---` front matter block at the top
/// of `SKILL.md`. Only simple one-line scalars are understood; anything else
/// is ignored rather than rejected, since Beskar does not own that format.
pub fn read_metadata(dir: &Path) -> SkillMetadata {
    let Ok(text) = fs::read_to_string(dir.join(SKILL_FILE)) else {
        return SkillMetadata::default();
    };
    parse_front_matter(&text)
}

fn parse_front_matter(text: &str) -> SkillMetadata {
    let mut meta = SkillMetadata::default();
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return meta;
    }
    for line in lines {
        let line = line.trim_end();
        if line.trim() == "---" {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "name" => meta.name = Some(value),
            "description" => meta.description = Some(value),
            _ => {}
        }
    }
    meta
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let inner = if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    };
    inner.to_string()
}

/// Directories below `root` that contain a `SKILL.md`, sorted. Hidden
/// directories and common dependency folders are not entered, and a skill's
/// own subdirectories are not searched.
pub fn discover(root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    if root.join(SKILL_FILE).is_file() {
        found.push(root.to_path_buf());
        return Ok(found);
    }
    discover_into(root, &mut found, 0)?;
    found.sort();
    Ok(found)
}

fn discover_into(dir: &Path, found: &mut Vec<PathBuf>, depth: usize) -> Result<()> {
    if depth > 8 {
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .at(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.')
            || matches!(name.as_str(), "node_modules" | "target" | "__pycache__")
        {
            continue;
        }
        if path.join(SKILL_FILE).is_file() {
            found.push(path);
        } else {
            discover_into(&path, found, depth + 1)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_simple_front_matter() {
        let md = "---\nname: playwright\ndescription: \"Browser automation\"\ntags:\n  - a\n---\n# Body\n";
        let meta = parse_front_matter(md);
        assert_eq!(meta.name.as_deref(), Some("playwright"));
        assert_eq!(meta.description.as_deref(), Some("Browser automation"));
    }

    #[test]
    fn no_front_matter_is_fine() {
        assert_eq!(
            parse_front_matter("# Just a title\n"),
            SkillMetadata::default()
        );
    }
}

//! A skill directory and the metadata Beskar reads from its `SKILL.md`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::frontmatter;
use crate::names::{self, SkillId};

/// The file that marks a directory as an Agent Skill.
pub const SKILL_FILE: &str = "SKILL.md";

/// A skill in the library.
#[derive(Clone, Debug)]
pub struct Skill {
    pub id: SkillId,
    pub path: PathBuf,
    pub meta: SkillMeta,
}

/// What Beskar knows about a skill directory from its `SKILL.md`. Beskar
/// manages any directory as a skill; this metadata is for display and for
/// `doctor`, never a requirement.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillMeta {
    /// Name of the skill file, if there is one (`SKILL.md`, or a
    /// differently cased variant).
    pub skill_file: Option<String>,
    /// `name` from the front matter.
    pub name: Option<String>,
    /// `description` from the front matter.
    pub description: Option<String>,
    /// Every front matter field, in file order.
    pub fields: Vec<(String, String)>,
    /// Ways in which this directory falls short of the Agent Skills format.
    pub problems: Vec<String>,
}

impl SkillMeta {
    /// Read the metadata of the skill directory at `dir`. `id` is the name
    /// the skill has (or will have) in the library; a front matter `name`
    /// that differs is reported as a problem.
    pub fn read(dir: &Path, id: Option<&SkillId>) -> SkillMeta {
        let mut meta = SkillMeta::default();
        let Some(file_name) = find_skill_file(dir) else {
            meta.problems.push(format!("no {SKILL_FILE}"));
            return meta;
        };
        if file_name != SKILL_FILE {
            meta.problems.push(format!(
                "the skill file is named `{file_name}`; Agent Skills expects `{SKILL_FILE}`"
            ));
        }
        let text = match fs::read(dir.join(&file_name)) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(err) => {
                meta.problems
                    .push(format!("cannot read {file_name}: {err}"));
                meta.skill_file = Some(file_name);
                return meta;
            }
        };
        meta.skill_file = Some(file_name.clone());
        match frontmatter::parse(&text) {
            None => meta.problems.push(format!(
                "{file_name} has no front matter (a `---` block with `name` and `description`)"
            )),
            Some(Err(problem)) => meta.problems.push(format!("{file_name}: {problem}")),
            Some(Ok(front)) => {
                meta.name = front
                    .get("name")
                    .filter(|v| !v.is_empty())
                    .map(str::to_string);
                meta.description = front
                    .get("description")
                    .filter(|v| !v.is_empty())
                    .map(str::to_string);
                meta.fields = front.fields;
                match &meta.name {
                    None => meta
                        .problems
                        .push(format!("{file_name} front matter has no `name`")),
                    Some(name) => {
                        if let Some(problem) = names::problem(name) {
                            meta.problems.push(format!(
                                "front matter name `{name}` is not a valid skill name: {problem}"
                            ));
                        } else if let Some(id) = id
                            && name != id.as_str()
                        {
                            meta.problems.push(format!("front matter name `{name}` does not match the directory name `{id}`"));
                        }
                    }
                }
                if meta.description.is_none() {
                    meta.problems
                        .push(format!("{file_name} front matter has no `description`"));
                }
            }
        }
        meta
    }

    /// The description squeezed onto one line.
    pub fn summary(&self) -> Option<String> {
        let description = self.description.as_deref()?;
        Some(description.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

/// Whether `dir` looks like a skill: it holds a `SKILL.md` (in any case).
pub fn is_skill_dir(dir: &Path) -> bool {
    find_skill_file(dir).is_some()
}

fn find_skill_file(dir: &Path) -> Option<String> {
    if dir.join(SKILL_FILE).is_file() {
        return Some(SKILL_FILE.to_string());
    }
    fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.eq_ignore_ascii_case(SKILL_FILE))
        .find(|name| dir.join(name).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn reads_name_and_description() {
        let tmp = TempDir::new();
        tmp.write(
            "pdf/SKILL.md",
            "---\nname: pdf\ndescription: Read PDFs.\n  Fill forms.\nlicense: MIT\n---\n# PDF\n",
        );
        let meta = SkillMeta::read(&tmp.path().join("pdf"), Some(&SkillId::new("pdf").unwrap()));
        assert_eq!(meta.name.as_deref(), Some("pdf"));
        assert_eq!(meta.description.as_deref(), Some("Read PDFs. Fill forms."));
        assert_eq!(meta.fields.len(), 3);
        assert!(meta.problems.is_empty(), "{:?}", meta.problems);
    }

    #[test]
    fn reports_problems_without_failing() {
        let tmp = TempDir::new();
        std::fs::create_dir_all(tmp.path().join("empty")).unwrap();
        assert_eq!(
            SkillMeta::read(&tmp.path().join("empty"), None).problems,
            ["no SKILL.md"]
        );

        tmp.write("x/skill.md", "# no front matter\n");
        let meta = SkillMeta::read(&tmp.path().join("x"), None);
        assert_eq!(meta.skill_file.as_deref(), Some("skill.md"));
        assert_eq!(meta.problems.len(), 2);

        tmp.write("y/SKILL.md", "---\nname: other\n---\n");
        let meta = SkillMeta::read(&tmp.path().join("y"), Some(&SkillId::new("y").unwrap()));
        assert_eq!(
            meta.problems,
            [
                "front matter name `other` does not match the directory name `y`",
                "SKILL.md front matter has no `description`"
            ]
        );
    }
}

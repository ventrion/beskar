use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::fingerprint::Fingerprint;
use crate::fsx::{self, PathKind};
use crate::id::SkillId;

pub const SKILL_FILE: &str = "SKILL.md";

/// What Beskar reads out of a skill's `SKILL.md` front matter. Every field is
/// optional: a skill is a directory first, and this is a convenience on top.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillMetadata {
    pub name: Option<String>,
    pub description: Option<String>,
    /// Every other top-level front matter key, values kept as written.
    pub fields: BTreeMap<String, String>,
}

impl SkillMetadata {
    /// Reads the `---` delimited front matter at the top of a `SKILL.md`.
    ///
    /// This handles the shapes skills use in practice: `key: value`, quoted
    /// values, and folded or literal block values. It is not a YAML parser.
    pub fn parse(text: &str) -> SkillMetadata {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut lines = text.lines();
        if lines.next().map(str::trim_end) != Some("---") {
            return SkillMetadata::default();
        }
        let mut front = Vec::new();
        let mut closed = false;
        for line in lines {
            let trimmed = line.trim_end();
            if trimmed == "---" || trimmed == "..." {
                closed = true;
                break;
            }
            front.push(line);
        }
        if !closed {
            return SkillMetadata::default();
        }

        let mut meta = SkillMetadata::default();
        let mut i = 0;
        while i < front.len() {
            let line = front[i];
            i += 1;
            let is_top_level = !line.starts_with([' ', '\t']);
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || !is_top_level {
                continue;
            }
            let Some((key, rest)) = trimmed.split_once(':') else { continue };
            let key = key.trim();
            if key.is_empty() || key.contains(char::is_whitespace) {
                continue;
            }
            let rest = rest.trim();
            let value = if rest.is_empty() || matches!(rest, ">" | ">-" | ">+" | "|" | "|-" | "|+")
            {
                let mut parts = Vec::new();
                while i < front.len()
                    && (front[i].starts_with([' ', '\t']) || front[i].trim().is_empty())
                {
                    let part = front[i].trim();
                    if !part.is_empty() {
                        parts.push(part);
                    }
                    i += 1;
                }
                parts.join(" ")
            } else {
                unquote(rest).to_string()
            };
            match key {
                "name" => meta.name = Some(value),
                "description" => meta.description = Some(value),
                _ => {
                    meta.fields.insert(key.to_string(), value);
                }
            }
        }
        meta
    }

    /// Reads the metadata of the skill at `dir`. `None` when it has no `SKILL.md`.
    pub fn read(dir: &Path) -> Result<Option<SkillMetadata>> {
        let file = dir.join(SKILL_FILE);
        if fsx::path_kind(&file)? == PathKind::Absent {
            return Ok(None);
        }
        let bytes = std::fs::read(&file)
            .map_err(|e| crate::Error::io(format!("cannot read {}", file.display()), &e))?;
        Ok(Some(SkillMetadata::parse(&String::from_utf8_lossy(&bytes))))
    }
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(quote).and_then(|v| v.strip_suffix(quote)) {
            return inner;
        }
    }
    value
}

/// A skill as it exists in the library.
#[derive(Debug, Clone)]
pub struct Skill {
    pub id: SkillId,
    pub path: PathBuf,
    /// `None` when the directory has no `SKILL.md`.
    pub metadata: Option<SkillMetadata>,
    pub fingerprint: Fingerprint,
}

impl Skill {
    pub fn description(&self) -> Option<&str> {
        self.metadata.as_ref()?.description.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_name_and_description() {
        let meta = SkillMetadata::parse(
            "---\nname: playwright\ndescription: Browser automation and testing\n---\n# Body\n",
        );
        assert_eq!(meta.name.as_deref(), Some("playwright"));
        assert_eq!(meta.description.as_deref(), Some("Browser automation and testing"));
        assert!(meta.fields.is_empty());
    }

    #[test]
    fn unquotes_values_and_keeps_other_keys() {
        let meta = SkillMetadata::parse(
            "---\nname: \"pdf\"\ndescription: 'Work with PDFs: read, merge'\nversion: 1.2.0\ntags: [a, b]\n---\n",
        );
        assert_eq!(meta.name.as_deref(), Some("pdf"));
        assert_eq!(meta.description.as_deref(), Some("Work with PDFs: read, merge"));
        assert_eq!(meta.fields["version"], "1.2.0");
        assert_eq!(meta.fields["tags"], "[a, b]");
    }

    #[test]
    fn folds_block_values() {
        let meta = SkillMetadata::parse(
            "---\nname: x\ndescription: >-\n  First line\n  second line\n\n  third\nother: y\n---\n",
        );
        assert_eq!(meta.description.as_deref(), Some("First line second line third"));
        assert_eq!(meta.fields["other"], "y");
    }

    #[test]
    fn ignores_nested_keys_and_comments() {
        let meta =
            SkillMetadata::parse("---\n# comment\nname: x\nmetadata:\n  author: someone\n---\n");
        assert_eq!(meta.name.as_deref(), Some("x"));
        assert_eq!(meta.fields["metadata"], "author: someone");
        assert!(!meta.fields.contains_key("author"));
    }

    #[test]
    fn no_front_matter_or_unclosed_front_matter_yields_nothing() {
        assert_eq!(SkillMetadata::parse("# Just markdown\n"), SkillMetadata::default());
        assert_eq!(SkillMetadata::parse("---\nname: x\n"), SkillMetadata::default());
        assert_eq!(SkillMetadata::parse(""), SkillMetadata::default());
    }

    #[test]
    fn tolerates_a_byte_order_mark_and_crlf() {
        let meta = SkillMetadata::parse("\u{feff}---\r\nname: x\r\n---\r\n");
        assert_eq!(meta.name.as_deref(), Some("x"));
    }
}

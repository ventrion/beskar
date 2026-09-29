//! Best-effort metadata extraction from `SKILL.md`.
//!
//! Skills are primarily *managed directories* — beskar does not require
//! any particular internal format. When a skill does ship a `SKILL.md`
//! with a YAML-ish frontmatter block, we pull out `name` and `description`
//! for display. Parsing is deliberately line-based and tolerant: unknown
//! keys are ignored, malformed blocks cost nothing.

use std::path::Path;

use crate::util::read_to_string;

#[derive(Debug, Clone, Default)]
pub struct SkillMeta {
    pub name: Option<String>,
    pub description: Option<String>,
}

/// Extract metadata from a skill directory (reads `SKILL.md` if present).
pub fn from_dir(dir: &Path) -> SkillMeta {
    from_skill_md(&dir.join("SKILL.md"))
}

pub fn from_skill_md(path: &Path) -> SkillMeta {
    read_to_string(path).map(|t| parse(&t)).unwrap_or_default()
}

/// Parse frontmatter from SKILL.md text: a `---` delimited block at the top.
pub fn parse(text: &str) -> SkillMeta {
    let mut meta = SkillMeta::default();
    let mut lines = text.lines();
    if lines.next().map(|l| l.trim()) != Some("---") {
        return meta;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break; // end of frontmatter
        }
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            // A multi-line value (like a tag list) — skip until next simple key.
            continue;
        };
        let value = clean_value(value);
        match key.trim() {
            "name" if !value.is_empty() && meta.name.is_none() => meta.name = Some(value),
            "description" if !value.is_empty() && meta.description.is_none() => {
                meta.description = Some(value);
            }
            _ => {}
        }
    }
    meta
}

fn clean_value(v: &str) -> String {
    let v = v.trim();
    let unquoted = if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        &v[1..v.len() - 1]
    } else {
        v
    };
    // Collapse whitespace runs so single-line display stays tidy.
    unquoted.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter() {
        let m = parse("---\nname: playwright\ndescription: \"Browser automation & testing\"\nversion: 1.2.0\n---\n\n# Playwright\nbody");
        assert_eq!(m.name.as_deref(), Some("playwright"));
        assert_eq!(m.description.as_deref(), Some("Browser automation & testing"));
    }

    #[test]
    fn tolerates_missing_or_odd_frontmatter() {
        assert!(parse("# no frontmatter\nname: x").name.is_none());
        assert_eq!(parse("---\nno colon here\n---\n").name, None);
        assert_eq!(parse("").name, None);
        // Multi-line lists are skipped without breaking other keys.
        let m = parse("---\ntags:\n  - a\n  - b\nname: keep\n---\n");
        assert_eq!(m.name.as_deref(), Some("keep"));
    }

    #[test]
    fn no_frontmatter_but_early_keys() {
        // Missing closing --- still stops at blank non-key lines; keys found first win.
        let m = parse("---\nname: open\ndescription: never closed\n");
        assert_eq!(m.name.as_deref(), Some("open"));
    }
}

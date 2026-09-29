//! Profiles: named sets of skills, stored as `profiles/<name>.bsk` in the
//! library.
//!
//! ```text
//! # Beskar profile. Each `skill: <name>` line adds a skill from the library.
//! description: Everyday software development
//! skill: code-review
//! skill: git
//! ```

use std::path::{Path, PathBuf};

use bsk::Document;

use crate::names::{ProfileName, SkillId};
use crate::{Error, Result};

pub const PROFILE_EXT: &str = "bsk";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: ProfileName,
    pub path: PathBuf,
    pub description: Option<String>,
    /// Skills in file order, without duplicates.
    pub skills: Vec<SkillId>,
}

impl Profile {
    /// Parse a profile file. The profile's name is its file name, so the
    /// file itself has no `name` key.
    pub fn parse(name: ProfileName, path: &Path, text: &str) -> Result<Profile> {
        let fail = |diagnostic: bsk::Error| Error::bsk(path, diagnostic);
        let doc = Document::parse(text).map_err(|diagnostic| {
            let error = fail(diagnostic);
            // YAML and TOML habits usually come with a plural `skills` key.
            if text
                .lines()
                .any(|line| line.trim_start().starts_with("skills"))
            {
                error.hint("a profile lists each skill on a line of its own: `skill: git`")
            } else {
                error
            }
        })?;
        if let Some(section) = doc.sections().next() {
            return Err(fail(section.error("profiles have no sections").with_help(
                "write `description: ...` and one `skill: <name>` line per skill",
            )));
        }
        let root = doc.root();
        for entry in root.entries() {
            match entry.key() {
                "name" => {
                    return Err(fail(entry.key_error("profiles have no `name` key").with_help(format!(
                        "a profile is named after its file ({name}.{PROFILE_EXT}); remove this line"
                    ))));
                }
                "skills" => {
                    return Err(fail(
                        entry
                            .key_error("unknown key `skills`")
                            .with_help("write one `skill: <name>` line per skill"),
                    ));
                }
                _ => {}
            }
        }
        root.check_keys(&["description", "skill"]).map_err(fail)?;
        let description = root
            .get("description")
            .map_err(fail)?
            .map(|entry| entry.value().to_string())
            .filter(|text| !text.is_empty());

        let mut skills: Vec<SkillId> = Vec::new();
        let mut lines: Vec<usize> = Vec::new();
        for entry in root.all("skill") {
            if entry.value().is_empty() {
                return Err(fail(entry.error("`skill:` needs a skill name").with_help(
                    "write the skill's name after `skill:`, or delete the line",
                )));
            }
            let skill = SkillId::new(entry.value()).map_err(|error| {
                let diagnostic = entry.error(error.message);
                fail(match error.hints.first() {
                    Some(hint) => diagnostic.with_help(hint.clone()),
                    None => diagnostic,
                })
            })?;
            if let Some(index) = skills.iter().position(|s| *s == skill) {
                return Err(fail(
                    entry
                        .error(format!("`{skill}` is listed twice"))
                        .with_help(format!(
                            "line {} already lists it; remove one of the lines",
                            lines[index]
                        )),
                ));
            }
            skills.push(skill);
            lines.push(entry.line());
        }
        Ok(Profile {
            name,
            path: path.to_path_buf(),
            description,
            skills,
        })
    }

    /// The text of a new profile file. The description is squeezed onto one
    /// line.
    pub fn template(description: Option<&str>, skills: &[SkillId]) -> Result<String> {
        let description =
            description.map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "));
        let description = description.as_deref();
        let mut doc = Document::new();
        doc.push_comment(
            "Beskar profile. Each `skill: <name>` line adds a skill from the library.",
        );
        doc.push_comment("`beskar help format` describes the syntax.");
        let entries: Vec<(&str, &str)> = description
            .filter(|text| !text.is_empty())
            .map(|text| ("description", text))
            .into_iter()
            .chain(skills.iter().map(|skill| ("skill", skill.as_str())))
            .collect();
        if !entries.is_empty() {
            doc.push_blank();
        }
        for (key, value) in entries {
            doc.push_entry(key, value)
                .map_err(|e| Error::invalid(format!("cannot store {key}: {}", e.message)))?;
        }
        Ok(doc.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Profile> {
        Profile::parse(
            ProfileName::new("coding").unwrap(),
            Path::new("/lib/profiles/coding.bsk"),
            text,
        )
    }

    #[test]
    fn parses_description_and_skills() {
        let profile =
            parse("# comment\ndescription: Everyday coding\nskill: git\n\nskill: code-review\n")
                .unwrap();
        assert_eq!(profile.description.as_deref(), Some("Everyday coding"));
        let skills: Vec<&str> = profile.skills.iter().map(SkillId::as_str).collect();
        assert_eq!(skills, ["git", "code-review"]);
    }

    #[test]
    fn an_empty_profile_is_valid() {
        let profile = parse("").unwrap();
        assert!(profile.skills.is_empty());
        assert_eq!(profile.description, None);
    }

    #[test]
    fn yaml_habits_get_specific_advice() {
        let error = parse("name: coding\n").unwrap_err();
        assert_eq!(error.message, "profiles have no `name` key");
        assert_eq!(
            error.hints,
            ["a profile is named after its file (coding.bsk); remove this line"]
        );

        let error = parse("skills:\n").unwrap_err();
        assert_eq!(error.hints, ["write one `skill: <name>` line per skill"]);

        let error = parse("skills:\n  - git\n").unwrap_err();
        assert_eq!(error.message, "BSK has no `-` list items");
        assert_eq!(error.diagnostic.as_ref().unwrap().line, 2);
    }

    #[test]
    fn invalid_and_duplicate_skills_are_rejected() {
        let error = parse("skill: Code Review\n").unwrap_err();
        assert!(
            error
                .message
                .starts_with("`Code Review` is not a valid skill name")
        );
        assert_eq!(error.hints, ["try `code-review`"]);

        let error = parse("skill: git\nskill: pdf\nskill: git\n").unwrap_err();
        assert_eq!(error.message, "`git` is listed twice");
        assert_eq!(error.diagnostic.as_ref().unwrap().line, 3);
        assert_eq!(
            error.hints,
            ["line 1 already lists it; remove one of the lines"]
        );
    }

    #[test]
    fn template_round_trips() {
        let skills = [SkillId::new("git").unwrap(), SkillId::new("pdf").unwrap()];
        let text = Profile::template(Some("Research work"), &skills).unwrap();
        let profile = parse(&text).unwrap();
        assert_eq!(profile.description.as_deref(), Some("Research work"));
        assert_eq!(profile.skills, skills);
        assert!(
            parse(&Profile::template(None, &[]).unwrap())
                .unwrap()
                .skills
                .is_empty()
        );
        let text = Profile::template(Some("  two\n lines  "), &[]).unwrap();
        assert_eq!(
            parse(&text).unwrap().description.as_deref(),
            Some("two lines")
        );
    }
}

//! Profiles: named sets of skills.
//!
//! A profile is one small bsk file in the library, `profiles/<name>.bsk`:
//!
//! ```text
//! # Everyday software engineering.
//! description Skills for writing, reviewing and testing code
//!
//! skill code-review
//! skill git
//! skill testing
//! ```
//!
//! The file name is the profile's name, so the two can never disagree. A
//! profile refers to skills by id and never copies them.

use std::collections::BTreeSet;

use bsk::{Cardinality, Diagnostics, Document, Schema};

use crate::ids::{ProfileName, SkillId};

/// The file extension of a profile.
pub const EXTENSION: &str = "bsk";

/// A named set of skills.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    /// The profile's name.
    pub name: ProfileName,
    /// A line saying what the profile is for.
    pub description: Option<String>,
    /// The skills it selects. Order and repetition in the file carry no meaning.
    pub skills: BTreeSet<SkillId>,
}

impl Profile {
    fn schema() -> Schema {
        Schema::new()
            .key("description", Cardinality::Optional)
            .key("skill", Cardinality::Many)
    }

    /// Reads a profile from the text of its file.
    pub fn parse(name: ProfileName, text: &str) -> Result<Profile, Diagnostics> {
        let doc = Document::parse(text)?;
        Profile::schema().check(&doc)?;
        let root = doc.root();
        let mut problems = Vec::new();
        let mut skills = BTreeSet::new();
        for entry in root.all("skill") {
            match SkillId::parse(entry.value()) {
                Ok(id) => {
                    skills.insert(id);
                }
                Err(error) => {
                    let diagnostic = entry.diagnostic(error.message());
                    problems.push(match (diagnostic.hint(), error.hint()) {
                        (None, Some(hint)) => diagnostic.with_hint(hint),
                        _ => diagnostic,
                    });
                }
            }
        }
        if let Some(problems) = Diagnostics::from_vec(problems) {
            return Err(problems);
        }
        let description = root
            .value("description")
            .filter(|d| !d.is_empty())
            .map(str::to_string);
        Ok(Profile {
            name,
            description,
            skills,
        })
    }

    /// The text of a new profile file. Fails if the description cannot be stored in a bsk file.
    pub fn render_new(
        name: &ProfileName,
        description: Option<&str>,
        skills: &BTreeSet<SkillId>,
    ) -> Result<String, bsk::ValueError> {
        let mut doc = Document::new();
        doc.push_comment(&format!(
            "Profile '{name}'. One 'skill <id>' line per skill; see 'beskar help format'."
        ));
        doc.push_blank();
        if let Some(text) = description.map(str::trim).filter(|t| !t.is_empty()) {
            let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
            doc.push_entry("description", &single_line)?;
        }
        for skill in skills {
            doc.push_entry("skill", skill.as_str())?;
        }
        Ok(doc.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> ProfileName {
        ProfileName::parse(text).unwrap()
    }

    fn ids(list: &[&str]) -> BTreeSet<SkillId> {
        list.iter().map(|s| SkillId::parse(s).unwrap()).collect()
    }

    #[test]
    fn reads_description_and_skills() {
        let text = "# Everyday engineering\ndescription Write, review, test\n\nskill git\nskill code-review\n";
        let profile = Profile::parse(name("coding"), text).unwrap();
        assert_eq!(profile.description.as_deref(), Some("Write, review, test"));
        assert_eq!(profile.skills, ids(&["code-review", "git"]));
    }

    #[test]
    fn an_empty_profile_is_valid() {
        let profile = Profile::parse(name("minimal"), "# nothing yet\n").unwrap();
        assert!(profile.skills.is_empty());
        assert_eq!(profile.description, None);
    }

    #[test]
    fn repeated_skills_collapse_into_one() {
        let profile = Profile::parse(name("p"), "skill a\nskill b\nskill a\n").unwrap();
        assert_eq!(profile.skills.len(), 2);
    }

    #[test]
    fn a_description_may_be_empty_or_missing() {
        assert_eq!(
            Profile::parse(name("p"), "description\n")
                .unwrap()
                .description,
            None
        );
    }

    #[test]
    fn unknown_keys_are_rejected_with_a_suggestion() {
        let problems = Profile::parse(name("p"), "skils git\n").unwrap_err();
        assert_eq!(problems.first().message(), "unknown key 'skils'");
        assert_eq!(problems.first().hint(), Some("did you mean 'skill'?"));
    }

    #[test]
    fn two_descriptions_are_rejected() {
        assert!(Profile::parse(name("p"), "description a\ndescription b\n").is_err());
    }

    #[test]
    fn invalid_skill_ids_point_at_the_value() {
        let problems = Profile::parse(
            name("p"),
            "skill good\nskill = bad\nskill has space\nskill\n",
        )
        .unwrap_err();
        let found: Vec<(usize, usize)> = problems.iter().map(|d| (d.line(), d.column())).collect();
        assert_eq!(found, [(2, 7), (3, 7), (4, 1)]);
        assert!(problems.first().hint().unwrap().contains("no '=' or ':'"));
    }

    #[test]
    fn a_trailing_comment_is_part_of_the_value_and_is_caught() {
        let problems = Profile::parse(name("p"), "skill git # the vcs\n").unwrap_err();
        assert_eq!(problems.first().column(), 7);
        assert!(
            problems
                .first()
                .message()
                .contains("invalid skill id 'git # the vcs'")
        );
    }

    #[test]
    fn syntax_errors_come_through() {
        let problems = Profile::parse(name("p"), "skill: git\n").unwrap_err();
        assert_eq!(
            problems.first().message(),
            "unexpected ':' after key 'skill'"
        );
    }

    #[test]
    fn new_profiles_render_sorted_and_parse_back() {
        let skills = ids(&["testing", "git", "code-review"]);
        let text =
            Profile::render_new(&name("coding"), Some("  Write,\n review  "), &skills).unwrap();
        assert_eq!(
            text,
            "# Profile 'coding'. One 'skill <id>' line per skill; see 'beskar help format'.\n\ndescription Write, review\nskill code-review\nskill git\nskill testing\n"
        );
        let profile = Profile::parse(name("coding"), &text).unwrap();
        assert_eq!(profile.skills, skills);
        assert_eq!(profile.description.as_deref(), Some("Write, review"));
    }

    #[test]
    fn a_new_empty_profile_has_only_the_header() {
        let text = Profile::render_new(&name("empty"), None, &BTreeSet::new()).unwrap();
        assert_eq!(
            text,
            "# Profile 'empty'. One 'skill <id>' line per skill; see 'beskar help format'.\n\n"
        );
    }
}

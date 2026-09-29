use beskar_lines::Document;

use crate::id::{ProfileId, SkillId};

pub const PROFILE_EXTENSION: &str = "bsk";
const KEYS: [&str; 2] = ["description", "skill"];

/// A named set of skills. Profiles refer to skills by name and never copy them.
///
/// The file is a `.bsk` file in the library's `profiles/` directory, and its
/// name is the profile's identity:
///
/// ```text
/// description Everyday software engineering
/// skill code-review
/// skill git
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub id: ProfileId,
    pub description: Option<String>,
    /// In file order, without duplicates.
    pub skills: Vec<SkillId>,
}

impl Profile {
    pub fn parse(id: ProfileId, text: &str) -> Result<Profile, beskar_lines::Error> {
        let doc = Document::parse(text)?;
        let mut description: Option<(usize, String)> = None;
        let mut skills: Vec<SkillId> = Vec::new();
        for node in doc.nodes() {
            let entry = node.entry;
            if let Some(child) = node.children.first() {
                return Err(child.error("profiles have no nested entries; remove the indent"));
            }
            match entry.key() {
                "description" => {
                    if let Some((first, _)) = &description {
                        return Err(entry
                            .error(format!("`description` is set twice (first on line {first})")));
                    }
                    description = Some((entry.line(), entry.value().to_string()));
                }
                "skill" => {
                    let skill = SkillId::new(entry.value()).map_err(|e| {
                        let error =
                            entry.error(format!("`{}` is not a valid skill name", entry.value()));
                        match e.hint() {
                            Some(hint) => error.with_hint(hint),
                            None => error,
                        }
                    })?;
                    if !skills.contains(&skill) {
                        skills.push(skill);
                    }
                }
                _ => return Err(entry.unknown_key(&KEYS)),
            }
        }
        Ok(Profile {
            id,
            description: description.map(|(_, text)| text).filter(|t| !t.is_empty()),
            skills,
        })
    }

    /// The text of a new, empty profile file.
    pub fn template(description: Option<&str>) -> Result<String, beskar_lines::Error> {
        let mut doc = Document::new();
        doc.push_comment(
            "Beskar profile. Add one `skill <name>` line per skill, then run\n\
             `beskar repo update` in the repositories that use it.",
        );
        if let Some(text) = description.filter(|t| !t.is_empty()) {
            doc.push("description", text)?;
        }
        Ok(doc.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> ProfileId {
        ProfileId::new("coding").unwrap()
    }

    fn skills(profile: &Profile) -> Vec<&str> {
        profile.skills.iter().map(SkillId::as_str).collect()
    }

    #[test]
    fn parses_description_and_skills() {
        let profile = Profile::parse(
            id(),
            "# c\ndescription Everyday engineering\nskill git\n\nskill code-review\n",
        )
        .unwrap();
        assert_eq!(profile.description.as_deref(), Some("Everyday engineering"));
        assert_eq!(skills(&profile), ["git", "code-review"]);
    }

    #[test]
    fn an_empty_file_is_an_empty_profile() {
        let profile = Profile::parse(id(), "").unwrap();
        assert!(profile.skills.is_empty());
        assert_eq!(profile.description, None);
    }

    #[test]
    fn duplicate_skill_lines_collapse() {
        let profile = Profile::parse(id(), "skill git\nskill testing\nskill git\n").unwrap();
        assert_eq!(skills(&profile), ["git", "testing"]);
    }

    #[test]
    fn typos_in_keys_are_caught_with_a_suggestion() {
        let error = Profile::parse(id(), "description d\nskils git\n").unwrap_err();
        assert_eq!(error.line(), 2);
        assert!(error.message().contains("did you mean `skill`?"));
    }

    #[test]
    fn invalid_skill_names_are_pinned_to_their_line() {
        let error = Profile::parse(id(), "skill git\nskill Code Review\n").unwrap_err();
        assert_eq!(error.line(), 2);
        assert!(error.message().contains("not a valid skill name"));
    }

    #[test]
    fn a_second_description_is_an_error() {
        let error = Profile::parse(id(), "description a\ndescription b\n").unwrap_err();
        assert!(error.message().contains("set twice"));
    }

    #[test]
    fn yaml_habits_are_explained() {
        let error = Profile::parse(id(), "name: coding\n").unwrap_err();
        assert!(error.hint().unwrap().contains("no `:`"));
    }

    #[test]
    fn the_template_is_a_valid_profile() {
        let text = Profile::template(Some("Docs work")).unwrap();
        let profile = Profile::parse(id(), &text).unwrap();
        assert_eq!(profile.description.as_deref(), Some("Docs work"));
        assert!(Profile::parse(id(), &Profile::template(None).unwrap()).is_ok());
    }
}

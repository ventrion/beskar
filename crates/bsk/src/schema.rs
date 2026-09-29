//! Checks a document against the keys and sections a program expects.
//!
//! The parser knows only syntax. A schema adds the rules that catch typos and
//! contradictions, and it phrases every complaint so that a person or a coding
//! agent can fix the file without guessing.

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::document::{Document, Scope};
use crate::suggest::closest;

/// How often a key may appear in its scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cardinality {
    /// Exactly once.
    Required,
    /// Zero or one times.
    Optional,
    /// Any number of times. The values form a list.
    Many,
}

/// Whether a section header carries a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Naming {
    /// `[kind name]`. The name is required and must be unique among sections of the same kind.
    Named,
    /// `[kind]`. A name is not allowed.
    Unnamed,
}

type KeySpec = (&'static str, Cardinality);

#[derive(Clone, Debug)]
struct SectionSpec {
    kind: &'static str,
    naming: Naming,
    keys: Vec<KeySpec>,
}

/// The keys and sections a file may contain.
///
/// ```
/// use bsk::{Cardinality::*, Document, Naming, Schema};
///
/// let schema = Schema::new()
///     .key("version", Required)
///     .section("repo", Naming::Named, &[("profile", Many), ("synced", Optional)]);
///
/// let ok: Document = "version 1\n[repo /a]\nprofile x\n".parse().unwrap();
/// assert!(schema.check(&ok).is_ok());
///
/// let typo: Document = "version 1\n[repo /a]\nprofle x\n".parse().unwrap();
/// let problems = schema.check(&typo).unwrap_err();
/// assert_eq!(problems.first().hint(), Some("did you mean 'profile'?"));
/// ```
#[derive(Clone, Debug, Default)]
pub struct Schema {
    root: Vec<KeySpec>,
    sections: Vec<SectionSpec>,
}

impl Schema {
    /// A schema that allows nothing yet.
    pub fn new() -> Self {
        Schema::default()
    }

    /// Allows a key before the first section header.
    #[must_use]
    pub fn key(mut self, name: &'static str, cardinality: Cardinality) -> Self {
        self.root.push((name, cardinality));
        self
    }

    /// Allows a kind of section with the given keys.
    #[must_use]
    pub fn section(
        mut self,
        kind: &'static str,
        naming: Naming,
        keys: &[(&'static str, Cardinality)],
    ) -> Self {
        self.sections.push(SectionSpec {
            kind,
            naming,
            keys: keys.to_vec(),
        });
        self
    }

    /// Reports every way `doc` breaks the schema.
    pub fn check(&self, doc: &Document) -> Result<(), Diagnostics> {
        let mut problems = Vec::new();
        self.check_scope(doc.root(), &self.root, &mut problems);
        let mut seen: Vec<(&str, &str, usize)> = Vec::new();
        for scope in doc.sections() {
            let (Some(kind), Some(name)) = (scope.kind(), scope.name()) else {
                continue;
            };
            let Some(spec) = self.sections.iter().find(|s| s.kind == kind) else {
                problems.push(self.unknown_section(scope, kind));
                continue;
            };
            match (spec.naming, name.is_empty()) {
                (Naming::Named, true) => problems.push(
                    scope
                        .diagnostic(format!("section [{kind}] needs a name"))
                        .with_hint(format!("write it as [{kind} <name>]")),
                ),
                (Naming::Unnamed, false) => problems.push(
                    scope
                        .diagnostic(format!("section [{kind}] does not take a name"))
                        .with_hint(format!("write it as [{kind}]")),
                ),
                _ => {}
            }
            if let Some(&(_, _, first)) = seen.iter().find(|&&(k, n, _)| k == kind && n == name) {
                let header = if name.is_empty() {
                    format!("[{kind}]")
                } else {
                    format!("[{kind} {name}]")
                };
                problems.push(
                    scope
                        .diagnostic(format!("section {header} appears twice"))
                        .with_hint(format!("the first one is on line {first}; merge the two")),
                );
            } else {
                seen.push((kind, name, scope.line()));
            }
            self.check_scope(scope, &spec.keys, &mut problems);
        }
        match Diagnostics::from_vec(problems) {
            Some(problems) => Err(problems),
            None => Ok(()),
        }
    }

    fn check_scope(&self, scope: Scope<'_>, keys: &[KeySpec], problems: &mut Vec<Diagnostic>) {
        let mut first_seen: Vec<(&str, usize)> = Vec::new();
        for entry in scope.entries() {
            let key = entry.key();
            let Some(&(_, cardinality)) = keys.iter().find(|(name, _)| *name == key) else {
                problems.push(self.unknown_key(
                    entry.key_diagnostic(format!("unknown key '{key}'")),
                    scope,
                    keys,
                    key,
                ));
                continue;
            };
            match first_seen.iter().find(|(k, _)| *k == key) {
                Some(&(_, first)) if cardinality != Cardinality::Many => problems.push(
                    entry
                        .key_diagnostic(format!("'{key}' appears more than once"))
                        .with_hint(format!("the first one is on line {first}; keep only one")),
                ),
                Some(_) => {}
                None => first_seen.push((key, entry.line())),
            }
        }
        for &(name, cardinality) in keys {
            if cardinality == Cardinality::Required && !first_seen.iter().any(|(k, _)| *k == name) {
                problems.push(
                    scope
                        .diagnostic(format!("missing required key '{name}'"))
                        .with_hint(format!("add a line such as '{name} <value>'")),
                );
            }
        }
    }

    fn unknown_key(
        &self,
        diagnostic: Diagnostic,
        scope: Scope<'_>,
        keys: &[KeySpec],
        key: &str,
    ) -> Diagnostic {
        if keys.is_empty() {
            let hint = if scope.is_root() && !self.sections.is_empty() {
                let kinds: Vec<String> = self
                    .sections
                    .iter()
                    .map(|s| format!("[{}]", s.kind))
                    .collect();
                format!(
                    "entries here must sit under a section header: {}",
                    kinds.join(", ")
                )
            } else {
                "this part of the file takes no entries".to_string()
            };
            return diagnostic.with_hint(hint);
        }
        match closest(key, keys.iter().map(|(name, _)| *name)) {
            Some(near) => diagnostic.with_hint(format!("did you mean '{near}'?")),
            None => {
                let names: Vec<&str> = keys.iter().map(|(name, _)| *name).collect();
                diagnostic.with_hint(format!("known keys: {}", names.join(", ")))
            }
        }
    }

    fn unknown_section(&self, scope: Scope<'_>, kind: &str) -> Diagnostic {
        let diagnostic = scope.diagnostic(format!("unknown section kind '{kind}'"));
        if self.sections.is_empty() {
            return diagnostic.with_hint("this file has no sections");
        }
        match closest(kind, self.sections.iter().map(|s| s.kind)) {
            Some(near) => diagnostic.with_hint(format!("did you mean [{near}]?")),
            None => {
                let kinds: Vec<&str> = self.sections.iter().map(|s| s.kind).collect();
                diagnostic.with_hint(format!("known sections: {}", kinds.join(", ")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Cardinality::*;
    use super::*;

    fn registry_schema() -> Schema {
        Schema::new().key("version", Required).section(
            "repo",
            Naming::Named,
            &[("profile", Many), ("synced", Optional), ("skill", Many)],
        )
    }

    fn check(schema: &Schema, text: &str) -> Vec<String> {
        match schema.check(&Document::parse(text).unwrap()) {
            Ok(()) => Vec::new(),
            Err(all) => all
                .iter()
                .map(|d| {
                    format!(
                        "{}:{} {}{}",
                        d.line(),
                        d.column(),
                        d.message(),
                        d.hint().map(|h| format!(" [{h}]")).unwrap_or_default()
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn accepts_a_conforming_document() {
        let text = "version 1\n[repo /a]\nprofile x\nprofile y\nsynced t\n[repo /b]\n";
        assert!(check(&registry_schema(), text).is_empty());
    }

    #[test]
    fn suggests_the_nearest_key() {
        let out = check(&registry_schema(), "version 1\n[repo /a]\nprofle x\n");
        assert_eq!(out, ["3:1 unknown key 'profle' [did you mean 'profile'?]"]);
    }

    #[test]
    fn lists_known_keys_when_nothing_is_close() {
        let out = check(&registry_schema(), "version 1\n[repo /a]\nzzzzzz x\n");
        assert_eq!(
            out,
            ["3:1 unknown key 'zzzzzz' [known keys: profile, synced, skill]"]
        );
    }

    #[test]
    fn single_valued_keys_may_not_repeat() {
        let out = check(
            &registry_schema(),
            "version 1\n[repo /a]\nsynced a\nsynced b\n",
        );
        assert_eq!(
            out,
            ["4:1 'synced' appears more than once [the first one is on line 3; keep only one]"]
        );
    }

    #[test]
    fn required_keys_must_be_present() {
        let out = check(&registry_schema(), "[repo /a]\n");
        assert_eq!(
            out,
            ["1:1 missing required key 'version' [add a line such as 'version <value>']"]
        );
    }

    #[test]
    fn missing_required_key_inside_a_section_points_at_its_header() {
        let schema = Schema::new().section("repo", Naming::Named, &[("path", Required)]);
        let out = check(&schema, "\n[repo /a]\n");
        assert_eq!(
            out,
            ["2:1 missing required key 'path' [add a line such as 'path <value>']"]
        );
    }

    #[test]
    fn unknown_sections_get_suggestions() {
        let out = check(&registry_schema(), "version 1\n[repos /a]\n");
        assert_eq!(
            out,
            ["2:1 unknown section kind 'repos' [did you mean [repo]?]"]
        );
        let out = check(&registry_schema(), "version 1\n[qqqqqq /a]\n");
        assert_eq!(
            out,
            ["2:1 unknown section kind 'qqqqqq' [known sections: repo]"]
        );
    }

    #[test]
    fn section_names_follow_the_naming_rule() {
        let out = check(&registry_schema(), "version 1\n[repo]\n");
        assert_eq!(
            out,
            ["2:1 section [repo] needs a name [write it as [repo <name>]]"]
        );
        let schema = Schema::new().section("settings", Naming::Unnamed, &[]);
        assert_eq!(
            check(&schema, "[settings x]\n"),
            ["1:1 section [settings] does not take a name [write it as [settings]]"]
        );
    }

    #[test]
    fn duplicate_section_names_are_rejected() {
        let out = check(
            &registry_schema(),
            "version 1\n[repo /a]\n[repo /b]\n[repo /a]\n",
        );
        assert_eq!(
            out,
            ["4:1 section [repo /a] appears twice [the first one is on line 2; merge the two]"]
        );
    }

    #[test]
    fn a_duplicate_section_without_a_name_prints_a_clean_header() {
        let schema = Schema::new().section("settings", Naming::Unnamed, &[]);
        assert_eq!(
            check(&schema, "[settings]\n[settings]\n"),
            ["2:1 section [settings] appears twice [the first one is on line 1; merge the two]"]
        );
        // A section that must be named is reported for both problems, each without a stray space.
        let out = check(&registry_schema(), "version 1\n[repo]\n[repo]\n");
        assert_eq!(
            out,
            [
                "2:1 section [repo] needs a name [write it as [repo <name>]]",
                "3:1 section [repo] needs a name [write it as [repo <name>]]",
                "3:1 section [repo] appears twice [the first one is on line 2; merge the two]",
            ]
        );
    }

    #[test]
    fn entries_in_a_file_that_only_has_sections_explain_where_they_go() {
        let schema = Schema::new().section("repo", Naming::Named, &[]);
        let out = check(&schema, "profile x\n");
        assert_eq!(
            out,
            ["1:1 unknown key 'profile' [entries here must sit under a section header: [repo]]"]
        );
    }

    #[test]
    fn reports_every_problem_in_position_order() {
        let out = check(&registry_schema(), "bogus 1\n[repo /a]\nprofle x\n");
        assert_eq!(out.len(), 3);
        assert!(out[0].starts_with("1:1 unknown key 'bogus'"));
        assert!(
            out[1].starts_with("1:1 missing required key 'version'") || out[1].starts_with("3:1")
        );
    }

    #[test]
    fn flat_schema_with_many_keys() {
        let schema = Schema::new()
            .key("description", Optional)
            .key("skill", Many);
        assert!(check(&schema, "# c\ndescription d\nskill a\nskill b\nskill a\n").is_empty());
        let out = check(&schema, "skil a\n");
        assert_eq!(out, ["1:1 unknown key 'skil' [did you mean 'skill'?]"]);
    }
}

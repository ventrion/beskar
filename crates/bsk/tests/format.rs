//! Tests of the `bsk` public interface: what a caller can rely on.

use std::collections::BTreeMap;

use bsk::{Cardinality, Document, Naming, Schema};

/// Small deterministic generator, so failures reproduce without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const WORDS: &[&str] = &[
    "alpha",
    "beta",
    "gamma",
    "delta",
    "code-review",
    "git",
    "x",
    "my_key",
    "A1",
];
const TEXTS: &[&str] = &[
    "plain",
    "two words",
    "with # hash",
    "C# tools",
    "a = b",
    "key: value",
    "[bracket]",
    "\"quoted\"",
    "/home/me/my projects/api",
    "ünïcode ✓",
    "tab\tinside",
    "trailing ]",
    "",
];

fn random_text(rng: &mut Rng) -> String {
    let mut out = String::new();
    for _ in 0..rng.below(20) {
        match rng.below(10) {
            0 => out.push('\n'),
            1 => out.push_str("\r\n"),
            2 => out.push('#'),
            3 => out.push('['),
            4 => out.push(']'),
            5 => out.push(' '),
            6 => out.push('\t'),
            7 => out.push_str(rng.pick(WORDS)),
            8 => out.push(*rng.pick(&[
                ':', '=', '-', '"', 'é', '\u{7}', '\r', '\u{85}', '\u{9b}', '\u{a0}', '\u{200b}',
            ])),
            _ => out.push_str(rng.pick(TEXTS)),
        }
    }
    out
}

fn random_line(rng: &mut Rng) -> String {
    let indent = rng.pick(&["", "", "  ", "\t"]);
    let gap = rng.pick(&[" ", "  ", "\t", "      "]);
    let trail = rng.pick(&["", "", "  "]);
    match rng.below(6) {
        0 => String::new(),
        1 => format!("{indent}# {}", rng.pick(TEXTS)),
        2 => format!(
            "{indent}[{} {}]{trail}",
            rng.pick(WORDS),
            rng.pick(&["/a", "/b c", "x]"])
        ),
        _ => {
            let value = rng.pick(TEXTS).trim();
            if value.is_empty() {
                format!("{indent}{}{trail}", rng.pick(WORDS))
            } else {
                format!("{indent}{}{gap}{value}{trail}", rng.pick(WORDS))
            }
        }
    }
}

#[test]
fn well_formed_text_prints_back_byte_for_byte() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for _ in 0..1000 {
        let mut text = String::new();
        if rng.below(6) == 0 {
            text.push('\u{feff}');
        }
        for _ in 0..rng.below(15) {
            text.push_str(&random_line(&mut rng));
            text.push_str(rng.pick(&["\n", "\n", "\r\n"]));
        }
        if rng.below(4) == 0 {
            text.push_str(&random_line(&mut rng));
        }
        let doc = Document::parse(&text).unwrap_or_else(|e| panic!("{e}\n{text:?}"));
        assert_eq!(doc.to_string(), text);
    }
}

#[test]
fn arbitrary_text_never_panics_and_errors_point_inside_the_file() {
    let mut rng = Rng(0xDEADBEEFCAFEF00D);
    let mut parsed = 0;
    for _ in 0..3000 {
        let text = random_text(&mut rng);
        let line_count = text.split('\n').count();
        match Document::parse(&text) {
            Ok(doc) => {
                parsed += 1;
                let printed = doc.to_string();
                assert_eq!(printed, text);
                assert_eq!(Document::parse(&printed).unwrap(), doc);
            }
            Err(problems) => {
                for problem in &problems {
                    assert!(
                        problem.line() >= 1 && problem.line() <= line_count,
                        "{problem} in {text:?}"
                    );
                    assert!(problem.column() >= 1);
                    let rendered = problem.render("f.bsk", &text);
                    assert!(rendered.starts_with("error: "));
                    assert!(
                        !rendered.contains(|c: char| c.is_control() && c != '\n'),
                        "raw control character in {rendered:?}"
                    );
                }
            }
        }
    }
    assert!(parsed > 300, "only {parsed} random texts parsed");
}

/// Applies random edits to a document and to a plain model of it, and requires them to agree.
#[test]
fn edits_agree_with_a_simple_model() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let keys = ["skill", "profile", "description"];
    let values = ["a", "b", "c", "d two words", "e", ""];
    for round in 0..200 {
        let mut doc = Document::parse("# header\n\ndescription first\nskill b\nskill a\n").unwrap();
        let mut model: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        model.insert("description", vec!["first".into()]);
        model.insert("skill", vec!["b".into(), "a".into()]);

        for step in 0..30 {
            let key = *rng.pick(&keys);
            let value = *rng.pick(&values);
            let entry = model.entry(key).or_default();
            match rng.below(4) {
                0 => {
                    doc.root_mut().set(key, value).unwrap();
                    *entry = vec![value.to_string()];
                }
                1 => {
                    let added = doc.root_mut().add(key, value).unwrap();
                    assert_eq!(
                        added,
                        !entry.contains(&value.to_string()),
                        "round {round} step {step}"
                    );
                    if added {
                        entry.push(value.to_string());
                    }
                }
                2 => {
                    let removed = doc.root_mut().remove_value(key, value);
                    let before = entry.len();
                    entry.retain(|v| v != value);
                    assert_eq!(removed, before - entry.len());
                }
                _ => {
                    let removed = doc.root_mut().remove(key);
                    assert_eq!(removed, entry.len());
                    entry.clear();
                }
            }

            for key in keys {
                let mut actual: Vec<String> = doc.root().values(key).map(String::from).collect();
                let mut expected = model.get(key).cloned().unwrap_or_default();
                actual.sort();
                expected.sort();
                assert_eq!(
                    actual, expected,
                    "key {key} round {round} step {step}\n{doc}"
                );
            }
            let reparsed = Document::parse(&doc.to_string()).unwrap();
            assert_eq!(reparsed, doc);
            assert!(
                doc.to_string().starts_with("# header\n\n"),
                "comments must survive\n{doc}"
            );
        }
    }
}

#[test]
fn edits_never_separate_a_comment_from_the_line_it_describes() {
    // A comment above a header belongs to that header, not to the section before it.
    let mut doc = Document::parse("[repo /a]\n\n# for b\n[repo /b]\n").unwrap();
    doc.section_mut("repo", "/a")
        .unwrap()
        .add("k", "1")
        .unwrap();
    assert_eq!(doc.to_string(), "[repo /a]\nk 1\n\n# for b\n[repo /b]\n");

    // The same comment above the first header does not become a comment on a new top-level entry.
    let mut doc = Document::parse("# The API repo\n[repo /a]\n").unwrap();
    doc.root_mut().add("version", "1").unwrap();
    assert_eq!(doc.to_string(), "version 1\n# The API repo\n[repo /a]\n");

    // A sorted insertion goes above the comment of the entry it comes before.
    let mut doc = Document::parse("skill a\n# good\nskill c\n").unwrap();
    doc.root_mut().add("skill", "b").unwrap();
    assert_eq!(doc.to_string(), "skill a\nskill b\n# good\nskill c\n");
}

#[test]
fn removing_entries_leaves_their_comments_behind() {
    let mut doc = Document::parse("# about a\nskill a\n# about b\nskill b\n").unwrap();
    doc.root_mut().remove("skill");
    assert_eq!(doc.to_string(), "# about a\n# about b\n");
}

#[test]
fn new_lines_align_only_when_the_file_shows_alignment() {
    let mut doc = Document::parse("description Foo\n").unwrap();
    doc.root_mut().add("skill", "git").unwrap();
    assert_eq!(doc.to_string(), "description Foo\nskill git\n");

    let mut doc = Document::parse("[repo /a]\n  profile  x\n  synced   now\n").unwrap();
    doc.section_mut("repo", "/a")
        .unwrap()
        .add("skill", "git")
        .unwrap();
    assert_eq!(
        doc.to_string(),
        "[repo /a]\n  profile  x\n  synced   now\n  skill    git\n"
    );
}

/// A line of the text and the line ending after it ("" for an unterminated last line).
type LineAndEnding = (String, &'static str);

fn lines_with_endings(text: &str) -> Vec<LineAndEnding> {
    let mut rest = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Vec::new();
    while !rest.is_empty() {
        let Some(at) = rest.find('\n') else {
            out.push((rest.to_string(), ""));
            break;
        };
        let line = &rest[..at];
        out.push(match line.strip_suffix('\r') {
            Some(line) => (line.to_string(), "\r\n"),
            None => (line.to_string(), "\n"),
        });
        rest = &rest[at + 1..];
    }
    out
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// A random document with unique comments, mixed line endings and sometimes no final line break.
fn random_edit_document(rng: &mut Rng, counter: &mut usize) -> String {
    let mut text = String::new();
    let count = rng.below(12);
    for i in 0..count {
        *counter += 1;
        let line = match rng.below(8) {
            0 => String::new(),
            1 | 2 => format!("# note {counter}"),
            3 => format!("[repo /r{counter}]"),
            _ => format!(
                "{}{} {}",
                rng.pick(&["", "  "]),
                rng.pick(&["skill", "profile"]),
                rng.pick(&["a", "b", "c", "d", "e"])
            ),
        };
        text.push_str(&line);
        if i + 1 < count || rng.below(4) != 0 {
            text.push_str(rng.pick(&["\n", "\n", "\r\n"]));
        }
    }
    text
}

/// Checks what one edit did to the text. `removal` says the edit may only delete lines.
fn check_edit(before: &str, after: &str, removal: bool, context: &str) {
    let (b, a) = (lines_with_endings(before), lines_with_endings(after));
    let comments = |lines: &[LineAndEnding]| -> Vec<String> {
        lines
            .iter()
            .filter(|(line, _)| is_comment(line))
            .map(|(line, _)| line.clone())
            .collect()
    };
    assert_eq!(comments(&a), comments(&b), "comments changed: {context}");
    if removal {
        // Every line that is left is exactly as it was, with its own line ending.
        let mut old = b.iter();
        for line in &a {
            assert!(old.any(|o| o == line), "{line:?} was changed: {context}");
        }
    } else if a.len() == b.len() {
        let endings = |lines: &[LineAndEnding]| lines.iter().map(|l| l.1).collect::<Vec<_>>();
        assert_eq!(endings(&a), endings(&b), "line endings changed: {context}");
    } else if a.len() == b.len() + 1 {
        let at = (0..b.len()).find(|&i| a[i].0 != b[i].0).unwrap_or(b.len());
        let default = b
            .iter()
            .map(|l| l.1)
            .find(|eol| !eol.is_empty())
            .unwrap_or("\n");
        assert_eq!(a[at].1, default, "new line ending: {context}");
        for (i, old) in b.iter().enumerate() {
            let new = &a[if i < at { i } else { i + 1 }];
            assert_eq!(new.0, old.0, "another line changed: {context}");
            // The only old line that may change its ending is an unterminated last line
            // that a new line now follows.
            let expected = if old.1.is_empty() && at == b.len() {
                default
            } else {
                old.1
            };
            assert_eq!(new.1, expected, "another line ending changed: {context}");
        }
        if at > 0 && at < b.len() {
            assert!(
                !(is_comment(&b[at - 1].0) && !is_blank(&b[at].0)),
                "new line went between a comment and the line below it: {context}"
            );
        }
    }
}

/// Applies random edits to documents that mix line endings and comments, and checks each one.
#[test]
fn random_edits_keep_comments_line_endings_and_round_trip() {
    let mut rng = Rng(0x0BAD_5EED_1234_ABCD);
    let mut counter = 0;
    for round in 0..400 {
        let mut text = random_edit_document(&mut rng, &mut counter);
        if rng.below(5) == 0 {
            text.insert(0, '\u{feff}');
        }
        let mut doc = Document::parse(&text).unwrap_or_else(|e| panic!("{e}\n{text:?}"));
        assert_eq!(doc.to_string(), text);

        for step in 0..25 {
            let before = doc.to_string();
            let key = *rng.pick(&["skill", "profile"]);
            let value = *rng.pick(&["a", "b", "c", "d", "e"]);
            let op = rng.below(9);
            let mut removal = false;
            let label = match op {
                0..=5 => {
                    let pick = rng.below(doc.sections().len() + 1);
                    let name = (pick > 0).then(|| {
                        doc.sections()[pick - 1]
                            .name()
                            .unwrap_or_default()
                            .to_string()
                    });
                    let mut scope = match &name {
                        None => doc.root_mut(),
                        Some(name) => doc.section_mut("repo", name).expect("the section exists"),
                    };
                    match op {
                        0..=2 => {
                            scope.add(key, value).unwrap();
                            "add"
                        }
                        3 => {
                            scope.set(key, value).unwrap();
                            "set"
                        }
                        4 => {
                            scope.remove(key);
                            removal = true;
                            "remove"
                        }
                        _ => {
                            scope.remove_value(key, value);
                            removal = true;
                            "remove_value"
                        }
                    }
                }
                6 => {
                    doc.push_entry(key, value).unwrap();
                    "push_entry"
                }
                7 => {
                    doc.push_blank();
                    "push_blank"
                }
                _ => {
                    counter += 1;
                    doc.push_section("repo", &format!("/p{counter}")).unwrap();
                    "push_section"
                }
            };
            let after = doc.to_string();
            let context = format!(
                "round {round} step {step} {label}({key}, {value})\nbefore {before:?}\nafter  {after:?}"
            );
            check_edit(&before, &after, removal, &context);
            let reparsed = Document::parse(&after).unwrap_or_else(|e| panic!("{e}\n{context}"));
            assert_eq!(reparsed, doc, "{context}");
        }
    }
}

#[test]
fn c1_control_characters_are_errors() {
    for text in [
        "skill a\u{85}b\n",
        "skill a\u{9b}b\n",
        "\u{80}\n",
        "# comment \u{9f}\n",
        "[repo /x\u{90}]\n",
    ] {
        let error = Document::parse(text).unwrap_err();
        assert_eq!(error.first().line(), 1, "{text:?}");
        assert!(
            error
                .first()
                .message()
                .starts_with("control character U+00"),
            "{text:?}: {}",
            error.first().message()
        );
    }
    // U+00A0 is the first character after the C1 block. It is ordinary text in a value.
    let doc = Document::parse("note a\u{a0}b\n").unwrap();
    assert_eq!(doc.root().value("note"), Some("a\u{a0}b"));
}

#[test]
fn a_flood_of_problems_is_cut_off_and_says_so() {
    let source = "- item\n".repeat(80);
    let error = Document::parse(&source).unwrap_err();
    assert_eq!(error.len(), 50);
    assert!(error.truncated());
    assert!(
        error
            .to_string()
            .ends_with("\nnote: only the first 50 problems are shown")
    );
    assert!(
        error
            .render("f.bsk", &source)
            .ends_with("\n\nnote: only the first 50 problems are shown\n")
    );

    let source = "- item\n".repeat(50);
    let error = Document::parse(&source).unwrap_err();
    assert_eq!(error.len(), 50);
    assert!(!error.truncated());
    assert!(!error.render("f.bsk", &source).contains("note:"));

    // Problems that other code collects are never marked as cut off.
    let one = bsk::Diagnostics::one(bsk::Diagnostic::new(1, 1, 1, "x"));
    assert!(!one.truncated());
}

#[test]
fn text_after_the_closing_bracket_is_reported_where_it_is() {
    let source = "[repo /x] # my repo\n";
    let error = Document::parse(source).unwrap_err();
    assert_eq!(
        error.first().message(),
        "unexpected text after the closing ']'"
    );
    assert_eq!(
        error.render("f.bsk", source),
        "error: unexpected text after the closing ']'\n --> f.bsk:1:11\n  |\n1 | [repo /x] # my repo\n  |           ^^^^^^^^^\n  = hint: bsk has no trailing comments; put a comment on its own line\n"
    );
    // No bracket at all is still a missing bracket.
    let error = Document::parse("[repo /x\n").unwrap_err();
    assert_eq!(
        error.first().message(),
        "section header is missing its closing ']'"
    );
    // The last ']' closes the header, whatever comes before it.
    let doc = Document::parse("[repo /x] y]\n").unwrap();
    assert_eq!(doc.sections()[0].name(), Some("/x] y"));
}

#[test]
fn invisible_characters_are_named_and_never_printed_raw() {
    let error = Document::parse("skill\u{a0}git\n").unwrap_err();
    assert_eq!(
        error.first().message(),
        "unexpected U+00A0 after key 'skill'"
    );

    let source = "skill a\u{1b}[2Jb\n";
    let rendered = Document::parse(source).unwrap_err().render("f.bsk", source);
    assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
    assert!(rendered.contains("1 | skill a\u{241b}[2Jb\n"), "{rendered}");
}

#[test]
fn every_bsk_example_in_the_spec_parses() {
    let mut count = 0;
    let mut lines = bsk::SPEC.lines();
    while let Some(line) = lines.next() {
        if line.trim_end() != "```bsk" {
            continue;
        }
        let block: Vec<&str> = lines
            .by_ref()
            .take_while(|l| l.trim_end() != "```")
            .collect();
        let text = block.join("\n") + "\n";
        Document::parse(&text)
            .unwrap_or_else(|e| panic!("spec example does not parse:\n{text}\n{e}"));
        count += 1;
    }
    assert!(
        count >= 8,
        "expected the spec to contain examples, found {count}"
    );
}

#[test]
fn the_spec_example_reads_as_documented() {
    let doc: Document = "# Everyday software engineering.\ndescription Skills for writing, reviewing and testing code\n\nskill code-review\nskill git\nskill testing\n"
        .parse()
        .unwrap();
    assert_eq!(
        doc.root().value("description"),
        Some("Skills for writing, reviewing and testing code")
    );
    assert_eq!(
        doc.root().values("skill").collect::<Vec<_>>(),
        ["code-review", "git", "testing"]
    );
}

#[test]
fn hash_after_a_value_is_part_of_the_value() {
    let doc: Document = "skill git # my tool\n".parse().unwrap();
    assert_eq!(doc.root().value("skill"), Some("git # my tool"));
}

#[test]
fn schema_rejects_the_mistakes_the_spec_lists() {
    let schema = Schema::new()
        .key("description", Cardinality::Optional)
        .key("skill", Cardinality::Many);
    for (text, expect) in [
        ("skill: git\n", "unexpected ':' after key 'skill'"),
        ("skill=git\n", "unexpected '=' after key 'skill'"),
        ("- git\n", "unexpected '-': bsk has no bullet lists"),
    ] {
        let error = Document::parse(text).unwrap_err();
        assert_eq!(error.first().message(), expect, "for {text:?}");
        assert!(error.first().hint().is_some());
    }
    let equals: Document = "skill = git\n".parse().unwrap();
    assert_eq!(equals.root().value("skill"), Some("= git"));
    schema.check(&equals).unwrap();
    let diagnostic = equals
        .root()
        .get("skill")
        .unwrap()
        .diagnostic("invalid skill id '= git'");
    assert!(diagnostic.hint().unwrap().contains("no '=' or ':'"));
}

#[test]
fn parse_errors_render_like_compiler_errors() {
    let source = "description ok\nskill: git\n";
    let error = Document::parse(source).unwrap_err();
    let text = error.render("coding.bsk", source);
    assert_eq!(
        text,
        "error: unexpected ':' after key 'skill'\n --> coding.bsk:2:6\n  |\n2 | skill: git\n  |      ^\n  = hint: bsk lines are 'key value'; write 'skill <value>' without the ':'\n"
    );
}

#[test]
fn a_registry_shaped_document_can_be_built_read_and_checked() {
    let mut doc = Document::new();
    doc.push_comment("Machine-local state.");
    doc.push_entry("version", "1").unwrap();
    for (path, profiles) in [
        ("/home/me/api", &["backend", "coding"][..]),
        ("/home/me/my site", &["frontend"][..]),
    ] {
        doc.push_blank();
        doc.push_section("repo", path).unwrap();
        for profile in profiles {
            doc.push_entry_aligned("profile", profile, 10).unwrap();
        }
    }
    let reparsed: Document = doc.to_string().parse().unwrap();
    assert_eq!(reparsed, doc);
    let schema = Schema::new().key("version", Cardinality::Required).section(
        "repo",
        Naming::Named,
        &[("profile", Cardinality::Many)],
    );
    schema.check(&reparsed).unwrap();
    let names: Vec<&str> = reparsed
        .sections()
        .iter()
        .filter_map(|s| s.name())
        .collect();
    assert_eq!(names, ["/home/me/api", "/home/me/my site"]);
}

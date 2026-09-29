use std::fmt;

use crate::error::Error;
use crate::suggest::closest;

const INDENT: &str = "    ";

/// A parsed Beskar lines file that writes back exactly what it read.
///
/// Every line keeps its own text, including comments, blank lines, alignment
/// and trailing whitespace. Edits touch only the lines they name. A document
/// that was never edited renders byte for byte as it was parsed, as long as its
/// line endings were uniform.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    lines: Vec<Line>,
    crlf: bool,
    final_newline: bool,
    bom: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    Blank(String),
    Comment(String),
    Entry(Entry),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    lead: String,
    key: String,
    gap: String,
    value: String,
    trail: String,
}

impl Entry {
    fn new(indented: bool, key: &str, value: &str) -> Entry {
        Entry {
            lead: if indented { INDENT.to_string() } else { String::new() },
            key: key.to_string(),
            gap: String::new(),
            value: value.to_string(),
            trail: String::new(),
        }
    }

    fn indented(&self) -> bool {
        !self.lead.is_empty()
    }

    fn text(&self) -> String {
        let gap = if self.value.is_empty() {
            ""
        } else if self.gap.is_empty() {
            " "
        } else {
            &self.gap
        };
        format!("{}{}{}{}{}", self.lead, self.key, gap, self.value, self.trail)
    }

    fn set_value(&mut self, value: &str) {
        self.value = value.to_string();
        self.trail.clear();
    }
}

/// A read-only view of one entry, with its 1-based line number.
#[derive(Debug, Clone, Copy)]
pub struct EntryRef<'a> {
    line: usize,
    entry: &'a Entry,
}

impl<'a> EntryRef<'a> {
    pub fn line(&self) -> usize {
        self.line
    }

    pub fn key(&self) -> &'a str {
        &self.entry.key
    }

    /// The value, taken literally. Empty when the entry has none.
    pub fn value(&self) -> &'a str {
        &self.entry.value
    }

    pub fn is_indented(&self) -> bool {
        self.entry.indented()
    }

    /// The value split on runs of spaces and tabs. Schemas that pack several
    /// fields into one value (a registry `skill` line, for example) use this.
    pub fn fields(&self) -> Vec<&'a str> {
        self.entry.value.split([' ', '\t']).filter(|f| !f.is_empty()).collect()
    }

    /// The line as it appears in the file.
    pub fn text(&self) -> String {
        self.entry.text()
    }

    /// An error pinned to this line.
    pub fn error(&self, message: impl Into<String>) -> Error {
        Error::new(message).at_line(self.line, self.text())
    }

    /// An error for a key the schema does not know, with a suggestion when a
    /// known key is close.
    pub fn unknown_key(&self, known: &[&str]) -> Error {
        let key = self.key();
        match closest(key, known) {
            Some(suggestion) => {
                self.error(format!("unknown key `{key}`, did you mean `{suggestion}`?"))
            }
            None => self
                .error(format!("unknown key `{key}`"))
                .with_hint(format!("keys allowed here: {}", known.join(", "))),
        }
    }
}

/// An unindented entry together with the indented entries that belong to it.
#[derive(Debug, Clone)]
pub struct Node<'a> {
    pub entry: EntryRef<'a>,
    pub children: Vec<EntryRef<'a>>,
}

impl Document {
    pub fn new() -> Document {
        Document::default()
    }

    /// Parses `text`, or reports the first line that breaks the rules.
    pub fn parse(text: &str) -> Result<Document, Error> {
        let (text, bom) = match text.strip_prefix('\u{feff}') {
            Some(rest) => (rest, true),
            None => (text, false),
        };
        let mut document = Document {
            crlf: text.contains("\r\n"),
            final_newline: text.ends_with('\n'),
            bom,
            ..Document::default()
        };
        if text.is_empty() {
            return Ok(document);
        }
        let body = text.strip_suffix('\n').unwrap_or(text);
        let mut seen_parent = false;
        for (index, raw) in body.split('\n').enumerate() {
            let raw = raw.strip_suffix('\r').unwrap_or(raw);
            let line = parse_line(index + 1, raw)?;
            if let Line::Entry(entry) = &line {
                if entry.indented() && !seen_parent {
                    return Err(Error::new("indented entry has no parent above it")
                        .at_line(index + 1, raw)
                        .with_hint(
                            "an indented line belongs to the unindented entry above it; \
                             remove the indent or add a parent first",
                        ));
                }
                seen_parent |= !entry.indented();
            }
            document.lines.push(line);
        }
        Ok(document)
    }

    /// Every entry, top level and indented, in file order.
    pub fn entries(&self) -> Vec<EntryRef<'_>> {
        self.lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| match line {
                Line::Entry(entry) => Some(EntryRef { line: i + 1, entry }),
                _ => None,
            })
            .collect()
    }

    /// Unindented entries in file order, each with its indented children.
    pub fn nodes(&self) -> Vec<Node<'_>> {
        let mut nodes: Vec<Node<'_>> = Vec::new();
        for entry in self.entries() {
            if entry.is_indented() {
                if let Some(node) = nodes.last_mut() {
                    node.children.push(entry);
                }
            } else {
                nodes.push(Node { entry, children: Vec::new() });
            }
        }
        nodes
    }

    /// The value of the first unindented entry called `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.nodes().into_iter().find(|n| n.entry.key() == key).map(|n| n.entry.value())
    }

    /// The values of every unindented entry called `key`, in file order.
    pub fn get_all(&self, key: &str) -> Vec<&str> {
        self.nodes().into_iter().filter(|n| n.entry.key() == key).map(|n| n.entry.value()).collect()
    }

    /// Appends a blank line.
    pub fn push_blank(&mut self) {
        self.lines.push(Line::Blank(String::new()));
        self.final_newline = true;
    }

    /// Appends a comment. Each line of `text` becomes one `# ` line.
    pub fn push_comment(&mut self, text: &str) {
        for line in text.lines() {
            let comment = if line.is_empty() { "#".to_string() } else { format!("# {line}") };
            self.lines.push(Line::Comment(comment));
        }
        self.final_newline = true;
    }

    /// Appends an unindented entry.
    pub fn push(&mut self, key: &str, value: &str) -> Result<(), Error> {
        let entry = checked_entry(false, key, value)?;
        self.lines.push(Line::Entry(entry));
        self.final_newline = true;
        Ok(())
    }

    /// Appends an indented entry under the last unindented entry.
    pub fn push_child(&mut self, key: &str, value: &str) -> Result<(), Error> {
        if self.nodes().is_empty() {
            return Err(Error::new("cannot add a child before any unindented entry exists"));
        }
        let entry = checked_entry(true, key, value)?;
        self.lines.push(Line::Entry(entry));
        self.final_newline = true;
        Ok(())
    }

    /// Adds an unindented entry next to the others called `key`: after the last
    /// one and its children, or at the end of the file when there is none.
    pub fn insert_grouped(&mut self, key: &str, value: &str) -> Result<(), Error> {
        let entry = checked_entry(false, key, value)?;
        let at = match self.last_top_level(key) {
            Some(index) => self.group_end(index) + 1,
            None => self.lines.len(),
        };
        self.lines.insert(at, Line::Entry(entry));
        self.final_newline = true;
        Ok(())
    }

    /// Makes `key` have exactly one unindented entry, with `value`.
    ///
    /// The first existing entry is edited in place and keeps its position and
    /// alignment. Later duplicates are removed. With none, one is appended.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), Error> {
        let checked = checked_entry(false, key, value)?;
        let indexes = self.top_level_indexes(key);
        match indexes.split_first() {
            None => {
                self.lines.push(Line::Entry(checked));
                self.final_newline = true;
            }
            Some((&first, rest)) => {
                if let Line::Entry(entry) = &mut self.lines[first] {
                    entry.set_value(value);
                }
                for &index in rest.iter().rev() {
                    self.remove_group(index);
                }
            }
        }
        Ok(())
    }

    /// Removes every unindented entry with this key and value, along with its
    /// indented children. Comments and blank lines stay. Returns how many
    /// entries were removed.
    pub fn remove(&mut self, key: &str, value: &str) -> usize {
        let indexes: Vec<usize> = self
            .top_level_indexes(key)
            .into_iter()
            .filter(|&i| matches!(&self.lines[i], Line::Entry(e) if e.value == value))
            .collect();
        for &index in indexes.iter().rev() {
            self.remove_group(index);
        }
        indexes.len()
    }

    fn top_level_indexes(&self, key: &str) -> Vec<usize> {
        self.lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| match line {
                Line::Entry(e) if !e.indented() && e.key == key => Some(i),
                _ => None,
            })
            .collect()
    }

    fn last_top_level(&self, key: &str) -> Option<usize> {
        self.top_level_indexes(key).last().copied()
    }

    /// Index of the last entry line that belongs to the group starting at `start`.
    fn group_end(&self, start: usize) -> usize {
        let mut end = start;
        for (offset, line) in self.lines[start + 1..].iter().enumerate() {
            match line {
                Line::Entry(e) if e.indented() => end = start + 1 + offset,
                Line::Entry(_) => break,
                _ => {}
            }
        }
        end
    }

    fn remove_group(&mut self, start: usize) {
        let mut doomed = vec![start];
        for (offset, line) in self.lines[start + 1..].iter().enumerate() {
            match line {
                Line::Entry(e) if e.indented() => doomed.push(start + 1 + offset),
                Line::Entry(_) => break,
                _ => {}
            }
        }
        for index in doomed.into_iter().rev() {
            self.lines.remove(index);
        }
    }
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let eol = if self.crlf { "\r\n" } else { "\n" };
        if self.bom {
            f.write_str("\u{feff}")?;
        }
        for (i, line) in self.lines.iter().enumerate() {
            match line {
                Line::Blank(text) | Line::Comment(text) => f.write_str(text)?,
                Line::Entry(entry) => f.write_str(&entry.text())?,
            }
            if i + 1 < self.lines.len() || self.final_newline {
                f.write_str(eol)?;
            }
        }
        Ok(())
    }
}

fn is_blank_char(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn parse_line(number: usize, raw: &str) -> Result<Line, Error> {
    let body = raw.trim_start_matches(is_blank_char);
    if body.is_empty() {
        return Ok(Line::Blank(raw.to_string()));
    }
    if body.starts_with('#') {
        return Ok(Line::Comment(raw.to_string()));
    }
    let lead = &raw[..raw.len() - body.len()];
    let key_end = body.find(is_blank_char).unwrap_or(body.len());
    let key = &body[..key_end];
    check_key(key).map_err(|error| error.at_line(number, raw))?;

    let rest = &body[key_end..];
    let after_gap = rest.trim_start_matches(is_blank_char);
    let value = after_gap.trim_end_matches(is_blank_char);
    check_value(value).map_err(|error| error.at_line(number, raw))?;

    let (gap, trail) = if value.is_empty() {
        ("", rest)
    } else {
        (&rest[..rest.len() - after_gap.len()], &after_gap[value.len()..])
    };
    Ok(Line::Entry(Entry {
        lead: lead.to_string(),
        key: key.to_string(),
        gap: gap.to_string(),
        value: value.to_string(),
        trail: trail.to_string(),
    }))
}

fn checked_entry(indented: bool, key: &str, value: &str) -> Result<Entry, Error> {
    check_key(key)?;
    check_value(value)?;
    if value != value.trim_matches(is_blank_char) {
        return Err(Error::new(format!(
            "value for `{key}` starts or ends with a space or tab, which would not survive a round trip"
        )));
    }
    Ok(Entry::new(indented, key, value))
}

fn check_key(key: &str) -> Result<(), Error> {
    let valid = key.starts_with(|c: char| c.is_ascii_lowercase())
        && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        return Ok(());
    }
    let error = Error::new(format!("invalid key `{key}`"));
    Err(error.with_hint(key_hint(key)))
}

/// Guesses what the author meant. The likeliest slips come from YAML, TOML and
/// JSON habits, so those get their own advice.
fn key_hint(key: &str) -> String {
    if let Some(stripped) = key.strip_suffix([':', '='])
        && check_key(stripped).is_ok()
    {
        return format!(
            "separate the key and value with a space, like `{stripped} value`; \
             there is no `:` or `=`"
        );
    }
    if key.starts_with('-') {
        return "list items are repeated keys, not `-` bullets: write `skill git`, one per line"
            .to_string();
    }
    if key.starts_with('[') {
        return "there are no [sections]; use an unindented entry and indent its children"
            .to_string();
    }
    let lower = key.to_ascii_lowercase();
    if lower != key && check_key(&lower).is_ok() {
        return format!("keys are lowercase, try `{lower}`");
    }
    "keys are lowercase letters, digits and `-`, and start with a letter".to_string()
}

fn check_value(value: &str) -> Result<(), Error> {
    match value.chars().find(|&c| c.is_control() && c != '\t') {
        Some(c) => {
            Err(Error::new(format!("value contains the control character U+{:04X}", u32::from(c))))
        }
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Document {
        Document::parse(text).expect("valid document")
    }

    #[test]
    fn parses_keys_and_values() {
        let doc = parse("name coding\nskill git\nskill code-review\n");
        assert_eq!(doc.get("name"), Some("coding"));
        assert_eq!(doc.get_all("skill"), ["git", "code-review"]);
        assert_eq!(doc.get("missing"), None);
    }

    #[test]
    fn value_is_everything_after_the_key() {
        let doc = parse("library /home/ana/my skills # not a comment\n");
        assert_eq!(doc.get("library"), Some("/home/ana/my skills # not a comment"));
    }

    #[test]
    fn value_may_be_empty() {
        let doc = parse("flag\nother   \n");
        assert_eq!(doc.get("flag"), Some(""));
        assert_eq!(doc.get("other"), Some(""));
    }

    #[test]
    fn tabs_separate_key_and_value() {
        let doc = parse("key\t\tvalue with\ttab\t\n");
        assert_eq!(doc.get("key"), Some("value with\ttab"));
    }

    #[test]
    fn quotes_are_part_of_the_value() {
        let doc = parse("title \"quoted\"\n");
        assert_eq!(doc.get("title"), Some("\"quoted\""));
    }

    #[test]
    fn comments_need_their_own_line() {
        let doc = parse("# top\n   # indented comment\nkey value\n");
        assert_eq!(doc.entries().len(), 1);
        assert_eq!(doc.get("key"), Some("value"));
    }

    #[test]
    fn children_belong_to_the_entry_above() {
        let doc = parse("repo /a\n    profile one\n\t  profile two\nrepo /b\n  profile three\n");
        let nodes = doc.nodes();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].entry.value(), "/a");
        let first: Vec<_> = nodes[0].children.iter().map(|c| c.value()).collect();
        assert_eq!(first, ["one", "two"]);
        assert_eq!(nodes[1].children.len(), 1);
        assert_eq!(nodes[1].children[0].value(), "three");
    }

    #[test]
    fn comments_and_blank_lines_do_not_end_a_group() {
        let doc = parse("repo /a\n    profile one\n\n# note\n    profile two\n");
        assert_eq!(doc.nodes()[0].children.len(), 2);
    }

    #[test]
    fn deeper_indent_does_not_nest_deeper() {
        let doc = parse("repo /a\n  one x\n        two y\n");
        assert_eq!(doc.nodes().len(), 1);
        assert_eq!(doc.nodes()[0].children.len(), 2);
    }

    #[test]
    fn child_without_parent_is_an_error() {
        let error = Document::parse("# hi\n  orphan x\n").unwrap_err();
        assert_eq!(error.line(), 2);
        assert!(error.message().contains("no parent"));
    }

    #[test]
    fn entry_line_numbers_are_one_based() {
        let doc = parse("# c\n\nkey a\nkey b\n");
        let lines: Vec<_> = doc.entries().iter().map(|e| e.line()).collect();
        assert_eq!(lines, [3, 4]);
    }

    #[test]
    fn rejects_yaml_style_key() {
        let error = Document::parse("description: text\n").unwrap_err();
        assert_eq!(error.line(), 1);
        assert!(error.hint().unwrap().contains("`description value`"));
    }

    #[test]
    fn rejects_yaml_bullets() {
        let error = Document::parse("skills\n  - git\n").unwrap_err();
        assert_eq!(error.line(), 2);
        assert!(error.hint().unwrap().contains("repeated keys"));
    }

    #[test]
    fn rejects_toml_sections() {
        let error = Document::parse("[profile]\n").unwrap_err();
        assert!(error.hint().unwrap().contains("sections"));
    }

    #[test]
    fn rejects_uppercase_keys_and_suggests_lowercase() {
        let error = Document::parse("Skill git\n").unwrap_err();
        assert!(error.hint().unwrap().contains("`skill`"));
    }

    #[test]
    fn rejects_control_characters_in_values() {
        let error = Document::parse("key a\u{7}b\n").unwrap_err();
        assert!(error.message().contains("U+0007"));
        assert_eq!(error.line(), 1);
    }

    #[test]
    fn rejects_lone_carriage_return() {
        assert!(Document::parse("key a\rb\n").is_err());
    }

    #[test]
    fn error_render_shows_the_line() {
        let error = Document::parse("ok 1\nBad 2\n").unwrap_err();
        let text = error.render("x.bsk");
        assert!(text.starts_with("x.bsk:2: invalid key `Bad`"), "{text}");
        assert!(text.contains("   2 | Bad 2"), "{text}");
        assert!(text.contains("hint:"), "{text}");
    }

    #[test]
    fn unknown_key_suggests_close_keys() {
        let doc = parse("skils git\n");
        let error = doc.entries()[0].unknown_key(&["description", "skill"]);
        assert_eq!(error.message(), "unknown key `skils`, did you mean `skill`?");
        assert_eq!(error.line(), 1);
    }

    #[test]
    fn unknown_key_lists_alternatives_when_nothing_is_close() {
        let doc = parse("banana yes\n");
        let error = doc.entries()[0].unknown_key(&["description", "skill"]);
        assert_eq!(error.hint(), Some("keys allowed here: description, skill"));
    }

    #[test]
    fn unedited_documents_round_trip_exactly() {
        for text in [
            "",
            "\n",
            "# only a comment",
            "key value",
            "key value\n",
            "  # odd comment  \nkey   spaced   value  \n\n\nrepo /a\n\tchild x\n\n",
            "\u{feff}key value\n",
            "a 1\r\nb 2\r\n",
            "flag   \n",
        ] {
            assert_eq!(parse(text).to_string(), text, "round trip of {text:?}");
        }
    }

    #[test]
    fn insert_grouped_goes_after_the_last_sibling() {
        let mut doc = parse("description d\nskill a\n# keep\nskill b\n\nother x\n");
        doc.insert_grouped("skill", "c").unwrap();
        assert_eq!(
            doc.to_string(),
            "description d\nskill a\n# keep\nskill b\nskill c\n\nother x\n"
        );
    }

    #[test]
    fn insert_grouped_appends_when_key_is_new() {
        let mut doc = parse("description d\n");
        doc.insert_grouped("skill", "a").unwrap();
        assert_eq!(doc.to_string(), "description d\nskill a\n");
    }

    #[test]
    fn insert_grouped_skips_children_of_the_last_sibling() {
        let mut doc = parse("repo /a\n    p 1\nrepo /b\n    p 2\n# end\nother x\n");
        doc.insert_grouped("repo", "/c").unwrap();
        assert_eq!(
            doc.to_string(),
            "repo /a\n    p 1\nrepo /b\n    p 2\nrepo /c\n# end\nother x\n"
        );
    }

    #[test]
    fn remove_deletes_matching_entries_and_keeps_comments() {
        let mut doc = parse("# list\nskill a\n# about b\nskill b\nskill a\n");
        assert_eq!(doc.remove("skill", "a"), 2);
        assert_eq!(doc.to_string(), "# list\n# about b\nskill b\n");
    }

    #[test]
    fn remove_takes_children_with_their_parent() {
        let mut doc = parse("repo /a\n    p 1\n    p 2\nrepo /b\n    p 3\n");
        assert_eq!(doc.remove("repo", "/a"), 1);
        assert_eq!(doc.to_string(), "repo /b\n    p 3\n");
    }

    #[test]
    fn remove_reports_zero_when_nothing_matches() {
        let mut doc = parse("skill a\n");
        assert_eq!(doc.remove("skill", "z"), 0);
        assert_eq!(doc.to_string(), "skill a\n");
    }

    #[test]
    fn set_edits_in_place_and_keeps_alignment() {
        let mut doc = parse("# c\nlibrary      /old\non-conflict ask\n");
        doc.set("library", "/new").unwrap();
        assert_eq!(doc.to_string(), "# c\nlibrary      /new\non-conflict ask\n");
    }

    #[test]
    fn set_appends_when_missing_and_dedupes_when_repeated() {
        let mut doc = parse("a 1\n");
        doc.set("b", "2").unwrap();
        assert_eq!(doc.to_string(), "a 1\nb 2\n");
        let mut doc = parse("a 1\nb 2\na 3\n");
        doc.set("a", "9").unwrap();
        assert_eq!(doc.to_string(), "a 9\nb 2\n");
    }

    #[test]
    fn set_can_fill_an_empty_value() {
        let mut doc = parse("flag\n");
        doc.set("flag", "on").unwrap();
        assert_eq!(doc.to_string(), "flag on\n");
    }

    #[test]
    fn push_builds_documents() {
        let mut doc = Document::new();
        doc.push_comment("A registry\nsecond line");
        doc.push_blank();
        doc.push("repo", "/a").unwrap();
        doc.push_child("profile", "coding").unwrap();
        assert_eq!(doc.to_string(), "# A registry\n# second line\n\nrepo /a\n    profile coding\n");
        assert_eq!(parse(&doc.to_string()).nodes().len(), 1);
    }

    #[test]
    fn push_child_needs_a_parent() {
        let mut doc = Document::new();
        assert!(doc.push_child("profile", "x").is_err());
    }

    #[test]
    fn push_rejects_values_that_would_not_round_trip() {
        let mut doc = Document::new();
        assert!(doc.push("key", "two\nlines").is_err());
        assert!(doc.push("key", " padded").is_err());
        assert!(doc.push("key", "padded ").is_err());
        assert!(doc.push("Key", "x").is_err());
        assert!(doc.push("key", "ok value").is_ok());
    }

    #[test]
    fn pushing_onto_text_without_final_newline_adds_one() {
        let mut doc = parse("a 1");
        doc.push("b", "2").unwrap();
        assert_eq!(doc.to_string(), "a 1\nb 2\n");
    }

    #[test]
    fn crlf_documents_stay_crlf_after_edits() {
        let mut doc = parse("a 1\r\nb 2\r\n");
        doc.push("c", "3").unwrap();
        assert_eq!(doc.to_string(), "a 1\r\nb 2\r\nc 3\r\n");
    }

    #[test]
    fn fields_split_on_blank_runs() {
        let doc = parse("skill  code-review \t fp1:abc\n");
        assert_eq!(doc.entries()[0].fields(), ["code-review", "fp1:abc"]);
    }
}

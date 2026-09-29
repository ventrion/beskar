//! # Slate
//!
//! A line-oriented configuration format built for people and coding agents
//! who edit files by hand. There are four kinds of line and nothing else:
//!
//! ```text
//! # a comment            first non-blank character is '#'
//! [kind name]            a section header; the name is optional and may contain spaces
//! key = value            an entry; the value runs to the end of the line
//!                        a blank line
//! ```
//!
//! Rules that make it unambiguous:
//!
//! * Every line stands alone. Indentation is ignored, there are no
//!   continuations, and nothing spans lines.
//! * There are no quotes and no escapes. A value is the text after the first
//!   `=`, trimmed. A `#` inside a value is part of the value.
//! * Every value is a string. The *schema* decides whether `true` is a boolean
//!   or `007` is a number; the parser never guesses.
//! * A repeated key inside one section is a list, one item per line. A schema
//!   that expects a single value rejects the repeat with a line number.
//! * Keys and section kinds match `[A-Za-z0-9_.-]+`. Two sections may not share
//!   the same kind and name.
//!
//! [`Document`] keeps every line it read, so edits made through [`Document::set`],
//! [`Document::push`] and friends leave comments, ordering and blank lines
//! intact. Re-serialising a document Beskar only read gives back the same text
//! apart from whitespace normalisation around `=`.

use std::fmt;
use std::ops::Range;

/// A parse or schema error, always pointing at a line when one is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// 1-based line number, when the error concerns a specific line.
    pub line: Option<usize>,
    pub message: String,
}

impl Error {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Error {
            line: Some(line),
            message: message.into(),
        }
    }

    fn general(message: impl Into<String>) -> Self {
        Error {
            line: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(n) => write!(f, "line {n}: {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    Blank,
    Comment(String),
    Section { kind: String, name: String },
    Entry { key: String, value: String },
}

impl Line {
    fn render(&self, out: &mut String) {
        match self {
            Line::Blank => {}
            Line::Comment(text) => out.push_str(text),
            Line::Section { kind, name } => {
                out.push('[');
                out.push_str(kind);
                if !name.is_empty() {
                    out.push(' ');
                    out.push_str(name);
                }
                out.push(']');
            }
            Line::Entry { key, value } => {
                out.push_str(key);
                if value.is_empty() {
                    out.push_str(" =");
                } else {
                    out.push_str(" = ");
                    out.push_str(value);
                }
            }
        }
    }
}

/// True for the characters allowed in keys and section kinds.
pub fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// An in-memory Slate document: the exact lines that were read plus any edits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    lines: Vec<Line>,
}

/// A read-only view over the entries of one section (or the root).
#[derive(Debug, Clone)]
pub struct Entries<'a> {
    doc: &'a Document,
    range: Range<usize>,
    kind: &'a str,
    name: &'a str,
}

impl Document {
    /// An empty document.
    pub fn new() -> Self {
        Document { lines: Vec::new() }
    }

    /// A document that starts with the given comment lines. Each string is
    /// one line; a `#` is prepended when missing, and an empty string is a
    /// bare `#`.
    pub fn with_header(comment_lines: &[&str]) -> Self {
        let mut doc = Document::new();
        for text in comment_lines {
            doc.lines.push(Line::Comment(comment(text)));
        }
        doc
    }

    /// Parse Slate text. Fails on the first malformed line.
    pub fn parse(text: &str) -> Result<Document, Error> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut lines = Vec::new();
        let mut seen_sections: Vec<(String, String)> = Vec::new();
        for (idx, raw) in text.lines().enumerate() {
            let line_no = idx + 1;
            let trimmed = raw.trim();
            let parsed = if trimmed.is_empty() {
                Line::Blank
            } else if trimmed.starts_with('#') {
                Line::Comment(trimmed.to_string())
            } else if let Some(rest) = trimmed.strip_prefix('[') {
                let inner = rest
                    .strip_suffix(']')
                    .ok_or_else(|| Error::at(line_no, "section header must end with ']'"))?;
                let inner = inner.trim();
                if inner.is_empty() {
                    return Err(Error::at(
                        line_no,
                        "section header is empty; expected [kind name]",
                    ));
                }
                let (kind, name) = match inner.split_once(char::is_whitespace) {
                    Some((k, n)) => (k.to_string(), n.trim().to_string()),
                    None => (inner.to_string(), String::new()),
                };
                if !is_identifier(&kind) {
                    return Err(Error::at(
                        line_no,
                        format!("section kind '{kind}' may only contain letters, digits, '_', '-' and '.'"),
                    ));
                }
                let id = (kind.clone(), name.clone());
                if seen_sections.contains(&id) {
                    return Err(Error::at(
                        line_no,
                        format!("duplicate section [{}]", header_text(&kind, &name)),
                    ));
                }
                seen_sections.push(id);
                Line::Section { kind, name }
            } else {
                let (key, value) = trimmed.split_once('=').ok_or_else(|| {
                    Error::at(
                        line_no,
                        format!(
                            "expected 'key = value', a [section] or a # comment, found: {trimmed}"
                        ),
                    )
                })?;
                let key = key.trim();
                if key.is_empty() {
                    return Err(Error::at(line_no, "missing key before '='"));
                }
                if !is_identifier(key) {
                    return Err(Error::at(
                        line_no,
                        format!("key '{key}' may only contain letters, digits, '_', '-' and '.'"),
                    ));
                }
                Line::Entry {
                    key: key.to_string(),
                    value: value.trim().to_string(),
                }
            };
            lines.push(parsed);
        }
        Ok(Document { lines })
    }

    /// True when the document holds no entries and no sections.
    pub fn is_empty(&self) -> bool {
        !self
            .lines
            .iter()
            .any(|l| matches!(l, Line::Entry { .. } | Line::Section { .. }))
    }

    // ----- reading -------------------------------------------------------

    /// Entries that appear before the first section header.
    pub fn root(&self) -> Entries<'_> {
        Entries {
            doc: self,
            range: self.root_range(),
            kind: "",
            name: "",
        }
    }

    /// The section with this kind and name, if present.
    pub fn section(&self, kind: &str, name: &str) -> Option<Entries<'_>> {
        let start = self.section_index(kind, name)?;
        let end = self.section_end(start);
        let (kind, name) = match &self.lines[start] {
            Line::Section { kind, name } => (kind.as_str(), name.as_str()),
            _ => unreachable!(),
        };
        Some(Entries {
            doc: self,
            range: start + 1..end,
            kind,
            name,
        })
    }

    /// Every section, in file order.
    pub fn sections(&self) -> Vec<Entries<'_>> {
        let mut out = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            if let Line::Section { kind, name } = line {
                out.push(Entries {
                    doc: self,
                    range: i + 1..self.section_end(i),
                    kind,
                    name,
                });
            }
        }
        out
    }

    /// Every section of one kind, in file order.
    pub fn sections_of(&self, kind: &str) -> Vec<Entries<'_>> {
        self.sections()
            .into_iter()
            .filter(|s| s.kind() == kind)
            .collect()
    }

    /// Reject section kinds outside `allowed`, pointing at the offending line.
    pub fn check_section_kinds(&self, allowed: &[&str]) -> Result<(), Error> {
        for (i, line) in self.lines.iter().enumerate() {
            if let Line::Section { kind, .. } = line {
                if !allowed.contains(&kind.as_str()) {
                    return Err(Error::at(
                        i + 1,
                        format!(
                            "unknown section kind '{kind}'; expected one of: {}",
                            allowed.join(", ")
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    // ----- editing -------------------------------------------------------

    /// Set a single-valued key in the root, replacing any existing occurrences.
    pub fn set_root(&mut self, key: &str, value: &str) {
        let range = self.root_range();
        self.set_in(range, key, value);
    }

    /// Set a single-valued key in a section, creating the section when needed.
    pub fn set(&mut self, kind: &str, name: &str, key: &str, value: &str) {
        let start = self.ensure_section(kind, name);
        let range = start + 1..self.section_end(start);
        self.set_in(range, key, value);
    }

    /// Append `key = value` to the root unless that exact pair is present.
    /// Returns true when a line was added.
    pub fn push_root(&mut self, key: &str, value: &str) -> bool {
        let range = self.root_range();
        self.push_in(range, key, value)
    }

    /// Append `key = value` to a section (created when needed) unless that
    /// exact pair is present. Returns true when a line was added.
    pub fn push(&mut self, kind: &str, name: &str, key: &str, value: &str) -> bool {
        let start = self.ensure_section(kind, name);
        let range = start + 1..self.section_end(start);
        self.push_in(range, key, value)
    }

    /// Remove entries from the root. With `value`, only matching pairs go;
    /// without, every entry with that key. Returns how many lines went.
    pub fn remove_root(&mut self, key: &str, value: Option<&str>) -> usize {
        let range = self.root_range();
        self.remove_in(range, key, value)
    }

    /// Remove entries from a section. See [`Document::remove_root`].
    pub fn remove(&mut self, kind: &str, name: &str, key: &str, value: Option<&str>) -> usize {
        match self.section_index(kind, name) {
            Some(start) => {
                let range = start + 1..self.section_end(start);
                self.remove_in(range, key, value)
            }
            None => 0,
        }
    }

    /// Make sure a section exists, appending an empty one at the end when it
    /// does not. Returns the index of its header line.
    pub fn ensure_section(&mut self, kind: &str, name: &str) -> usize {
        if let Some(i) = self.section_index(kind, name) {
            return i;
        }
        assert!(
            is_identifier(kind),
            "section kind must be an identifier: {kind:?}"
        );
        self.trim_trailing_blanks();
        if !self.lines.is_empty() {
            self.lines.push(Line::Blank);
        }
        self.lines.push(Line::Section {
            kind: kind.to_string(),
            name: name.trim().to_string(),
        });
        self.lines.len() - 1
    }

    /// Remove a whole section including its entries. Returns true if it existed.
    pub fn remove_section(&mut self, kind: &str, name: &str) -> bool {
        match self.section_index(kind, name) {
            Some(start) => {
                let end = self.section_end(start);
                self.lines.drain(start..end);
                // Collapse the blank line that separated it from its neighbours.
                if start > 0 && matches!(self.lines.get(start - 1), Some(Line::Blank)) {
                    let at_end = start >= self.lines.len();
                    let next_blank = matches!(self.lines.get(start), Some(Line::Blank));
                    if at_end || next_blank {
                        self.lines.remove(start - 1);
                    }
                }
                true
            }
            None => false,
        }
    }

    /// Append a comment line at the very end of the document.
    pub fn push_comment(&mut self, text: &str) {
        self.lines.push(Line::Comment(comment(text)));
    }

    /// Append a blank line at the very end of the document.
    pub fn push_blank(&mut self) {
        self.lines.push(Line::Blank);
    }

    // ----- internals ------------------------------------------------------

    fn root_range(&self) -> Range<usize> {
        let end = self
            .lines
            .iter()
            .position(|l| matches!(l, Line::Section { .. }))
            .unwrap_or(self.lines.len());
        0..end
    }

    fn section_index(&self, kind: &str, name: &str) -> Option<usize> {
        let name = name.trim();
        self.lines.iter().position(|l| match l {
            Line::Section { kind: k, name: n } => k == kind && n == name,
            _ => false,
        })
    }

    /// Index one past the last line belonging to the section whose header is at `start`.
    fn section_end(&self, start: usize) -> usize {
        self.lines[start + 1..]
            .iter()
            .position(|l| matches!(l, Line::Section { .. }))
            .map(|p| start + 1 + p)
            .unwrap_or(self.lines.len())
    }

    fn set_in(&mut self, range: Range<usize>, key: &str, value: &str) {
        assert!(is_identifier(key), "key must be an identifier: {key:?}");
        let mut matches: Vec<usize> = range
            .clone()
            .filter(|&i| matches!(&self.lines[i], Line::Entry { key: k, .. } if k == key))
            .collect();
        if let Some(first) = matches.first().copied() {
            self.lines[first] = Line::Entry {
                key: key.to_string(),
                value: value.trim().to_string(),
            };
            matches.remove(0);
            for &i in matches.iter().rev() {
                self.lines.remove(i);
            }
        } else {
            self.insert_entry(range, key, value);
        }
    }

    fn push_in(&mut self, range: Range<usize>, key: &str, value: &str) -> bool {
        assert!(is_identifier(key), "key must be an identifier: {key:?}");
        let value = value.trim();
        let exists = range
            .clone()
            .any(|i| matches!(&self.lines[i], Line::Entry { key: k, value: v } if k == key && v == value));
        if exists {
            return false;
        }
        self.insert_entry(range, key, value);
        true
    }

    fn remove_in(&mut self, range: Range<usize>, key: &str, value: Option<&str>) -> usize {
        let value = value.map(str::trim);
        let hits: Vec<usize> = range
            .filter(|&i| match &self.lines[i] {
                Line::Entry { key: k, value: v } => k == key && value.is_none_or(|want| want == v),
                _ => false,
            })
            .collect();
        for &i in hits.iter().rev() {
            self.lines.remove(i);
        }
        hits.len()
    }

    /// Insert an entry after the last entry of the block, or after the block's
    /// leading comments when it has no entries yet.
    fn insert_entry(&mut self, range: Range<usize>, key: &str, value: &str) {
        let entry = Line::Entry {
            key: key.to_string(),
            value: value.trim().to_string(),
        };
        let last_entry = range
            .clone()
            .rev()
            .find(|&i| matches!(self.lines[i], Line::Entry { .. }));
        let at = match last_entry {
            Some(i) => i + 1,
            None => {
                // Skip the header comments (and the section header itself is
                // already outside `range`). Keep one blank line between a
                // comment block and the first entry so headers stay readable.
                let mut i = range.start;
                while i < range.end && matches!(self.lines[i], Line::Comment(_)) {
                    i += 1;
                }
                if i > range.start && i < range.end && matches!(self.lines[i], Line::Blank) {
                    i += 1;
                } else if i > range.start && matches!(self.lines[i - 1], Line::Comment(_)) {
                    self.lines.insert(i, Line::Blank);
                    i += 1;
                }
                i
            }
        };
        self.lines.insert(at, entry);
    }

    fn trim_trailing_blanks(&mut self) {
        while matches!(self.lines.last(), Some(Line::Blank)) {
            self.lines.pop();
        }
    }
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        for line in &self.lines {
            line.render(&mut out);
            out.push('\n');
        }
        f.write_str(&out)
    }
}

impl<'a> Entries<'a> {
    /// The section kind, or "" for the root.
    pub fn kind(&self) -> &'a str {
        self.kind
    }

    /// The section name, or "" for the root or an unnamed section.
    pub fn name(&self) -> &'a str {
        self.name
    }

    /// 1-based line number of the section header (None for the root).
    pub fn header_line(&self) -> Option<usize> {
        if self.range.start == 0 {
            None
        } else {
            Some(self.range.start)
        }
    }

    /// Every `(key, value)` pair in file order.
    pub fn entries(&self) -> Vec<(&'a str, &'a str)> {
        self.range
            .clone()
            .filter_map(|i| match &self.doc.lines[i] {
                Line::Entry { key, value } => Some((key.as_str(), value.as_str())),
                _ => None,
            })
            .collect()
    }

    /// Distinct keys in order of first appearance.
    pub fn keys(&self) -> Vec<&'a str> {
        let mut keys: Vec<&str> = Vec::new();
        for (k, _) in self.entries() {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        keys
    }

    /// The value of a single-valued key. A repeated key is an error.
    pub fn get(&self, key: &str) -> Result<Option<&'a str>, Error> {
        let mut found: Option<&'a str> = None;
        for i in self.range.clone() {
            if let Line::Entry { key: k, value } = &self.doc.lines[i] {
                if k == key {
                    if found.is_some() {
                        return Err(Error::at(
                            i + 1,
                            format!("'{key}' may only appear once in {}", self.describe()),
                        ));
                    }
                    found = Some(value.as_str());
                }
            }
        }
        Ok(found)
    }

    /// Like [`Entries::get`], but a missing key is an error too.
    pub fn require(&self, key: &str) -> Result<&'a str, Error> {
        self.get(key)?.ok_or_else(|| match self.header_line() {
            Some(line) => Error::at(line, format!("'{key}' is required in {}", self.describe())),
            None => Error::general(format!("'{key}' is required")),
        })
    }

    /// Every value for a key, in file order (the list reading).
    pub fn get_all(&self, key: &str) -> Vec<&'a str> {
        self.entries()
            .into_iter()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v)
            .collect()
    }

    /// Fail on the first key that is not in `allowed`, naming the line.
    pub fn check_keys(&self, allowed: &[&str]) -> Result<(), Error> {
        for i in self.range.clone() {
            if let Line::Entry { key, .. } = &self.doc.lines[i] {
                if !allowed.contains(&key.as_str()) {
                    return Err(Error::at(
                        i + 1,
                        format!(
                            "unknown key '{key}' in {}; expected one of: {}",
                            self.describe(),
                            allowed.join(", ")
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn describe(&self) -> String {
        if self.kind.is_empty() {
            "the top of the file".to_string()
        } else {
            format!("[{}]", header_text(self.kind, self.name))
        }
    }
}

fn header_text(kind: &str, name: &str) -> String {
    if name.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} {name}")
    }
}

fn comment(text: &str) -> String {
    let text = text.trim_end();
    if text.is_empty() {
        "#".to_string()
    } else if text.starts_with('#') {
        text.to_string()
    } else {
        format!("# {text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Beskar registry
# second header line

[repo /home/u/my project]
profile = coding
profile = backend
synced = 2026-09-29T10:00:00Z

[repo /home/u/docs]
profile = writing
";

    #[test]
    fn parses_sections_and_lists() {
        let doc = Document::parse(SAMPLE).unwrap();
        let repos = doc.sections_of("repo");
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].name(), "/home/u/my project");
        assert_eq!(repos[0].get_all("profile"), vec!["coding", "backend"]);
        assert_eq!(
            repos[0].get("synced").unwrap(),
            Some("2026-09-29T10:00:00Z")
        );
        assert_eq!(repos[1].get("synced").unwrap(), None);
        assert!(doc.root().entries().is_empty());
    }

    #[test]
    fn roundtrip_preserves_comments_and_layout() {
        let doc = Document::parse(SAMPLE).unwrap();
        assert_eq!(doc.to_string(), SAMPLE);
    }

    #[test]
    fn hash_and_equals_inside_values_are_literal() {
        let doc = Document::parse("path = /tmp/#weird = yes\nempty =\n").unwrap();
        assert_eq!(doc.root().get("path").unwrap(), Some("/tmp/#weird = yes"));
        assert_eq!(doc.root().get("empty").unwrap(), Some(""));
    }

    #[test]
    fn indentation_and_spacing_do_not_matter() {
        let doc = Document::parse("   key=value  \n\t[kind   the name  ]\n  k  =  v\n").unwrap();
        assert_eq!(doc.root().get("key").unwrap(), Some("value"));
        let s = doc.section("kind", "the name").unwrap();
        assert_eq!(s.get("k").unwrap(), Some("v"));
    }

    #[test]
    fn repeated_scalar_is_an_error_with_line() {
        let doc = Document::parse("a = 1\nb = 2\na = 3\n").unwrap();
        let err = doc.root().get("a").unwrap_err();
        assert_eq!(err.line, Some(3));
    }

    #[test]
    fn malformed_lines_report_line_numbers() {
        let err = Document::parse("ok = 1\nthis is not an entry\n").unwrap_err();
        assert_eq!(err.line, Some(2));
        let err = Document::parse("[unterminated\n").unwrap_err();
        assert_eq!(err.line, Some(1));
        let err = Document::parse("[a x]\n[a x]\n").unwrap_err();
        assert_eq!(err.line, Some(2));
        let err = Document::parse("bad key = 1\n").unwrap_err();
        assert_eq!(err.line, Some(1));
    }

    #[test]
    fn check_keys_rejects_unknown() {
        let doc = Document::parse("library = x\nlibary = y\n").unwrap();
        let err = doc.root().check_keys(&["library"]).unwrap_err();
        assert_eq!(err.line, Some(2));
        assert!(err.message.contains("libary"));
    }

    #[test]
    fn push_and_remove_keep_surroundings() {
        let mut doc = Document::parse(SAMPLE).unwrap();
        assert!(doc.push("repo", "/home/u/docs", "profile", "research"));
        assert!(!doc.push("repo", "/home/u/docs", "profile", "research"));
        assert_eq!(
            doc.remove("repo", "/home/u/my project", "profile", Some("backend")),
            1
        );
        let expected = "\
# Beskar registry
# second header line

[repo /home/u/my project]
profile = coding
synced = 2026-09-29T10:00:00Z

[repo /home/u/docs]
profile = writing
profile = research
";
        assert_eq!(doc.to_string(), expected);
    }

    #[test]
    fn set_replaces_and_creates() {
        let mut doc = Document::with_header(&["Config", ""]);
        doc.set_root("library", "~/lib");
        doc.set_root("library", "~/other");
        doc.set("repo", "/x", "synced", "now");
        let expected = "# Config\n#\n\nlibrary = ~/other\n\n[repo /x]\nsynced = now\n";
        assert_eq!(doc.to_string(), expected);
        assert_eq!(doc.root().get("library").unwrap(), Some("~/other"));
    }

    #[test]
    fn remove_section_collapses_blank() {
        let mut doc = Document::parse(SAMPLE).unwrap();
        assert!(doc.remove_section("repo", "/home/u/my project"));
        assert!(!doc.remove_section("repo", "/home/u/my project"));
        let expected =
            "# Beskar registry\n# second header line\n\n[repo /home/u/docs]\nprofile = writing\n";
        assert_eq!(doc.to_string(), expected);
        assert!(doc.remove_section("repo", "/home/u/docs"));
        assert!(doc.is_empty());
    }

    #[test]
    fn first_entry_goes_after_header_comments() {
        let mut doc = Document::parse("# header\n# more\n").unwrap();
        doc.push_root("skill", "git");
        assert_eq!(doc.to_string(), "# header\n# more\n\nskill = git\n");
        let mut doc = Document::parse("[repo /a]\n# note\n").unwrap();
        doc.push("repo", "/a", "profile", "coding");
        assert_eq!(doc.to_string(), "[repo /a]\n# note\n\nprofile = coding\n");
    }

    #[test]
    fn bom_and_crlf_are_tolerated() {
        let doc = Document::parse("\u{feff}a = 1\r\n[s]\r\nb = 2\r\n").unwrap();
        assert_eq!(doc.root().get("a").unwrap(), Some("1"));
        assert_eq!(doc.section("s", "").unwrap().get("b").unwrap(), Some("2"));
    }
}

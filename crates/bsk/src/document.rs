use std::fmt;
use std::str::FromStr;

use crate::{BLANK, Error, closest, is_key, parse};

/// A parsed BSK file. It keeps every line, including comments and blank
/// lines, so rendering an unedited document reproduces its input (apart from
/// a missing final newline, which is added).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    pub(crate) lines: Vec<Line>,
    pub(crate) crlf: bool,
    pub(crate) bom: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) text: String,
    pub(crate) kind: Kind,
    /// Line number in the parsed text, or 0 for lines added by an edit.
    pub(crate) number: usize,
    /// Byte offset of the first non-blank character.
    pub(crate) indent: usize,
    /// Byte offset of the value (entries) or the label (section headers).
    pub(crate) value_at: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Blank,
    Comment,
    Header { name: String, label: String },
    Entry { key: String, value: String },
}

impl Line {
    pub(crate) fn new(
        text: &str,
        kind: Kind,
        number: usize,
        indent: usize,
        value_at: usize,
    ) -> Self {
        Line {
            text: text.to_string(),
            kind,
            number,
            indent,
            value_at,
        }
    }

    fn entry(indent: &str, key: &str, value: &str) -> Self {
        let prefix = format!("{indent}{key}:{}", if value.is_empty() { "" } else { " " });
        Line {
            text: format!("{prefix}{value}"),
            kind: Kind::Entry {
                key: key.to_string(),
                value: value.to_string(),
            },
            number: 0,
            indent: indent.len(),
            value_at: prefix.len(),
        }
    }

    fn blank() -> Self {
        Line::new("", Kind::Blank, 0, 0, 0)
    }

    fn is_blank(&self) -> bool {
        self.kind == Kind::Blank
    }

    fn is_header(&self) -> bool {
        matches!(self.kind, Kind::Header { .. })
    }

    fn key(&self) -> Option<&str> {
        match &self.kind {
            Kind::Entry { key, .. } => Some(key),
            _ => None,
        }
    }

    fn value(&self) -> &str {
        match &self.kind {
            Kind::Entry { value, .. } => value,
            Kind::Header { label, .. } => label,
            _ => "",
        }
    }

    fn indentation(&self) -> &str {
        &self.text[..self.indent]
    }

    fn set_value(&mut self, new: &str) {
        let Kind::Entry { value, .. } = &mut self.kind else {
            unreachable!("set_value on a line that is not an entry")
        };
        let mut prefix = self.text[..self.value_at].to_string();
        if value.is_empty() && !new.is_empty() && !prefix.ends_with(BLANK) {
            prefix.push(' ');
        }
        *value = new.to_string();
        self.value_at = prefix.len();
        self.text = prefix + new;
    }
}

/// Which block of a document an edit applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target<'a> {
    /// The entries before the first section header.
    Root,
    /// The first section with this name and label.
    Section(&'a str, &'a str),
}

impl Document {
    /// An empty document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse BSK text. Fails on the first line that is not blank, a comment,
    /// a section header or an entry.
    pub fn parse(text: &str) -> Result<Self, Error> {
        parse::parse(text)
    }

    /// The entries before the first section header.
    pub fn root(&self) -> Block<'_> {
        let end = self
            .lines
            .iter()
            .position(Line::is_header)
            .unwrap_or(self.lines.len());
        Block {
            doc: self,
            header: None,
            start: 0,
            end,
        }
    }

    /// Every section, in file order.
    pub fn sections(&self) -> impl Iterator<Item = Block<'_>> {
        let headers: Vec<usize> = (0..self.lines.len())
            .filter(|&i| self.lines[i].is_header())
            .collect();
        let len = self.lines.len();
        (0..headers.len()).map(move |n| Block {
            doc: self,
            header: Some(headers[n]),
            start: headers[n],
            end: headers.get(n + 1).copied().unwrap_or(len),
        })
    }

    /// The first section with this name and label.
    pub fn section(&self, name: &str, label: &str) -> Option<Block<'_>> {
        self.sections()
            .find(|block| block.name() == Some(name) && block.label() == label)
    }

    /// The block an edit with this target would change, if it exists.
    pub fn block(&self, target: Target<'_>) -> Option<Block<'_>> {
        match target {
            Target::Root => Some(self.root()),
            Target::Section(name, label) => self.section(name, label),
        }
    }

    /// Give `key` exactly one value. An existing entry keeps its position,
    /// indentation and spacing; only the value changes. Further entries with
    /// the same key are removed. Without an existing entry, one is added as
    /// [`Document::add`] would. A missing section is created.
    pub fn set(&mut self, target: Target<'_>, key: &str, value: &str) -> Result<(), Error> {
        check_key(key)?;
        check_value(value)?;
        let (header, start, end) = self.ensure(target)?;
        let existing = self.entries_with(start, end, key, None);
        match existing.split_first() {
            Some((&first, rest)) => {
                for &index in rest.iter().rev() {
                    self.lines.remove(index);
                }
                self.lines[first].set_value(value);
            }
            None => self.insert(header, start, end, key, value),
        }
        Ok(())
    }

    /// Add an entry, keeping existing entries. The new line goes after the
    /// last entry with the same key, or in sorted position if those entries
    /// are sorted by value. With no such entries it goes after the block's
    /// last entry. A missing section is created.
    pub fn add(&mut self, target: Target<'_>, key: &str, value: &str) -> Result<(), Error> {
        check_key(key)?;
        check_value(value)?;
        let (header, start, end) = self.ensure(target)?;
        self.insert(header, start, end, key, value);
        Ok(())
    }

    /// Remove entries with `key` (and `value`, if given) from a block.
    /// Returns how many lines were removed.
    pub fn remove(&mut self, target: Target<'_>, key: &str, value: Option<&str>) -> usize {
        let Some(block) = self.block(target) else {
            return 0;
        };
        let (start, end) = (block.start, block.end);
        let doomed = self.entries_with(start, end, key, value);
        for &index in doomed.iter().rev() {
            self.lines.remove(index);
            self.collapse_blanks_at(index);
        }
        doomed.len()
    }

    /// Append a section header at the end of the document, separated from
    /// what comes before by a blank line.
    pub fn add_section(&mut self, name: &str, label: &str) -> Result<(), Error> {
        check_key(name)?;
        check_value(label)?;
        if self.lines.last().is_some_and(|line| !line.is_blank()) {
            self.lines.push(Line::blank());
        }
        self.lines.push(header_line(name, label));
        Ok(())
    }

    /// Remove the first section with this name and label, with its entries.
    /// Comments directly above the next section header stay, because they
    /// describe that section. Returns whether a section was removed.
    pub fn remove_section(&mut self, name: &str, label: &str) -> bool {
        let Some(block) = self.section(name, label) else {
            return false;
        };
        let (start, mut end) = (block.start, block.end);
        if end < self.lines.len() {
            while end > start + 1 && self.lines[end - 1].kind == Kind::Comment {
                end -= 1;
            }
        }
        self.lines.drain(start..end);
        if start > 0
            && self.lines[start - 1].is_blank()
            && self.lines.get(start).is_none_or(Line::is_blank)
        {
            self.lines.remove(start - 1);
        }
        while self.lines.last().is_some_and(Line::is_blank) {
            self.lines.pop();
        }
        true
    }

    /// Append a comment line. Each line of `text` becomes its own comment.
    pub fn push_comment(&mut self, text: &str) {
        for line in text.split('\n') {
            let line: String = line
                .chars()
                .filter(|c| !c.is_control() || *c == '\t')
                .collect();
            let line = line.trim_end_matches(BLANK);
            let text = if line.is_empty() {
                "#".to_string()
            } else {
                format!("# {line}")
            };
            self.lines.push(Line::new(&text, Kind::Comment, 0, 0, 0));
        }
    }

    /// Append a blank line.
    pub fn push_blank(&mut self) {
        self.lines.push(Line::blank());
    }

    /// Append an entry at the end of the document, which puts it in the last
    /// block.
    pub fn push_entry(&mut self, key: &str, value: &str) -> Result<(), Error> {
        check_key(key)?;
        check_value(value)?;
        self.lines.push(Line::entry("", key, value));
        Ok(())
    }

    /// Append a section header at the end of the document.
    pub fn push_section(&mut self, name: &str, label: &str) -> Result<(), Error> {
        check_key(name)?;
        check_value(label)?;
        self.lines.push(header_line(name, label));
        Ok(())
    }

    fn ensure(&mut self, target: Target<'_>) -> Result<(Option<usize>, usize, usize), Error> {
        if self.block(target).is_none()
            && let Target::Section(name, label) = target
        {
            self.add_section(name, label)?;
        }
        let block = self.block(target).expect("block exists after creation");
        Ok((block.header, block.start, block.end))
    }

    fn entries_with(&self, start: usize, end: usize, key: &str, value: Option<&str>) -> Vec<usize> {
        (start..end)
            .filter(|&i| {
                let line = &self.lines[i];
                line.key() == Some(key) && value.is_none_or(|value| line.value() == value)
            })
            .collect()
    }

    fn insert(&mut self, header: Option<usize>, start: usize, end: usize, key: &str, value: &str) {
        let entries: Vec<usize> = (start..end)
            .filter(|&i| self.lines[i].key().is_some())
            .collect();
        let same: Vec<usize> = entries
            .iter()
            .copied()
            .filter(|&i| self.lines[i].key() == Some(key))
            .collect();

        if let Some(&last) = same.last() {
            let sorted = same
                .windows(2)
                .all(|w| self.lines[w[0]].value() <= self.lines[w[1]].value());
            let before = if sorted {
                same.iter()
                    .copied()
                    .find(|&i| self.lines[i].value() > value)
            } else {
                None
            };
            let at = before.unwrap_or(last + 1);
            let indent = self.lines[before.unwrap_or(last)].indentation().to_string();
            self.lines.insert(at, Line::entry(&indent, key, value));
            return;
        }
        if let Some(&last) = entries.last() {
            let indent = self.lines[last].indentation().to_string();
            self.lines
                .insert(last + 1, Line::entry(&indent, key, value));
            return;
        }
        if let Some(header) = header {
            self.lines.insert(header + 1, Line::entry("", key, value));
            return;
        }
        // An empty root: go below any leading comments, set off by a blank
        // line, and keep a blank line before a following section header.
        let at = (start..end)
            .rev()
            .find(|&i| !self.lines[i].is_blank())
            .map_or(start, |i| i + 1);
        let mut new = Vec::new();
        if at > 0 && self.lines[at - 1].kind == Kind::Comment {
            new.push(Line::blank());
        }
        new.push(Line::entry("", key, value));
        if self.lines.get(at).is_some_and(Line::is_header) {
            new.push(Line::blank());
        }
        self.lines.splice(at..at, new);
    }

    fn collapse_blanks_at(&mut self, index: usize) {
        if index > 0
            && index < self.lines.len()
            && self.lines[index - 1].is_blank()
            && self.lines[index].is_blank()
        {
            self.lines.remove(index);
        }
    }
}

fn header_line(name: &str, label: &str) -> Line {
    let (text, label_at) = if label.is_empty() {
        (format!("[{name}]"), name.len() + 1)
    } else {
        (format!("[{name} {label}]"), name.len() + 2)
    };
    let kind = Kind::Header {
        name: name.to_string(),
        label: label.to_string(),
    };
    Line::new(&text, kind, 0, 0, label_at)
}

fn check_key(key: &str) -> Result<(), Error> {
    if is_key(key) {
        Ok(())
    } else {
        Err(Error::document(format!(
            "`{key}` is not a valid key: keys are a lowercase letter followed by lowercase letters, digits and `-`"
        )))
    }
}

fn check_value(value: &str) -> Result<(), Error> {
    if value.chars().any(|c| c.is_control() && c != '\t') {
        return Err(Error::document(format!(
            "{value:?} cannot be stored: BSK values are single lines without control characters"
        )));
    }
    if value.trim_matches(BLANK) != value {
        return Err(Error::document(format!(
            "{value:?} cannot be stored: BSK values do not keep leading or trailing whitespace"
        )));
    }
    Ok(())
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bom {
            f.write_str("\u{feff}")?;
        }
        let newline = if self.crlf { "\r\n" } else { "\n" };
        for line in &self.lines {
            f.write_str(&line.text)?;
            f.write_str(newline)?;
        }
        Ok(())
    }
}

impl FromStr for Document {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        Document::parse(text)
    }
}

/// A view of the entries before the first section header (the root block),
/// or of one section.
#[derive(Clone, Copy, Debug)]
pub struct Block<'a> {
    doc: &'a Document,
    header: Option<usize>,
    start: usize,
    end: usize,
}

impl<'a> Block<'a> {
    /// The section name, or `None` for the root block.
    pub fn name(self) -> Option<&'a str> {
        match &self.doc.lines[self.header?].kind {
            Kind::Header { name, .. } => Some(name),
            _ => None,
        }
    }

    /// The section label, empty for the root block and label-less sections.
    pub fn label(self) -> &'a str {
        self.header.map_or("", |i| self.doc.lines[i].value())
    }

    /// Line number of the section header, 0 for the root block.
    pub fn line(self) -> usize {
        self.header.map_or(0, |i| self.doc.lines[i].number)
    }

    /// The block's entries, in file order.
    pub fn entries(self) -> impl Iterator<Item = Entry<'a>> {
        let doc = self.doc;
        (self.start..self.end).filter_map(move |i| {
            let line = &doc.lines[i];
            line.key().is_some().then_some(Entry { line })
        })
    }

    /// Every entry with this key, in file order.
    pub fn all(self, key: &str) -> Vec<Entry<'a>> {
        self.entries().filter(|entry| entry.key() == key).collect()
    }

    /// The entry with this key, if there is one. Fails if the key appears
    /// more than once, because a single value was expected.
    pub fn get(self, key: &str) -> Result<Option<Entry<'a>>, Error> {
        let all = self.all(key);
        match all.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(*only)),
            [first, second, ..] => Err(second
                .key_error(format!("`{key}` is given more than once"))
                .with_help(format!(
                    "`{key}` takes a single value and line {} already sets it; keep one line",
                    first.line()
                ))),
        }
    }

    /// The entry with this key. Fails if it is missing or repeated.
    pub fn require(self, key: &str) -> Result<Entry<'a>, Error> {
        match self.get(key)? {
            Some(entry) => Ok(entry),
            None => Err(self
                .error(format!("missing `{key}`"))
                .with_help(format!("add a line `{key}: <value>`"))),
        }
    }

    /// Fail on the first entry whose key is not in `allowed`, suggesting the
    /// closest allowed key.
    pub fn check_keys(self, allowed: &[&str]) -> Result<(), Error> {
        let Some(entry) = self.entries().find(|entry| !allowed.contains(&entry.key())) else {
            return Ok(());
        };
        let error = entry.key_error(format!("unknown key `{}`", entry.key()));
        Err(match closest(entry.key(), allowed.iter().copied()) {
            Some(close) => error.with_help(format!("did you mean `{close}`?")),
            None if allowed.is_empty() => error.with_help("this block takes no keys"),
            None => error.with_help(format!(
                "expected {}",
                allowed
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        })
    }

    /// An error about the block as a whole, pointing at its header.
    pub fn error(self, message: impl Into<String>) -> Error {
        match self.header {
            Some(i) => {
                let line = &self.doc.lines[i];
                Error::at(
                    &line.text,
                    line.number,
                    line.indent,
                    line.text.len() - line.indent,
                    message,
                )
            }
            None => Error::document(message),
        }
    }

    /// An error about the section label.
    pub fn label_error(self, message: impl Into<String>) -> Error {
        match self.header {
            Some(i) => {
                let line = &self.doc.lines[i];
                let len = line.value().len();
                Error::at(&line.text, line.number, line.value_at, len, message)
            }
            None => Error::document(message),
        }
    }
}

/// One `key: value` line.
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    line: &'a Line,
}

impl<'a> Entry<'a> {
    pub fn key(self) -> &'a str {
        self.line.key().unwrap_or_default()
    }

    pub fn value(self) -> &'a str {
        self.line.value()
    }

    /// Line number, or 0 if the entry was added by an edit.
    pub fn line(self) -> usize {
        self.line.number
    }

    /// An error about the value, underlining it.
    pub fn error(self, message: impl Into<String>) -> Error {
        let line = self.line;
        Error::at(
            &line.text,
            line.number,
            line.value_at,
            line.value().len(),
            message,
        )
    }

    /// An error about the key, underlining it.
    pub fn key_error(self, message: impl Into<String>) -> Error {
        let line = self.line;
        Error::at(
            &line.text,
            line.number,
            line.indent,
            self.key().len(),
            message,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Document {
        Document::parse(text).expect("document parses")
    }

    #[test]
    fn rendering_an_unedited_document_reproduces_it() {
        for text in [
            "",
            "# only a comment\n",
            "a: 1\n\n# note\n  b:   2  \n[repo /x y]\nc: 3\n",
            "a: 1\r\nb: 2\r\n",
            "\u{feff}a: 1\n",
        ] {
            assert_eq!(doc(text).to_string(), text);
        }
        assert_eq!(doc("a: 1").to_string(), "a: 1\n");
    }

    #[test]
    fn root_holds_entries_before_the_first_section() {
        let d = doc("a: 1\nb: 2\n[s one]\nc: 3\n[s two]\n[t]\nd: 4\n");
        let root: Vec<_> = d.root().entries().map(|e| (e.key(), e.value())).collect();
        assert_eq!(root, [("a", "1"), ("b", "2")]);
        let sections: Vec<_> = d
            .sections()
            .map(|s| (s.name().unwrap(), s.label(), s.entries().count()))
            .collect();
        assert_eq!(sections, [("s", "one", 1), ("s", "two", 0), ("t", "", 1)]);
        assert_eq!(d.section("t", "").unwrap().all("d")[0].value(), "4");
        assert_eq!(d.section("s", "two").unwrap().line(), 5);
    }

    #[test]
    fn repeated_keys_form_a_list() {
        let d = doc("skill: git\ndescription: x\nskill: pdf\n");
        let skills: Vec<_> = d.root().all("skill").iter().map(|e| e.value()).collect();
        assert_eq!(skills, ["git", "pdf"]);
    }

    #[test]
    fn get_rejects_a_repeated_single_value_key() {
        let d = doc("library: a\nlibrary: b\n");
        let error = d.root().get("library").unwrap_err();
        assert_eq!(error.line, 2);
        assert_eq!(error.message, "`library` is given more than once");
    }

    #[test]
    fn require_reports_missing_keys() {
        let d = doc("[repo /x]\n");
        let error = d
            .section("repo", "/x")
            .unwrap()
            .require("path")
            .unwrap_err();
        assert_eq!(error.message, "missing `path`");
        assert_eq!(error.line, 1);
        let error = d.root().require("version").unwrap_err();
        assert_eq!(error.line, 0);
    }

    #[test]
    fn unknown_keys_get_a_suggestion() {
        let d = doc("skils: git\n");
        let error = d.root().check_keys(&["skill", "description"]).unwrap_err();
        assert_eq!(error.message, "unknown key `skils`");
        assert_eq!(error.help.as_deref(), Some("did you mean `skill`?"));
        assert_eq!((error.line, error.column, error.width), (1, 1, 5));
        let d = doc("zzz: 1\n");
        let error = d.root().check_keys(&["skill", "description"]).unwrap_err();
        assert_eq!(
            error.help.as_deref(),
            Some("expected `skill`, `description`")
        );
    }

    #[test]
    fn value_errors_underline_the_value() {
        let d = doc("  on-conflict:  maybe\n");
        let entry = d.root().require("on-conflict").unwrap();
        let error = entry.error("bad policy");
        assert_eq!((error.column, error.width), (17, 5));
    }

    #[test]
    fn set_replaces_a_value_in_place() {
        let mut d = doc("# the library\nlibrary:    ~/old   \nregistry: r\n");
        d.set(Target::Root, "library", "~/new").unwrap();
        assert_eq!(
            d.to_string(),
            "# the library\nlibrary:    ~/new\nregistry: r\n"
        );
        d.set(Target::Root, "registry", "").unwrap();
        d.set(Target::Root, "registry", "back").unwrap();
        assert_eq!(
            d.to_string(),
            "# the library\nlibrary:    ~/new\nregistry: back\n"
        );
    }

    #[test]
    fn set_collapses_duplicates_and_appends_missing_keys() {
        let mut d = doc("a: 1\na: 2\nb: 3\n");
        d.set(Target::Root, "a", "9").unwrap();
        d.set(Target::Root, "c", "4").unwrap();
        assert_eq!(d.to_string(), "a: 9\nb: 3\nc: 4\n");
    }

    #[test]
    fn add_appends_after_the_last_entry_with_the_same_key() {
        let mut d = doc("description: x\nskill: zeta\nskill: alpha\n\n# trailing note\n");
        d.add(Target::Root, "skill", "beta").unwrap();
        assert_eq!(
            d.to_string(),
            "description: x\nskill: zeta\nskill: alpha\nskill: beta\n\n# trailing note\n"
        );
    }

    #[test]
    fn add_keeps_a_sorted_list_sorted() {
        let mut d = doc("skill: alpha\n  skill: gamma\n");
        d.add(Target::Root, "skill", "beta").unwrap();
        d.add(Target::Root, "skill", "delta").unwrap();
        d.add(Target::Root, "skill", "aardvark").unwrap();
        assert_eq!(
            d.to_string(),
            "skill: aardvark\nskill: alpha\n  skill: beta\n  skill: delta\n  skill: gamma\n"
        );
    }

    #[test]
    fn add_to_a_commented_empty_root_leaves_a_blank_line() {
        let mut d = doc("# Profile: coding\n# One skill per line.\n");
        d.add(Target::Root, "skill", "git").unwrap();
        assert_eq!(
            d.to_string(),
            "# Profile: coding\n# One skill per line.\n\nskill: git\n"
        );
    }

    #[test]
    fn add_to_an_empty_root_before_a_section() {
        let mut d = doc("[repo /x]\nprofile: a\n");
        d.add(Target::Root, "version", "1").unwrap();
        assert_eq!(d.to_string(), "version: 1\n\n[repo /x]\nprofile: a\n");
        let mut d = doc("# header\n\n[repo /x]\n");
        d.add(Target::Root, "version", "1").unwrap();
        assert_eq!(d.to_string(), "# header\n\nversion: 1\n\n[repo /x]\n");
    }

    #[test]
    fn add_to_sections() {
        let mut d = doc("[repo /a]\nprofile: x\n\n# about b\n[repo /b]\n");
        d.add(Target::Section("repo", "/a"), "profile", "y")
            .unwrap();
        d.add(Target::Section("repo", "/b"), "profile", "z")
            .unwrap();
        d.add(Target::Section("repo", "/c"), "profile", "w")
            .unwrap();
        assert_eq!(
            d.to_string(),
            "[repo /a]\nprofile: x\nprofile: y\n\n# about b\n[repo /b]\nprofile: z\n\n[repo /c]\nprofile: w\n"
        );
    }

    #[test]
    fn remove_deletes_matching_entries() {
        let mut d = doc("skill: a\nskill: b\nskill: a\ndescription: d\n");
        assert_eq!(d.remove(Target::Root, "skill", Some("a")), 2);
        assert_eq!(d.to_string(), "skill: b\ndescription: d\n");
        assert_eq!(d.remove(Target::Root, "skill", None), 1);
        assert_eq!(d.remove(Target::Section("nope", ""), "skill", None), 0);
        assert_eq!(d.to_string(), "description: d\n");
    }

    #[test]
    fn remove_does_not_leave_double_blank_lines() {
        let mut d = doc("a: 1\n\nb: 2\n\nc: 3\n");
        d.remove(Target::Root, "b", None);
        assert_eq!(d.to_string(), "a: 1\n\nc: 3\n");
    }

    #[test]
    fn remove_section_keeps_the_next_sections_comment() {
        let mut d =
            doc("version: 1\n\n[repo /a]\nprofile: x\n\n# about b\n[repo /b]\nprofile: y\n");
        assert!(d.remove_section("repo", "/a"));
        assert_eq!(
            d.to_string(),
            "version: 1\n\n# about b\n[repo /b]\nprofile: y\n"
        );
        assert!(d.remove_section("repo", "/b"));
        assert_eq!(d.to_string(), "version: 1\n\n# about b\n");
        assert!(!d.remove_section("repo", "/b"));
    }

    #[test]
    fn edits_reject_values_that_cannot_round_trip() {
        let mut d = Document::new();
        assert!(d.set(Target::Root, "a", "two\nlines").is_err());
        assert!(d.set(Target::Root, "a", " padded").is_err());
        assert!(d.set(Target::Root, "Bad", "x").is_err());
        assert!(d.add_section("repo", "trailing ").is_err());
    }

    #[test]
    fn builder_output_parses_back() {
        let mut d = Document::new();
        d.push_comment("Registry\nsecond line");
        d.push_entry("version", "1").unwrap();
        d.push_blank();
        d.push_section("repo", "/home/me/My Code").unwrap();
        d.push_entry("profile", "coding").unwrap();
        d.push_entry("empty", "").unwrap();
        let text = d.to_string();
        assert_eq!(
            text,
            "# Registry\n# second line\nversion: 1\n\n[repo /home/me/My Code]\nprofile: coding\nempty:\n"
        );
        let back = doc(&text);
        assert_eq!(
            back.section("repo", "/home/me/My Code")
                .unwrap()
                .require("profile")
                .unwrap()
                .value(),
            "coding"
        );
    }
}

#[cfg(test)]
mod properties {
    use super::*;

    /// xorshift64*: deterministic randomness without a dependency.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> usize {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as usize
        }

        fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
            items[self.next() % items.len()]
        }
    }

    #[test]
    fn arbitrary_input_never_panics_and_rendering_is_idempotent() {
        let pieces = [
            "a",
            "b",
            "key",
            "x1",
            "-",
            ":",
            " ",
            "\t",
            "#",
            "[",
            "]",
            "=",
            "\"",
            "é",
            "\n",
            "\r\n",
            "\r",
            "\u{feff}",
            "\u{7}",
            "repo /x",
            "skill: git",
            "[repo /a]",
            "  ",
            "Z",
        ];
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut parsed = 0;
        for _ in 0..20_000 {
            let len = rng.next() % 30;
            let text: String = (0..len).map(|_| rng.pick(&pieces)).collect();
            let Ok(doc) = Document::parse(&text) else {
                continue;
            };
            parsed += 1;
            let rendered = doc.to_string();
            let again = Document::parse(&rendered).expect("rendered output parses");
            assert_eq!(again.to_string(), rendered, "input {text:?}");
        }
        assert!(
            parsed > 1000,
            "the generator should produce valid documents too ({parsed})"
        );
    }

    #[test]
    fn edits_match_a_reference_model() {
        let keys = ["skill", "profile", "description"];
        let values = [
            "git",
            "pdf",
            "code-review",
            "alpha",
            "zeta",
            "",
            "a b",
            "#hash",
            "x: y",
        ];
        let blocks = [
            Target::Root,
            Target::Section("repo", "/a"),
            Target::Section("repo", "/b b"),
        ];
        let mut rng = Rng(0xD1B5_4A32_D192_ED03);
        for round in 0..300 {
            let mut doc =
                Document::parse("# header\n\n[repo /a]\n# about a\nprofile: x\n").unwrap();
            let mut model: Vec<(Target, &str, Vec<String>)> =
                vec![(Target::Section("repo", "/a"), "profile", vec!["x".into()])];
            for _ in 0..25 {
                let target = blocks[rng.next() % blocks.len()];
                let key = rng.pick(&keys);
                let value = rng.pick(&values);
                let slot = match model.iter().position(|(t, k, _)| *t == target && *k == key) {
                    Some(i) => i,
                    None => {
                        model.push((target, key, Vec::new()));
                        model.len() - 1
                    }
                };
                let list = &mut model[slot].2;
                match rng.next() % 3 {
                    0 => {
                        doc.set(target, key, value).unwrap();
                        *list = vec![value.to_string()];
                    }
                    1 => {
                        let sorted = list.windows(2).all(|w| w[0] <= w[1]);
                        doc.add(target, key, value).unwrap();
                        match list
                            .iter()
                            .position(|v| v.as_str() > value)
                            .filter(|_| sorted)
                        {
                            Some(at) => list.insert(at, value.to_string()),
                            None => list.push(value.to_string()),
                        }
                    }
                    _ => {
                        doc.remove(target, key, Some(value));
                        list.retain(|v| v != value);
                    }
                }
            }
            let text = doc.to_string();
            let reparsed =
                Document::parse(&text).unwrap_or_else(|e| panic!("round {round}: {e}\n{text}"));
            assert!(
                text.starts_with("# header\n"),
                "round {round}: comments stay\n{text}"
            );
            for (target, key, expected) in &model {
                let actual: Vec<String> = reparsed
                    .block(*target)
                    .map(|block| {
                        block
                            .all(key)
                            .iter()
                            .map(|e| e.value().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(&actual, expected, "round {round}, {target:?} {key}\n{text}");
            }
        }
    }
}

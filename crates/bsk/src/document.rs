//! A parsed bsk file that prints back byte for byte.
//!
//! The document keeps every line's original text and its own line ending.
//! Reading never changes it. Editing touches only the lines it has to, so
//! comments, blank lines, indentation, column alignment and line endings
//! written by a human survive a round trip through a program.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::str::FromStr;

use crate::diagnostic::{Diagnostic, Diagnostics, MAX_DIAGNOSTICS};
use crate::syntax::{self, Kind, Span, ValueError, is_blank};

/// How one line ends. Every line keeps its own ending, so a file that mixes
/// LF and CRLF prints back exactly as it was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Eol {
    /// No line break: only the last line of a file can end this way.
    Missing,
    Lf,
    CrLf,
}

impl Eol {
    fn as_str(self) -> &'static str {
        match self {
            Eol::Missing => "",
            Eol::Lf => "\n",
            Eol::CrLf => "\r\n",
        }
    }
}

/// Cuts `text` into lines. A line break at the end of the text ends the last
/// line and does not start another one.
fn split_lines(text: &str) -> impl Iterator<Item = (&str, Eol)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let Some(at) = rest.find('\n') else {
            return Some((std::mem::take(&mut rest), Eol::Missing));
        };
        let (line, tail) = (&rest[..at], &rest[at + 1..]);
        rest = tail;
        Some(match line.strip_suffix('\r') {
            Some(line) => (line, Eol::CrLf),
            None => (line, Eol::Lf),
        })
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    /// The text of the line without its line break.
    raw: String,
    eol: Eol,
    kind: Kind,
}

impl Line {
    /// Builds a line from text that is known to be valid.
    fn from_valid(raw: String, eol: Eol) -> Line {
        let kind =
            syntax::classify(&raw).expect("text built from checked parts is always a valid line");
        Line { raw, eol, kind }
    }
}

/// The text of a `key value` line, after checking that it reads back as one.
fn entry_text(key: &str, value: &str, indent: &str, gap: &str) -> Result<String, ValueError> {
    syntax::check_word(key)?;
    syntax::check_value(value)?;
    Ok(if value.is_empty() {
        format!("{indent}{key}")
    } else {
        format!("{indent}{key}{gap}{value}")
    })
}

/// A bsk file.
///
/// ```
/// use bsk::Document;
///
/// let mut doc: Document = "# my profile\nskill git\n".parse().unwrap();
/// doc.root_mut().add("skill", "testing").unwrap();
/// assert_eq!(doc.to_string(), "# my profile\nskill git\nskill testing\n");
/// ```
///
/// Every line keeps its own line ending, so text that mixes LF and CRLF, or has no
/// line break at the end, prints back exactly as it was read:
///
/// ```
/// use bsk::Document;
///
/// let text = "a 1\r\nb 2\nc 3";
/// let doc: Document = text.parse().unwrap();
/// assert_eq!(doc.to_string(), text);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    bom: bool,
    lines: Vec<Line>,
}

impl Default for Document {
    fn default() -> Self {
        Document::new()
    }
}

impl Document {
    /// An empty document.
    pub fn new() -> Self {
        Document {
            bom: false,
            lines: Vec::new(),
        }
    }

    /// Parses `text`, reporting the syntax problems it finds with their line and column.
    ///
    /// The error holds at most 50 problems. When there are more, [`Diagnostics::truncated`] is true.
    /// Parsing checks syntax only. To check that the keys make sense, use a [`Schema`](crate::Schema).
    ///
    /// Every line remembers its own line ending (LF, CRLF, or none on the last line),
    /// so text that parses prints back exactly as it was written.
    pub fn parse(text: &str) -> Result<Document, Diagnostics> {
        let (bom, text) = match text.strip_prefix('\u{feff}') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let mut lines = Vec::new();
        let mut problems = Vec::new();
        let mut truncated = false;
        for (i, (raw, eol)) in split_lines(text).enumerate() {
            match syntax::classify(raw) {
                Ok(kind) => lines.push(Line {
                    raw: raw.to_string(),
                    eol,
                    kind,
                }),
                Err(_) if problems.len() == MAX_DIAGNOSTICS => {
                    truncated = true;
                    break;
                }
                Err(error) => problems.push(error.into_diagnostic(i + 1)),
            }
        }
        match Diagnostics::collect(problems, truncated) {
            Some(problems) => Err(problems),
            None => Ok(Document { bom, lines }),
        }
    }

    /// The entries that come before the first section header.
    pub fn root(&self) -> Scope<'_> {
        let end = self.bounds()[0].2;
        Scope {
            doc: self,
            header: None,
            start: 0,
            end,
        }
    }

    /// Every section, in file order.
    pub fn sections(&self) -> Vec<Scope<'_>> {
        self.bounds()
            .into_iter()
            .filter_map(|(header, start, end)| {
                header.map(|_| Scope {
                    doc: self,
                    header,
                    start,
                    end,
                })
            })
            .collect()
    }

    /// The first section with this kind and name.
    pub fn section(&self, kind: &str, name: &str) -> Option<Scope<'_>> {
        self.sections()
            .into_iter()
            .find(|s| s.kind() == Some(kind) && s.name() == Some(name))
    }

    /// Edits the entries that come before the first section header.
    pub fn root_mut(&mut self) -> ScopeMut<'_> {
        let end = self.bounds()[0].2;
        ScopeMut {
            doc: self,
            header: None,
            start: 0,
            end,
        }
    }

    /// Edits the first section with this kind and name.
    pub fn section_mut(&mut self, kind: &str, name: &str) -> Option<ScopeMut<'_>> {
        let found = self
            .sections()
            .into_iter()
            .find(|s| s.kind() == Some(kind) && s.name() == Some(name))?;
        let (header, start, end) = (found.header, found.start, found.end);
        Some(ScopeMut {
            doc: self,
            header,
            start,
            end,
        })
    }

    /// Appends a comment. Each line of `text` becomes one `# ...` line.
    ///
    /// Every appended line ends like the first terminated line of the document, or with LF
    /// if there is none. If the document's last line has no line break, it gets one first.
    pub fn push_comment(&mut self, text: &str) {
        for line in text.lines() {
            let clean: String = line
                .chars()
                .map(|c| if syntax::is_forbidden(c) { ' ' } else { c })
                .collect();
            let raw = if clean.trim().is_empty() {
                "#".to_string()
            } else {
                format!("# {}", clean.trim_end())
            };
            self.insert_line(self.lines.len(), raw);
        }
    }

    /// Appends an empty line.
    pub fn push_blank(&mut self) {
        self.insert_line(self.lines.len(), String::new());
    }

    /// Appends `key value` at the end of the document, so inside the last section.
    pub fn push_entry(&mut self, key: &str, value: &str) -> Result<(), ValueError> {
        let raw = entry_text(key, value, "", " ")?;
        self.insert_line(self.lines.len(), raw);
        Ok(())
    }

    /// Appends a `key value` line whose value starts at `column` (1-based), padding with spaces.
    /// A `column` that is too small falls back to a single space.
    pub fn push_entry_aligned(
        &mut self,
        key: &str,
        value: &str,
        column: usize,
    ) -> Result<(), ValueError> {
        let gap = " ".repeat(column.saturating_sub(1 + key.chars().count()).max(1));
        let raw = entry_text(key, value, "", &gap)?;
        self.insert_line(self.lines.len(), raw);
        Ok(())
    }

    /// Appends a section header `[kind name]`. An empty `name` writes `[kind]`.
    pub fn push_section(&mut self, kind: &str, name: &str) -> Result<(), ValueError> {
        syntax::check_word(kind)?;
        syntax::check_value(name)?;
        let raw = if name.is_empty() {
            format!("[{kind}]")
        } else {
            format!("[{kind} {name}]")
        };
        self.insert_line(self.lines.len(), raw);
        Ok(())
    }

    /// The line ending that new lines get: the one on the first line that has one, else LF.
    fn default_eol(&self) -> Eol {
        self.lines
            .iter()
            .map(|line| line.eol)
            .find(|&eol| eol != Eol::Missing)
            .unwrap_or(Eol::Lf)
    }

    /// Inserts a new line at index `at` with the default line ending.
    ///
    /// If it lands right after a last line that has no line break, that line gets one,
    /// so the two lines cannot run together. No other line changes.
    fn insert_line(&mut self, at: usize, raw: String) {
        let eol = self.default_eol();
        if let Some(previous) = at.checked_sub(1).and_then(|i| self.lines.get_mut(i))
            && previous.eol == Eol::Missing
        {
            previous.eol = eol;
        }
        self.lines.insert(at, Line::from_valid(raw, eol));
    }

    /// (header line, first body line, end of body) for the root and every section.
    fn bounds(&self) -> Vec<(Option<usize>, usize, usize)> {
        let mut out = vec![(None, 0, self.lines.len())];
        for (i, line) in self.lines.iter().enumerate() {
            if matches!(line.kind, Kind::Header { .. }) {
                if let Some(last) = out.last_mut() {
                    last.2 = i;
                }
                out.push((Some(i), i + 1, self.lines.len()));
            }
        }
        out
    }
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bom {
            f.write_str("\u{feff}")?;
        }
        for line in &self.lines {
            f.write_str(&line.raw)?;
            f.write_str(line.eol.as_str())?;
        }
        Ok(())
    }
}

impl FromStr for Document {
    type Err = Diagnostics;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Document::parse(text)
    }
}

/// One `key value` line.
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    index: usize,
    raw: &'a str,
    key: Span,
    value: Span,
}

impl<'a> Entry<'a> {
    /// The key.
    pub fn key(&self) -> &'a str {
        self.key.slice(self.raw)
    }

    /// The value: the rest of the line with surrounding whitespace removed. Empty if there is none.
    pub fn value(&self) -> &'a str {
        self.value.slice(self.raw)
    }

    /// The 1-based line number.
    pub fn line(&self) -> usize {
        self.index + 1
    }

    /// The 1-based column where the key starts.
    pub fn key_column(&self) -> usize {
        syntax::column_of(self.raw, self.key.start)
    }

    /// The 1-based column where the value starts.
    pub fn value_column(&self) -> usize {
        syntax::column_of(self.raw, self.value.start)
    }

    /// A problem with this entry's value, underlined in place. Falls back to the key when the value is empty.
    ///
    /// A value that starts with `=` or `:` gets a hint, because that is what people write
    /// when they think bsk lines look like `key = value`.
    pub fn diagnostic(&self, message: impl Into<String>) -> Diagnostic {
        let value = self.value();
        if value.is_empty() {
            return self.key_diagnostic(message);
        }
        let diagnostic = Diagnostic::new(
            self.line(),
            self.value_column(),
            value.chars().count(),
            message,
        );
        if value.starts_with(['=', ':']) {
            diagnostic.with_hint("bsk lines are 'key value', with no '=' or ':' between them")
        } else {
            diagnostic
        }
    }

    /// A problem with this entry's key, underlined in place.
    pub fn key_diagnostic(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(
            self.line(),
            self.key_column(),
            self.key().chars().count(),
            message,
        )
    }
}

/// The lines under one section header, or the lines before the first header.
#[derive(Clone, Copy, Debug)]
pub struct Scope<'a> {
    doc: &'a Document,
    header: Option<usize>,
    start: usize,
    end: usize,
}

impl<'a> Scope<'a> {
    /// True for the part of the file before any section header.
    pub fn is_root(self) -> bool {
        self.header.is_none()
    }

    /// The section kind, such as `repo` in `[repo /path]`. `None` for the root.
    pub fn kind(self) -> Option<&'a str> {
        let line = &self.doc.lines[self.header?];
        match line.kind {
            Kind::Header { kind, .. } => Some(kind.slice(&line.raw)),
            _ => None,
        }
    }

    /// The section name, such as `/path` in `[repo /path]`. Empty when the header has none. `None` for the root.
    pub fn name(self) -> Option<&'a str> {
        let line = &self.doc.lines[self.header?];
        match line.kind {
            Kind::Header { name, .. } => Some(name.slice(&line.raw)),
            _ => None,
        }
    }

    /// The 1-based line of the section header. 0 for the root.
    pub fn line(self) -> usize {
        self.header.map_or(0, |i| i + 1)
    }

    /// A problem with the section header, underlined in place. For the root it points at line 1.
    pub fn diagnostic(self, message: impl Into<String>) -> Diagnostic {
        match self.header {
            Some(i) => {
                let raw = &self.doc.lines[i].raw;
                let indent = raw.len() - raw.trim_start_matches(is_blank).len();
                let width = raw.trim().chars().count();
                Diagnostic::new(i + 1, syntax::column_of(raw, indent), width, message)
            }
            None => Diagnostic::new(1, 1, 1, message),
        }
    }

    /// Every entry, in file order.
    pub fn entries(self) -> impl Iterator<Item = Entry<'a>> {
        (self.start..self.end).filter_map(move |i| entry_at(self.doc, i))
    }

    /// The first entry with this key.
    pub fn get(self, key: &str) -> Option<Entry<'a>> {
        self.entries().find(|e| e.key() == key)
    }

    /// Every entry with this key, in file order.
    pub fn all(self, key: &str) -> impl Iterator<Item = Entry<'a>> {
        let key = key.to_string();
        self.entries().filter(move |e| e.key() == key)
    }

    /// The value of the first entry with this key.
    pub fn value(self, key: &str) -> Option<&'a str> {
        self.get(key).map(|e| e.value())
    }

    /// The values of every entry with this key, in file order.
    pub fn values(self, key: &str) -> impl Iterator<Item = &'a str> {
        self.all(key).map(|e| e.value())
    }
}

fn entry_at(doc: &Document, index: usize) -> Option<Entry<'_>> {
    let line = &doc.lines[index];
    match line.kind {
        Kind::Entry { key, value } => Some(Entry {
            index,
            raw: &line.raw,
            key,
            value,
        }),
        _ => None,
    }
}

/// Edits the lines of one scope in place.
///
/// Edits change only the lines they mean to:
///
/// * A comment directly above a line, with no blank line between, is attached to that
///   line. Edits never delete comments, and a new line never goes between a comment and
///   the line it is attached to.
/// * New lines copy the indentation of the nearest entry. They line their value up with
///   the others only when the scope already does that: at least two entries whose keys
///   differ in length have their values in the same column.
/// * Every line keeps its own line ending. New lines use the ending of the document's
///   first terminated line, or LF if it has none.
#[derive(Debug)]
pub struct ScopeMut<'a> {
    doc: &'a mut Document,
    header: Option<usize>,
    start: usize,
    end: usize,
}

impl ScopeMut<'_> {
    /// A read-only view of this scope.
    pub fn view(&self) -> Scope<'_> {
        Scope {
            doc: self.doc,
            header: self.header,
            start: self.start,
            end: self.end,
        }
    }

    /// Gives `key` exactly one entry with this value.
    ///
    /// Replaces the value of the first entry in place and deletes further entries with the same key.
    /// If there is none, adds one after the last entry of the scope. Comments stay where they are.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), ValueError> {
        syntax::check_word(key)?;
        syntax::check_value(value)?;
        let indices = self.indices_of(key);
        match indices.split_first() {
            Some((&first, rest)) => {
                self.replace_value(first, value);
                for &i in rest.iter().rev() {
                    self.remove_line(i);
                }
            }
            None => self.insert_new(key, value)?,
        }
        Ok(())
    }

    /// Adds `key value` unless that exact entry exists. Returns whether it added a line.
    ///
    /// The new line goes after the last entry with the same key. If the existing values
    /// are in ascending order, it goes to its sorted position instead, above the comments
    /// of the entry it comes before.
    pub fn add(&mut self, key: &str, value: &str) -> Result<bool, ValueError> {
        syntax::check_word(key)?;
        syntax::check_value(value)?;
        if self.view().all(key).any(|e| e.value() == value) {
            return Ok(false);
        }
        self.insert_new(key, value)?;
        Ok(true)
    }

    /// Deletes every entry with this key. Returns how many lines it deleted.
    /// Comments above the deleted lines stay.
    pub fn remove(&mut self, key: &str) -> usize {
        let indices = self.indices_of(key);
        for &i in indices.iter().rev() {
            self.remove_line(i);
        }
        indices.len()
    }

    /// Deletes every entry with this key and value. Returns how many lines it deleted.
    pub fn remove_value(&mut self, key: &str, value: &str) -> usize {
        let indices: Vec<usize> = self
            .indices_of(key)
            .into_iter()
            .filter(|&i| entry_at(self.doc, i).is_some_and(|e| e.value() == value))
            .collect();
        for &i in indices.iter().rev() {
            self.remove_line(i);
        }
        indices.len()
    }

    fn indices_of(&self, key: &str) -> Vec<usize> {
        (self.start..self.end)
            .filter(|&i| entry_at(self.doc, i).is_some_and(|e| e.key() == key))
            .collect()
    }

    fn remove_line(&mut self, index: usize) {
        self.doc.lines.remove(index);
        self.end -= 1;
    }

    fn replace_value(&mut self, index: usize, value: &str) {
        let line = &mut self.doc.lines[index];
        let Kind::Entry { key, value: old } = line.kind else {
            return;
        };
        let (head, tail) = if old.start == old.end {
            (format!("{} ", &line.raw[..key.end]), "")
        } else {
            (line.raw[..old.start].to_string(), &line.raw[old.end..])
        };
        let mut raw = format!("{head}{value}{tail}");
        if value.is_empty() {
            raw.truncate(raw.trim_end_matches(is_blank).len());
        }
        *line = Line::from_valid(raw, line.eol);
    }

    fn insert_new(&mut self, key: &str, value: &str) -> Result<(), ValueError> {
        let at = self.insertion_index(key, value);
        let (indent, gap) = self.style_near(at, key);
        let raw = entry_text(key, value, &indent, &gap)?;
        self.doc.insert_line(at, raw);
        self.end += 1;
        Ok(())
    }

    /// Where a new `key value` line goes. It never lands between a comment and the line
    /// the comment is attached to (see [`ScopeMut`]).
    fn insertion_index(&self, key: &str, value: &str) -> usize {
        let same = self.indices_of(key);
        if let Some(&last) = same.last() {
            let values: Vec<&str> = same
                .iter()
                .filter_map(|&i| entry_at(self.doc, i))
                .map(|e| e.value())
                .collect();
            if values.windows(2).all(|w| w[0] <= w[1])
                && let Some(pos) = values.iter().position(|v| *v > value)
            {
                // Going in front of an entry means going in front of its comments too.
                return self.above_comments(same[pos]);
            }
            return last + 1;
        }
        let last_entry = (self.start..self.end)
            .rev()
            .find(|&i| entry_at(self.doc, i).is_some());
        match last_entry {
            Some(i) => i + 1,
            None => self.index_in_empty_scope(),
        }
    }

    /// Where the comments attached to the line at `index` begin: the run of comment lines
    /// directly above it, or `index` itself if there are none.
    fn above_comments(&self, index: usize) -> usize {
        (self.start..index)
            .rev()
            .take_while(|&i| matches!(self.doc.lines[i].kind, Kind::Comment))
            .last()
            .unwrap_or(index)
    }

    /// Where the first entry of a scope goes when the scope has no entries yet.
    fn index_in_empty_scope(&self) -> usize {
        if self.header.is_none() {
            // Top level. If a section follows, the entry ends the top level but stays clear of the
            // comments attached to that header. Without a section it goes at the end of the file.
            return if self.end < self.doc.lines.len() {
                self.above_comments(self.end)
            } else {
                self.end
            };
        }
        // A section holds only comments and blank lines here. Comments at its top describe the
        // section when a blank line follows them, so the entry goes below them. Comments that
        // run straight into the next header belong to that header, and the entry goes above
        // them, directly under this section's header.
        let comments = (self.start..self.end)
            .take_while(|&i| matches!(self.doc.lines[i].kind, Kind::Comment))
            .count();
        let block_end = self.start + comments;
        if comments > 0 && block_end < self.end {
            block_end
        } else {
            self.start
        }
    }

    /// The indentation and key/value gap that make a new line at `at` look like its neighbours.
    ///
    /// The indentation comes from the nearest entry. The value lines up with the others only
    /// when the scope already lines its values up.
    fn style_near(&self, at: usize, key: &str) -> (String, String) {
        let before = (self.start..at.min(self.end))
            .rev()
            .find_map(|i| entry_at(self.doc, i));
        let neighbour = before.or_else(|| (at..self.end).find_map(|i| entry_at(self.doc, i)));
        let indent = neighbour.map_or_else(String::new, |n| n.raw[..n.key.start].to_string());
        let gap = match self.aligned_column(at) {
            Some(column) => {
                let used = indent.chars().count() + key.chars().count();
                " ".repeat(column.saturating_sub(used).max(1))
            }
            None => " ".to_string(),
        };
        (indent, gap)
    }

    /// The 0-based column where values start, if the scope lines them up.
    ///
    /// That takes at least two entries whose keys end in different columns but whose values
    /// start in the same one. If the scope has several such columns, the one nearest to `at` wins.
    fn aligned_column(&self, at: usize) -> Option<usize> {
        // (line, column where the key ends, column where the value starts), counted in characters.
        // Entries without a value, or with a tab between key and value, tell nothing about alignment.
        let marks: Vec<(usize, usize, usize)> = (self.start..self.end)
            .filter_map(|i| {
                let entry = entry_at(self.doc, i)?;
                let gap = &entry.raw[entry.key.end..entry.value.start];
                if entry.value().is_empty() || gap.contains('\t') {
                    return None;
                }
                let key_end = entry.raw[..entry.key.end].chars().count();
                let value_start = entry.raw[..entry.value.start].chars().count();
                Some((i, key_end, value_start))
            })
            .collect();
        let mut first_key_end: HashMap<usize, usize> = HashMap::new();
        let mut aligned: HashSet<usize> = HashSet::new();
        for &(_, key_end, value_start) in &marks {
            if *first_key_end.entry(value_start).or_insert(key_end) != key_end {
                aligned.insert(value_start);
            }
        }
        marks
            .iter()
            .filter(|&&(_, _, value_start)| aligned.contains(&value_start))
            .min_by_key(|&&(line, _, _)| {
                if line < at {
                    (at - 1 - line, 0)
                } else {
                    (line - at, 1)
                }
            })
            .map(|&(_, _, value_start)| value_start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Document {
        Document::parse(text).unwrap()
    }

    #[test]
    fn reads_root_entries_and_sections() {
        let doc = parse("version 1\n\n[repo /a]\nprofile x\nprofile y\n\n[repo /b]\nsynced now\n");
        assert_eq!(doc.root().value("version"), Some("1"));
        let sections = doc.sections();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].kind(), Some("repo"));
        assert_eq!(sections[0].name(), Some("/a"));
        assert_eq!(
            sections[0].values("profile").collect::<Vec<_>>(),
            ["x", "y"]
        );
        assert_eq!(sections[1].value("synced"), Some("now"));
        assert_eq!(sections[1].value("profile"), None);
        assert_eq!(sections[0].line(), 3);
        assert!(doc.root().is_root());
        assert_eq!(doc.section("repo", "/b").map(|s| s.line()), Some(7));
        assert!(doc.section("repo", "/c").is_none());
    }

    #[test]
    fn round_trips_exactly() {
        let cases = [
            "",
            "\n",
            "a\n",
            "a",
            "a\n\n",
            "# c\n  skill\tgit   \n\n[x  y]  \n",
            "a 1\r\nb 2\r\n",
            "\u{feff}a 1\n",
            "   \n\t\n",
            "no final newline # c",
        ];
        for case in cases {
            assert_eq!(parse(case).to_string(), case, "case {case:?}");
        }
    }

    #[test]
    fn mixed_line_endings_are_preserved() {
        let cases = [
            "a 1\r\nb 2\n",
            "a 1\nb 2\r\n",
            "a\r\n\nb\r\n",
            "\r\n\n\r\n",
            "a 1\nb 2\r\nc 3",
            "a 1\r\nb 2\nc 3",
            "\u{feff}a 1\r\nb 2\nc 3",
            "# c\r\n[s n]\n  k v\r\n",
        ];
        for case in cases {
            assert_eq!(parse(case).to_string(), case, "case {case:?}");
        }
    }

    #[test]
    fn new_lines_end_like_the_first_terminated_line() {
        let mut doc = parse("a 1\r\nb 2\n");
        doc.root_mut().add("c", "3").unwrap();
        assert_eq!(doc.to_string(), "a 1\r\nb 2\nc 3\r\n");

        let mut doc = parse("a 1\nb 2\r\n");
        doc.root_mut().add("c", "3").unwrap();
        assert_eq!(doc.to_string(), "a 1\nb 2\r\nc 3\n");

        let mut doc = parse("skill a\r\nskill c\n");
        doc.root_mut().add("skill", "b").unwrap();
        assert_eq!(doc.to_string(), "skill a\r\nskill b\r\nskill c\n");

        let mut doc = Document::new();
        doc.root_mut().add("a", "1").unwrap();
        assert_eq!(doc.to_string(), "a 1\n");
    }

    #[test]
    fn a_line_added_after_an_unterminated_last_line_does_not_merge_with_it() {
        let mut doc = parse("a 1\r\nb 2");
        doc.root_mut().add("c", "3").unwrap();
        assert_eq!(doc.to_string(), "a 1\r\nb 2\r\nc 3\r\n");

        let mut doc = parse("a 1");
        doc.root_mut().add("c", "3").unwrap();
        assert_eq!(doc.to_string(), "a 1\nc 3\n");

        let mut doc = parse("# only a comment");
        doc.root_mut().add("k", "v").unwrap();
        assert_eq!(doc.to_string(), "# only a comment\nk v\n");
    }

    #[test]
    fn builders_use_the_first_line_ending_and_terminate_the_line_before() {
        let mut doc = parse("a 1\r\nb 2");
        doc.push_blank();
        doc.push_comment("one\ntwo");
        doc.push_entry("k", "v").unwrap();
        doc.push_section("s", "n").unwrap();
        doc.push_entry_aligned("z", "1", 4).unwrap();
        assert_eq!(
            doc.to_string(),
            "a 1\r\nb 2\r\n\r\n# one\r\n# two\r\nk v\r\n[s n]\r\nz  1\r\n"
        );
        assert_eq!(Document::parse(&doc.to_string()).unwrap(), doc);

        let mut doc = parse("a 1");
        doc.push_entry("b", "2").unwrap();
        assert_eq!(doc.to_string(), "a 1\nb 2\n");
    }

    #[test]
    fn pushing_nothing_leaves_an_unterminated_last_line_alone() {
        let mut doc = parse("a 1");
        doc.push_comment("");
        assert_eq!(doc.to_string(), "a 1");
    }

    #[test]
    fn removing_lines_keeps_every_other_line_ending() {
        let mut doc = parse("a 1\r\nb 2\nc 3\r\nd 4\n");
        doc.root_mut().remove("b");
        assert_eq!(doc.to_string(), "a 1\r\nc 3\r\nd 4\n");
        doc.root_mut().remove("a");
        assert_eq!(doc.to_string(), "c 3\r\nd 4\n");
        doc.root_mut().remove_value("d", "4");
        assert_eq!(doc.to_string(), "c 3\r\n");
    }

    #[test]
    fn removing_the_unterminated_last_line_still_reparses_to_the_same_document() {
        let mut doc = parse("a 1\r\nb 2");
        doc.root_mut().remove("b");
        assert_eq!(doc.to_string(), "a 1\r\n");
        assert_eq!(Document::parse(&doc.to_string()).unwrap(), doc);

        let mut doc = parse("a 1");
        doc.root_mut().remove("a");
        assert_eq!(doc.to_string(), "");
        assert_eq!(Document::parse(&doc.to_string()).unwrap(), doc);
        assert_eq!(doc, Document::new());
    }

    #[test]
    fn set_in_place_keeps_the_line_ending() {
        let mut doc = parse("a 1\r\nb 2\nc 3");
        doc.root_mut().set("a", "9").unwrap();
        doc.root_mut().set("b", "8").unwrap();
        doc.root_mut().set("c", "7").unwrap();
        assert_eq!(doc.to_string(), "a 9\r\nb 8\nc 7");
    }

    #[test]
    fn reports_all_syntax_errors_with_positions() {
        let error = Document::parse("ok 1\nskill: x\n- y\n[open\n").unwrap_err();
        let lines: Vec<usize> = error.iter().map(|d| d.line()).collect();
        assert_eq!(lines, [2, 3, 4]);
        assert_eq!(error.first().column(), 6);
    }

    #[test]
    fn parse_keeps_fifty_problems_and_says_when_there_were_more() {
        let bad = |count: usize| "- x\n".repeat(count);
        let error = Document::parse(&bad(3)).unwrap_err();
        assert_eq!((error.len(), error.truncated()), (3, false));

        // Exactly fifty problems are all shown, so nothing was left out.
        let error = Document::parse(&bad(50)).unwrap_err();
        assert_eq!((error.len(), error.truncated()), (50, false));
        assert!(!error.to_string().contains("note:"));

        let error = Document::parse(&bad(51)).unwrap_err();
        assert_eq!((error.len(), error.truncated()), (50, true));
        assert!(
            error
                .to_string()
                .ends_with("\nnote: only the first 50 problems are shown")
        );
        let text = error.render("f", &bad(51));
        assert!(text.ends_with("\n\nnote: only the first 50 problems are shown\n"));
        assert_eq!(error.first().line(), 1);

        // Good lines between the problems do not count, and a late problem still counts.
        let sparse = format!("{}ok 1\n{}", bad(50), "ok 2\n".repeat(100));
        assert!(!Document::parse(&sparse).unwrap_err().truncated());
        let sparse = format!("{}{}- late\n", bad(50), "ok 2\n".repeat(100));
        assert!(Document::parse(&sparse).unwrap_err().truncated());
    }

    #[test]
    fn stray_carriage_return_is_an_error() {
        assert!(Document::parse("a 1\rb 2\n").is_err());
        assert!(Document::parse("a 1\r").is_err());
    }

    #[test]
    fn set_replaces_in_place_and_keeps_layout() {
        let mut doc = parse("# top\n  key   old value  \nother 1\n");
        doc.root_mut().set("key", "new").unwrap();
        assert_eq!(doc.to_string(), "# top\n  key   new  \nother 1\n");
    }

    #[test]
    fn set_removes_duplicates_and_adds_when_missing() {
        let mut doc = parse("k 1\nx y\nk 2\n");
        doc.root_mut().set("k", "3").unwrap();
        assert_eq!(doc.to_string(), "k 3\nx y\n");
        doc.root_mut().set("fresh", "v").unwrap();
        assert_eq!(doc.to_string(), "k 3\nx y\nfresh v\n");
    }

    #[test]
    fn set_to_empty_and_from_empty() {
        let mut doc = parse("k value\nj\n");
        doc.root_mut().set("k", "").unwrap();
        doc.root_mut().set("j", "now set").unwrap();
        assert_eq!(doc.to_string(), "k\nj now set\n");
    }

    #[test]
    fn add_appends_after_the_last_matching_entry_when_unsorted() {
        let mut doc = parse("skill b\nskill a\ndescription x\n");
        assert!(doc.root_mut().add("skill", "c").unwrap());
        assert_eq!(
            doc.to_string(),
            "skill b\nskill a\nskill c\ndescription x\n"
        );
    }

    #[test]
    fn add_keeps_a_sorted_list_sorted() {
        let mut doc = parse("skill a\nskill c\nskill e\n");
        doc.root_mut().add("skill", "d").unwrap();
        doc.root_mut().add("skill", "0").unwrap();
        doc.root_mut().add("skill", "z").unwrap();
        assert_eq!(
            doc.to_string(),
            "skill 0\nskill a\nskill c\nskill d\nskill e\nskill z\n"
        );
    }

    #[test]
    fn add_is_idempotent() {
        let mut doc = parse("skill a\n");
        assert!(!doc.root_mut().add("skill", "a").unwrap());
        assert_eq!(doc.to_string(), "skill a\n");
    }

    #[test]
    fn add_to_a_file_with_only_comments_goes_below_them() {
        let mut doc = parse("# profile\n# more\n\n");
        doc.root_mut().add("skill", "git").unwrap();
        assert_eq!(doc.to_string(), "# profile\n# more\n\nskill git\n");
        let mut tight = parse("# profile\n");
        tight.root_mut().add("skill", "git").unwrap();
        assert_eq!(tight.to_string(), "# profile\nskill git\n");
    }

    #[test]
    fn add_to_an_empty_section_goes_right_under_its_header() {
        let mut doc = parse("[a x]\n\n[b y]\nk 1\n");
        doc.section_mut("a", "x").unwrap().add("k", "0").unwrap();
        assert_eq!(doc.to_string(), "[a x]\nk 0\n\n[b y]\nk 1\n");
    }

    #[test]
    fn add_to_an_empty_document() {
        let mut doc = Document::new();
        doc.root_mut().add("skill", "git").unwrap();
        assert_eq!(doc.to_string(), "skill git\n");
    }

    #[test]
    fn add_copies_indentation_and_alignment() {
        let mut doc = parse("[repo /a]\n  profile  x\n  synced   now\n");
        doc.section_mut("repo", "/a")
            .unwrap()
            .add("skill", "git")
            .unwrap();
        assert_eq!(
            doc.to_string(),
            "[repo /a]\n  profile  x\n  synced   now\n  skill    git\n"
        );
    }

    #[test]
    fn add_never_creates_a_negative_gap() {
        let mut doc = parse("a 1\n");
        doc.root_mut().add("longer-key", "2").unwrap();
        assert_eq!(doc.to_string(), "a 1\nlonger-key 2\n");
    }

    #[test]
    fn add_to_a_section_leaves_the_comment_of_the_next_header_alone() {
        let mut doc = parse("[repo /a]\n\n# for b\n[repo /b]\n");
        doc.section_mut("repo", "/a")
            .unwrap()
            .add("k", "1")
            .unwrap();
        assert_eq!(doc.to_string(), "[repo /a]\nk 1\n\n# for b\n[repo /b]\n");

        let mut doc = parse("[repo /a]\n\n# for b\n[repo /b]\n");
        doc.section_mut("repo", "/a")
            .unwrap()
            .set("k", "1")
            .unwrap();
        assert_eq!(doc.to_string(), "[repo /a]\nk 1\n\n# for b\n[repo /b]\n");
    }

    #[test]
    fn add_to_the_top_level_stays_clear_of_the_comment_on_the_first_header() {
        let mut doc = parse("# The API repo\n[repo /a]\n");
        doc.root_mut().add("version", "1").unwrap();
        assert_eq!(doc.to_string(), "version 1\n# The API repo\n[repo /a]\n");

        let mut doc = parse("# file\n\n# one\n# two\n[repo /a]\n");
        doc.root_mut().add("version", "1").unwrap();
        assert_eq!(
            doc.to_string(),
            "# file\n\nversion 1\n# one\n# two\n[repo /a]\n"
        );

        let mut doc = parse("\n[repo /a]\n");
        doc.root_mut().add("version", "1").unwrap();
        assert_eq!(doc.to_string(), "\nversion 1\n[repo /a]\n");
    }

    #[test]
    fn add_to_the_top_level_after_the_last_entry_is_unchanged() {
        let mut doc = parse("version 1\n# The API repo\n[repo /a]\n");
        doc.root_mut().add("name", "x").unwrap();
        assert_eq!(
            doc.to_string(),
            "version 1\nname x\n# The API repo\n[repo /a]\n"
        );
    }

    #[test]
    fn a_sorted_insert_goes_above_the_comments_of_the_entry_it_precedes() {
        let mut doc = parse("skill a\n# good\nskill c\n");
        doc.root_mut().add("skill", "b").unwrap();
        assert_eq!(doc.to_string(), "skill a\nskill b\n# good\nskill c\n");

        let mut doc = parse("# one\n# two\nskill c\n");
        doc.root_mut().add("skill", "a").unwrap();
        assert_eq!(doc.to_string(), "skill a\n# one\n# two\nskill c\n");

        let mut doc = parse("[s x]\n# one\nskill c\n");
        doc.section_mut("s", "x")
            .unwrap()
            .add("skill", "a")
            .unwrap();
        assert_eq!(doc.to_string(), "[s x]\nskill a\n# one\nskill c\n");
    }

    #[test]
    fn a_comment_after_a_blank_line_is_not_attached_to_the_entry_below_the_blank() {
        let mut doc = parse("skill a\n# loose\n\nskill c\n");
        doc.root_mut().add("skill", "b").unwrap();
        assert_eq!(doc.to_string(), "skill a\n# loose\n\nskill b\nskill c\n");
    }

    #[test]
    fn add_after_the_last_matching_entry_goes_before_the_comment_that_follows_it() {
        let mut doc = parse("skill b\nskill a\n# about description\ndescription x\n");
        doc.root_mut().add("skill", "c").unwrap();
        assert_eq!(
            doc.to_string(),
            "skill b\nskill a\nskill c\n# about description\ndescription x\n"
        );
    }

    #[test]
    fn add_to_an_empty_section_goes_below_a_comment_that_describes_it() {
        let mut doc = parse("[a x]\n# about a\n\n[b y]\n");
        doc.section_mut("a", "x").unwrap().add("k", "1").unwrap();
        assert_eq!(doc.to_string(), "[a x]\n# about a\nk 1\n\n[b y]\n");
    }

    #[test]
    fn add_to_an_empty_section_goes_above_comments_that_run_into_the_next_header() {
        let mut doc = parse("[a x]\n# about b\n# more about b\n[b y]\n");
        doc.section_mut("a", "x").unwrap().add("k", "1").unwrap();
        assert_eq!(
            doc.to_string(),
            "[a x]\nk 1\n# about b\n# more about b\n[b y]\n"
        );
    }

    #[test]
    fn the_last_section_follows_the_same_comment_rules_as_the_others() {
        // No blank line after the comments, and no header after them either: not a description.
        let mut doc = parse("[a x]\n# note\n");
        doc.section_mut("a", "x").unwrap().add("k", "1").unwrap();
        assert_eq!(doc.to_string(), "[a x]\nk 1\n# note\n");

        let mut doc = parse("[a x]\n# note\n\n");
        doc.section_mut("a", "x").unwrap().add("k", "1").unwrap();
        assert_eq!(doc.to_string(), "[a x]\n# note\nk 1\n\n");

        let mut doc = parse("[a x]\n\n# later\n");
        doc.section_mut("a", "x").unwrap().add("k", "1").unwrap();
        assert_eq!(doc.to_string(), "[a x]\nk 1\n\n# later\n");
    }

    #[test]
    fn set_and_remove_never_delete_comments() {
        let mut doc = parse("# about a\nskill a\n# about b\nskill b\nskill c\n");
        doc.root_mut().set("skill", "x").unwrap();
        assert_eq!(doc.to_string(), "# about a\nskill x\n# about b\n");
        assert_eq!(doc.root_mut().remove("skill"), 1);
        assert_eq!(doc.to_string(), "# about a\n# about b\n");
        let mut doc = parse("# about a\nskill a\n# about b\nskill b\n");
        assert_eq!(doc.root_mut().remove_value("skill", "b"), 1);
        assert_eq!(doc.to_string(), "# about a\nskill a\n# about b\n");
    }

    #[test]
    fn add_does_not_align_a_lone_entry() {
        let mut doc = parse("description Foo\n");
        doc.root_mut().add("skill", "git").unwrap();
        assert_eq!(doc.to_string(), "description Foo\nskill git\n");
    }

    #[test]
    fn add_copies_indentation_without_alignment() {
        let mut doc = parse("[repo /a]\n\tprofile x\n");
        doc.section_mut("repo", "/a")
            .unwrap()
            .add("skill", "git")
            .unwrap();
        assert_eq!(doc.to_string(), "[repo /a]\n\tprofile x\n\tskill git\n");
    }

    #[test]
    fn add_needs_keys_of_different_lengths_to_see_alignment() {
        let mut doc = parse("aaaa   1\nbbbb   2\n");
        doc.root_mut().add("cc", "3").unwrap();
        assert_eq!(doc.to_string(), "aaaa   1\nbbbb   2\ncc 3\n");

        let mut doc = parse("ab 1\nabcd 2\n");
        doc.root_mut().add("x", "3").unwrap();
        assert_eq!(doc.to_string(), "ab 1\nabcd 2\nx 3\n");

        let mut doc = parse("a\nabc 1\nabcde 2\n");
        doc.root_mut().add("x", "3").unwrap();
        assert_eq!(doc.to_string(), "a\nabc 1\nabcde 2\nx 3\n");
    }

    #[test]
    fn add_lines_its_value_up_with_a_column_the_file_uses() {
        let mut doc = parse("  profile  x\n  synced   now\n");
        doc.root_mut().add("skill", "git").unwrap();
        assert_eq!(
            doc.to_string(),
            "  profile  x\n  synced   now\n  skill    git\n"
        );

        // A key that already reaches the column gets one space.
        let mut doc = parse("  profile  x\n  synced   now\n");
        doc.root_mut().add("abcdefgh", "1").unwrap();
        doc.root_mut().add("a-very-long-key", "2").unwrap();
        assert_eq!(
            doc.to_string(),
            "  profile  x\n  synced   now\n  abcdefgh 1\n  a-very-long-key 2\n"
        );
    }

    #[test]
    fn add_follows_the_alignment_nearest_to_the_new_line() {
        let mut doc = parse("  profile  x\n  synced   now\n\n  a       1\n  bb      2\n");
        doc.root_mut().add("key", "v").unwrap();
        assert_eq!(
            doc.to_string(),
            "  profile  x\n  synced   now\n\n  a       1\n  bb      2\n  key     v\n"
        );
    }

    #[test]
    fn tabs_between_key_and_value_are_not_alignment() {
        let mut doc = parse("a\t\t1\nab\t1\n");
        doc.root_mut().add("x", "v").unwrap();
        assert_eq!(doc.to_string(), "a\t\t1\nab\t1\nx v\n");
    }

    #[test]
    fn section_edits_do_not_leak_into_other_scopes() {
        let mut doc = parse("v 1\n[a x]\nk 1\n[b y]\nk 2\n");
        doc.section_mut("a", "x").unwrap().add("k", "9").unwrap();
        doc.section_mut("b", "y").unwrap().remove("k");
        doc.root_mut().set("v", "2").unwrap();
        assert_eq!(doc.to_string(), "v 2\n[a x]\nk 1\nk 9\n[b y]\n");
    }

    #[test]
    fn remove_and_remove_value() {
        let mut doc = parse("# keep me\nskill a\nskill b\nskill a\nother 1\n");
        assert_eq!(doc.root_mut().remove_value("skill", "a"), 2);
        assert_eq!(doc.to_string(), "# keep me\nskill b\nother 1\n");
        assert_eq!(doc.root_mut().remove("skill"), 1);
        assert_eq!(doc.root_mut().remove("skill"), 0);
        assert_eq!(doc.to_string(), "# keep me\nother 1\n");
    }

    #[test]
    fn edits_reject_values_that_cannot_round_trip() {
        let mut doc = Document::new();
        assert_eq!(doc.root_mut().set("k", "a\nb"), Err(ValueError::LineBreak));
        assert_eq!(
            doc.root_mut().add("k", " lead"),
            Err(ValueError::EdgeWhitespace)
        );
        assert!(doc.root_mut().set("bad key", "v").is_err());
        assert_eq!(doc.to_string(), "");
    }

    #[test]
    fn builders_write_canonical_text() {
        let mut doc = Document::new();
        doc.push_comment("first line\n\nthird line");
        doc.push_blank();
        doc.push_entry("version", "1").unwrap();
        doc.push_section("repo", "/home/me/my project").unwrap();
        doc.push_entry_aligned("profile", "coding", 10).unwrap();
        doc.push_entry_aligned("synced", "2026-01-01T00:00:00Z", 10)
            .unwrap();
        doc.push_section("empty", "").unwrap();
        assert_eq!(
            doc.to_string(),
            "# first line\n#\n# third line\n\nversion 1\n[repo /home/me/my project]\nprofile  coding\nsynced   2026-01-01T00:00:00Z\n[empty]\n"
        );
        let again = Document::parse(&doc.to_string()).unwrap();
        assert_eq!(again, doc);
    }

    #[test]
    fn builders_reject_bad_input() {
        let mut doc = Document::new();
        assert!(doc.push_entry("k", "line\nbreak").is_err());
        assert!(doc.push_entry("1k", "v").is_err());
        assert!(doc.push_section("repo", "trailing ").is_err());
        assert!(doc.push_section("re po", "x").is_err());
    }

    #[test]
    fn comments_are_sanitised() {
        let mut doc = Document::new();
        doc.push_comment("bell\u{7} here");
        assert!(Document::parse(&doc.to_string()).is_ok());
    }

    #[test]
    fn comments_lose_c1_and_other_control_characters() {
        let mut doc = Document::new();
        doc.push_comment("next\u{85}line and csi\u{9b}x and del\u{7f}y and nul\u{0}z\rw");
        assert_eq!(
            doc.to_string(),
            "# next line and csi x and del y and nul z w\n"
        );
        assert_eq!(Document::parse(&doc.to_string()).unwrap(), doc);

        // U+00A0 is not a control character, so it stays.
        let mut doc = Document::new();
        doc.push_comment("a\u{a0}b");
        assert_eq!(doc.to_string(), "# a\u{a0}b\n");
    }

    #[test]
    fn edits_reject_c1_control_characters_in_values() {
        let mut doc = Document::new();
        assert_eq!(
            doc.root_mut().set("k", "a\u{85}b"),
            Err(ValueError::Control('\u{85}'))
        );
        assert_eq!(
            doc.push_entry("k", "\u{9b}"),
            Err(ValueError::Control('\u{9b}'))
        );
        assert_eq!(
            doc.push_section("repo", "/a\u{90}"),
            Err(ValueError::Control('\u{90}'))
        );
        assert_eq!(doc.to_string(), "");
    }

    #[test]
    fn entry_positions_and_diagnostics() {
        let doc = parse("  skill   = git\n");
        let entry = doc.root().get("skill").unwrap();
        assert_eq!(entry.line(), 1);
        assert_eq!(entry.key_column(), 3);
        assert_eq!(entry.value_column(), 11);
        let diagnostic = entry.diagnostic("invalid skill id");
        assert_eq!((diagnostic.line(), diagnostic.column()), (1, 11));
        assert!(diagnostic.hint().unwrap().contains("no '=' or ':'"));
    }

    #[test]
    fn diagnostic_for_an_empty_value_points_at_the_key() {
        let doc = parse("skill\n");
        let diagnostic = doc.root().get("skill").unwrap().diagnostic("needs a value");
        assert_eq!((diagnostic.column(), diagnostic.hint()), (1, None));
    }

    #[test]
    fn section_header_diagnostic_underlines_the_header() {
        let doc = parse("v 1\n  [repo /x]  \n");
        let diagnostic = doc.sections()[0].diagnostic("bad");
        assert_eq!((diagnostic.line(), diagnostic.column()), (2, 3));
        let rendered = diagnostic.render("f", "v 1\n  [repo /x]  \n");
        assert!(rendered.contains("  |   ^^^^^^^^^\n"), "{rendered}");
    }

    #[test]
    fn crlf_documents_stay_crlf_after_edits() {
        let mut doc = parse("a 1\r\nb 2\r\n");
        doc.root_mut().add("c", "3").unwrap();
        assert_eq!(doc.to_string(), "a 1\r\nb 2\r\nc 3\r\n");
    }

    #[test]
    fn missing_final_newline_survives_an_edit_that_keeps_the_last_line() {
        let mut doc = parse("a 1\nb 2");
        doc.root_mut().set("a", "9").unwrap();
        assert_eq!(doc.to_string(), "a 9\nb 2");
    }
}

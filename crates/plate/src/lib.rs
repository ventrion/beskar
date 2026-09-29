//! Plate — a line-oriented, typeless configuration format.
//!
//! Every line of a Plate document is exactly one of:
//!
//! ```text
//! # a comment                  (only whole-line comments exist)
//! [kind optional label]        (a section header)
//! key = value                  (a single value: the rest of the line)
//! key:                         (a list; items follow on their own lines)
//!   - item                     (a list item: the rest of the line)
//! ```
//!
//! Values are always strings. There is no quoting, no escaping, no type
//! inference and no significant indentation. What you see after `=` or `- `
//! is the value, byte for byte (minus surrounding whitespace). The consumer
//! decides what a value means and reports errors with line numbers.
//!
//! The full specification lives in `SPEC.md` next to this crate.

use std::fmt;

mod edit;
mod write;

pub use edit::Target;
pub use write::Writer;

/// A parse or schema error, always tied to a line (1-based; 0 = whole document).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub message: String,
    pub hint: Option<String>,
}

impl Error {
    pub fn new(line: usize, message: impl Into<String>) -> Self {
        Error { line, message: message.into(), hint: None }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "line {}: {}", self.line, self.message)?;
        } else {
            f.write_str(&self.message)?;
        }
        if let Some(hint) = &self.hint {
            write!(f, "\n  hint: {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

/// Something legal but probably not what the author meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// A value: either a single string or a list of strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Scalar(String),
    List(Vec<Item>),
}

/// One `- item` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub value: String,
    pub line: usize,
}

/// A `key = value` or `key:` entry together with its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    key: String,
    line: usize,
    last_line: usize,
    value: Value,
}

impl Entry {
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Line of the `key = value` / `key:` line.
    pub fn line(&self) -> usize {
        self.line
    }

    pub fn value(&self) -> &Value {
        &self.value
    }
}

/// The top level of a document, or one `[kind label]` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    kind: String,
    label: String,
    line: usize,
    entries: Vec<Entry>,
}

impl Section {
    /// Section kind (the first word of the header); empty for the top level.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Everything after the kind in the header; may be empty.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Header line; 0 for the top level.
    pub fn line(&self) -> usize {
        self.line
    }

    pub fn is_root(&self) -> bool {
        self.line == 0
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.key == key)
    }

    /// Human description used in error messages.
    pub fn describe(&self) -> String {
        if self.is_root() {
            "the top level".to_string()
        } else if self.label.is_empty() {
            format!("section [{}]", self.kind)
        } else {
            format!("section [{} {}]", self.kind, self.label)
        }
    }

    /// The value of `key` if it is present, as a single value.
    pub fn scalar(&self, key: &str) -> Result<Option<&str>, Error> {
        match self.get(key) {
            None => Ok(None),
            Some(Entry { value: Value::Scalar(s), .. }) => Ok(Some(s)),
            Some(entry) => Err(Error::new(entry.line, format!("`{key}` must be a single value, not a list"))
                .hint(format!("write it on one line: {key} = <value>"))),
        }
    }

    /// Like [`Section::scalar`], but the key must be present.
    pub fn require_scalar(&self, key: &str) -> Result<&str, Error> {
        self.scalar(key)?.ok_or_else(|| {
            Error::new(self.line, format!("{} is missing `{key}`", self.describe()))
                .hint(format!("add a line: {key} = <value>"))
        })
    }

    /// The items of `key` if it is present, as a list.
    pub fn list(&self, key: &str) -> Result<Option<&[Item]>, Error> {
        match self.get(key) {
            None => Ok(None),
            Some(Entry { value: Value::List(items), .. }) => Ok(Some(items)),
            Some(entry) => Err(Error::new(entry.line, format!("`{key}` must be a list, not a single value"))
                .hint(format!("write it as a list:\n        {key}:\n          - <item>"))),
        }
    }

    /// Reject keys outside `allowed`, suggesting the closest allowed key.
    pub fn check_keys(&self, allowed: &[&str]) -> Result<(), Error> {
        for entry in &self.entries {
            if !allowed.contains(&entry.key.as_str()) {
                let mut err = Error::new(entry.line, format!("unknown key `{}` in {}", entry.key, self.describe()));
                err = match closest(&entry.key, allowed) {
                    Some(best) => err.hint(format!("did you mean `{best}`?")),
                    None => err.hint(format!("allowed keys: {}", allowed.join(", "))),
                };
                return Err(err);
            }
        }
        Ok(())
    }
}

/// A parsed Plate document. Keeps the original text so edits preserve
/// comments, blank lines, ordering and alignment.
#[derive(Debug, Clone)]
pub struct Document {
    /// Raw lines, each keeping its own `\r` if it had a CRLF ending, so
    /// untouched lines are written back byte-identical even in files with
    /// mixed line endings.
    lines: Vec<String>,
    root: Section,
    sections: Vec<Section>,
    warnings: Vec<Warning>,
    bom: bool,
    /// Whether lines added by edits end in CRLF (most existing lines do).
    crlf: bool,
}

impl Document {
    pub fn parse(text: &str) -> Result<Document, Error> {
        let bom = text.starts_with('\u{feff}');
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        // A trailing newline produces one empty final element; drop it so
        // rendering round-trips.
        if lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        let crlf_lines = lines.iter().filter(|l| l.ends_with('\r')).count();
        let crlf = crlf_lines * 2 > lines.len();
        let mut doc = Document::from_lines(lines)?;
        doc.bom = bom;
        doc.crlf = crlf;
        Ok(doc)
    }

    pub(crate) fn from_lines(lines: Vec<String>) -> Result<Document, Error> {
        let mut root = Section { kind: String::new(), label: String::new(), line: 0, entries: Vec::new() };
        let mut sections: Vec<Section> = Vec::new();
        let mut warnings = Vec::new();
        // Whether the most recent entry is a list that still accepts items.
        let mut list_open = false;

        for (idx, raw) in lines.iter().enumerate() {
            let n = idx + 1;
            let t = trim(content(raw));
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let current = sections.last_mut().unwrap_or(&mut root);

            if t.starts_with('[') {
                list_open = false;
                let (kind, label) = parse_header(t, n)?;
                if sections.iter().any(|s| s.kind == kind && s.label == label) {
                    let what = if label.is_empty() { kind.clone() } else { format!("{kind} {label}") };
                    return Err(Error::new(n, format!("duplicate section [{what}]"))
                        .hint("each section may appear only once; merge the two"));
                }
                sections.push(Section { kind, label, line: n, entries: Vec::new() });
                continue;
            }

            if let Some(rest) = t.strip_prefix('-') {
                if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
                    return Err(Error::new(n, "a list item needs a space after the dash")
                        .hint(format!("write: - {}", trim(rest))));
                }
                let value = trim(rest);
                if value.is_empty() {
                    return Err(Error::new(n, "empty list item").hint("remove the line, or put a value after `- `"));
                }
                check_value(value, n)?;
                lint_value(value, n, &mut warnings);
                let entry = match current.entries.last_mut() {
                    Some(entry) if list_open => entry,
                    _ => {
                        return Err(Error::new(n, "list item outside of a list")
                            .hint("start the list with a `key:` line above the items"));
                    }
                };
                let Value::List(items) = &mut entry.value else { unreachable!("open list is always a list") };
                items.push(Item { value: value.to_string(), line: n });
                entry.last_line = n;
                continue;
            }

            list_open = false;
            let (key, rest) = split_key(t, n)?;
            if current.entries.iter().any(|e| e.key == key) {
                return Err(Error::new(n, format!("duplicate key `{key}` in {}", current.describe()))
                    .hint("each key may appear only once per section; lists hold several values"));
            }
            let value = if let Some(after) = rest.strip_prefix('=') {
                let value = trim(after);
                check_value(value, n)?;
                lint_value(value, n, &mut warnings);
                Value::Scalar(value.to_string())
            } else if let Some(after) = rest.strip_prefix(':') {
                let after = trim(after);
                if !after.is_empty() {
                    return Err(Error::new(n, format!("unexpected text after `{key}:`")).hint(format!(
                        "for a single value write `{key} = {after}`; for a list put `- {after}` on the next line"
                    )));
                }
                list_open = true;
                Value::List(Vec::new())
            } else {
                return Err(Error::new(n, format!("expected `=` or `:` after `{key}`"))
                    .hint(format!("write `{key} = <value>` or start a list with `{key}:`")));
            };
            current.entries.push(Entry { key: key.to_string(), line: n, last_line: n, value });
        }

        Ok(Document { lines, root, sections, warnings, bom: false, crlf: false })
    }

    /// Top-level entries (before the first section header).
    pub fn root(&self) -> &Section {
        &self.root
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub fn section(&self, kind: &str, label: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.kind == kind && s.label == label)
    }

    pub fn sections_of<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Section> + 'a {
        self.sections.iter().filter(move |s| s.kind == kind)
    }

    /// Reject section kinds outside `allowed`.
    pub fn check_section_kinds(&self, allowed: &[&str]) -> Result<(), Error> {
        for s in &self.sections {
            if !allowed.contains(&s.kind.as_str()) {
                let err = Error::new(s.line, format!("unknown section kind `{}`", s.kind));
                return Err(match closest(&s.kind, allowed) {
                    Some(best) => err.hint(format!("did you mean [{best} ...]?")),
                    None if allowed.is_empty() => err.hint("this file does not use sections"),
                    None => err.hint(format!("allowed sections: {}", allowed.join(", "))),
                });
            }
        }
        Ok(())
    }

    /// Lines that parse fine but look like a mistake (e.g. a `#` that the
    /// author probably meant as an inline comment).
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    pub(crate) fn lines(&self) -> &[String] {
        &self.lines
    }
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bom {
            f.write_str("\u{feff}")?;
        }
        for line in &self.lines {
            f.write_str(line)?;
            f.write_str("\n")?;
        }
        Ok(())
    }
}

/// A raw line without its CR, if it had a CRLF ending.
pub(crate) fn content(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// Trim the whitespace Plate cares about: spaces and tabs.
fn trim(s: &str) -> &str {
    s.trim_matches([' ', '\t'])
}

pub(crate) fn is_key(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('a'..='z')) && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-' | '_'))
}

fn split_key(t: &str, n: usize) -> Result<(&str, &str), Error> {
    let end = t.find(|c: char| !matches!(c, 'a'..='z' | '0'..='9' | '-' | '_')).unwrap_or(t.len());
    let (key, rest) = t.split_at(end);
    if key.is_empty() || !is_key(key) {
        let word_end = t.find([' ', '\t', '=', ':']).unwrap_or(t.len());
        let word = &t[..word_end];
        let mut err = Error::new(n, format!("cannot read `{t}`"));
        err = if word.chars().any(|c| c.is_ascii_uppercase()) && is_key(&word.to_ascii_lowercase()) {
            err.hint(format!("keys are lowercase: `{}`", word.to_ascii_lowercase()))
        } else if t.starts_with('"') || t.starts_with('\'') {
            err.hint("keys are never quoted")
        } else {
            err.hint("every line is `key = value`, `key:`, `- item`, `[section]`, `# comment` or blank; keys use a-z, 0-9, `-` and `_`")
        };
        return Err(err);
    }
    let rest = trim(rest);
    if !(rest.starts_with('=') || rest.starts_with(':'))
        && rest.chars().next().is_some_and(|c| c.is_ascii_uppercase() || c == '.')
    {
        return Err(Error::new(n, format!("cannot read `{t}`")).hint("keys use only a-z, 0-9, `-` and `_`"));
    }
    Ok((key, rest))
}

fn parse_header(t: &str, n: usize) -> Result<(String, String), Error> {
    let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return Err(Error::new(n, "a section header must end with `]`").hint("write: [kind label]"));
    };
    if inner.starts_with('[') {
        return Err(Error::new(n, "nested brackets are not a thing in Plate").hint("write a single pair: [kind label]"));
    }
    let inner = trim(inner);
    let (kind, label) = match inner.find([' ', '\t']) {
        Some(i) => (&inner[..i], trim(&inner[i..])),
        None => (inner, ""),
    };
    if !is_key(kind) {
        return Err(Error::new(n, format!("invalid section kind `{kind}`"))
            .hint("the first word of a header is its kind and uses a-z, 0-9, `-` and `_`"));
    }
    Ok((kind.to_string(), label.to_string()))
}

/// Errors for values that Plate refuses because they almost always mean the
/// author expected quoting to work.
fn check_value(value: &str, n: usize) -> Result<(), Error> {
    if is_quoted(value) {
        let inner = &value[1..value.len() - 1];
        return Err(Error::new(n, "values are never quoted in Plate")
            .hint(format!("drop the quotes: everything after `=` or `- ` is the value, e.g. {inner}")));
    }
    Ok(())
}

/// Wholly wrapped in one pair of quotes: `"x"`, but not `"a" or "b"`.
pub(crate) fn is_quoted(value: &str) -> bool {
    ['"', '\''].iter().any(|&q| {
        value.len() >= 2 && value.starts_with(q) && value.ends_with(q) && !value[1..value.len() - 1].contains(q)
    })
}

fn lint_value(value: &str, n: usize, warnings: &mut Vec<Warning>) {
    if value.contains(" #") || value.contains("\t#") {
        warnings.push(Warning {
            line: n,
            message: "`#` inside a value is kept as text; comments must be on their own line".to_string(),
        });
    }
}

/// The allowed word closest to `word`, if it is close enough to be a typo.
fn closest<'a>(word: &str, allowed: &[&'a str]) -> Option<&'a str> {
    allowed
        .iter()
        .map(|a| (levenshtein(word, a), *a))
        .filter(|(d, a)| *d <= 2.max(a.len() / 3))
        .min_by_key(|(d, _)| *d)
        .map(|(_, a)| a)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Document {
        Document::parse(s).unwrap_or_else(|e| panic!("{e}"))
    }

    fn err(s: &str) -> Error {
        Document::parse(s).expect_err("expected an error")
    }

    #[test]
    fn scalars_lists_and_sections() {
        let doc = parse(
            "# comment\n\
             description = Everyday coding: git, review  \n\
             skills:\n\
             \x20 - code-review\n\
             \n\
             \x20 # a comment inside the list\n\
             \x20 - git\n\
             [repo /home/me/my project]\n\
             profiles:\n\
             synced = 2026-09-29T10:00:00Z\n",
        );
        let root = doc.root();
        assert_eq!(root.scalar("description").unwrap(), Some("Everyday coding: git, review"));
        let items: Vec<_> = root.list("skills").unwrap().unwrap().iter().map(|i| i.value.as_str()).collect();
        assert_eq!(items, ["code-review", "git"]);
        let repo = doc.section("repo", "/home/me/my project").unwrap();
        assert_eq!(repo.list("profiles").unwrap().unwrap().len(), 0);
        assert_eq!(repo.scalar("synced").unwrap(), Some("2026-09-29T10:00:00Z"));
        assert_eq!(repo.line(), 8);
    }

    #[test]
    fn values_are_verbatim() {
        let doc = parse("url = https://x.org/a?b=c#frag\nexpr = a = b\nempty =\nnum = 007\nno = no\n");
        let r = doc.root();
        assert_eq!(r.scalar("url").unwrap(), Some("https://x.org/a?b=c#frag"));
        assert_eq!(r.scalar("expr").unwrap(), Some("a = b"));
        assert_eq!(r.scalar("empty").unwrap(), Some(""));
        assert_eq!(r.scalar("num").unwrap(), Some("007"));
        assert_eq!(r.scalar("no").unwrap(), Some("no"));
    }

    #[test]
    fn crlf_bom_and_indentation_are_tolerated() {
        let doc = parse("\u{feff}  a = 1\r\n\tb:\r\n- x\r\n");
        assert_eq!(doc.root().scalar("a").unwrap(), Some("1"));
        assert_eq!(doc.root().list("b").unwrap().unwrap()[0].value, "x");
    }

    #[test]
    fn header_label_may_contain_brackets_and_spaces() {
        let doc = parse("[repo /tmp/a [b]]\n[settings]\n");
        assert!(doc.section("repo", "/tmp/a [b]").is_some());
        assert!(doc.section("settings", "").is_some());
    }

    #[test]
    fn helpful_errors() {
        assert!(err("key: value\n").hint.unwrap().contains("key = value"));
        assert!(err("Library = x\n").hint.unwrap().contains("lowercase"));
        assert!(err("a = \"quoted\"\n").message.contains("never quoted"));
        assert!(err("- orphan\n").message.contains("outside of a list"));
        assert!(err("a = 1\na = 2\n").message.contains("duplicate key"));
        assert!(err("[x]\n[x]\n").message.contains("duplicate section"));
        assert!(err("list:\n-nospace\n").message.contains("space after the dash"));
        assert!(err("list:\n  -\n").message.contains("empty list item"));
        assert!(err("[x\n").message.contains("must end with"));
        assert!(err("just words\n").message.contains("expected `=` or `:`"));
        assert_eq!(err("a = 1\n\nb c\n").line, 3);
    }

    #[test]
    fn a_list_ends_at_the_next_key() {
        let e = err("a:\n- 1\nb = 2\n- 3\n");
        assert_eq!(e.line, 4);
    }

    #[test]
    fn schema_helpers() {
        let doc = parse("skils:\n- a\nname = x\n");
        let e = doc.root().check_keys(&["skills", "description"]).unwrap_err();
        assert_eq!(e.hint.as_deref(), Some("did you mean `skills`?"));
        assert!(doc.root().list("name").is_err());
        assert!(doc.root().scalar("skils").is_err());
        assert!(doc.root().require_scalar("missing").is_err());
    }

    #[test]
    fn inline_hash_is_a_warning() {
        let doc = parse("dir = .agents/skills  # default\nlang = C#\n");
        assert_eq!(doc.warnings().len(), 1);
        assert_eq!(doc.warnings()[0].line, 1);
    }

    #[test]
    fn quotes_inside_values_are_fine() {
        let doc = parse("a = \"x\" or \"y\"\nb = it's\n");
        assert_eq!(doc.root().scalar("a").unwrap(), Some("\"x\" or \"y\""));
    }

    #[test]
    fn crlf_and_bom_survive_edits() {
        let mut doc = parse("\u{feff}# c\r\nskills:\r\n  - a\r\n");
        doc.push(Target::Root, "skills", "b").unwrap();
        assert_eq!(doc.to_string(), "\u{feff}# c\r\nskills:\r\n  - a\r\n  - b\r\n");
    }

    #[test]
    fn mixed_line_endings_survive_edits() {
        let src = "# lf\nskills:\r\n  - a\n  - b\r\nname = x\n";
        let mut doc = parse(src);
        assert_eq!(doc.to_string(), src);
        doc.set(Target::Root, "name", "y").unwrap();
        doc.push(Target::Root, "skills", "c").unwrap();
        assert_eq!(doc.to_string(), "# lf\nskills:\r\n  - a\n  - b\r\n  - c\nname = y\n");
    }

    #[test]
    fn rendering_round_trips() {
        let src = "# c\n\n  a   =  1\nlist:\n  - x\n\n[s  label ]\nk = v\n";
        assert_eq!(parse(src).to_string(), src);
    }
}

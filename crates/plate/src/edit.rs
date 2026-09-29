//! In-place edits that keep every untouched line byte-for-byte intact, so
//! comments, blank lines, ordering and alignment written by a human survive
//! a program changing one value.

use crate::{Document, Error, Section, Value, is_key, is_quoted, trim};

/// Which section an edit applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target<'a> {
    /// Entries before the first section header.
    Root,
    /// The section `[kind label]` (use `""` for a header without a label).
    Section(&'a str, &'a str),
}

impl Document {
    fn target(&self, target: Target<'_>) -> Result<&Section, Error> {
        match target {
            Target::Root => Ok(self.root()),
            Target::Section(kind, label) => self
                .section(kind, label)
                .ok_or_else(|| Error::new(0, format!("no section [{} {}]", kind, label).replace(" ]", "]"))),
        }
    }

    /// Set `key = value`, replacing the value in place if the key exists.
    pub fn set(&mut self, target: Target<'_>, key: &str, value: &str) -> Result<(), Error> {
        validate_key(key)?;
        validate_value(value, true)?;
        let section = self.target(target)?;
        let mut lines = self.lines().to_vec();
        match section.get(key) {
            Some(entry) => {
                let Value::Scalar(_) = entry.value() else {
                    return Err(Error::new(entry.line(), format!("`{key}` is a list, not a single value")));
                };
                let raw = &lines[entry.line() - 1];
                let eq = raw.find('=').expect("scalar line has `=`");
                let after = &raw[eq + 1..];
                let gap = &after[..after.len() - after.trim_start_matches([' ', '\t']).len()];
                let gap = if gap.is_empty() { " " } else { gap };
                let new =
                    if value.is_empty() { raw[..=eq].to_string() } else { format!("{}{gap}{value}", &raw[..=eq]) };
                lines[entry.line() - 1] = new;
            }
            None => {
                let (at, indent) = self.insertion_point(section);
                let line =
                    if value.is_empty() { format!("{indent}{key} =") } else { format!("{indent}{key} = {value}") };
                insert(&mut lines, at, section.is_root(), vec![line]);
            }
        }
        self.reload(lines)
    }

    /// Append `value` to the list `key`, creating the list if needed.
    pub fn push(&mut self, target: Target<'_>, key: &str, value: &str) -> Result<(), Error> {
        validate_key(key)?;
        validate_value(value, false)?;
        let section = self.target(target)?;
        let mut lines = self.lines().to_vec();
        match section.get(key) {
            Some(entry) => {
                let Value::List(items) = entry.value() else {
                    return Err(Error::new(entry.line(), format!("`{key}` is a single value, not a list")));
                };
                let indent = match items.last() {
                    Some(item) => indent_of(&lines[item.line - 1]).to_string(),
                    None => format!("{}  ", indent_of(&lines[entry.line() - 1])),
                };
                let at = items.last().map_or(entry.line(), |i| i.line);
                lines.insert(at, format!("{indent}- {value}"));
            }
            None => {
                let (at, indent) = self.insertion_point(section);
                insert(
                    &mut lines,
                    at,
                    section.is_root(),
                    vec![format!("{indent}{key}:"), format!("{indent}  - {value}")],
                );
            }
        }
        self.reload(lines)
    }

    /// Remove the first item equal to `value` from the list `key`.
    /// Returns whether an item was removed.
    pub fn remove_item(&mut self, target: Target<'_>, key: &str, value: &str) -> Result<bool, Error> {
        let section = self.target(target)?;
        let Some(entry) = section.get(key) else { return Ok(false) };
        let Value::List(items) = entry.value() else {
            return Err(Error::new(entry.line(), format!("`{key}` is a single value, not a list")));
        };
        let Some(item) = items.iter().find(|i| i.value == value) else { return Ok(false) };
        let mut lines = self.lines().to_vec();
        lines.remove(item.line - 1);
        self.reload(lines)?;
        Ok(true)
    }

    /// Remove `key` and, for a list, all of its items.
    pub fn remove_key(&mut self, target: Target<'_>, key: &str) -> Result<bool, Error> {
        let section = self.target(target)?;
        let Some(entry) = section.get(key) else { return Ok(false) };
        let mut lines = self.lines().to_vec();
        lines.drain(entry.line() - 1..entry.last_line);
        self.reload(lines)?;
        Ok(true)
    }

    /// Append an empty `[kind label]` section at the end of the document.
    pub fn add_section(&mut self, kind: &str, label: &str) -> Result<(), Error> {
        validate_key(kind)?;
        validate_label(label)?;
        if self.section(kind, label).is_some() {
            return Ok(());
        }
        let mut lines = self.lines().to_vec();
        if lines.last().is_some_and(|l| !trim(l).is_empty()) {
            lines.push(String::new());
        }
        lines.push(if label.is_empty() { format!("[{kind}]") } else { format!("[{kind} {label}]") });
        self.reload(lines)
    }

    /// Remove a section: its header and every line up to the next header.
    pub fn remove_section(&mut self, kind: &str, label: &str) -> Result<bool, Error> {
        let Some(pos) = self.sections().iter().position(|s| s.kind() == kind && s.label() == label) else {
            return Ok(false);
        };
        let start = self.sections()[pos].line() - 1;
        let end = self.sections().get(pos + 1).map_or(self.lines().len(), |s| s.line() - 1);
        let mut lines = self.lines().to_vec();
        lines.drain(start..end);
        self.reload(lines)?;
        Ok(true)
    }

    /// Where a new entry for `section` goes (0-based line index) and the
    /// indentation it should use.
    fn insertion_point(&self, section: &Section) -> (usize, String) {
        let lines = self.lines();
        let indent = section.entries().first().map_or(String::new(), |e| indent_of(&lines[e.line() - 1]).to_string());
        if let Some(last) = section.entries().last() {
            return (last.last_line, indent);
        }
        if !section.is_root() {
            return (section.line(), indent);
        }
        let mut at = self.sections().first().map_or(lines.len(), |s| s.line() - 1);
        while at > 0 && trim(&lines[at - 1]).is_empty() {
            at -= 1;
        }
        (at, indent)
    }

    fn reload(&mut self, lines: Vec<String>) -> Result<(), Error> {
        let mut doc = Document::from_lines(lines).map_err(|e| {
            Error::new(e.line, format!("internal error: edit produced an invalid document: {}", e.message))
        })?;
        (doc.bom, doc.crlf) = (self.bom, self.crlf);
        *self = doc;
        Ok(())
    }
}

/// Insert `new` at `at`. New top-level entries are kept visually apart
/// from a section header that directly follows them.
fn insert(lines: &mut Vec<String>, at: usize, root: bool, mut new: Vec<String>) {
    if root && lines.get(at).is_some_and(|l| trim(l).starts_with('[')) {
        new.push(String::new());
    }
    lines.splice(at..at, new);
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

pub(crate) fn validate_key(key: &str) -> Result<(), Error> {
    if is_key(key) {
        Ok(())
    } else {
        Err(Error::new(0, format!("invalid key `{key}`"))
            .hint("keys start with a-z and use only a-z, 0-9, `-` and `_`"))
    }
}

pub(crate) fn validate_label(label: &str) -> Result<(), Error> {
    if label.contains(['\n', '\r']) || trim(label) != label {
        return Err(Error::new(0, format!("section label {label:?} cannot be written"))
            .hint("labels cannot contain line breaks or start/end with whitespace"));
    }
    Ok(())
}

/// Whether `value` survives a write/parse round trip unchanged.
pub(crate) fn validate_value(value: &str, allow_empty: bool) -> Result<(), Error> {
    let problem = if value.contains(['\n', '\r']) {
        Some("contains a line break")
    } else if trim(value) != value {
        Some("starts or ends with whitespace")
    } else if value.is_empty() && !allow_empty {
        Some("is empty")
    } else if is_quoted(value) {
        Some("is wrapped in quotes")
    } else {
        None
    };
    match problem {
        Some(p) => Err(Error::new(0, format!("value {value:?} {p} and cannot be written in Plate"))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use crate::{Document, Target};

    fn doc(s: &str) -> Document {
        Document::parse(s).unwrap()
    }

    #[test]
    fn push_keeps_comments_and_indentation() {
        let mut d =
            doc("# My profile\ndescription = x\n\nskills:\n    - a   \n    # b is great\n    - b\n\n# trailing\n");
        d.push(Target::Root, "skills", "c").unwrap();
        assert_eq!(
            d.to_string(),
            "# My profile\ndescription = x\n\nskills:\n    - a   \n    # b is great\n    - b\n    - c\n\n# trailing\n"
        );
    }

    #[test]
    fn push_creates_list_before_first_section() {
        let mut d = doc("# header\n\n[repo /x]\na = 1\n");
        d.push(Target::Root, "skills", "git").unwrap();
        assert_eq!(d.to_string(), "# header\nskills:\n  - git\n\n[repo /x]\na = 1\n");
        let mut d = doc("[repo /x]\na = 1\n[repo /y]\n");
        d.push(Target::Section("repo", "/y"), "profiles", "coding").unwrap();
        d.push(Target::Section("repo", "/x"), "profiles", "p").unwrap();
        assert_eq!(d.to_string(), "[repo /x]\na = 1\nprofiles:\n  - p\n[repo /y]\nprofiles:\n  - coding\n");
    }

    #[test]
    fn set_replaces_value_preserving_alignment() {
        let mut d = doc("name    =   old\nother = 1\n");
        d.set(Target::Root, "name", "new value").unwrap();
        d.set(Target::Root, "added", "yes").unwrap();
        assert_eq!(d.to_string(), "name    =   new value\nother = 1\nadded = yes\n");
    }

    #[test]
    fn remove_item_key_and_section() {
        let mut d = doc("skills:\n  - a\n  - b\nx = 1\n[s one]\nk = v\n\n[s two]\nk = w\n");
        assert!(d.remove_item(Target::Root, "skills", "a").unwrap());
        assert!(!d.remove_item(Target::Root, "skills", "zzz").unwrap());
        assert!(d.remove_section("s", "one").unwrap());
        assert!(d.remove_key(Target::Root, "skills").unwrap());
        assert_eq!(d.to_string(), "x = 1\n[s two]\nk = w\n");
    }

    #[test]
    fn rejects_unwritable_values() {
        let mut d = doc("");
        assert!(d.set(Target::Root, "a", " padded").is_err());
        assert!(d.set(Target::Root, "a", "two\nlines").is_err());
        assert!(d.set(Target::Root, "a", "\"q\"").is_err());
        assert!(d.push(Target::Root, "l", "").is_err());
        assert!(d.set(Target::Root, "Bad", "x").is_err());
        d.add_section("repo", "/p").unwrap();
        d.set(Target::Section("repo", "/p"), "k", "v").unwrap();
        assert_eq!(d.to_string(), "[repo /p]\nk = v\n");
    }
}

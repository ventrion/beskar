//! The versioned, line-oriented .bsk syntax. No implicit types or expansion.
use crate::{Result, io};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Record {
    pub line: usize,
    pub fields: Vec<String>,
}

impl Record {
    pub fn is(&self, key: &str, count: usize) -> bool {
        self.fields.first().is_some_and(|v| v == key) && self.fields.len() == count
    }

    pub fn error(&self, message: &str) -> String {
        format!("line {}: {message}", self.line)
    }
}

pub fn read(path: &Path) -> Result<Vec<Record>> {
    let source = io(path.display(), std::fs::read_to_string(path))?;
    parse(&source).map_err(|error| format!("{}:{error}", path.display()))
}

pub fn parse(source: &str) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let mut chars = line.chars().peekable();
        let mut fields = Vec::new();
        let fail = |message: &str| format!("line {}: {message}", index + 1);
        loop {
            while chars.peek().is_some_and(|c| matches!(c, ' ' | '\t')) {
                chars.next();
            }
            let Some(&first) = chars.peek() else { break };
            if first == '#' {
                break;
            }
            let mut value = String::new();
            if first == '"' {
                chars.next();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => value.push(match chars.next() {
                            Some('"') => '"',
                            Some('\\') => '\\',
                            Some('n') => '\n',
                            Some('r') => '\r',
                            Some('t') => '\t',
                            _ => {
                                return Err(fail(
                                    "unknown escape; use \\\\, \\\", \\n, \\r or \\t",
                                ));
                            }
                        }),
                        Some(c) if !c.is_control() => value.push(c),
                        Some(_) => return Err(fail("control character in quoted string")),
                        None => return Err(fail("unterminated quoted string")),
                    }
                }
                if chars.peek().is_some_and(|c| !matches!(c, ' ' | '\t' | '#')) {
                    return Err(fail("expected whitespace after quoted string"));
                }
            } else {
                while let Some(&c) = chars.peek() {
                    if matches!(c, ' ' | '\t' | '#') {
                        break;
                    }
                    if !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':')) {
                        return Err(fail(
                            "bare values use letters, digits, - _ . / :; quote other values",
                        ));
                    }
                    value.push(c);
                    chars.next();
                }
            }
            fields.push(value);
        }
        if !fields.is_empty() {
            records.push(Record {
                line: index + 1,
                fields,
            });
        }
    }
    if !records
        .first()
        .is_some_and(|r| r.is("beskar", 2) && r.fields[1] == "1")
    {
        return Err("line 1: expected 'beskar 1' as the first record".into());
    }
    records.remove(0);
    Ok(records)
}

pub fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn path_text(path: &Path) -> Result<&str> {
    let value = path
        .to_str()
        .ok_or_else(|| format!("{}: Beskar requires UTF-8 paths", path.display()))?;
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(format!(
            "{}: unsupported control character in path",
            path.display()
        ));
    }
    Ok(value)
}

pub fn name(value: &str) -> Result<()> {
    if value.len() > 64
        || value.is_empty()
        || !value.as_bytes()[0].is_ascii_lowercase() && !value.as_bytes()[0].is_ascii_digit()
        || !value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'_'))
    {
        return Err(format!(
            "invalid name {value:?}; use 1-64 lowercase letters, digits, - or _, starting with a letter or digit"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strings_comments_and_crlf() {
        let value = "C:\\my skills\\\"#x\"\n\t雪";
        let doc = format!(
            "# example\r\nbeskar 1\r\n path {} # comment\r\n",
            quote(value)
        );
        assert_eq!(parse(&doc).unwrap()[0].fields, ["path", value]);
    }
    #[test]
    fn rejects_ambiguous_syntax_and_versions() {
        for source in [
            "beskar 2",
            "beskar 1\nx \"bad",
            "beskar 1\nx \"x\"y",
            "beskar 1\nx [a,b]",
            "beskar 1\nx \"\\q\"",
        ] {
            assert!(parse(source).is_err(), "{source}");
        }
        for value in ["../x", "A", "-a", "", "a/b"] {
            assert!(name(value).is_err());
        }
    }
}

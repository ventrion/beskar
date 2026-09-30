//! The versioned, line-oriented .bsk syntax. No implicit types or expansion.
pub type Result<T> = std::result::Result<T, String>;
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
    let source =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
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
    }
}

/// Replace a singleton record while preserving unrelated bytes, comments and line endings.
pub fn set_record(source: &str, key: &str, values: &[&str]) -> Result<String> {
    let records = parse(source)?;
    let matching: Vec<_> = records.iter().filter(|r| r.fields[0] == key).collect();
    if matching.len() != 1 {
        return Err(format!("expected exactly one {key} record"));
    }
    let line_number = matching[0].line;
    let replacement = std::iter::once(key.to_string())
        .chain(values.iter().map(|v| quote(v)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = String::new();
    for (i, line) in source.split_inclusive('\n').enumerate() {
        if i + 1 != line_number {
            out.push_str(line);
            continue;
        }
        let content = line.trim_end_matches(['\r', '\n']);
        let ending = &line[content.len()..];
        let indent = &content[..content.len() - content.trim_start_matches([' ', '\t']).len()];
        out.push_str(indent);
        out.push_str(&replacement);
        if let Some(comment) = comment_start(content) {
            let before = &content[..comment];
            let whitespace = &before[before.trim_end_matches([' ', '\t']).len()..];
            out.push_str(if whitespace.is_empty() {
                " "
            } else {
                whitespace
            });
            out.push_str(&content[comment..]);
        }
        out.push_str(ending);
    }
    parse(&out)?;
    Ok(out)
}
fn comment_start(line: &str) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '#' if !quoted => return Some(i),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod editor_tests {
    use super::*;
    #[test]
    fn edits_preserve_mixed_endings_comments_and_hashes_inside_values() {
        let source = "# header\r\nbeskar 1\n  library \"/a/#b\"  # keep\r\nregistry /r";
        let out = set_record(source, "library", &["/new/雪 # path"]).unwrap();
        assert_eq!(
            out,
            "# header\r\nbeskar 1\n  library \"/new/雪 # path\"  # keep\r\nregistry /r"
        );
        assert_eq!(parse(&out).unwrap()[0].fields[1], "/new/雪 # path");
    }
}

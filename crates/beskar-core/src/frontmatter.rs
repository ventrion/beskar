//! The YAML front matter at the top of a `SKILL.md` file.
//!
//! Beskar reads only the part of YAML that skill files use: top-level
//! `key: value` pairs whose values are plain, quoted or block (`|`, `>`)
//! scalars, block or flow sequences, and nested mappings, which are
//! flattened to `parent.child` keys. Anything else is skipped rather than
//! rejected. Front matter is metadata to display; Beskar's correctness never
//! depends on it.

/// Top-level fields of a front matter block, in file order. Sequences are
/// joined with ", ".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frontmatter {
    pub fields: Vec<(String, String)>,
}

impl Frontmatter {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Parse the front matter at the start of `text`. `None` if the text does
/// not start with a `---` line; an error if that block is never closed.
pub fn parse(text: &str) -> Option<Result<Frontmatter, String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    let mut body = Vec::new();
    for line in lines {
        if matches!(line.trim_end(), "---" | "...") {
            return Some(Ok(Frontmatter {
                fields: mapping(&body),
            }));
        }
        body.push(line);
    }
    Some(Err("the front matter has no closing `---` line".to_string()))
}

enum Value {
    Text(String),
    Map(Vec<(String, String)>),
}

fn mapping(lines: &[&str]) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || starts_indented(line) {
            continue;
        }
        let Some((key, rest)) = split_key(line) else {
            continue;
        };
        let rest = rest.trim();
        let start = i;
        while i < lines.len() {
            let next = lines[i];
            let sequence_item =
                rest.is_empty() && (next.starts_with("- ") || next.trim_end() == "-");
            if !(next.trim().is_empty() || starts_indented(next) || sequence_item) {
                break;
            }
            i += 1;
        }
        match value(rest, &lines[start..i]) {
            Value::Text(text) => fields.push((key, text)),
            Value::Map(pairs) => {
                fields.extend(pairs.into_iter().map(|(k, v)| (format!("{key}.{k}"), v)))
            }
        }
    }
    fields
}

fn starts_indented(line: &str) -> bool {
    line.starts_with([' ', '\t'])
}

fn indentation(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// Split `key: rest`. The separator is the first `:` followed by whitespace
/// or the end of the line.
fn split_key(line: &str) -> Option<(String, &str)> {
    if line.starts_with("- ") || line.starts_with('?') {
        return None;
    }
    if let Some(quote) = line.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let end = line[1..].find(quote)? + 1;
        let rest = line[end + 1..].trim_start().strip_prefix(':')?;
        return Some((line[1..end].to_string(), rest));
    }
    let bytes = line.as_bytes();
    let at = (0..bytes.len()).find(|&i| {
        bytes[i] == b':' && (i + 1 == bytes.len() || matches!(bytes[i + 1], b' ' | b'\t'))
    })?;
    let key = line[..at].trim();
    (!key.is_empty()).then(|| (key.to_string(), &line[at + 1..]))
}

fn value(rest: &str, block: &[&str]) -> Value {
    match rest.chars().next() {
        None => nested(block),
        Some('|' | '>') => Value::Text(block_scalar(rest, block)),
        Some('"') => Value::Text(double_quoted(&join_flow(rest, block))),
        Some('\'') => Value::Text(single_quoted(&join_flow(rest, block))),
        Some('[') => Value::Text(flow_sequence(&join_flow(rest, block))),
        Some('{') => Value::Text(join_flow(rest, block)),
        _ => Value::Text(plain(rest, block)),
    }
}

/// A key with nothing after the colon: a block sequence, a nested mapping,
/// or an empty value.
fn nested(block: &[&str]) -> Value {
    let content: Vec<&str> = block
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let Some(first) = content.first() else {
        return Value::Text(String::new());
    };
    if first.trim_start().starts_with('-') {
        let items: Vec<String> = content
            .iter()
            .filter_map(|line| line.trim_start().strip_prefix('-'))
            .map(|item| inline_scalar(item.trim()))
            .filter(|item| !item.is_empty())
            .collect();
        return Value::Text(items.join(", "));
    }
    let indent = content.iter().map(|l| indentation(l)).min().unwrap_or(0);
    let dedented: Vec<&str> = block
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                ""
            } else {
                &line[indent.min(indentation(line))..]
            }
        })
        .collect();
    Value::Map(mapping(&dedented))
}

fn block_scalar(header: &str, block: &[&str]) -> String {
    let folded = header.starts_with('>');
    let explicit = header
        .chars()
        .find_map(|c| c.to_digit(10))
        .map(|d| d as usize);
    let indent = explicit.unwrap_or_else(|| {
        block
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| indentation(l))
            .min()
            .unwrap_or(0)
    });
    let lines = block.iter().map(|line| {
        if line.trim().is_empty() {
            ""
        } else {
            line[indent.min(indentation(line))..].trim_end()
        }
    });
    if !folded {
        return lines.collect::<Vec<_>>().join("\n").trim_end().to_string();
    }
    let mut out = String::new();
    for line in lines {
        if line.is_empty() {
            out.push('\n');
        } else {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push(' ');
            }
            out.push_str(line);
        }
    }
    out.trim().to_string()
}

/// Join a flow value that continues on indented lines, folding line breaks
/// into spaces.
fn join_flow(rest: &str, block: &[&str]) -> String {
    let mut out = rest.to_string();
    for line in block {
        let line = line.trim();
        if line.is_empty() {
            out.push('\n');
        } else {
            if !out.ends_with('\n') {
                out.push(' ');
            }
            out.push_str(line);
        }
    }
    out
}

fn plain(rest: &str, block: &[&str]) -> String {
    let first = strip_comment(rest);
    let joined = join_flow(first, block);
    let text = joined.trim();
    if text == "~" || text == "null" {
        String::new()
    } else {
        text.to_string()
    }
}

fn strip_comment(text: &str) -> &str {
    match text.find(" #").or_else(|| text.find("\t#")) {
        Some(at) => text[..at].trim_end(),
        None => text,
    }
}

fn inline_scalar(text: &str) -> String {
    match text.chars().next() {
        Some('"') => double_quoted(text),
        Some('\'') => single_quoted(text),
        _ => strip_comment(text).trim().to_string(),
    }
}

fn double_quoted(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().skip(1);
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some('u') => {
                    let code: String = chars.by_ref().take(4).collect();
                    out.extend(u32::from_str_radix(&code, 16).ok().and_then(char::from_u32));
                }
                Some('x') => {
                    let code: String = chars.by_ref().take(2).collect();
                    out.extend(u32::from_str_radix(&code, 16).ok().and_then(char::from_u32));
                }
                Some(' ') => out.push(' '),
                Some(other) => out.push(other),
                None => break,
            },
            other => out.push(other),
        }
    }
    out
}

fn single_quoted(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().skip(1).peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                out.push('\'');
            } else {
                break;
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn flow_sequence(text: &str) -> String {
    let inner = text.trim().strip_prefix('[').unwrap_or(text);
    let inner = match inner.rfind(']') {
        Some(end) => &inner[..end],
        None => inner,
    };
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in inner.chars() {
        match (quote, c) {
            (None, ',') => items.push(std::mem::take(&mut current)),
            (None, '"' | '\'') => {
                quote = Some(c);
                current.push(c);
            }
            (Some(q), c) if c == q => {
                quote = None;
                current.push(c);
            }
            _ => current.push(c),
        }
    }
    items.push(current);
    items
        .iter()
        .map(|item| inline_scalar(item.trim()))
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(text: &str) -> Vec<(String, String)> {
        parse(text)
            .expect("has front matter")
            .expect("front matter closes")
            .fields
    }

    fn get(text: &str, key: &str) -> String {
        fields(text)
            .into_iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("no `{key}`"))
    }

    #[test]
    fn plain_values() {
        let text = "---\nname: pdf\ndescription: Extract text from PDFs. Use for: forms # comment\n---\n# Body\n";
        assert_eq!(
            fields(text),
            [
                ("name".into(), "pdf".into()),
                (
                    "description".into(),
                    "Extract text from PDFs. Use for: forms".into()
                )
            ]
        );
    }

    #[test]
    fn no_front_matter() {
        assert_eq!(parse("# Just markdown\n"), None);
        assert_eq!(parse(""), None);
        assert!(parse("---\nname: x\n").unwrap().is_err());
    }

    #[test]
    fn quoted_values() {
        assert_eq!(get("---\nname: \"pdf\"\n---\n", "name"), "pdf");
        assert_eq!(
            get(
                "---\ndescription: \"Say \\\"hi\\\"\\nthen \\u00e9\"\n---\n",
                "description"
            ),
            "Say \"hi\"\nthen \u{e9}"
        );
        assert_eq!(
            get("---\ndescription: 'It''s here: yes'\n---\n", "description"),
            "It's here: yes"
        );
    }

    #[test]
    fn multi_line_plain_and_quoted_values_fold() {
        let text = "---\ndescription: Browser automation\n  and testing\n  with Playwright.\nname: pw\n---\n";
        assert_eq!(
            get(text, "description"),
            "Browser automation and testing with Playwright."
        );
        assert_eq!(get(text, "name"), "pw");
        let text = "---\ndescription: \"one\n  two\"\n---\n";
        assert_eq!(get(text, "description"), "one two");
    }

    #[test]
    fn block_scalars() {
        let text = "---\ndescription: |\n  line one\n  line two\n\nname: x\n---\n";
        assert_eq!(get(text, "description"), "line one\nline two");
        let text = "---\ndescription: >-\n  folded one\n  folded two\n\n  new paragraph\nlicense: MIT\n---\n";
        assert_eq!(
            get(text, "description"),
            "folded one folded two\nnew paragraph"
        );
        assert_eq!(get(text, "license"), "MIT");
    }

    #[test]
    fn sequences_and_nested_mappings() {
        let text = "---\ntags: [browser, \"testing, e2e\"]\nallowed-tools:\n  - Bash\n  - Read\nmetadata:\n  version: 1.2.0\n  author: \"me\"\n  nested:\n    deep: yes\nlist:\n- a\n- b\n---\n";
        assert_eq!(get(text, "tags"), "browser, testing, e2e");
        assert_eq!(get(text, "allowed-tools"), "Bash, Read");
        assert_eq!(get(text, "metadata.version"), "1.2.0");
        assert_eq!(get(text, "metadata.author"), "me");
        assert_eq!(get(text, "metadata.nested.deep"), "yes");
        assert_eq!(get(text, "list"), "a, b");
    }

    #[test]
    fn crlf_and_bom() {
        assert_eq!(get("\u{feff}---\r\nname: pdf\r\n---\r\n", "name"), "pdf");
    }

    #[test]
    fn junk_is_skipped_not_fatal() {
        let text = "---\n: nothing\n  stray indented\nname: ok\nweird line without colon\n---\n";
        assert_eq!(fields(text), [("name".into(), "ok".into())]);
    }
}

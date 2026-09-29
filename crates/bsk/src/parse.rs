//! Line classification. Every line is parsed on its own; the only state
//! carried between lines is the line number.

use crate::document::{Document, Kind, Line};
use crate::{BLANK, Error, is_key, is_key_byte};

pub(crate) fn parse(text: &str) -> Result<Document, Error> {
    let (bom, text) = match text.strip_prefix('\u{feff}') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let mut doc = Document {
        lines: Vec::new(),
        crlf: false,
        bom,
    };
    for (index, raw) in text.split_inclusive('\n').enumerate() {
        let content = match raw.strip_suffix('\n') {
            Some(content) => {
                if index == 0 {
                    doc.crlf = content.ends_with('\r');
                }
                content.strip_suffix('\r').unwrap_or(content)
            }
            None => raw,
        };
        doc.lines.push(parse_line(content, index + 1)?);
    }
    Ok(doc)
}

pub(crate) fn parse_line(text: &str, number: usize) -> Result<Line, Error> {
    if let Some((at, c)) = text
        .char_indices()
        .find(|&(_, c)| c.is_control() && c != '\t')
    {
        return Err(Error::at(
            text,
            number,
            at,
            c.len_utf8(),
            format!("control character U+{:04X} is not allowed", c as u32),
        )
        .with_help(
            "BSK files are plain text: one `key: value`, `[section]` or `# comment` per line",
        ));
    }
    let start = text.len() - text.trim_start_matches(BLANK).len();
    let body = text[start..].trim_end_matches(BLANK);
    let Some(first) = body.chars().next() else {
        return Ok(Line::new(text, Kind::Blank, number, 0, 0));
    };
    match first {
        '#' => Ok(Line::new(text, Kind::Comment, number, start, start)),
        '[' => parse_header(text, number, start, body),
        'a'..='z' => parse_entry(text, number, start, body),
        _ => Err(bad_start(text, number, start, body, first)),
    }
}

fn parse_entry(text: &str, number: usize, start: usize, body: &str) -> Result<Line, Error> {
    let key_len = body.bytes().take_while(|&b| is_key_byte(b)).count();
    let key = &body[..key_len];
    let rest = &body[key_len..];
    let gap = rest.len() - rest.trim_start_matches(BLANK).len();
    let after_gap = &rest[gap..];

    if let Some(after_colon) = after_gap.strip_prefix(':') {
        let pad = after_colon.len() - after_colon.trim_start_matches(BLANK).len();
        let kind = Kind::Entry {
            key: key.to_string(),
            value: after_colon[pad..].to_string(),
        };
        let value_at = start + key_len + gap + 1 + pad;
        return Ok(Line::new(text, kind, number, start, value_at));
    }

    let at = start + key_len;
    Err(match after_gap.chars().next() {
        None => Error::at(text, number, at, 0, format!("missing `:` after `{key}`"))
            .with_help(format!("write `{key}: <value>`")),
        Some('=') => {
            let value = after_gap[1..].trim_start_matches(BLANK);
            Error::at(
                text,
                number,
                at + gap,
                1,
                "use `:` between a key and its value",
            )
            .with_help(suggest_entry(key, value))
        }
        Some(_) if gap > 0 => {
            Error::at(text, number, at, gap, format!("missing `:` after `{key}`"))
                .with_help(suggest_entry(key, after_gap))
        }
        Some(c) => {
            let raw_key = body.split([':', '=', ' ', '\t']).next().unwrap_or(body);
            let error = Error::at(
                text,
                number,
                at,
                c.len_utf8(),
                format!("`{c}` is not allowed in a key"),
            );
            match normalize_key(raw_key) {
                Some(key) => error.with_help(format!(
                    "keys use lowercase letters, digits and `-`: write `{key}`"
                )),
                None => error.with_help("keys use lowercase letters, digits and `-`"),
            }
        }
    })
}

fn parse_header(text: &str, number: usize, start: usize, body: &str) -> Result<Line, Error> {
    let Some(inner) = body.strip_prefix('[').and_then(|b| b.strip_suffix(']')) else {
        return Err(Error::at(
            text,
            number,
            start + body.len(),
            0,
            "section header does not end with `]`",
        )
        .with_help("write `[name]` or `[name label]` on a line of its own"));
    };
    let lead = inner.len() - inner.trim_start_matches(BLANK).len();
    let inner_at = start + 1 + lead;
    let inner = inner.trim_matches(BLANK);
    if inner.is_empty() {
        return Err(
            Error::at(text, number, start, body.len(), "empty section header")
                .with_help("write `[name]` or `[name label]`"),
        );
    }
    let name_len = inner.bytes().take_while(|&b| is_key_byte(b)).count();
    let name = &inner[..name_len];
    if !is_key(name) {
        let first = inner.chars().next().unwrap_or('[');
        let error = Error::at(
            text,
            number,
            inner_at,
            first.len_utf8(),
            "section names start with a lowercase letter",
        );
        return Err(if first == '[' {
            error.with_help(format!(
                "section headers use single brackets: `[{}]`",
                inner.trim_matches(['[', ']'])
            ))
        } else {
            error.with_help("write `[name]` or `[name label]`, with a lowercase name")
        });
    }
    let rest = &inner[name_len..];
    if let Some(c) = rest.chars().next()
        && !BLANK.contains(&c)
    {
        return Err(Error::at(
            text,
            number,
            inner_at + name_len,
            c.len_utf8(),
            format!("`{c}` is not allowed in a section name"),
        )
        .with_help(
            "section names use lowercase letters, digits and `-`; put a space before the label",
        ));
    }
    let label = rest.trim_matches(BLANK);
    let label_at = inner_at + name_len + (rest.len() - rest.trim_start_matches(BLANK).len());
    let kind = Kind::Header {
        name: name.to_string(),
        label: label.to_string(),
    };
    Ok(Line::new(text, kind, number, start, label_at))
}

fn bad_start(text: &str, number: usize, start: usize, body: &str, first: char) -> Error {
    let error = |message: String| Error::at(text, number, start, first.len_utf8(), message);
    match first {
        '-' | '*' | '+' => {
            let item = body[first.len_utf8()..].trim_matches(BLANK);
            error(format!("BSK has no `{first}` list items")).with_help(format!(
                "write one `key: value` line per item, repeating the key: `<key>: {item}`"
            ))
        }
        '"' | '\'' => error("keys are not quoted".to_string()).with_help(
            "write `key: value`; values are taken verbatim, so they need no quotes either",
        ),
        'A'..='Z' => {
            let raw_key = body.split([':', '=', ' ', '\t']).next().unwrap_or(body);
            let error = error("keys are lowercase".to_string());
            match normalize_key(raw_key) {
                Some(key) => error.with_help(format!("write `{key}`")),
                None => error.with_help("keys use lowercase letters, digits and `-`"),
            }
        }
        ';' => error("comments start with `#`".to_string())
            .with_help(format!("write `#{}`", &body[1..])),
        '/' if body.starts_with("//") => error("comments start with `#`".to_string())
            .with_help(format!("write `#{}`", &body[2..])),
        ':' | '=' => error(format!("missing key before `{first}`")).with_help("write `key: value`"),
        '0'..='9' | '_' => error("keys start with a lowercase letter".to_string())
            .with_help("keys use lowercase letters, digits and `-`"),
        _ if first.is_alphabetic() => error(format!("`{first}` is not allowed in a key"))
            .with_help("keys use ASCII lowercase letters, digits and `-`"),
        _ => error(format!("unexpected `{first}` at the start of a line"))
            .with_help("each line is `key: value`, `[section]`, `# comment` or blank"),
    }
}

/// The entry someone probably meant, for a line written in another
/// format's style: quotes around the value dropped, and bracketed lists
/// explained instead of copied.
fn suggest_entry(key: &str, value: &str) -> String {
    if value.starts_with('[') {
        return format!("BSK has no `[a, b]` lists: write one `{key}: <item>` line per item");
    }
    let unquoted = match value.as_bytes() {
        [q @ (b'"' | b'\''), .., last] if value.len() >= 2 && last == q => {
            &value[1..value.len() - 1]
        }
        _ => value,
    };
    format!("write `{key}: {unquoted}`")
}

/// Turn a near-miss like `skillsDir`, `Skills_Dir` or `skills dir` into a
/// valid key, if that is possible.
pub(crate) fn normalize_key(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut after_lower = false;
    for c in raw.chars() {
        if c.is_ascii_uppercase() {
            if after_lower {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
            after_lower = false;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            after_lower = true;
        } else if matches!(c, '_' | '-' | ' ' | '.') {
            if !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
            after_lower = false;
        } else {
            return None;
        }
    }
    let out = out.trim_matches('-').to_string();
    is_key(&out).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(text: &str) -> Kind {
        parse_line(text, 1).expect("line parses").kind
    }

    fn fail(text: &str) -> Error {
        parse_line(text, 1).expect_err("line is rejected")
    }

    fn entry(key: &str, value: &str) -> Kind {
        Kind::Entry {
            key: key.into(),
            value: value.into(),
        }
    }

    fn header(name: &str, label: &str) -> Kind {
        Kind::Header {
            name: name.into(),
            label: label.into(),
        }
    }

    #[test]
    fn first_character_decides_the_line_kind() {
        assert_eq!(kind(""), Kind::Blank);
        assert_eq!(kind(" \t "), Kind::Blank);
        assert_eq!(kind("# note"), Kind::Comment);
        assert_eq!(kind("   #indented"), Kind::Comment);
        assert_eq!(kind("[repo]"), header("repo", ""));
        assert_eq!(kind("key: value"), entry("key", "value"));
    }

    #[test]
    fn values_are_verbatim_after_the_first_colon() {
        assert_eq!(
            kind("synced: 2026-09-29T10:15:03Z"),
            entry("synced", "2026-09-29T10:15:03Z")
        );
        assert_eq!(
            kind("description: C# helpers # not a comment"),
            entry("description", "C# helpers # not a comment")
        );
        assert_eq!(
            kind("path: \"~/My Library\""),
            entry("path", "\"~/My Library\"")
        );
        assert_eq!(kind("list: [a, b]"), entry("list", "[a, b]"));
    }

    #[test]
    fn whitespace_around_keys_and_values_is_ignored() {
        assert_eq!(
            kind("  key  :   spaced value  \t"),
            entry("key", "spaced value")
        );
        assert_eq!(kind("key:value"), entry("key", "value"));
        assert_eq!(kind("key:"), entry("key", ""));
        assert_eq!(kind("key:   "), entry("key", ""));
        assert_eq!(kind("\tkey: tabbed"), entry("key", "tabbed"));
    }

    #[test]
    fn value_offset_points_at_the_value() {
        let line = parse_line("  key :  value", 1).unwrap();
        assert_eq!(&line.text[line.value_at..], "value");
        let line = parse_line("key:", 1).unwrap();
        assert_eq!(line.value_at, 4);
    }

    #[test]
    fn section_headers_have_a_name_and_an_optional_label() {
        assert_eq!(
            kind("[repo /home/me/My Code]"),
            header("repo", "/home/me/My Code")
        );
        assert_eq!(kind("[ repo   /tmp/x ]"), header("repo", "/tmp/x"));
        assert_eq!(kind("[repo /odd]path]"), header("repo", "/odd]path"));
        assert_eq!(kind("[settings]   "), header("settings", ""));
        let line = parse_line("[repo  /a/b]", 1).unwrap();
        assert_eq!(&line.text[line.value_at..line.value_at + 4], "/a/b");
    }

    #[test]
    fn yaml_list_items_are_rejected_with_advice() {
        let error = fail("  - git");
        assert_eq!(error.message, "BSK has no `-` list items");
        assert_eq!(error.column, 3);
        assert_eq!(
            error.help.as_deref(),
            Some("write one `key: value` line per item, repeating the key: `<key>: git`")
        );
    }

    #[test]
    fn equals_sign_separator_is_rejected_with_advice() {
        let error = fail("library = ~/lib");
        assert_eq!(error.message, "use `:` between a key and its value");
        assert_eq!(error.column, 9);
        assert_eq!(error.help.as_deref(), Some("write `library: ~/lib`"));
        let error = fail("library=~/lib");
        assert_eq!(error.help.as_deref(), Some("write `library: ~/lib`"));
    }

    #[test]
    fn toml_habits_get_a_valid_suggestion() {
        assert_eq!(
            fail("skill = \"tdd\"").help.as_deref(),
            Some("write `skill: tdd`")
        );
        assert_eq!(
            fail("skills = [\"a\", \"b\"]").help.as_deref(),
            Some("BSK has no `[a, b]` lists: write one `skills: <item>` line per item")
        );
    }

    #[test]
    fn missing_colon_is_reported_at_the_end_of_the_key() {
        let error = fail("skill git");
        assert_eq!(error.message, "missing `:` after `skill`");
        assert_eq!(error.column, 6);
        assert_eq!(error.help.as_deref(), Some("write `skill: git`"));
        let error = fail("skill");
        assert_eq!(error.help.as_deref(), Some("write `skill: <value>`"));
    }

    #[test]
    fn badly_cased_keys_get_a_corrected_spelling() {
        assert_eq!(
            fail("skills_dir: x").help.as_deref(),
            Some("keys use lowercase letters, digits and `-`: write `skills-dir`")
        );
        assert_eq!(
            fail("skillsDir: x").help.as_deref(),
            Some("keys use lowercase letters, digits and `-`: write `skills-dir`")
        );
        let error = fail("Library: x");
        assert_eq!(error.message, "keys are lowercase");
        assert_eq!(error.help.as_deref(), Some("write `library`"));
    }

    #[test]
    fn foreign_comment_styles_are_rejected() {
        assert_eq!(fail("; note").help.as_deref(), Some("write `# note`"));
        assert_eq!(fail("// note").help.as_deref(), Some("write `# note`"));
    }

    #[test]
    fn quoted_keys_are_rejected() {
        assert_eq!(fail("\"key\": \"value\"").message, "keys are not quoted");
    }

    #[test]
    fn malformed_headers_are_rejected() {
        assert_eq!(
            fail("[repo /x").message,
            "section header does not end with `]`"
        );
        assert_eq!(fail("[]").message, "empty section header");
        let error = fail("[[repo]]");
        assert_eq!(
            error.help.as_deref(),
            Some("section headers use single brackets: `[repo]`")
        );
        assert_eq!(
            fail("[Repo]").message,
            "section names start with a lowercase letter"
        );
        let error = fail("[repo:/x]");
        assert_eq!(error.message, "`:` is not allowed in a section name");
        assert_eq!(error.column, 6);
    }

    #[test]
    fn control_characters_are_rejected() {
        let error = fail("key: a\u{7}b");
        assert_eq!(error.message, "control character U+0007 is not allowed");
        assert_eq!(error.column, 7);
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        let error = fail("  \u{e9}t\u{e9}: x");
        assert_eq!(error.column, 3);
        assert_eq!(error.message, "`\u{e9}` is not allowed in a key");
        let error = fail("k\u{e9}y: x");
        assert_eq!(error.column, 2);
    }

    #[test]
    fn documents_accept_crlf_and_a_byte_order_mark() {
        let doc = parse("\u{feff}a: 1\r\nb: 2\r\n").unwrap();
        assert!(doc.bom);
        assert!(doc.crlf);
        assert_eq!(doc.lines.len(), 2);
        assert_eq!(doc.lines[1].kind, entry("b", "2"));
    }

    #[test]
    fn errors_carry_the_line_number() {
        let error = parse("a: 1\n\n- nope\n").unwrap_err();
        assert_eq!(error.line, 3);
        assert_eq!(error.source_line, "- nope");
    }

    #[test]
    fn a_lone_carriage_return_inside_a_line_is_a_control_character() {
        let error = parse("a: 1\rb: 2\n").unwrap_err();
        assert_eq!(error.message, "control character U+000D is not allowed");
    }
}

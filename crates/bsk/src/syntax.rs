//! The whole bsk grammar lives in this file.
//!
//! A line is classified on its own, without looking at its neighbours. That is
//! the property that makes bsk easy to parse, easy to edit line by line, and
//! easy to merge in version control.

use crate::diagnostic::Diagnostic;

/// A byte range inside a line's raw text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    pub(crate) fn slice(self, raw: &str) -> &str {
        &raw[self.start..self.end]
    }
}

/// What a single line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Blank,
    Comment,
    Header { kind: Span, name: Span },
    Entry { key: Span, value: Span },
}

/// A syntax error inside one line. The caller adds the line number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SyntaxError {
    pub column: usize,
    pub width: usize,
    pub message: String,
    pub hint: Option<String>,
}

impl SyntaxError {
    fn at(raw: &str, byte: usize, width: usize, message: impl Into<String>) -> Self {
        SyntaxError {
            column: column_of(raw, byte),
            width,
            message: message.into(),
            hint: None,
        }
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub(crate) fn into_diagnostic(self, line: usize) -> Diagnostic {
        let diagnostic = Diagnostic::new(line, self.column, self.width, self.message);
        match self.hint {
            Some(hint) => diagnostic.with_hint(hint),
            None => diagnostic,
        }
    }
}

/// Characters that separate a key from its value.
pub(crate) fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn is_key_start(c: char) -> bool {
    c.is_ascii_alphabetic()
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// True for keys and section kinds: a letter, then letters, digits, `-` or `_`.
pub(crate) fn is_word(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_key_start) && chars.all(is_key_char)
}

/// Control characters other than tab never appear in a bsk file: the C0 controls
/// (U+0000 to U+001F), DEL (U+007F) and the C1 controls (U+0080 to U+009F).
pub(crate) fn is_forbidden(c: char) -> bool {
    c.is_control() && c != '\t'
}

/// True for a character that would print as nothing or as something else than itself:
/// a control character, an invisible format character, or whitespace other than a plain space.
///
/// A value may hold some of these, a no-break space for one, but a message must not print them raw.
pub(crate) fn is_invisible(c: char) -> bool {
    c.is_control()
        || (c.is_whitespace() && c != ' ')
        || matches!(
            c,
            '\u{ad}' // soft hyphen
            | '\u{61c}' // Arabic letter mark
            | '\u{200b}'..='\u{200f}' // zero width space and joiners, direction marks
            | '\u{202a}'..='\u{202e}' // embedded direction changes and overrides
            | '\u{2060}'..='\u{206f}' // word joiner, invisible operators, isolates
            | '\u{feff}' // zero width no-break space
            | '\u{e0000}'..='\u{e007f}' // tag characters
        )
}

/// Names one character for a message: the character in quotes when it is visible,
/// otherwise its code point, such as `U+00A0`.
pub(crate) fn describe_char(c: char) -> String {
    if is_invisible(c) {
        format!("U+{:04X}", c as u32)
    } else {
        format!("'{c}'")
    }
}

/// 1-based column, counted in characters, of a byte offset.
pub(crate) fn column_of(raw: &str, byte: usize) -> usize {
    raw[..byte].chars().count() + 1
}

const WORD_HINT: &str = "a key is a word of letters, digits, '-' or '_' that starts with a letter";
const HEADER_HINT: &str =
    "write a section header as [kind name] on one line, for example [repo /home/me/project]";

/// Decides what one line is, or explains why it is not valid bsk.
pub(crate) fn classify(raw: &str) -> Result<Kind, SyntaxError> {
    if let Some((at, c)) = raw.char_indices().find(|&(_, c)| is_forbidden(c)) {
        return Err(SyntaxError::at(
            raw,
            at,
            1,
            format!("control character U+{:04X} is not allowed", c as u32),
        )
        .hint("bsk files are plain text; delete the character or replace it with a space"));
    }
    let start = raw.len() - raw.trim_start_matches(is_blank).len();
    match raw[start..].chars().next() {
        None => Ok(Kind::Blank),
        Some('#') => Ok(Kind::Comment),
        Some('[') => classify_header(raw, start),
        Some(_) => classify_entry(raw, start),
    }
}

fn classify_header(raw: &str, start: usize) -> Result<Kind, SyntaxError> {
    let end = raw.trim_end_matches(is_blank).len();
    let chars = raw[start..end].chars().count();
    if !raw[start..end].ends_with(']') {
        // The last ']' closes the header. Whatever follows it is text that does not belong there.
        return Err(match raw[start..end].rfind(']') {
            Some(close) => {
                // Underline the text itself, not the blanks between it and the bracket.
                let text = end
                    - raw[start + close + 1..end]
                        .trim_start_matches(is_blank)
                        .len();
                SyntaxError::at(
                    raw,
                    text,
                    raw[text..end].chars().count(),
                    "unexpected text after the closing ']'",
                )
                .hint("bsk has no trailing comments; put a comment on its own line")
            }
            None => SyntaxError::at(
                raw,
                start,
                chars,
                "section header is missing its closing ']'",
            )
            .hint(HEADER_HINT),
        });
    }
    let inner_start = start + 1;
    let inner_end = end - 1;
    let inner = &raw[inner_start..inner_end];
    let trimmed = inner.trim_matches(is_blank);
    if trimmed.is_empty() {
        return Err(SyntaxError::at(raw, start, chars, "empty section header").hint(HEADER_HINT));
    }
    let kind_start = inner_start + (inner.len() - inner.trim_start_matches(is_blank).len());
    let kind_len = trimmed.find(is_blank).unwrap_or(trimmed.len());
    let kind = &raw[kind_start..kind_start + kind_len];
    if !is_word(kind) {
        return Err(SyntaxError::at(
            raw,
            kind_start,
            kind.chars().count(),
            format!("invalid section kind '{kind}'"),
        )
        .hint(
            "a section kind is a word of letters, digits, '-' or '_' that starts with a letter",
        ));
    }
    let kind_end = kind_start + kind_len;
    let after = &trimmed[kind_len..];
    let name_start = kind_end + (after.len() - after.trim_start_matches(is_blank).len());
    let name_end = kind_start + trimmed.len();
    Ok(Kind::Header {
        kind: Span::new(kind_start, kind_end),
        name: Span::new(name_start, name_end),
    })
}

fn classify_entry(raw: &str, start: usize) -> Result<Kind, SyntaxError> {
    let rest = &raw[start..];
    let first = rest.chars().next().unwrap_or(' ');
    if !is_key_start(first) {
        return Err(bad_key_start(raw, start, first, rest));
    }
    let key_len = rest.find(|c| !is_key_char(c)).unwrap_or(rest.len());
    let key_end = start + key_len;
    let key = Span::new(start, key_end);
    let after = &raw[key_end..];
    let Some(next) = after.chars().next() else {
        return Ok(Kind::Entry {
            key,
            value: Span::new(key_end, key_end),
        });
    };
    if !is_blank(next) {
        let name = key.slice(raw);
        let error = SyntaxError::at(
            raw,
            key_end,
            1,
            format!("unexpected {} after key '{name}'", describe_char(next)),
        );
        return Err(match next {
            ':' | '=' => error.hint(format!(
                "bsk lines are 'key value'; write '{name} <value>' without the '{next}'"
            )),
            _ => error.hint(
                "a key may only contain letters, digits, '-' and '_'; separate it from its value with a space",
            ),
        });
    }
    let value_start = key_end + (after.len() - after.trim_start_matches(is_blank).len());
    let value_end = raw.trim_end_matches(is_blank).len();
    let value = if value_end > value_start {
        Span::new(value_start, value_end)
    } else {
        Span::new(key_end, key_end)
    };
    Ok(Kind::Entry { key, value })
}

fn bad_key_start(raw: &str, start: usize, first: char, rest: &str) -> SyntaxError {
    let error = |message: String| SyntaxError::at(raw, start, 1, message);
    // Every message that quotes the character goes through `describe_char`, so an
    // invisible character is named by its code point instead of printed.
    let shown = describe_char(first);
    match first {
        '-' if rest.len() == 1 || rest[1..].starts_with(is_blank) => {
            error(format!("unexpected {shown}: bsk has no bullet lists"))
                .hint("give every item its own 'key value' line instead")
        }
        '"' | '\'' => error(format!("unexpected quote {first}: keys are bare words"))
            .hint("bsk has no quoting; remove the quotes (values are never quoted either)"),
        '=' | ':' => error(format!("a line cannot start with {shown}"))
            .hint("start the line with a key, then a space, then the value"),
        '{' | '}' | ',' | ';' => error(format!(
            "unexpected {shown}: bsk has no braces, commas or semicolons"
        ))
        .hint("write one 'key value' entry per line"),
        c if c.is_ascii_digit() => {
            error(format!("a key must start with a letter, found {shown}")).hint(WORD_HINT)
        }
        _ => error(format!("expected a key, found {shown}")).hint(WORD_HINT),
    }
}

/// Why a string cannot be stored as a value or a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueError {
    /// The text contains a line break.
    LineBreak,
    /// The text contains a control character other than tab: a C0 control, DEL or a C1 control.
    Control(char),
    /// The text starts or ends with a space or tab, which a reader would drop.
    EdgeWhitespace,
    /// The text is not a valid key or section kind.
    NotAWord(String),
}

impl std::fmt::Display for ValueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueError::LineBreak => f.write_str("value contains a line break"),
            ValueError::Control(c) => write!(
                f,
                "value contains the control character U+{:04X}",
                *c as u32
            ),
            ValueError::EdgeWhitespace => f.write_str("value starts or ends with whitespace"),
            ValueError::NotAWord(word) => write!(
                f,
                "'{word}' is not a valid key: use letters, digits, '-' or '_', starting with a letter"
            ),
        }
    }
}

impl std::error::Error for ValueError {}

pub(crate) fn check_value(value: &str) -> Result<(), ValueError> {
    if value.contains(['\n', '\r']) {
        return Err(ValueError::LineBreak);
    }
    if let Some(c) = value.chars().find(|&c| is_forbidden(c)) {
        return Err(ValueError::Control(c));
    }
    if value.starts_with(is_blank) || value.ends_with(is_blank) {
        return Err(ValueError::EdgeWhitespace);
    }
    Ok(())
}

pub(crate) fn check_word(word: &str) -> Result<(), ValueError> {
    if is_word(word) {
        Ok(())
    } else {
        Err(ValueError::NotAWord(word.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(raw: &str) -> (String, String) {
        match classify(raw).unwrap() {
            Kind::Entry { key, value } => {
                (key.slice(raw).to_string(), value.slice(raw).to_string())
            }
            other => panic!("not an entry: {other:?}"),
        }
    }

    fn header(raw: &str) -> (String, String) {
        match classify(raw).unwrap() {
            Kind::Header { kind, name } => {
                (kind.slice(raw).to_string(), name.slice(raw).to_string())
            }
            other => panic!("not a header: {other:?}"),
        }
    }

    #[test]
    fn blank_and_comment_lines() {
        assert_eq!(classify("").unwrap(), Kind::Blank);
        assert_eq!(classify(" \t ").unwrap(), Kind::Blank);
        assert_eq!(classify("# hi").unwrap(), Kind::Comment);
        assert_eq!(classify("   # indented").unwrap(), Kind::Comment);
    }

    #[test]
    fn entry_value_is_the_rest_of_the_line_trimmed() {
        assert_eq!(entry("skill git"), ("skill".into(), "git".into()));
        assert_eq!(
            entry("  skill \t  a  b  c   "),
            ("skill".into(), "a  b  c".into())
        );
    }

    #[test]
    fn hash_inside_a_value_is_content() {
        assert_eq!(
            entry("description C# and F# tools #1"),
            ("description".into(), "C# and F# tools #1".into())
        );
    }

    #[test]
    fn entry_without_value_has_an_empty_value() {
        assert_eq!(entry("flag"), ("flag".into(), String::new()));
        assert_eq!(entry("flag   "), ("flag".into(), String::new()));
    }

    #[test]
    fn value_may_contain_characters_that_other_formats_reserve() {
        assert_eq!(
            entry("path C:\\Users\\me\\a b"),
            ("path".into(), "C:\\Users\\me\\a b".into())
        );
        assert_eq!(
            entry(r#"note "quoted" = [x], {y}"#),
            ("note".into(), r#""quoted" = [x], {y}"#.into())
        );
    }

    #[test]
    fn header_splits_kind_and_name_at_the_first_blank() {
        assert_eq!(
            header("[repo /home/me/my project]"),
            ("repo".into(), "/home/me/my project".into())
        );
        assert_eq!(header("[ repo   /x  ]  "), ("repo".into(), "/x".into()));
        assert_eq!(header("[repo]"), ("repo".into(), String::new()));
        assert_eq!(
            header("[repo /tmp/odd]]"),
            ("repo".into(), "/tmp/odd]".into())
        );
    }

    #[test]
    fn unclosed_header_is_an_error() {
        let e = classify("[repo /x").unwrap_err();
        assert_eq!(e.message, "section header is missing its closing ']'");
        assert_eq!(e.column, 1);
        assert_eq!(e.width, 8);
        assert!(classify("[").is_err());
        assert_eq!(
            classify("  [repo /x # not closed").unwrap_err().message,
            "section header is missing its closing ']'"
        );
    }

    #[test]
    fn text_after_the_closing_bracket_is_reported_at_that_text() {
        let e = classify("[repo /x] # my repo").unwrap_err();
        assert_eq!(e.message, "unexpected text after the closing ']'");
        assert_eq!((e.column, e.width), (11, 9));
        assert_eq!(
            e.hint.as_deref(),
            Some("bsk has no trailing comments; put a comment on its own line")
        );

        // Indentation shifts the column, and trailing blanks are not underlined.
        let e = classify("  [repo] extra   ").unwrap_err();
        assert_eq!(e.message, "unexpected text after the closing ']'");
        assert_eq!((e.column, e.width), (10, 5));

        let e = classify("[]x").unwrap_err();
        assert_eq!(e.message, "unexpected text after the closing ']'");
        assert_eq!((e.column, e.width), (3, 1));
    }

    #[test]
    fn text_after_the_closing_bracket_is_measured_in_characters() {
        let e = classify("[repo é] ünï").unwrap_err();
        assert_eq!(e.message, "unexpected text after the closing ']'");
        assert_eq!((e.column, e.width), (10, 3));
    }

    #[test]
    fn the_last_bracket_closes_a_header_even_when_the_name_has_brackets() {
        assert_eq!(header("[repo /x] y]"), ("repo".into(), "/x] y".into()));
        assert_eq!(header("[repo a] [b]"), ("repo".into(), "a] [b".into()));
        // The last ']' is the closing one, so what follows it is the trailing text.
        let e = classify("[repo a] b] c").unwrap_err();
        assert_eq!(e.message, "unexpected text after the closing ']'");
        assert_eq!((e.column, e.width), (13, 1));
    }

    #[test]
    fn empty_header_is_an_error() {
        assert_eq!(classify("[]").unwrap_err().message, "empty section header");
        assert_eq!(
            classify("[   ]").unwrap_err().message,
            "empty section header"
        );
    }

    #[test]
    fn header_kind_must_be_a_word() {
        let e = classify("[/etc/passwd]").unwrap_err();
        assert_eq!(e.message, "invalid section kind '/etc/passwd'");
        assert_eq!(e.column, 2);
    }

    #[test]
    fn colon_after_key_gets_a_specific_hint() {
        let e = classify("skill: git").unwrap_err();
        assert_eq!(e.message, "unexpected ':' after key 'skill'");
        assert_eq!(e.column, 6);
        assert_eq!(
            e.hint.as_deref(),
            Some("bsk lines are 'key value'; write 'skill <value>' without the ':'")
        );
        assert!(
            classify("skill=git")
                .unwrap_err()
                .hint
                .unwrap()
                .contains("without the '='")
        );
    }

    #[test]
    fn bullets_quotes_and_digits_are_explained() {
        assert_eq!(
            classify("- git").unwrap_err().message,
            "unexpected '-': bsk has no bullet lists"
        );
        assert!(
            classify("\"skill\" git")
                .unwrap_err()
                .message
                .contains("quote")
        );
        assert!(
            classify("9lives x")
                .unwrap_err()
                .message
                .contains("must start with a letter")
        );
        assert!(
            classify("= git")
                .unwrap_err()
                .message
                .contains("cannot start with '='")
        );
    }

    #[test]
    fn dash_inside_a_key_is_fine() {
        assert_eq!(
            entry("on-conflict keep"),
            ("on-conflict".into(), "keep".into())
        );
        assert_eq!(
            entry("agent_skills .agents/skills"),
            ("agent_skills".into(), ".agents/skills".into())
        );
    }

    #[test]
    fn control_characters_are_rejected_with_their_position() {
        let e = classify("skill a\u{7}b").unwrap_err();
        assert_eq!(e.message, "control character U+0007 is not allowed");
        assert_eq!(e.column, 8);
        assert!(classify("skill a\rb").is_err());
        assert!(classify("skill\ta").is_ok());
    }

    #[test]
    fn c1_control_characters_are_rejected_like_the_others() {
        for c in [
            '\u{80}', '\u{85}', '\u{9b}', '\u{9f}', '\u{7f}', '\u{0}', '\u{1f}',
        ] {
            let e = classify(&format!("skill a{c}b")).unwrap_err();
            assert_eq!(
                e.message,
                format!("control character U+{:04X} is not allowed", c as u32)
            );
            assert_eq!(e.column, 8);
            assert!(
                classify(&format!("# comment {c}")).is_err(),
                "U+{:04X}",
                c as u32
            );
            assert!(
                classify(&format!("[repo {c}]")).is_err(),
                "U+{:04X}",
                c as u32
            );
        }
        // U+00A0 is the first character after the C1 block. It is ordinary text in a value.
        assert_eq!(entry("note a\u{a0}b"), ("note".into(), "a\u{a0}b".into()));
        assert!(is_forbidden('\u{9f}'));
        assert!(!is_forbidden('\u{a0}'));
        assert!(!is_forbidden('\t'));
    }

    #[test]
    fn invisible_characters_are_named_by_code_point_in_messages() {
        assert_eq!(describe_char('a'), "'a'");
        assert_eq!(describe_char('é'), "'é'");
        assert_eq!(describe_char('✓'), "'✓'");
        assert_eq!(describe_char(':'), "':'");
        assert_eq!(describe_char(' '), "' '");
        assert_eq!(describe_char('\u{a0}'), "U+00A0");
        assert_eq!(describe_char('\u{7}'), "U+0007");
        assert_eq!(describe_char('\t'), "U+0009");
        assert_eq!(describe_char('\u{85}'), "U+0085");
        assert_eq!(describe_char('\u{ad}'), "U+00AD");
        assert_eq!(describe_char('\u{200b}'), "U+200B");
        assert_eq!(describe_char('\u{202e}'), "U+202E");
        assert_eq!(describe_char('\u{2066}'), "U+2066");
        assert_eq!(describe_char('\u{3000}'), "U+3000");
        assert_eq!(describe_char('\u{feff}'), "U+FEFF");
        assert_eq!(describe_char('\u{e0041}'), "U+E0041");
    }

    #[test]
    fn messages_that_quote_a_character_never_print_an_invisible_one() {
        let e = classify("skill\u{a0}git").unwrap_err();
        assert_eq!(e.message, "unexpected U+00A0 after key 'skill'");
        assert_eq!(e.column, 6);
        let e = classify("skill\u{200b}git").unwrap_err();
        assert_eq!(e.message, "unexpected U+200B after key 'skill'");
        let e = classify("skill;git").unwrap_err();
        assert_eq!(e.message, "unexpected ';' after key 'skill'");

        let e = classify("\u{a0}skill git").unwrap_err();
        assert_eq!(e.message, "expected a key, found U+00A0");
        let e = classify("  \u{200b}x").unwrap_err();
        assert_eq!(e.message, "expected a key, found U+200B");
        assert_eq!(e.column, 3);
        let e = classify("é: x").unwrap_err();
        assert_eq!(e.message, "expected a key, found 'é'");
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        let e = classify("é: x").unwrap_err();
        assert_eq!(e.column, 1);
        let e = classify("  ü").unwrap_err();
        assert_eq!(e.column, 3);
    }

    #[test]
    fn value_and_word_checks() {
        assert!(check_value("plain text").is_ok());
        assert!(check_value("").is_ok());
        assert_eq!(check_value("a\nb"), Err(ValueError::LineBreak));
        assert_eq!(check_value(" a"), Err(ValueError::EdgeWhitespace));
        assert_eq!(check_value("a "), Err(ValueError::EdgeWhitespace));
        assert_eq!(check_value("a\u{1}"), Err(ValueError::Control('\u{1}')));
        assert_eq!(check_value("a\u{85}"), Err(ValueError::Control('\u{85}')));
        assert_eq!(check_value("\u{9f}"), Err(ValueError::Control('\u{9f}')));
        assert_eq!(check_value("a\u{7f}b"), Err(ValueError::Control('\u{7f}')));
        assert!(check_value("a\u{a0}b").is_ok());
        assert!(check_value("tab\tinside").is_ok());
        assert!(check_word("on-conflict").is_ok());
        assert!(check_word("1x").is_err());
        assert!(check_word("").is_err());
        assert!(check_word("a b").is_err());
    }
}

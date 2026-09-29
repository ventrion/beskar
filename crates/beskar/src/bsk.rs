//! bsk — Beskar's native configuration format.
//!
//! One tiny format serves the config file, profile definitions, and the
//! registry, because all three are shallow trees of named values. The whole
//! grammar:
//!
//! ```text
//! document  := (entry | comment-line | blank)*
//! entry     := word+ [ "{" (entry | comment-line)* "}" ] newline
//! word      := bare | quoted
//! bare      := [A-Za-z0-9._/:=,+@^-]+          (never empty, no '#')
//! quoted    := '"' (escape | any char except '"' '\' newline)* '"'
//! escape    := \" \\ \n \t \r
//! ```
//!
//! Design rules, chosen so the format is unambiguous to parse and friendly
//! to edit by hand or by an LLM:
//!
//! - **Keyword-led.** The first word of an entry says what it is
//!   (`skill code-review`); the rest are values. Meaning never depends on
//!   position beyond that.
//! - **Newlines are structure.** One entry per line; `{` opens a block on
//!   the entry's line, `}` sits alone on its own line. No commas,
//!   semicolons, or indentation rules — indentation is a courtesy.
//! - **No type inference.** Everything is a word until the domain layer
//!   interprets it. No YAML-style surprises, no multi-line scalars, no
//!   anchors, no dotted keys.
//! - **Comments survive machine edits.** Whole-line and trailing comments,
//!   and blank lines ahead of them, are kept in the parse tree and
//!   rewritten verbatim, so `beskar profile add` does not mangle a
//!   hand-curated file.
//!
//! Values are quoted only when they must be (spaces, empty string, or
//! characters outside the bare set); a bare word is always preferable.

use std::fmt;
use std::path::Path;

use crate::error::{Error, Result};

/// Characters allowed in a bare (unquoted) word.
const BARE: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._/:=,+@^-~";

/// One entry: `key value... [ { children } ]` with its cosmetics.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Entry {
    /// Words of the entry; `words[0]` is the keyword. Never empty.
    pub words: Vec<String>,
    /// Line of the first word (1-based), for error messages.
    pub line: usize,
    /// Whole-line comments directly above the entry, without the leading `#`.
    pub comments: Vec<String>,
    /// Blank lines between the comment block and the entry (capped at 2).
    pub blanks: u8,
    /// Blank lines between whatever precedes and the comment block (capped
    /// at 2). Kept separately from `blanks` so hand layouts round-trip
    /// byte-exactly.
    pub comment_blanks: u8,
    /// Trailing comment on the entry's own line, without the leading `#`.
    pub trailing_comment: Option<String>,
    /// Entries inside this entry's `{ ... }` block.
    pub children: Vec<Entry>,
}

impl Entry {
    /// Shorthand for building entries in code: `Entry::of(&["version", "1"])`.
    pub fn of(words: &[&str]) -> Entry {
        Entry {
            words: words.iter().map(|w| (*w).to_string()).collect(),
            ..Entry::default()
        }
    }

    /// Like [`Entry::of`] but with a trailing comment.
    pub fn of_commented(words: &[&str], comment: &str) -> Entry {
        Entry { trailing_comment: Some(comment.to_string()), ..Entry::of(words) }
    }

    pub fn keyword(&self) -> &str {
        &self.words[0]
    }

    /// The first value word, if present.
    pub fn value(&self) -> Option<&str> {
        self.words.get(1).map(|w| w.as_str())
    }

    /// All value words after the keyword.
    pub fn values(&self) -> &[String] {
        &self.words[1.min(self.words.len())..]
    }

    pub fn is(&self, keyword: &str) -> bool {
        self.words.first().map(|w| w == keyword).unwrap_or(false)
    }
}

/// A parsed document: top-level entries plus any comments after the last one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    pub entries: Vec<Entry>,
    /// Whole-line comments appearing after the last top-level entry.
    pub trailer: Vec<String>,
}

/// A word with quoting applied only when necessary.
pub fn quote_word(word: &str) -> String {
    if !word.is_empty() && word.chars().all(|c| BARE.contains(c)) {
        return word.to_string();
    }
    let mut out = String::with_capacity(word.len() + 2);
    out.push('"');
    for c in word.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Render a document back to text. Comments and blank lines stored in the
/// tree are reproduced, so round-tripping a hand-edited file keeps it
/// looking hand-edited.
pub fn write_document(doc: &Document) -> String {
    let mut out = String::new();
    write_entries(&mut out, &doc.entries, 0, &doc.trailer);
    out
}

fn write_entries(out: &mut String, entries: &[Entry], depth: usize, trailer: &[String]) {
    let indent = "  ".repeat(depth);
    for e in entries {
        for _ in 0..e.comment_blanks {
            out.push('\n');
        }
        for c in &e.comments {
            out.push_str(&indent);
            out.push('#');
            out.push_str(c);
            out.push('\n');
        }
        for _ in 0..e.blanks {
            out.push('\n');
        }
        out.push_str(&indent);
        for (i, w) in e.words.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(&quote_word(w));
        }
        if let Some(tc) = &e.trailing_comment {
            out.push_str("  #");
            out.push_str(tc);
        }
        if !e.children.is_empty() {
            out.push_str(" {\n");
            write_entries(out, &e.children, depth + 1, &[]);
            out.push_str(&indent);
            out.push('}');
        }
        out.push('\n');
    }
    for c in trailer {
        out.push('#');
        out.push_str(c);
        out.push('\n');
    }
}

/// Parse `text` as a bsk document. `file` is used only in error messages.
pub fn parse_document(file: &Path, text: &str) -> Result<Document> {
    let mut top: Vec<Entry> = Vec::new();
    // Open blocks, innermost last. While a block is open, every completed
    // entry becomes its child.
    let mut stack: Vec<Entry> = Vec::new();
    let mut pending_comments: Vec<String> = Vec::new();
    // Blank lines before the first pending comment vs. after it: preserving
    // the former keeps common hand layouts byte-stable.
    let mut blanks_pre: u8 = 0;
    let mut blanks_post: u8 = 0;

    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            let slot = if pending_comments.is_empty() { &mut blanks_pre } else { &mut blanks_post };
            *slot = (*slot + 1).min(2);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            pending_comments.push(rest.to_string());
            continue;
        }

        let words = tokenize(file, line_no, trimmed)?;

        // `}` closes the innermost open block; it must be alone on its line.
        if words.terminated_brace {
            let entry = stack.pop().ok_or_else(|| {
                Error::parse(file, line_no, "unexpected `}` — there is no open block here")
            })?;
            if let Some(parent) = stack.last_mut() {
                parent.children.push(entry);
            } else {
                top.push(entry);
            }
            pending_comments.clear();
            blanks_pre = 0;
            blanks_post = 0;
            continue;
        }

        let entry = Entry {
            words: words.tokens,
            line: line_no,
            comments: std::mem::take(&mut pending_comments),
            blanks: blanks_post,
            comment_blanks: blanks_pre,
            trailing_comment: words.trailing_comment,
            children: Vec::new(),
        };
        blanks_pre = 0;
        blanks_post = 0;

        if words.opened_brace {
            stack.push(entry);
        } else if let Some(parent) = stack.last_mut() {
            parent.children.push(entry);
        } else {
            top.push(entry);
        }
    }

    if let Some(entry) = stack.last() {
        return Err(Error::parse(file, entry.line,
            "block opened here is never closed — missing `}`"));
    }

    // Comments after the last top-level entry are the document's trailer.
    Ok(Document { entries: top, trailer: pending_comments })
}

#[derive(Debug, Default)]
struct LineWords {
    tokens: Vec<String>,
    /// The line ends with `{` (opens a block).
    opened_brace: bool,
    /// The line is a lone `}` (closes a block).
    terminated_brace: bool,
    trailing_comment: Option<String>,
}

/// Tokenize one line. `{` is only allowed as the final token (block opener);
/// a bare `}` alone is reported via `terminated_brace`.
fn tokenize(file: &Path, line_no: usize, line: &str) -> Result<LineWords> {
    let mut out = LineWords::default();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '#' {
            out.trailing_comment = Some(chars[i + 1..].iter().collect());
            break;
        }
        if c == '{' {
            if !out.tokens.is_empty() && i + 1 == chars.len() {
                out.opened_brace = true;
                return Ok(out);
            }
            if out.tokens.is_empty() {
                return Err(Error::parse(file, line_no,
                    "`{` must open the block of an entry written on the same line"));
            }
            return Err(Error::parse(file, line_no, "`{` must be the last thing on its line"));
        }
        if c == '}' {
            if out.tokens.is_empty() && i + 1 == chars.len() {
                out.terminated_brace = true;
                return Ok(out);
            }
            return Err(Error::parse(file, line_no, "`}` must be on its own line"));
        }
        if c == '"' {
            i += 1;
            let mut s = String::new();
            let mut closed = false;
            while i < chars.len() {
                let c = chars[i];
                if c == '"' {
                    closed = true;
                    i += 1;
                    break;
                }
                if c == '\\' {
                    i += 1;
                    let Some(esc) = chars.get(i) else {
                        return Err(Error::parse(file, line_no, "string ends with a lone `\\`"));
                    };
                    match esc {
                        '"' => s.push('"'),
                        '\\' => s.push('\\'),
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        other => {
                            return Err(Error::parse(file, line_no,
                                format!("unknown escape `\\{other}` (allowed: \\\" \\\\ \\n \\t \\r)")));
                        }
                    }
                    i += 1;
                    continue;
                }
                if (c as u32) < 0x20 {
                    return Err(Error::parse(file, line_no,
                        "raw control character inside a string — use \\n, \\t or \\r"));
                }
                s.push(c);
                i += 1;
            }
            if !closed {
                return Err(Error::parse(file, line_no,
                    "quoted string is never closed — missing `\"`"));
            }
            out.tokens.push(s);
            continue;
        }
        // Bare word: maximal run of BARE characters.
        if !BARE.contains(c) {
            return Err(Error::parse(file, line_no,
                format!("unexpected character `{c}` — quote strings that contain spaces, and note that `#` starts a comment")));
        }
        let start = i;
        while i < chars.len()
            && !matches!(chars[i], '"' | '{' | '}' | '#')
            && !chars[i].is_whitespace()
        {
            if !BARE.contains(chars[i]) {
                return Err(Error::parse(file, line_no,
                    format!("invalid character `{}` in word — quote it", chars[i])));
            }
            i += 1;
        }
        // A `#`, quote, or brace glued to the end of a bare word would
        // silently change its meaning, so refuse.
        if i < chars.len() && matches!(chars[i], '"' | '{' | '}' | '#') {
            return Err(Error::parse(file, line_no,
                format!("`{}` must be separated from words by whitespace", chars[i])));
        }
        out.tokens.push(chars[start..i].iter().collect());
    }
    Ok(out)
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&write_document(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Document> {
        parse_document(Path::new("test.bsk"), text)
    }

    #[test]
    fn parses_basic_entries() {
        let doc = parse("version 1\nname coding\nskill git\n").unwrap();
        assert_eq!(doc.entries.len(), 3);
        assert!(doc.entries[0].is("version"));
        assert_eq!(doc.entries[0].value(), Some("1"));
        assert_eq!(doc.entries[1].values().len(), 1);
    }

    #[test]
    fn parses_nested_blocks() {
        let doc = parse("repo \"/x/y\" {\n  profile a\n  profile b\n  installed c {\n    fingerprint z\n  }\n}\n").unwrap();
        let repo = &doc.entries[0];
        assert_eq!(repo.value(), Some("/x/y"));
        assert_eq!(repo.children.len(), 3);
        assert_eq!(repo.children[1].value(), Some("b"));
        assert_eq!(repo.children[2].children[0].value(), Some("z"));
        assert_eq!(repo.children[2].line, 4);
    }

    #[test]
    fn preserves_comments_and_blanks() {
        let src = "# header comment\n# second line\n\nname test  # trailing\n\n\n# after blank run\nskill a\n# trailer\n";
        let doc = parse(src).unwrap();
        let rt = write_document(&doc);
        assert_eq!(rt, src);
    }

    #[test]
    fn blank_before_comment_block_is_kept() {
        let src = "a 1\n\n\n# note\nb 2\n";
        assert_eq!(write_document(&parse(src).unwrap()), src);
    }

    #[test]
    fn round_trips_quoting() {
        let src = "repo \"/home/u/My Project\" {\n  note \"line1\\nline2\\t tab \\\\ \\\"quoted\\\"\"\n}\n";
        let doc = parse(src).unwrap();
        assert_eq!(doc.entries[0].value(), Some("/home/u/My Project"));
        assert_eq!(write_document(&doc), src);
    }

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(quote_word("simple"), "simple");
        assert_eq!(quote_word("a b"), "\"a b\"");
        assert_eq!(quote_word(""), "\"\"");
        assert_eq!(quote_word("h\"i"), "\"h\\\"i\"");
        assert_eq!(quote_word("/a/b.c-d_e"), "/a/b.c-d_e");
    }

    #[test]
    fn rejects_errors_with_lines() {
        let cases = [
            ("skill a {\n", "never closed"),
            ("}\n", "no open block"),
            ("skill a b }\n", "`}` must be on its own line"),
            ("skill \"unclosed\n", "never closed"),
            ("skill \"bad \\x escape\"\n", "unknown escape"),
            ("a#comment-glued\n", "must be separated"),
            ("{ solo\n", "must open the block"),
            ("skill a{\n}\n", "must be separated"),
            ("skill \"a\\\" \n", "never closed"),
        ];
        for (src, needle) in cases {
            let err = parse(src).unwrap_err().to_string();
            assert!(err.contains(needle), "for {src:?}: {err}");
            assert!(err.contains("test.bsk:"), "error should carry file:line: {err}");
        }
    }

    #[test]
    fn trailer_comments_survive() {
        let src = "name x\n# tail comment\n";
        let doc = parse(src).unwrap();
        assert_eq!(write_document(&doc), src);
    }

    #[test]
    fn comment_only_file() {
        let src = "# just a note\n";
        assert_eq!(write_document(&parse(src).unwrap()), src);
    }

    #[test]
    fn blank_cap() {
        let doc = parse("a 1\n\n\n\n\nb 2\n").unwrap();
        assert_eq!(doc.entries[1].comment_blanks, 2);
        assert_eq!(write_document(&doc), "a 1\n\n\nb 2\n");
    }

    #[test]
    fn entry_helpers() {
        let e = Entry::of(&["version", "1"]);
        assert!(e.is("version"));
        assert_eq!(e.keyword(), "version");
        assert_eq!(e.value(), Some("1"));
        let solo = Entry::of(&["justakey"]);
        assert_eq!(solo.value(), None);
        let c = Entry::of_commented(&["skill", "git"], " why");
        assert_eq!(c.trailing_comment.as_deref(), Some(" why"));
    }
}

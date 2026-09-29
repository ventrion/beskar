//! A forgiving reader for the metadata block at the top of a `SKILL.md`.
//!
//! Skills describe themselves in a YAML block between two `---` lines. Beskar
//! needs only the top-level keys, so this reads the shapes that appear in
//! practice and skips the rest:
//!
//! * plain scalars, which may continue on indented lines;
//! * single and double quoted scalars, which may also span lines;
//! * block scalars (`|` and `>`);
//! * block lists and flow lists such as `[a, b]`.
//!
//! It never fails. A file without a metadata block yields `None`, and a nested
//! mapping is skipped. Shapes it does not read come out as plain text or are
//! cut short:
//!
//! * anchors, tags and flow mappings stay in the text as written;
//! * a flow list that spans lines is plain text;
//! * a quoted scalar continues only on indented lines, and text after its
//!   closing quote is dropped;
//! * a block list item has to fit on one line.
//!
//! Skill files can come from anywhere and their names and descriptions are
//! printed to a terminal, so [`Value::as_line`] replaces control characters.

/// The value of one metadata key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// A single piece of text.
    Text(String),
    /// A list of texts.
    List(Vec<String>),
}

impl Value {
    /// The value on one line, safe to print.
    ///
    /// Control characters become U+FFFD, runs of whitespace (tabs and line
    /// breaks included) collapse to one space, and lists are joined with
    /// commas.
    pub fn as_line(&self) -> String {
        match self {
            Value::Text(text) => one_line(text),
            Value::List(items) => items
                .iter()
                .map(|item| one_line(item))
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

/// Replaces control characters with U+FFFD, then collapses whitespace.
///
/// Tab and line feed are whitespace and collapse into spaces, and so does the
/// carriage return of a CRLF line break. Every other control character is
/// replaced: NUL, ESC, DEL, a bare carriage return and the C1 range U+0080 to
/// U+009F. Without that, text from an untrusted file could carry terminal
/// escape sequences.
fn one_line(text: &str) -> String {
    let mut printable = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let harmless = !c.is_control()
            || matches!(c, '\t' | '\n')
            || (c == '\r' && chars.peek() == Some(&'\n'));
        printable.push(if harmless { c } else { '\u{fffd}' });
    }
    printable.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The top-level metadata of a skill file, in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frontmatter {
    entries: Vec<(String, Value)>,
}

impl Frontmatter {
    /// Reads the metadata block of a Markdown file, if it has one.
    pub fn parse(markdown: &str) -> Option<Frontmatter> {
        let text = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
        let mut lines = text.lines();
        if lines.next()?.trim_end() != "---" {
            return None;
        }
        let mut block = Vec::new();
        let mut closed = false;
        for line in lines {
            if matches!(line.trim_end(), "---" | "...") {
                closed = true;
                break;
            }
            block.push(line);
        }
        closed.then(|| Frontmatter {
            entries: parse_block(&block),
        })
    }

    /// The text of a key on one line, as [`Value::as_line`] gives it.
    pub fn text(&self, key: &str) -> Option<String> {
        self.get(key).map(Value::as_line)
    }

    /// The value of a key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Every key and value, in file order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

/// Space and tab, the only whitespace YAML allows inside a line.
const BLANKS: [char; 2] = [' ', '\t'];

/// A line with nothing on it but spaces and tabs.
fn is_blank(line: &str) -> bool {
    line.trim_matches(BLANKS).is_empty()
}

/// A line that starts with a space or a tab. YAML indents with spaces only,
/// but a tab-indented line is still read as a continuation, not as a new key.
fn is_indented(line: &str) -> bool {
    line.starts_with(BLANKS)
}

/// A line that holds nothing but a comment.
fn is_comment(line: &str) -> bool {
    line.trim_start_matches(BLANKS).starts_with('#')
}

/// How many lines at the start of `lines` are blank or indented.
fn indented_run(lines: &[&str]) -> usize {
    lines
        .iter()
        .take_while(|line| is_blank(line) || is_indented(line))
        .count()
}

fn parse_block(block: &[&str]) -> Vec<(String, Value)> {
    let mut entries = Vec::new();
    for (at, line) in block.iter().enumerate() {
        if is_blank(line) || line.starts_with([' ', '\t', '#', '-']) {
            continue;
        }
        let Some((key, rest)) = split_key(line) else {
            continue;
        };
        // Lines that belong to a value are indented or are list items, so this
        // loop skips them without being told how many a value used.
        let following = block.get(at + 1..).unwrap_or_default();
        if let Some(value) = read_value(rest, following) {
            entries.push((key.to_string(), value));
        }
    }
    entries
}

fn split_key(line: &str) -> Option<(&str, &str)> {
    let (key, rest) = line.split_once(':')?;
    let valid_key = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (valid_key && (rest.is_empty() || rest.starts_with(BLANKS))).then_some((key, rest))
}

/// The value of a key. `rest` is the text after the colon and `following` the
/// lines below the key.
fn read_value(rest: &str, following: &[&str]) -> Option<Value> {
    let rest = rest.trim_start_matches(BLANKS);
    if rest.trim_end_matches(BLANKS).is_empty() || rest.starts_with('#') {
        return value_below(following);
    }
    if let Some(folded) = block_scalar_header(rest) {
        return Some(block_scalar(folded, following));
    }
    Some(inline_value(rest, following))
}

/// The value of a key that has nothing after its colon. It starts on the next
/// line with content: a block list, a quoted or plain scalar, or a nested
/// mapping, which is skipped.
fn value_below(following: &[&str]) -> Option<Value> {
    let start = following
        .iter()
        .position(|line| !is_blank(line) && !is_comment(line))?;
    let lines = following.get(start..)?;
    let (first, after) = lines.split_first()?;
    if list_item(first).is_some() {
        return Some(block_list(lines));
    }
    if !is_indented(first) {
        return None;
    }
    let text = first.trim_start_matches(BLANKS);
    if is_mapping_line(text) {
        return None;
    }
    Some(inline_value(text, after))
}

/// Whether `text` reads as `key: value` or `"key": value`.
fn is_mapping_line(text: &str) -> bool {
    split_key(text).is_some()
        || (text.starts_with(['"', '\''])
            && quoted_scalar(text, &[]).is_some_and(|quoted| starts_mapping_value(quoted.after)))
}

/// Whether `after` begins with the colon that follows a quoted key.
fn starts_mapping_value(after: &str) -> bool {
    after
        .trim_start_matches(BLANKS)
        .strip_prefix(':')
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(BLANKS))
}

/// A value that starts with `rest`: a quoted scalar, a flow list or a plain
/// scalar. `following` holds the lines below, for a value that continues.
fn inline_value(rest: &str, following: &[&str]) -> Value {
    if rest.starts_with(['"', '\''])
        && let Some(quoted) = quoted_scalar(rest, following)
    {
        return Value::Text(quoted.text);
    }
    if rest.starts_with('[')
        && let Some(items) = flow_list(rest)
    {
        return Value::List(items);
    }
    plain_scalar(rest, following)
}

/// A plain scalar: `first`, then the indented lines below it. A line break
/// folds to a space, and each blank line in between becomes a newline.
fn plain_scalar(first: &str, following: &[&str]) -> Value {
    let mut text = strip_comment(first).to_string();
    let mut blank_lines = 0;
    for line in following {
        if is_blank(line) {
            blank_lines += 1;
        } else if !is_indented(line) {
            break;
        } else if !is_comment(line) {
            if blank_lines > 0 {
                text.push_str(&"\n".repeat(blank_lines));
            } else {
                text.push(' ');
            }
            blank_lines = 0;
            text.push_str(strip_comment(line.trim_start_matches(BLANKS)));
        }
    }
    Value::Text(text)
}

/// Cuts a trailing comment from a plain scalar. A `#` starts a comment only
/// after a space or a tab, so `C#` stays whole.
fn strip_comment(text: &str) -> &str {
    let blank_then_hash = text
        .as_bytes()
        .windows(2)
        .position(|pair| matches!(pair, [b' ' | b'\t', b'#']));
    let end = blank_then_hash.map_or(text.len(), |at| at + 1);
    text.get(..end).unwrap_or(text).trim_end_matches(BLANKS)
}

/// The text after the dash of a list item line, if the line is one.
fn list_item(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(BLANKS).strip_prefix('-')?;
    (rest.is_empty() || rest.starts_with(BLANKS)).then_some(rest)
}

/// The items of a block list. `lines` starts with the first item. Lines that
/// continue an item, such as a nested mapping, are skipped.
fn block_list(lines: &[&str]) -> Value {
    let mut items = Vec::new();
    for line in lines {
        if let Some(item) = list_item(line) {
            items.push(item_text(item));
        } else if !(is_blank(line) || is_comment(line) || is_indented(line)) {
            break;
        }
    }
    Value::List(items)
}

/// The text of one block list item: quoted, or plain with any trailing comment
/// removed. It has to fit on one line.
fn item_text(text: &str) -> String {
    let text = text.trim_start_matches(BLANKS);
    if text.starts_with('#') {
        return String::new();
    }
    if text.starts_with(['"', '\''])
        && let Some(quoted) = quoted_scalar(text, &[])
    {
        return quoted.text;
    }
    strip_comment(text).to_string()
}

/// Whether `text` is a block scalar header such as `|`, `>-` or `|2+ # note`.
/// Returns `Some(true)` for the folded style `>` and `Some(false)` for `|`.
fn block_scalar_header(text: &str) -> Option<bool> {
    let mut chars = text.chars();
    let style = chars.next().filter(|c| matches!(c, '|' | '>'))?;
    let mut tail = chars.as_str();
    let (mut has_chomping, mut has_indent) = (false, false);
    while let Some(c) = tail.chars().next() {
        match c {
            '+' | '-' if !has_chomping => has_chomping = true,
            '1'..='9' if !has_indent => has_indent = true,
            _ => break,
        }
        tail = tail.get(1..)?;
    }
    let after = tail.trim_start_matches(BLANKS);
    let valid = after.is_empty() || (after.starts_with('#') && tail.starts_with(BLANKS));
    valid.then_some(style == '>')
}

/// The lines under a `|` or `>` header, without their common indentation.
/// Chomping and indentation indicators are not applied; the text is trimmed.
fn block_scalar(folded: bool, following: &[&str]) -> Value {
    let body = following.get(..indented_run(following)).unwrap_or_default();
    // Only spaces indent, so count exactly those. A tab or a no-break space is
    // content and must not be measured as indentation.
    let indent = body
        .iter()
        .filter(|line| !is_blank(line))
        .map(|line| line.len() - line.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    let lines: Vec<&str> = body
        .iter()
        .map(|line| line.get(indent..).unwrap_or("").trim_end_matches(BLANKS))
        .collect();
    let text = if folded {
        fold(&lines)
    } else {
        lines.join("\n")
    };
    Value::Text(text.trim().to_string())
}

fn fold(lines: &[&str]) -> String {
    let mut out = String::new();
    let mut previous_blank = false;
    for line in lines {
        if line.is_empty() {
            out.push('\n');
            previous_blank = true;
        } else {
            if !out.is_empty() && !previous_blank {
                out.push(' ');
            }
            out.push_str(line);
            previous_blank = false;
        }
    }
    out
}

/// A flow list such as `[a, "b, c"]`. It is a list only when the bracket that
/// opens it is closed by the last character of the value, not counting a
/// trailing comment. `[Draft] see [x]` is plain text.
fn flow_list(text: &str) -> Option<Vec<String>> {
    let (items, tail) = flow_sequence(text)?;
    let after = tail.trim_start_matches(BLANKS);
    let ends_here = after.is_empty() || (after.starts_with('#') && tail.starts_with(BLANKS));
    ends_here.then_some(items)
}

/// Reads the items of the flow list at the start of `text`, which begins with
/// `[`. Returns them with the text after the closing bracket, or `None` if the
/// bracket is never closed. Brackets inside quotes do not count, and a nested
/// list is kept as text.
fn flow_sequence(text: &str) -> Option<(Vec<String>, &str)> {
    let mut rest = text.strip_prefix('[')?;
    let mut items = Vec::new();
    loop {
        rest = rest.trim_start_matches(BLANKS);
        let first = rest.chars().next()?;
        if first == ']' {
            return Some((items, rest.get(1..)?));
        }
        if first == ',' {
            rest = rest.get(1..)?;
            continue;
        }
        if matches!(first, '"' | '\'')
            && let Some(quoted) = quoted_scalar(rest, &[])
        {
            items.push(quoted.text);
            // Whatever sits between the closing quote and the next separator
            // is ignored.
            let next = quoted.after.find([',', ']'])?;
            rest = quoted.after.get(next..)?;
            continue;
        }
        let end = plain_item_end(rest)?;
        items.push(rest.get(..end)?.trim_end_matches(BLANKS).to_string());
        rest = rest.get(end..)?;
    }
}

/// Where a plain flow list item ends: at the first comma or closing bracket
/// that is not inside a nested `[]` or `{}`.
fn plain_item_end(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (at, c) in text.char_indices() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' if depth > 0 => depth -= 1,
            ',' | ']' if depth == 0 => return Some(at),
            _ => {}
        }
    }
    None
}

/// A quoted scalar, and what follows its closing quote.
struct Quoted<'a> {
    text: String,
    after: &'a str,
}

/// How a line of a quoted scalar ended.
enum LineEnd<'a> {
    /// The closing quote came first. Holds the text after it.
    Closed(&'a str),
    /// The line ended inside the scalar.
    Open,
    /// The line ended in a backslash, which joins it to the next line.
    Escaped,
}

/// Reads a quoted scalar that starts with the quote at the start of `first`
/// and may continue on the indented lines in `following`. A line break inside
/// the quotes folds to a space, a blank line becomes a newline, and the
/// indentation of continuation lines is dropped. Returns `None` when the
/// closing quote is missing.
fn quoted_scalar<'a>(first: &'a str, following: &[&'a str]) -> Option<Quoted<'a>> {
    let quote = first.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let mut line = first.get(1..)?;
    let mut text = String::new();
    // The part of `text` that folding may not trim: escapes such as `\t` or
    // `\ ` produce whitespace that belongs to the text.
    let mut fixed = 0;
    let mut next_line = 0;
    loop {
        let end = if quote == '"' {
            scan_double_quoted(line, &mut text, &mut fixed)
        } else {
            scan_single_quoted(line, &mut text)
        };
        let escaped = match end {
            LineEnd::Closed(after) => return Some(Quoted { text, after }),
            LineEnd::Escaped => true,
            LineEnd::Open => false,
        };
        if !escaped {
            while text.len() > fixed && text.ends_with(BLANKS) {
                text.pop();
            }
        }
        let mut blank_lines = 0;
        line = loop {
            let candidate = *following.get(next_line)?;
            next_line += 1;
            if is_blank(candidate) {
                blank_lines += 1;
            } else if is_indented(candidate) {
                break candidate.trim_start_matches(BLANKS);
            } else {
                return None;
            }
        };
        if blank_lines > 0 {
            text.push_str(&"\n".repeat(blank_lines));
        } else if !escaped {
            text.push(' ');
        }
    }
}

/// Adds the text of one line of a single quoted scalar to `out`. The only
/// escape is a doubled quote.
fn scan_single_quoted<'a>(line: &'a str, out: &mut String) -> LineEnd<'a> {
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\'' {
            out.push(c);
        } else if let Some(rest) = chars.as_str().strip_prefix('\'') {
            out.push('\'');
            chars = rest.chars();
        } else {
            return LineEnd::Closed(chars.as_str());
        }
    }
    LineEnd::Open
}

/// Adds the text of one line of a double quoted scalar to `out`, decoding
/// escapes. An escape YAML does not define is kept as written. `fixed` is set
/// to the length of `out` after each decoded escape.
fn scan_double_quoted<'a>(line: &'a str, out: &mut String, fixed: &mut usize) -> LineEnd<'a> {
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return LineEnd::Closed(chars.as_str()),
            '\\' => {
                let rest = chars.as_str();
                if rest.is_empty() {
                    return LineEnd::Escaped;
                }
                if let Some((decoded, used)) = escape(rest) {
                    out.push(decoded);
                    *fixed = out.len();
                    chars = rest.get(used..).unwrap_or_default().chars();
                } else {
                    // Keep the backslash. The next character is read as
                    // ordinary text.
                    out.push('\\');
                }
            }
            _ => out.push(c),
        }
    }
    LineEnd::Open
}

/// Decodes the escape that follows a backslash in a double quoted scalar.
/// Returns the character and how many bytes of `rest` the escape covers, or
/// `None` if it is not a valid escape.
fn escape(rest: &str) -> Option<(char, usize)> {
    let kind = rest.chars().next()?;
    let simple = match kind {
        '0' => Some('\0'),
        'a' => Some('\u{7}'),
        'b' => Some('\u{8}'),
        't' | '\t' => Some('\t'),
        'n' => Some('\n'),
        'v' => Some('\u{b}'),
        'f' => Some('\u{c}'),
        'r' => Some('\r'),
        'e' => Some('\u{1b}'),
        ' ' | '"' | '/' | '\\' => Some(kind),
        'N' => Some('\u{85}'),
        '_' => Some('\u{a0}'),
        'L' => Some('\u{2028}'),
        'P' => Some('\u{2029}'),
        _ => None,
    };
    if let Some(c) = simple {
        return Some((c, kind.len_utf8()));
    }
    let digits = match kind {
        'x' => 2,
        'u' => 4,
        'U' => 8,
        _ => return None,
    };
    let after = rest.get(1..)?;
    let code = hex_value(after, digits)?;
    if kind == 'u' && (0xD800..0xDC00).contains(&code) {
        // JSON writes a character above U+FFFF as two \u escapes.
        let low = after
            .get(digits..)?
            .strip_prefix("\\u")
            .and_then(|pair| hex_value(pair, 4))?;
        if !(0xDC00..0xE000).contains(&low) {
            return None;
        }
        let combined = 0x1_0000 + ((code - 0xD800) << 10) + (low - 0xDC00);
        return Some((char::from_u32(combined)?, 1 + digits + 2 + 4));
    }
    Some((char::from_u32(code)?, 1 + digits))
}

/// The number written by the first `width` characters of `text`, if they are
/// all hexadecimal digits.
fn hex_value(text: &str, width: usize) -> Option<u32> {
    let digits = text.get(..width)?;
    if digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        u32::from_str_radix(digits, 16).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn parse(text: &str) -> Frontmatter {
        Frontmatter::parse(text).expect("frontmatter")
    }

    /// Reads a metadata block that is written without its `---` lines.
    fn block(lines: &str) -> Frontmatter {
        parse(&format!("---\n{lines}\n---\n"))
    }

    /// The text of `key` in a metadata block, on one line.
    fn line_of(lines: &str, key: &str) -> Option<String> {
        block(lines).text(key)
    }

    /// The text of `key` in a metadata block, before whitespace is collapsed.
    fn raw_of(lines: &str, key: &str) -> Option<String> {
        match block(lines).get(key) {
            Some(Value::Text(text)) => Some(text.clone()),
            _ => None,
        }
    }

    #[test]
    fn reads_the_common_shape() {
        let fm = parse(
            "---\nname: pdf\ndescription: Extract text from PDF files\nlicense: MIT\n---\n# Body\n",
        );
        assert_eq!(fm.text("name").as_deref(), Some("pdf"));
        assert_eq!(
            fm.text("description").as_deref(),
            Some("Extract text from PDF files")
        );
        assert_eq!(fm.text("license").as_deref(), Some("MIT"));
        assert_eq!(fm.text("missing"), None);
    }

    #[test]
    fn returns_none_without_a_block() {
        assert!(Frontmatter::parse("# Just a heading\n").is_none());
        assert!(Frontmatter::parse("").is_none());
        assert!(Frontmatter::parse("---\nname: x\nnever closed\n").is_none());
    }

    #[test]
    fn handles_byte_order_marks_and_crlf() {
        let fm = parse("\u{feff}---\r\nname: x\r\n---\r\n");
        assert_eq!(fm.text("name").as_deref(), Some("x"));
    }

    #[test]
    fn unquotes_scalars() {
        let fm = parse(
            "---\na: \"double \\\"q\\\" and \\\\\"\nb: 'it''s'\nc: \"with: colon\"\nd: plain: still plain\n---\n",
        );
        assert_eq!(fm.text("a").as_deref(), Some("double \"q\" and \\"));
        assert_eq!(fm.text("b").as_deref(), Some("it's"));
        assert_eq!(fm.text("c").as_deref(), Some("with: colon"));
        assert_eq!(fm.text("d").as_deref(), Some("plain: still plain"));
    }

    #[test]
    fn strips_trailing_comments_from_plain_values_only() {
        let fm = parse("---\na: value # note\nb: \"quoted # kept\"\nc: C#\n---\n");
        assert_eq!(fm.text("a").as_deref(), Some("value"));
        assert_eq!(fm.text("b").as_deref(), Some("quoted # kept"));
        assert_eq!(fm.text("c").as_deref(), Some("C#"));
    }

    #[test]
    fn joins_plain_scalars_that_continue_on_indented_lines() {
        let fm =
            parse("---\ndescription: First part\n  second part\n    third part\nnext: x\n---\n");
        assert_eq!(
            fm.text("description").as_deref(),
            Some("First part second part third part")
        );
        assert_eq!(fm.text("next").as_deref(), Some("x"));
    }

    #[test]
    fn folds_and_keeps_block_scalars() {
        let fm = parse(
            "---\nfolded: >\n  one\n  two\n\n  three\nliteral: |-\n  a\n   b\nafter: y\n---\n",
        );
        assert_eq!(fm.text("folded").as_deref(), Some("one two three"));
        assert_eq!(fm.get("literal"), Some(&Value::Text("a\n b".to_string())));
        assert_eq!(fm.text("after").as_deref(), Some("y"));
    }

    #[test]
    fn reads_block_and_flow_lists() {
        let fm = parse(
            "---\ntags:\n  - browser\n  - \"testing\"\nother: [a, 'b c', d]\nempty: []\ndeps:\n- x\n- y\n---\n",
        );
        assert_eq!(
            fm.get("tags"),
            Some(&Value::List(vec!["browser".into(), "testing".into()]))
        );
        assert_eq!(
            fm.get("other"),
            Some(&Value::List(vec!["a".into(), "b c".into(), "d".into()]))
        );
        assert_eq!(fm.get("empty"), Some(&Value::List(vec![])));
        assert_eq!(
            fm.get("deps"),
            Some(&Value::List(vec!["x".into(), "y".into()]))
        );
        assert_eq!(fm.text("tags").as_deref(), Some("browser, testing"));
    }

    #[test]
    fn skips_nested_mappings_without_losing_the_keys_after_them() {
        let fm = parse(
            "---\nname: x\nmetadata:\n  author: me\n  version: \"1.0\"\nallowed-tools: Bash(git:*) Read\n---\n",
        );
        assert!(fm.get("metadata").is_none());
        assert_eq!(
            fm.text("allowed-tools").as_deref(),
            Some("Bash(git:*) Read")
        );
        let keys: Vec<&str> = fm.entries().map(|(k, _)| k).collect();
        assert_eq!(keys, ["name", "allowed-tools"]);
    }

    #[test]
    fn ignores_comments_and_junk_lines() {
        let fm = parse("---\n# comment\n\nname: x\nthis line has no key\n---\n");
        assert_eq!(fm.entries().count(), 1);
    }

    #[test]
    fn as_line_collapses_whitespace() {
        assert_eq!(Value::Text("a\n b   c".into()).as_line(), "a b c");
    }

    #[test]
    fn keeps_url_like_values_and_keys_with_dots_and_dashes() {
        let fm = block("homepage: https://x.y/z\nallowed-tools: Read\nmetadata.owner_id: 7");
        assert_eq!(fm.text("homepage").as_deref(), Some("https://x.y/z"));
        assert_eq!(fm.text("allowed-tools").as_deref(), Some("Read"));
        assert_eq!(fm.text("metadata.owner_id").as_deref(), Some("7"));
    }

    #[test]
    fn keeps_block_scalar_indicators_and_unindented_list_items() {
        let fm = block("a: |-\n  one\n  two\nb: >+2\n  three\n  four\nc:\n- x\n- y\nd: z");
        assert_eq!(
            raw_of("a: |-\n  one\n  two", "a").as_deref(),
            Some("one\ntwo")
        );
        assert_eq!(fm.text("a").as_deref(), Some("one two"));
        assert_eq!(fm.text("b").as_deref(), Some("three four"));
        assert_eq!(fm.text("c").as_deref(), Some("x, y"));
        assert_eq!(fm.text("d").as_deref(), Some("z"));
    }

    #[test]
    fn crlf_and_byte_order_marks_work_with_quoted_lines_that_continue() {
        let fm = parse("\u{feff}---\r\nname: \"a\r\n  b\"\r\ntags: [x, y]\r\n---\r\n");
        assert_eq!(fm.text("name").as_deref(), Some("a b"));
        assert_eq!(fm.text("tags").as_deref(), Some("x, y"));
    }

    // Indentation is measured in spaces, so odd whitespace cannot break the slicing.

    #[test]
    fn a_no_break_space_in_block_scalar_indentation_does_not_panic() {
        // U+00A0 is two bytes. `trim_start` strips it, so the old indent was
        // counted in bytes that did not line up with the slice.
        let fm = parse("---\nname: x\ndescription: |\n  a\n \u{a0}b\n---\n");
        assert_eq!(fm.text("name").as_deref(), Some("x"));
        assert_eq!(fm.text("description").as_deref(), Some("a b"));
    }

    #[test]
    fn block_scalar_indentation_counts_spaces_only() {
        for space in ['\u{a0}', '\u{2003}', '\u{3000}', '\u{feff}', '\t'] {
            let folded = format!("d: >\n  one\n {space}two\n  three\nafter: y");
            let literal = format!("d: |\n {space}one\n  two\n   {space}three\nafter: y");
            for text in [folded, literal] {
                let fm = block(&text);
                assert_eq!(fm.text("after").as_deref(), Some("y"), "{text:?}");
                let words = fm.text("d").unwrap_or_default();
                assert!(
                    ["one", "two", "three"].iter().all(|w| words.contains(w)),
                    "{text:?} gave {words:?}"
                );
            }
        }
    }

    #[test]
    fn a_line_of_no_break_spaces_ends_a_block_scalar() {
        let fm = block("d: |\n  a\n\u{a0}\nnext: y");
        assert_eq!(fm.text("d").as_deref(), Some("a"));
        assert_eq!(fm.text("next").as_deref(), Some("y"));
    }

    /// A small xorshift generator, so that a failing input can be reproduced.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, limit: usize) -> usize {
            (self.next() % limit as u64) as usize
        }
    }

    /// The pieces the random test glues together: the characters that steer
    /// the reader, some that are not ASCII, and a few whole tokens.
    #[rustfmt::skip]
    const PIECES: &[&str] = &[
        "-", ":", "|", ">", "\"", "'", "[", "]", "#", "\\", " ", "\t", "\n", "\u{a0}", "\u{e9}",
        "\u{2028}", "\u{85}", "a", "b", "x", "1", ",", "name: ", "description: ", "key:\n  ",
        "\n  ", "\n- ", "- ", ": ", "  ", "|-", ">+2", "\\u", "\\x4", "\\ud83d", "\u{1b}", "---",
        "...",
    ];

    fn random_document(rng: &mut Rng) -> String {
        let mut block = String::new();
        for _ in 0..rng.below(40) {
            block.push_str(PIECES[rng.below(PIECES.len())]);
        }
        format!("---\n{block}\n---\n")
    }

    /// Parses `document` and checks what the callers rely on.
    fn check_document(document: &str) {
        let fm = Frontmatter::parse(document).expect("the block is closed");
        for (key, value) in fm.entries() {
            assert!(!key.is_empty());
            let line = value.as_line();
            assert!(
                !line.chars().any(char::is_control),
                "control character in {line:?}"
            );
            assert!(
                line.chars().all(|c| c == ' ' || !c.is_whitespace()) && !line.contains("  "),
                "whitespace not collapsed in {line:?}"
            );
            // An empty list item can leave a space at the end of a list.
            if matches!(value, Value::Text(_)) {
                assert_eq!(line, line.trim(), "whitespace at the ends of {line:?}");
            }
        }
    }

    #[test]
    fn random_input_never_panics_or_hangs() {
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
            for _ in 0..30_000 {
                let document = random_document(&mut rng);
                if sender.send(Some(document.clone())).is_err() {
                    return;
                }
                check_document(&document);
            }
            let _ = sender.send(None);
        });
        let mut current = String::new();
        loop {
            match receiver.recv_timeout(Duration::from_secs(30)) {
                Ok(Some(document)) => current = document,
                Ok(None) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("hung on {current:?}"),
                Err(mpsc::RecvTimeoutError::Disconnected) => panic!("failed on {current:?}"),
            }
        }
        worker.join().expect("the worker finished");
    }

    // Quoted scalars that span lines.

    #[test]
    fn double_quoted_scalars_span_lines() {
        assert_eq!(
            raw_of("description: \"a\n  b\"", "description").as_deref(),
            Some("a b")
        );
        assert_eq!(
            raw_of("d: \"one\n      two\n   three\"\nnext: y", "d").as_deref(),
            Some("one two three")
        );
        // A blank line becomes a newline instead of a space.
        assert_eq!(raw_of("d: \"a\n\n  b\"", "d").as_deref(), Some("a\nb"));
        assert_eq!(raw_of("d: \"a\n\n\n  b\"", "d").as_deref(), Some("a\n\nb"));
        // Whitespace before a line break goes. Whitespace at the ends stays.
        assert_eq!(
            raw_of("d: \"  a   \n   b  \"", "d").as_deref(),
            Some("  a b  ")
        );
        assert_eq!(
            line_of("d: \"a\n  b\"\nnext: y", "next").as_deref(),
            Some("y")
        );
    }

    #[test]
    fn single_quoted_scalars_span_lines() {
        assert_eq!(
            raw_of("d: 'it''s\n  fine'", "d").as_deref(),
            Some("it's fine")
        );
        assert_eq!(raw_of("d: 'a\n\n  b'", "d").as_deref(), Some("a\nb"));
        assert_eq!(
            line_of("d: 'a\n  b'\nnext: y", "next").as_deref(),
            Some("y")
        );
    }

    #[test]
    fn an_unterminated_quote_falls_back_to_plain_text() {
        let fm = block("d: \"abc\n  def\nnext: y");
        assert_eq!(fm.text("d").as_deref(), Some("\"abc def"));
        assert_eq!(fm.text("next").as_deref(), Some("y"));
        let fm = block("d: 'abc\nnext: y");
        assert_eq!(fm.text("d").as_deref(), Some("'abc"));
        assert_eq!(fm.text("next").as_deref(), Some("y"));
    }

    #[test]
    fn a_quote_does_not_reach_across_an_unindented_line() {
        let fm = block("d: \"abc\nnext: \"y\"");
        assert_eq!(fm.text("d").as_deref(), Some("\"abc"));
        assert_eq!(fm.text("next").as_deref(), Some("y"));
    }

    // A value that starts on the line after its key.

    #[test]
    fn reads_a_quoted_value_that_starts_on_the_next_line() {
        let fm = block("name: x\ndescription:\n  \"some long\n   quoted text\"\nlicense: MIT");
        assert_eq!(fm.text("name").as_deref(), Some("x"));
        assert_eq!(
            fm.text("description").as_deref(),
            Some("some long quoted text")
        );
        assert_eq!(fm.text("license").as_deref(), Some("MIT"));
        assert_eq!(
            line_of("d:\n  'one\n   two'\nnext: y", "d").as_deref(),
            Some("one two")
        );
        assert_eq!(
            line_of("d:\n\n  # note\n  \"q\"", "d").as_deref(),
            Some("q")
        );
    }

    #[test]
    fn reads_a_plain_value_that_starts_on_the_next_line() {
        let fm = block("description:\n  first part\n    second part\n  third\nlicense: MIT");
        assert_eq!(
            fm.text("description").as_deref(),
            Some("first part second part third")
        );
        assert_eq!(fm.text("license").as_deref(), Some("MIT"));
        // Text with a colon and a space is a value unless it looks like a key.
        assert_eq!(
            line_of("d:\n  Use when: the user asks", "d").as_deref(),
            Some("Use when: the user asks")
        );
        assert_eq!(
            line_of("d:\n  https://x.y/z is the guide", "d").as_deref(),
            Some("https://x.y/z is the guide")
        );
        assert_eq!(
            line_of("d:\n  --verbose\n  --quiet", "d").as_deref(),
            Some("--verbose --quiet")
        );
    }

    #[test]
    fn a_value_below_its_key_can_be_a_list_or_a_nested_mapping() {
        let fm = block(
            "tags:\n  - a\n  - b\nmetadata:\n  author: me\n  tags:\n    - x\n  \"quoted key\": v\nname: n\nempty:\nlast: l",
        );
        assert_eq!(
            fm.get("tags"),
            Some(&Value::List(vec!["a".into(), "b".into()]))
        );
        assert!(fm.get("metadata").is_none());
        assert_eq!(fm.text("name").as_deref(), Some("n"));
        assert!(fm.get("empty").is_none());
        assert_eq!(fm.text("last").as_deref(), Some("l"));
        let keys: Vec<&str> = fm.entries().map(|(k, _)| k).collect();
        assert_eq!(keys, ["tags", "name", "last"]);
        // A quoted key starts a mapping, not a quoted value.
        let fm = block("metadata:\n  \"quoted key\": v\nname: n");
        assert!(fm.get("metadata").is_none());
        assert_eq!(fm.text("name").as_deref(), Some("n"));
    }

    #[test]
    fn a_comment_after_the_colon_leaves_the_value_to_the_next_line() {
        assert_eq!(
            line_of("d: # note\n  text\nnext: y", "d").as_deref(),
            Some("text")
        );
    }

    // Text after the closing quote of a quoted scalar.

    #[test]
    fn ignores_a_comment_after_a_quoted_scalar() {
        let fm = block(
            "a: \"x\" # c\nb: 'y' # c\nc: \"p # q\" # c\nd: \"z\"\ne: \"x\" tail\nf: 'y' \"z\"\ng: \"multi\n  line\" # c",
        );
        assert_eq!(fm.text("a").as_deref(), Some("x"));
        assert_eq!(fm.text("b").as_deref(), Some("y"));
        assert_eq!(fm.text("c").as_deref(), Some("p # q"));
        assert_eq!(fm.text("d").as_deref(), Some("z"));
        assert_eq!(fm.text("e").as_deref(), Some("x"));
        assert_eq!(fm.text("f").as_deref(), Some("y"));
        assert_eq!(fm.text("g").as_deref(), Some("multi line"));
    }

    #[test]
    fn list_items_may_be_quoted_and_carry_comments() {
        let fm = block("tags:\n  - \"a\" # c\n  - b # c\n  - 'c d'\n  - C#");
        assert_eq!(
            fm.get("tags"),
            Some(&Value::List(vec![
                "a".into(),
                "b".into(),
                "c d".into(),
                "C#".into()
            ]))
        );
    }

    // Escapes in double quoted scalars.

    #[test]
    fn decodes_double_quoted_escapes() {
        let cases = [
            (r"\0", "\0"),
            (r"\a", "\u{7}"),
            (r"\b", "\u{8}"),
            (r"\t", "\t"),
            ("\\\t", "\t"),
            (r"\n", "\n"),
            (r"\v", "\u{b}"),
            (r"\f", "\u{c}"),
            (r"\r", "\r"),
            (r"\e", "\u{1b}"),
            (r"\ ", " "),
            (r#"\""#, "\""),
            (r"\/", "/"),
            (r"\\", "\\"),
            (r"\N", "\u{85}"),
            (r"\_", "\u{a0}"),
            (r"\L", "\u{2028}"),
            (r"\P", "\u{2029}"),
            (r"\x41", "A"),
            (r"\xe9", "\u{e9}"),
            (r"\u00E9", "\u{e9}"),
            (r"\u263a", "\u{263a}"),
            (r"\U0001F600", "\u{1f600}"),
        ];
        for (written, decoded) in cases {
            let source = format!("d: \"<{written}>\"");
            assert_eq!(
                raw_of(&source, "d"),
                Some(format!("<{decoded}>")),
                "{source}"
            );
        }
    }

    #[test]
    fn combines_json_surrogate_pairs() {
        assert_eq!(
            raw_of(r#"d: "\uD83D\uDE00""#, "d").as_deref(),
            Some("\u{1f600}")
        );
        assert_eq!(
            raw_of(r#"d: "a\ud83d\ude00b""#, "d").as_deref(),
            Some("a\u{1f600}b")
        );
    }

    #[test]
    fn keeps_invalid_escapes_as_written() {
        let written = [
            r"\q",
            r"\xZZ",
            r"\x4",
            r"\u12",
            r"\uD83D",
            r"\uDE00",
            r"\U0000D800",
            r"\UFFFFFFFF",
            r"\'",
            r"\u",
        ];
        for text in written {
            let source = format!("d: \"<{text}>\"");
            assert_eq!(raw_of(&source, "d"), Some(format!("<{text}>")), "{source}");
        }
        // A high surrogate without its partner stays; the escape after it is valid.
        assert_eq!(
            raw_of(r#"d: "\uD83D\u0041""#, "d").as_deref(),
            Some(r"\uD83DA")
        );
    }

    #[test]
    fn a_backslash_at_the_end_of_a_line_joins_without_a_space() {
        assert_eq!(raw_of("d: \"ab\\\n   cd\"", "d").as_deref(), Some("abcd"));
        assert_eq!(raw_of("d: \"ab \\\n   cd\"", "d").as_deref(), Some("ab cd"));
        assert_eq!(raw_of("d: \"a\\\n\n  b\"", "d").as_deref(), Some("a\nb"));
        // Escaped whitespace belongs to the text and is not trimmed at a break.
        assert_eq!(raw_of("d: \"a\\t\n  b\"", "d").as_deref(), Some("a\t b"));
        assert_eq!(raw_of("d: \"a\\ \n  b\"", "d").as_deref(), Some("a  b"));
    }

    #[test]
    fn single_quoted_scalars_only_escape_the_quote() {
        assert_eq!(
            raw_of(r"d: 'a\nb\x41\u0041 ''q'' \'", "d").as_deref(),
            Some(r"a\nb\x41\u0041 'q' \")
        );
    }

    // What counts as a flow list.

    #[test]
    fn a_flow_list_must_close_at_the_end_of_the_value() {
        let fm = block(
            "a: [Draft] see [x]\nb: [a, \"b, c\", d]\nc: [a, b] # note\nd: [x]y\ne: [a, b] tail\nf: [unclosed",
        );
        assert_eq!(
            fm.get("a"),
            Some(&Value::Text("[Draft] see [x]".to_string()))
        );
        assert_eq!(
            fm.get("b"),
            Some(&Value::List(vec!["a".into(), "b, c".into(), "d".into()]))
        );
        assert_eq!(
            fm.get("c"),
            Some(&Value::List(vec!["a".into(), "b".into()]))
        );
        assert_eq!(fm.get("d"), Some(&Value::Text("[x]y".to_string())));
        assert_eq!(fm.get("e"), Some(&Value::Text("[a, b] tail".to_string())));
        assert_eq!(fm.get("f"), Some(&Value::Text("[unclosed".to_string())));
    }

    #[test]
    fn flow_lists_ignore_brackets_inside_quotes_and_track_nesting() {
        let fm = block(
            "a: [\"x]\", 'y[', z]\nb: [a, [b, c], d]\nc: [\"q\\\"r\", 's''t']\nd: [don't, stop]\ne: [ ]",
        );
        let list = |items: &[&str]| Value::List(items.iter().map(|s| s.to_string()).collect());
        assert_eq!(fm.get("a"), Some(&list(&["x]", "y[", "z"])));
        assert_eq!(fm.get("b"), Some(&list(&["a", "[b, c]", "d"])));
        assert_eq!(fm.get("c"), Some(&list(&["q\"r", "s't"])));
        assert_eq!(fm.get("d"), Some(&list(&["don't", "stop"])));
        assert_eq!(fm.get("e"), Some(&list(&[])));
    }

    // Control characters never reach the terminal.

    #[test]
    fn as_line_replaces_control_characters() {
        assert_eq!(Value::Text("a\u{1b}[2Jb".into()).as_line(), "a\u{fffd}[2Jb");
        // Every C0 control but tab and line feed, DEL, and the whole C1 range.
        let controls = ('\0'..='\u{1f}')
            .chain('\u{7f}'..='\u{9f}')
            .filter(|c| !matches!(c, '\t' | '\n'));
        for control in controls {
            assert_eq!(
                Value::Text(format!("x{control}y")).as_line(),
                "x\u{fffd}y",
                "{control:?}"
            );
        }
        // The replacement happens before whitespace collapses, so a control
        // character that Unicode calls whitespace still does not survive.
        assert_eq!(Value::Text("a \u{85} b".into()).as_line(), "a \u{fffd} b");
        assert_eq!(
            Value::Text("a\u{b}b\u{c}c".into()).as_line(),
            "a\u{fffd}b\u{fffd}c"
        );
    }

    #[test]
    fn as_line_still_collapses_whitespace_and_keeps_other_text() {
        assert_eq!(Value::Text(" a\t\tb \r\n c\n".into()).as_line(), "a b c");
        assert_eq!(Value::Text("a\r\nb\n\nc".into()).as_line(), "a b c");
        assert_eq!(
            Value::Text("a\u{a0}\u{2028}b\u{3000}c".into()).as_line(),
            "a b c"
        );
        assert_eq!(
            Value::Text("caf\u{e9} \u{1f600}".into()).as_line(),
            "caf\u{e9} \u{1f600}"
        );
        // A carriage return that is not part of a CRLF pair is a control character.
        assert_eq!(Value::Text("a\rb".into()).as_line(), "a\u{fffd}b");
    }

    #[test]
    fn as_line_cleans_every_list_item() {
        let list = Value::List(vec!["a\u{1b}b".into(), "c\nd".into(), "e".into()]);
        assert_eq!(list.as_line(), "a\u{fffd}b, c d, e");
    }

    #[test]
    fn control_characters_from_a_file_are_replaced_in_text() {
        let fm = block("description: x\u{1b}[2Jy\nname: \"a\\e[2Jb\\x9bc\"");
        assert_eq!(fm.text("description").as_deref(), Some("x\u{fffd}[2Jy"));
        assert_eq!(fm.text("name").as_deref(), Some("a\u{fffd}[2Jb\u{fffd}c"));
        for (_, value) in fm.entries() {
            assert!(!value.as_line().chars().any(char::is_control));
        }
    }

    // Plain scalars, tabs, list items and block scalar headers.

    #[test]
    fn plain_scalars_continue_across_blank_lines_and_skip_comment_lines() {
        assert_eq!(
            raw_of("d: First\n\n  second\n  # note\n  third\nnext: y", "d").as_deref(),
            Some("First\nsecond third")
        );
        assert_eq!(
            line_of("d: a\n  b # note\n  c", "d").as_deref(),
            Some("a b c")
        );
    }

    #[test]
    fn a_tab_still_marks_a_continuation_line() {
        assert_eq!(line_of("d: a\n\tb\nnext: y", "d").as_deref(), Some("a b"));
        assert_eq!(
            line_of("d: \"a\n\tb\"\nnext: y", "d").as_deref(),
            Some("a b")
        );
        assert_eq!(line_of("d: a\n\tb\nnext: y", "next").as_deref(), Some("y"));
    }

    #[test]
    fn only_a_dash_and_a_space_make_a_list_item() {
        let fm = block("tags:\n  - a\n  --flag\n  -b\n  - c");
        assert_eq!(
            fm.get("tags"),
            Some(&Value::List(vec!["a".into(), "c".into()]))
        );
    }

    #[test]
    fn a_block_scalar_needs_a_valid_header() {
        assert_eq!(line_of("v: >=1.0", "v").as_deref(), Some(">=1.0"));
        assert_eq!(line_of("v: |text", "v").as_deref(), Some("|text"));
        assert_eq!(
            line_of("v: >- # note\n  one\n  two\nnext: y", "v").as_deref(),
            Some("one two")
        );
        assert_eq!(
            line_of("v: |2-\n  one\nnext: y", "next").as_deref(),
            Some("y")
        );
    }
}

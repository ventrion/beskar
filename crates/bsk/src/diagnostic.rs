//! Errors that point at the exact place in a file and, where possible, say how to fix it.

use std::fmt;

use crate::syntax;

/// Shown in place of a character that would print as nothing, or as something misleading.
const REPLACEMENT: char = '\u{fffd}';

/// The character that stands for `c` when a source line is quoted, so that quoting a
/// line can never move the cursor or hide text. Each character maps to exactly one
/// character, which keeps the underline below it in the right columns.
fn visible(c: char) -> char {
    match c {
        '\t' => ' ',
        // Control pictures: U+2400 is the picture of U+0000, U+241B the picture of ESC.
        '\0'..='\u{1f}' => char::from_u32(0x2400 + u32::from(c)).unwrap_or(REPLACEMENT),
        '\u{7f}' => '\u{2421}',
        c if syntax::is_invisible(c) => REPLACEMENT,
        c => c,
    }
}

/// One problem in a bsk file: where it is, what is wrong, and often how to fix it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    line: usize,
    column: usize,
    width: usize,
    message: String,
    hint: Option<String>,
}

impl Diagnostic {
    /// Creates a diagnostic at a 1-based `line` and `column`. `width` is how many
    /// characters to underline when rendering.
    pub fn new(line: usize, column: usize, width: usize, message: impl Into<String>) -> Self {
        Diagnostic {
            line: line.max(1),
            column: column.max(1),
            width: width.max(1),
            message: message.into(),
            hint: None,
        }
    }

    /// Adds a suggestion for fixing the problem.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// The 1-based line the problem is on.
    pub fn line(&self) -> usize {
        self.line
    }

    /// The 1-based column, counted in characters.
    pub fn column(&self) -> usize {
        self.column
    }

    /// What is wrong.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// How to fix it, if known.
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// Renders the problem in a compiler-style layout that quotes the offending line.
    ///
    /// The quoted line is safe to print. Every control character, invisible character and
    /// whitespace other than a plain space is shown as a visible replacement.
    ///
    /// ```text
    /// error: unknown key 'skil'
    ///  --> coding.bsk:4:1
    ///   |
    /// 4 | skil git
    ///   | ^^^^
    ///   = hint: did you mean 'skill'?
    /// ```
    pub fn render(&self, file: &str, source: &str) -> String {
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);
        let quoted: String = source
            .lines()
            .nth(self.line - 1)
            .unwrap_or("")
            .chars()
            .map(visible)
            .collect();
        let number = self.line.to_string();
        let pad = " ".repeat(number.len());
        let mut out = format!("error: {}\n", self.message);
        out.push_str(&format!("{pad}--> {file}:{}:{}\n", self.line, self.column));
        out.push_str(&format!("{pad} |\n"));
        out.push_str(&format!("{number} | {quoted}\n"));
        out.push_str(&format!(
            "{pad} | {}{}\n",
            " ".repeat(self.column - 1),
            "^".repeat(self.width)
        ));
        if let Some(hint) = &self.hint {
            out.push_str(&format!("{pad} = hint: {hint}\n"));
        }
        out
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.message
        )?;
        if let Some(hint) = &self.hint {
            write!(f, " (hint: {hint})")?;
        }
        Ok(())
    }
}

/// Parsing stops collecting problems after this many, so one bad file cannot flood a terminal.
pub(crate) const MAX_DIAGNOSTICS: usize = 50;

/// A non-empty list of problems, sorted by position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
    truncated: bool,
}

impl Diagnostics {
    /// Wraps `items`, or returns `None` when there is nothing to report.
    pub fn from_vec(items: Vec<Diagnostic>) -> Option<Self> {
        Diagnostics::collect(items, false)
    }

    /// Like [`Diagnostics::from_vec`], for a list that leaves out problems it found.
    pub(crate) fn collect(mut items: Vec<Diagnostic>, truncated: bool) -> Option<Self> {
        if items.is_empty() {
            return None;
        }
        items.sort_by_key(|d| (d.line, d.column));
        Some(Diagnostics { items, truncated })
    }

    /// Wraps a single problem.
    pub fn one(item: Diagnostic) -> Self {
        Diagnostics {
            items: vec![item],
            truncated: false,
        }
    }

    /// Iterates over the problems in file order.
    pub fn iter(&self) -> std::slice::Iter<'_, Diagnostic> {
        self.items.iter()
    }

    /// How many problems there are. Never zero.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Always false; a `Diagnostics` holds at least one problem.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The first problem in the file.
    pub fn first(&self) -> &Diagnostic {
        &self.items[0]
    }

    /// True when the file has more problems than this list holds. Parsing keeps the first 50.
    /// [`Diagnostics::render`] and the `Display` output end with a note when this is true.
    ///
    /// ```
    /// let text = "- item\n".repeat(60);
    /// let problems = bsk::Document::parse(&text).unwrap_err();
    /// assert_eq!(problems.len(), 50);
    /// assert!(problems.truncated());
    /// assert!(problems.to_string().ends_with("note: only the first 50 problems are shown"));
    /// ```
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Renders every problem with [`Diagnostic::render`], separated by blank lines.
    /// A truncated list ends with a line saying so.
    pub fn render(&self, file: &str, source: &str) -> String {
        let parts: Vec<String> = self.items.iter().map(|d| d.render(file, source)).collect();
        let mut out = parts.join("\n");
        if self.truncated {
            out.push('\n');
            out.push_str(&truncation_note());
            out.push('\n');
        }
        out
    }
}

fn truncation_note() -> String {
    format!("note: only the first {MAX_DIAGNOSTICS} problems are shown")
}

impl<'a> IntoIterator for &'a Diagnostics {
    type Item = &'a Diagnostic;
    type IntoIter = std::slice::Iter<'a, Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, item) in self.items.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{item}")?;
        }
        if self.truncated {
            write!(f, "\n{}", truncation_note())?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostics {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_quotes_the_line_and_underlines_the_span() {
        let d = Diagnostic::new(2, 1, 4, "unknown key 'skil'").with_hint("did you mean 'skill'?");
        let text = d.render("coding.bsk", "description x\nskil git\n");
        assert_eq!(
            text,
            "error: unknown key 'skil'\n --> coding.bsk:2:1\n  |\n2 | skil git\n  | ^^^^\n  = hint: did you mean 'skill'?\n"
        );
    }

    #[test]
    fn render_aligns_the_gutter_for_wide_line_numbers() {
        let source: String = (1..=12).map(|i| format!("k{i} v\n")).collect();
        let text = Diagnostic::new(12, 4, 1, "bad").render("f", &source);
        assert!(text.contains("\n   --> f:12:4\n") || text.contains("  --> f:12:4\n"));
        assert!(text.contains("12 | k12 v\n"));
        assert!(text.contains("   |    ^\n"));
    }

    #[test]
    fn render_keeps_tab_columns_aligned() {
        let text = Diagnostic::new(1, 3, 1, "bad").render("f", "\t\tx\n");
        assert!(text.contains("1 |   x\n"));
        assert!(text.contains("  |   ^\n"));
    }

    #[test]
    fn render_shows_control_characters_as_control_pictures() {
        // C0 controls become U+2400 plus the code point, DEL becomes U+2421, tab a space.
        let source = "a\u{0}\u{1b}\r\u{1f}\u{7f}\tb\n";
        let text = Diagnostic::new(1, 1, 1, "bad").render("f", source);
        assert!(
            text.contains("1 | a\u{2400}\u{241b}\u{240d}\u{241f}\u{2421} b\n"),
            "{text}"
        );
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn render_replaces_invisible_characters_with_the_replacement_character() {
        // C1 control, soft hyphen, zero width space, direction marks and overrides, isolates,
        // a byte order mark inside the line, and whitespace that is not a plain space.
        let source = "a\u{85}\u{ad}\u{200b}\u{200f}\u{202e}\u{2060}\u{2066}\u{206f}\u{feff}\u{a0}\u{3000}\u{2028}b\n";
        let text = Diagnostic::new(1, 1, 1, "bad").render("f", source);
        let expected = format!("1 | a{}b\n", "\u{fffd}".repeat(12));
        assert!(text.contains(&expected), "{text}");
    }

    #[test]
    fn render_keeps_one_output_character_for_each_input_character() {
        let line = "x\u{1b}[31m\u{85}\u{a0}\tz\u{7f}";
        let text = Diagnostic::new(1, 1, 1, "bad").render("f", line);
        let quoted = text.lines().find(|l| l.starts_with("1 | ")).unwrap();
        assert_eq!(quoted["1 | ".len()..].chars().count(), line.chars().count());
    }

    #[test]
    fn render_leaves_visible_characters_alone() {
        let line = "ünïcode ✓ 日本語 ∑ \u{1F600} [x] # y";
        let text = Diagnostic::new(1, 1, 1, "bad").render("f", line);
        assert!(text.contains(&format!("1 | {line}\n")), "{text}");
    }

    #[test]
    fn render_drops_a_leading_byte_order_mark_and_the_carriage_return_of_crlf() {
        let text = Diagnostic::new(2, 1, 1, "bad").render("f", "\u{feff}a 1\r\nc d\r\n");
        assert!(text.contains("2 | c d\n"), "{text}");
        let text = Diagnostic::new(1, 1, 1, "bad").render("f", "\u{feff}a 1\r\n");
        assert!(text.contains("1 | a 1\n"), "{text}");
    }

    #[test]
    fn render_survives_a_line_past_the_end() {
        let text = Diagnostic::new(9, 1, 1, "missing").render("f", "a b\n");
        assert!(text.contains("9 | \n"));
    }

    #[test]
    fn diagnostics_sort_by_position_and_refuse_to_be_empty() {
        assert!(Diagnostics::from_vec(vec![]).is_none());
        let all = Diagnostics::from_vec(vec![
            Diagnostic::new(5, 1, 1, "later"),
            Diagnostic::new(2, 9, 1, "second"),
            Diagnostic::new(2, 3, 1, "first"),
        ])
        .unwrap();
        let order: Vec<&str> = all.iter().map(|d| d.message()).collect();
        assert_eq!(order, ["first", "second", "later"]);
        assert_eq!(all.first().line(), 2);
    }

    #[test]
    fn a_truncated_list_says_so_when_rendered_and_displayed() {
        let items = vec![Diagnostic::new(1, 1, 1, "a"), Diagnostic::new(2, 1, 1, "b")];
        let all = Diagnostics::collect(items, true).unwrap();
        assert!(all.truncated());
        assert_eq!(
            all.to_string(),
            "line 1, column 1: a\nline 2, column 1: b\nnote: only the first 50 problems are shown"
        );
        let text = all.render("f", "x\ny\n");
        assert!(
            text.ends_with("^\n\nnote: only the first 50 problems are shown\n"),
            "{text}"
        );
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn a_complete_list_has_no_note() {
        let all = Diagnostics::from_vec(vec![Diagnostic::new(1, 1, 1, "a")]).unwrap();
        assert!(!all.truncated());
        assert!(!all.to_string().contains("note:"));
        assert!(!all.render("f", "x\n").contains("note:"));
        assert!(!Diagnostics::one(Diagnostic::new(1, 1, 1, "a")).truncated());
    }

    #[test]
    fn display_is_one_line_per_problem() {
        let all = Diagnostics::from_vec(vec![
            Diagnostic::new(1, 2, 1, "a").with_hint("h"),
            Diagnostic::new(3, 1, 1, "b"),
        ])
        .unwrap();
        assert_eq!(
            all.to_string(),
            "line 1, column 2: a (hint: h)\nline 3, column 1: b"
        );
    }
}

//! Terminal output: styling, tables, error rendering.

use std::io::{self, Write};
use std::path::Path;

use beskar_core::Error;

/// ANSI styling, active only when writing to a terminal without NO_COLOR.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    on: bool,
}

impl Style {
    pub fn new(on: bool) -> Self {
        Style { on }
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.on && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }

    pub fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    pub fn red(self, text: &str) -> String {
        self.paint("31", text)
    }

    pub fn green(self, text: &str) -> String {
        self.paint("32", text)
    }

    pub fn yellow(self, text: &str) -> String {
        self.paint("33", text)
    }

    pub fn blue(self, text: &str) -> String {
        self.paint("34", text)
    }

    pub fn cyan(self, text: &str) -> String {
        self.paint("36", text)
    }

    pub fn bold_red(self, text: &str) -> String {
        self.paint("1;31", text)
    }

    pub fn bold_cyan(self, text: &str) -> String {
        self.paint("1;36", text)
    }
}

/// Standard output and error. Output to a closed pipe (`beskar ... | head`)
/// is dropped quietly instead of aborting halfway through a change.
pub struct Output {
    stdout_style: Style,
    stderr_style: Style,
    closed: bool,
}

impl Output {
    pub fn new(stdout_color: bool, stderr_color: bool) -> Self {
        Output {
            stdout_style: Style::new(stdout_color),
            stderr_style: Style::new(stderr_color),
            closed: false,
        }
    }

    /// Drop text lines from now on; [`Output::raw`] still writes. `--json`
    /// uses this so that nothing but the JSON document reaches standard
    /// output.
    pub fn silence(&mut self) {
        self.closed = true;
    }

    /// Write `text` to standard output as it is, even when silenced.
    pub fn raw(&self, text: &str) {
        let mut stdout = io::stdout().lock();
        let _ = stdout
            .write_all(text.as_bytes())
            .and_then(|()| stdout.flush());
    }

    /// Style for text going to standard output.
    pub fn style(&self) -> Style {
        self.stdout_style
    }

    /// Style for text going to standard error.
    pub fn err_style(&self) -> Style {
        self.stderr_style
    }

    pub fn line(&mut self, text: impl AsRef<str>) {
        if self.closed {
            return;
        }
        let mut stdout = io::stdout().lock();
        if writeln!(stdout, "{}", text.as_ref()).is_err() {
            self.closed = true;
        }
    }

    pub fn blank(&mut self) {
        self.line("");
    }

    pub fn err_line(&self, text: impl AsRef<str>) {
        let _ = writeln!(io::stderr().lock(), "{}", text.as_ref());
    }

    /// Print a prompt without a newline and flush it.
    pub fn prompt(&self, text: &str) {
        let mut stderr = io::stderr().lock();
        let _ = write!(stderr, "{text}");
        let _ = stderr.flush();
    }
}

/// A table cell: the plain text (for width) and its styled rendering.
pub struct Cell {
    plain: String,
    styled: String,
}

impl Cell {
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        Cell {
            styled: text.clone(),
            plain: text,
        }
    }

    pub fn styled(text: impl Into<String>, paint: impl Fn(&str) -> String) -> Self {
        let plain = text.into();
        Cell {
            styled: paint(&plain),
            plain,
        }
    }
}

/// Lay out rows in aligned columns, separated by two spaces, each line
/// prefixed with `indent`. The last column is not padded.
pub fn table(rows: Vec<Vec<Cell>>, indent: &str) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|row| row.get(c))
                .map(|cell| cell.plain.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.into_iter()
        .map(|row| {
            let last = row
                .iter()
                .rposition(|cell| !cell.plain.is_empty())
                .unwrap_or(0);
            let mut line = indent.to_string();
            for (c, cell) in row.into_iter().enumerate().take(last + 1) {
                line.push_str(&cell.styled);
                if c < last {
                    line.push_str(&" ".repeat(widths[c] - cell.plain.chars().count() + 2));
                }
            }
            line
        })
        .collect()
}

/// Shorten `text` to at most `width` characters, ending with `…` when cut.
pub fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let cut: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// Replace the home directory with `~` in free text such as error messages.
pub fn tilde(text: &str, home: Option<&Path>) -> String {
    match home.and_then(Path::to_str) {
        Some(home) if home.len() > 1 => {
            let home = home.trim_end_matches('/');
            text.replace(&format!("{home}/"), "~/")
                .replace(&format!("{home} "), "~ ")
                .replace(&format!("{home}:"), "~:")
                .replace(&format!("{home}`"), "~`")
        }
        _ => text.to_string(),
    }
}

/// Render an error the way rustc does: message, location, source line with
/// the problem underlined, and suggested fixes.
/// Make untrusted text safe to print: control characters other than tab
/// (terminal escape sequences, for instance, from a skill's description)
/// become visible `\u{..}` escapes.
pub fn clean(text: &str) -> String {
    if !text.chars().any(|c| c.is_control() && c != '\t') {
        return text.to_string();
    }
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\t' {
                c.escape_unicode().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

pub fn render_error(error: &Error, style: Style, home: Option<&Path>, level: &str) -> Vec<String> {
    let label = if level == "error" {
        style.bold_red(level)
    } else {
        style.bold(level)
    };
    let mut lines = vec![format!(
        "{label}: {}",
        style.bold(&clean(&tilde(&error.message, home)))
    )];
    let mut hints: Vec<&String> = error.hints.iter().collect();
    if let Some(path) = &error.path {
        let shown = clean(&beskar_core::config::display_path(path, home));
        match &error.diagnostic {
            Some(diagnostic) if diagnostic.line > 0 => {
                let number = diagnostic.line.to_string();
                let pad = " ".repeat(number.len());
                lines.push(format!(
                    "{pad}{} {shown}:{}:{}",
                    style.blue("-->"),
                    diagnostic.line,
                    diagnostic.column
                ));
                lines.push(format!("{pad} {}", style.blue("|")));
                lines.push(format!(
                    "{} {} {}",
                    style.blue(&number),
                    style.blue("|"),
                    clean(&diagnostic.source_line)
                ));
                let before: String = diagnostic
                    .source_line
                    .chars()
                    .take(diagnostic.column.saturating_sub(1))
                    .collect();
                let lead: String = clean(&before)
                    .chars()
                    .map(|c| if c == '\t' { '\t' } else { ' ' })
                    .collect();
                let carets = "^".repeat(diagnostic.width.max(1));
                lines.push(format!(
                    "{pad} {} {lead}{}",
                    style.blue("|"),
                    style.red(&carets)
                ));
                if let Some(help) = &diagnostic.help {
                    hints.retain(|hint| *hint != help);
                    lines.push(format!("{}: {}", style.bold_cyan("help"), clean(help)));
                }
            }
            _ => lines.push(format!("  {} {shown}", style.blue("-->"))),
        }
    }
    for hint in hints {
        lines.push(format!(
            "{}: {}",
            style.bold_cyan("help"),
            clean(&tilde(hint, home))
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_align_on_plain_width() {
        let style = Style::new(true);
        let rows = vec![
            vec![Cell::styled("git", |t| style.green(t)), Cell::plain("one")],
            vec![Cell::plain("code-review"), Cell::plain("two")],
        ];
        let lines = table(rows, "  ");
        assert_eq!(lines[0], "  \x1b[32mgit\x1b[0m          one");
        assert_eq!(lines[1], "  code-review  two");
    }

    #[test]
    fn empty_trailing_cells_add_no_padding() {
        let lines = table(
            vec![
                vec![Cell::plain("a"), Cell::plain("")],
                vec![Cell::plain("bbb"), Cell::plain("x")],
            ],
            "",
        );
        assert_eq!(lines, ["a", "bbb  x"]);
    }

    #[test]
    fn control_characters_become_visible() {
        assert_eq!(clean("plain\ttext"), "plain\ttext");
        assert_eq!(
            clean("raw \u{1b}]0;TITLE\u{7} done"),
            "raw \\u{1b}]0;TITLE\\u{7} done"
        );
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("a longer sentence", 10), "a longer…");
    }

    #[test]
    fn tilde_replaces_home_prefixes_only() {
        let home = Some(Path::new("/home/me"));
        assert_eq!(
            tilde("cannot read /home/me/.beskar/x: gone", home),
            "cannot read ~/.beskar/x: gone"
        );
        assert_eq!(tilde("/home/meow/x", home), "/home/meow/x");
    }

    #[test]
    fn errors_render_with_a_snippet() {
        let diagnostic = bsk::Error {
            line: 4,
            column: 1,
            width: 5,
            message: "unknown key `skils`".into(),
            help: Some("did you mean `skill`?".into()),
            source_line: "skils: git".into(),
        };
        let error = Error::bsk(
            Path::new("/home/me/.beskar/library/profiles/coding.bsk"),
            diagnostic,
        );
        let lines = render_error(
            &error,
            Style::new(false),
            Some(Path::new("/home/me")),
            "error",
        );
        assert_eq!(
            lines,
            [
                "error: unknown key `skils`",
                " --> ~/.beskar/library/profiles/coding.bsk:4:1",
                "  |",
                "4 | skils: git",
                "  | ^^^^^",
                "help: did you mean `skill`?",
            ]
        );
    }
}

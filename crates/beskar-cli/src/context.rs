//! Everything a command needs from the outside world, gathered in one place.
//!
//! Commands never touch `std::env`, the process's working directory or the standard streams
//! directly. They use a [`Context`], so tests can run the whole tool in-process against fake
//! terminals and a fake environment.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use beskar_core::Env;
use beskar_core::config::shorten;
pub use beskar_core::text::{count, names, sanitize};

/// The outside world, as seen by one invocation.
pub struct Context {
    /// The working directory. Relative paths on the command line start here.
    pub cwd: PathBuf,
    /// The variables that decide where Beskar looks.
    pub env: Env,
    /// Use colors on standard output.
    pub color: bool,
    /// Use colors on standard error.
    pub err_color: bool,
    /// A person is at the terminal and can answer questions.
    pub interactive: bool,
    /// Shorten long text to fit a terminal.
    pub truncate: bool,
    /// Where answers are read from.
    pub input: Box<dyn BufRead>,
    /// Standard output: results.
    pub out: Box<dyn Write>,
    /// Standard error: problems and questions.
    pub err: Box<dyn Write>,
}

impl Context {
    /// The real process: its directory, environment and terminal.
    pub fn from_process() -> io::Result<Context> {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
            || std::env::var("TERM").is_ok_and(|t| t == "dumb");
        let stdout_tty = io::stdout().is_terminal();
        let stderr_tty = io::stderr().is_terminal();
        Ok(Context {
            cwd: std::env::current_dir()?,
            env: Env::from_process(),
            color: stdout_tty && !no_color,
            err_color: stderr_tty && !no_color,
            interactive: io::stdin().is_terminal() && stderr_tty,
            truncate: stdout_tty,
            input: Box::new(io::stdin().lock()),
            out: Box::new(io::stdout()),
            err: Box::new(io::stderr()),
        })
    }

    /// Colors for standard output.
    pub fn style(&self) -> Style {
        Style::new(self.color)
    }

    /// Colors for standard error.
    pub fn err_style(&self) -> Style {
        Style::new(self.err_color)
    }

    /// Writes a line of results. A closed pipe, as in `beskar ... | head`, is not an error.
    pub fn say(&mut self, text: impl AsRef<str>) {
        let _ = writeln!(self.out, "{}", text.as_ref());
    }

    /// Writes a line to standard error.
    pub fn say_err(&mut self, text: impl AsRef<str>) {
        let _ = writeln!(self.err, "{}", text.as_ref());
    }

    /// A path as a person would type it: `~` for the home directory. Control characters in a
    /// folder's name are replaced, so a strange name cannot rewrite the terminal it is shown on.
    pub fn tilde(&self, path: &Path) -> String {
        sanitize(&shorten(path, self.env.user_home.as_deref()))
    }
}

/// Terminal colors, or nothing at all.
#[derive(Clone, Copy, Debug)]
pub struct Style(bool);

impl Style {
    /// Colors when `enabled`.
    pub fn new(enabled: bool) -> Style {
        Style(enabled)
    }

    fn wrap(self, code: &str, text: &str) -> String {
        if self.0 {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    /// Bold text.
    pub fn bold(self, text: &str) -> String {
        self.wrap("1", text)
    }

    /// Dim text, for secondary information.
    pub fn dim(self, text: &str) -> String {
        self.wrap("2", text)
    }

    /// Red text, for problems and removals.
    pub fn red(self, text: &str) -> String {
        self.wrap("31", text)
    }

    /// Green text, for success and additions.
    pub fn green(self, text: &str) -> String {
        self.wrap("32", text)
    }

    /// Yellow text, for warnings and changes.
    pub fn yellow(self, text: &str) -> String {
        self.wrap("33", text)
    }

    /// Cyan text, for names.
    pub fn cyan(self, text: &str) -> String {
        self.wrap("36", text)
    }
}

/// Lays out rows in aligned columns. The last column is not padded, and columns that hold only
/// numbers are right-aligned.
pub fn table(headers: &[&str], rows: &[Vec<String>], style: Style, indent: usize) -> String {
    let columns = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(columns) {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let numeric: Vec<bool> = (0..columns)
        .map(|i| {
            !rows.is_empty()
                && rows.iter().all(|row| {
                    row.get(i).is_some_and(|c| {
                        !c.is_empty() && c.chars().all(|ch| ch.is_ascii_digit() || ch == '?')
                    })
                })
        })
        .collect();
    let line = |cells: &[String], dim: bool| -> String {
        let mut out = " ".repeat(indent);
        for (i, cell) in cells.iter().enumerate().take(columns) {
            let last = i + 1 == columns;
            let padded = match (numeric[i], last) {
                (true, true) => format!("{cell:>width$}", width = widths[i]),
                (true, false) => format!("{cell:>width$}  ", width = widths[i]),
                (false, true) => cell.clone(),
                (false, false) => format!("{cell:<width$}  ", width = widths[i]),
            };
            out.push_str(&if dim { style.dim(&padded) } else { padded });
        }
        out.trim_end().to_string()
    };
    let head: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
    let mut lines = vec![line(&head, true)];
    lines.extend(rows.iter().map(|row| line(row, false)));
    lines.join("\n")
}

/// Cuts text to `max` characters, marking the cut with `...`, when `enabled`.
pub fn clip(text: &str, max: usize, enabled: bool) -> String {
    if !enabled || text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(3)).collect();
    format!("{}...", kept.trim_end())
}

/// A byte count for people: "812 B", "12 KiB", "3.4 MiB".
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else if value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_adds_codes_only_when_enabled() {
        assert_eq!(Style::new(false).red("x"), "x");
        assert_eq!(Style::new(true).red("x"), "\x1b[31mx\x1b[0m");
        assert_eq!(Style::new(true).bold("x"), "\x1b[1mx\x1b[0m");
    }

    #[test]
    fn tables_align_columns_and_leave_the_last_unpadded() {
        let rows = vec![
            vec!["git".to_string(), "Version control".to_string()],
            vec!["code-review".to_string(), "Review".to_string()],
        ];
        assert_eq!(
            table(&["NAME", "ABOUT"], &rows, Style::new(false), 2),
            "  NAME         ABOUT\n  git          Version control\n  code-review  Review"
        );
    }

    #[test]
    fn numeric_columns_are_right_aligned() {
        let rows = vec![
            vec![
                "coding".to_string(),
                "3".to_string(),
                "Everyday".to_string(),
            ],
            vec!["research".to_string(), "12".to_string(), String::new()],
        ];
        assert_eq!(
            table(&["NAME", "SKILLS", "ABOUT"], &rows, Style::new(false), 0),
            "NAME      SKILLS  ABOUT\ncoding         3  Everyday\nresearch      12"
        );
    }

    #[test]
    fn tables_count_characters_not_bytes() {
        let rows = vec![vec!["é".to_string(), "x".to_string()]];
        assert_eq!(
            table(&["A", "B"], &rows, Style::new(false), 0),
            "A  B\né  x"
        );
    }

    #[test]
    fn clip_marks_the_cut() {
        assert_eq!(clip("short", 10, true), "short");
        assert_eq!(
            clip("a very long description indeed", 12, true),
            "a very lo..."
        );
        assert_eq!(
            clip("a very long description indeed", 12, false),
            "a very long description indeed"
        );
    }

    #[test]
    fn counts_and_sizes_read_naturally() {
        assert_eq!(count(1, "skill"), "1 skill");
        assert_eq!(count(0, "skill"), "0 skills");
        assert_eq!(count(3, "repository"), "3 repositories");
        assert_eq!(count(1, "repository"), "1 repository");
        assert_eq!(count(2, "key"), "2 keys");
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(812), "812 B");
        assert_eq!(bytes(2048), "2.0 KiB");
        assert_eq!(bytes(12 * 1024), "12 KiB");
        assert_eq!(bytes(3_500_000), "3.3 MiB");
        assert_eq!(names(Vec::<String>::new()), "(none)");
        assert_eq!(names(["a", "b"]), "a, b");
    }
}

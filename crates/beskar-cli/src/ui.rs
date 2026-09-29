//! Terminal presentation: colors, aligned tables and prompts.

use std::io::{BufRead, IsTerminal, Write};

use beskar_core::{State, paths};

#[derive(Debug, Clone, Copy)]
pub struct Ui {
    color: bool,
    interactive: bool,
}

#[derive(Clone, Copy)]
pub enum Style {
    Bold,
    Dim,
    Green,
    Yellow,
    Red,
    Cyan,
    Magenta,
}

impl Ui {
    pub fn new(no_color: bool) -> Ui {
        let color =
            !no_color && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()) && std::io::stdout().is_terminal();
        let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        Ui { color, interactive }
    }

    /// Whether prompts can be answered by a person.
    pub fn interactive(&self) -> bool {
        self.interactive
    }

    pub fn paint(&self, text: &str, style: Style) -> String {
        if !self.color {
            return text.to_string();
        }
        let code = match style {
            Style::Bold => "1",
            Style::Dim => "2",
            Style::Green => "32",
            Style::Yellow => "33",
            Style::Red => "31",
            Style::Cyan => "36",
            Style::Magenta => "35",
        };
        format!("\x1b[{code}m{text}\x1b[0m")
    }

    pub fn bold(&self, t: &str) -> String {
        self.paint(t, Style::Bold)
    }

    pub fn dim(&self, t: &str) -> String {
        self.paint(t, Style::Dim)
    }

    pub fn state_style(state: State) -> Style {
        match state {
            State::Install | State::Restore | State::Adopt => Style::Green,
            State::Update => Style::Cyan,
            State::Remove | State::Forget => Style::Red,
            State::Conflict(_) | State::Missing => Style::Magenta,
            State::Modified => Style::Yellow,
            State::Clean | State::Untracked => Style::Dim,
        }
    }

    pub fn symbol(&self, state: State) -> String {
        self.paint(&state.symbol().to_string(), Ui::state_style(state))
    }

    /// Ask a question; `None` when input is closed.
    pub fn ask(&self, question: &str) -> Option<String> {
        print!("{question}");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim().to_string()),
        }
    }

    pub fn confirm(&self, question: &str, default_yes: bool) -> bool {
        let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
        loop {
            match self.ask(&format!("{question} {suffix} ")).as_deref().map(str::to_ascii_lowercase).as_deref() {
                None => return false,
                Some("") => return default_yes,
                Some("y" | "yes") => return true,
                Some("n" | "no") => return false,
                Some(_) => println!("Please answer y or n."),
            }
        }
    }
}

/// How a path is shown.
pub fn show(path: &std::path::Path) -> String {
    paths::display(path)
}

/// Render rows as left-aligned columns separated by two spaces. Widths
/// ignore ANSI color codes; trailing whitespace is trimmed.
pub fn table(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(visible_len(cell));
        }
    }
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            line.push_str(cell);
            if i + 1 < row.len() {
                line.push_str(&" ".repeat(widths[i] - visible_len(cell) + 2));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn visible_len(s: &str) -> usize {
    let mut n = 0;
    let mut in_escape = false;
    for c in s.chars() {
        match (in_escape, c) {
            (false, '\x1b') => in_escape = true,
            (true, 'm') => in_escape = false,
            (true, _) => {}
            (false, _) => n += 1,
        }
    }
    n
}

/// Shorten `s` to at most `max` characters, marking the cut with `…`.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The candidate closest to `word`, if close enough to be a typo.
pub fn suggest<'a>(word: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .into_iter()
        .map(|c| (distance(word, c), c))
        .filter(|(d, c)| *d <= 2.max(c.len() / 3))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_alignment_ignores_color() {
        let rows = vec![
            vec!["\x1b[32m+\x1b[0m".to_string(), "ab".into(), "x".into()],
            vec!["-".into(), "abcd".into(), "".into()],
        ];
        assert_eq!(table(&rows), "\x1b[32m+\x1b[0m  ab    x\n-  abcd\n");
    }

    #[test]
    fn suggestions() {
        assert_eq!(suggest("dryrun", ["dry-run", "repo"]), Some("dry-run"));
        assert_eq!(suggest("profiel", ["profile", "repo"]), Some("profile"));
        assert_eq!(suggest("xyz", ["profile"]), None);
    }
}

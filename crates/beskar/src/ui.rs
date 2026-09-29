//! Terminal output and prompts. Domain logic never prints; it hands
//! messages here so a future TUI/GUI can replace this layer wholesale.

use std::io::{IsTerminal, Read, Write};
use std::path::Path;

use crate::error::Result;

pub struct Ui {
    pub color: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Ui { color: true }
    }
}

impl Ui {
    pub fn detect() -> Ui {
        let color = std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
        Ui { color }
    }

    pub fn interactive_stdin() -> bool {
        std::io::stdin().is_terminal()
    }

    // ---- colors -----------------------------------------------------------

    fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.paint("31", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.paint("33", s)
    }
    pub fn cyan(&self, s: &str) -> String {
        self.paint("36", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    pub fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }

    /// Colorize a plan symbol (+ ~ - = ! ?).
    pub fn symbol(&self, sym: &str) -> String {
        if !self.color {
            return sym.to_string();
        }
        let code = match sym {
            "+" => "32",   // green: added
            "~" => "33",   // yellow: updated
            "-" => "31",   // red: removed
            "=" => "2",    // dim: unchanged
            _ => "31",     // attention
        };
        self.paint(code, sym)
    }

    // ---- printing ---------------------------------------------------------

    pub fn action_line(&self, sym: &str, skill: &str, note: &str, dry_run: bool) {
        println!("{}", self.format_action_line(sym, skill, note));
        let _ = dry_run;
    }

    /// One plan row. The symbol is colorized unpadded and padded outside
    /// the escape codes, so the color lookup sees the bare symbol and the
    /// column alignment survives coloring.
    pub fn format_action_line(&self, sym: &str, skill: &str, note: &str) -> String {
        let pad = " ".repeat(sym.chars().count().max(2) - sym.chars().count());
        format!(" {pad}{} {} {}", self.symbol(sym), self.bold(skill), self.dim(note))
    }

    pub fn header(&self, text: &str) {
        println!("{}", self.bold(text));
    }

    pub fn note(&self, text: &str) {
        println!("{}", self.dim(text));
    }

    pub fn ok(&self, text: &str) {
        println!("{} {}", self.green("✓"), text);
    }

    pub fn warn(&self, text: &str) {
        println!("{} {}", self.yellow("!"), text);
    }

    pub fn fail(&self, text: &str) {
        println!("{} {}", self.red("✗"), text);
    }

    pub fn conflict_header(&self, skill: &str, reason: &str, ws_dir: &Path) {
        println!(
            "{} {} ({})",
            self.red("Conflict:"),
            self.bold(skill),
            reason
        );
        println!("  {}", self.dim(&format!("workspace copy: {}", ws_dir.display())));
    }

    /// The §11 conflict menu. Returns the chosen key; EOF keeps local.
    pub fn conflict_menu(&self, skill: &str) -> Result<char> {
        loop {
            print!(
                "\n  {skill} — [k] {}  [l] {}  [p] {}  [d] {}  [a] {}\n> ",
                self.bold("keep local"),
                self.bold("use library"),
                self.bold("promote to library"),
                self.bold("show diff"),
                self.bold("abort"),
            );
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
                return Ok('k'); // stdin closed: keep local, never destroy
            }
            match line.trim().chars().next().unwrap_or(' ') {
                'k' => return Ok('k'),
                'l' => return Ok('l'),
                'p' => return Ok('p'),
                'd' => return Ok('d'),
                'a' => return Ok('a'),
                _ => continue,
            }
        }
    }

    /// Yes/no confirmation with a default. EOF takes the default.
    pub fn confirm(&self, prompt: &str, default_yes: bool) -> Result<bool> {
        let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
        loop {
            print!("{prompt} {hint} ");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
                return Ok(default_yes);
            }
            match line.trim().to_lowercase().as_str() {
                "" => return Ok(default_yes),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => continue,
            }
        }
    }

    /// Read raw lines until EOF (used by `--body-file -`-style flows, if any).
    pub fn _stdin_lines(&self) -> Vec<String> {
        let mut buf = String::new();
        let _ = std::io::stdin().read_to_string(&mut buf);
        buf.lines().map(|s| s.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_lines_align_and_color_symbols() {
        // Plain mode pins the layout: symbol right-aligned to width 2.
        let plain = Ui { color: false };
        assert_eq!(plain.format_action_line("+", "skill", "installed"), "  + skill installed");
        assert_eq!(plain.format_action_line("!!", "skill", "conflict"), " !! skill conflict");

        // Colored mode must match on the bare symbol: `+` is green, and a
        // padded `" +"` would fall into the red fallback.
        let colored = Ui { color: true };
        let line = colored.format_action_line("+", "skill", "installed");
        assert!(line.contains("\x1b[32m+\x1b[0m"), "{line}");
        assert!(!line.contains("\x1b[31m"), "{line}");
        let line = colored.format_action_line("=", "skill", "unchanged");
        assert!(line.contains("\x1b[2m=\x1b[0m"), "{line}");
    }
}

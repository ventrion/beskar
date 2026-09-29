//! Terminal interaction: prompts, confirmations and simple tables. The only
//! module that reads stdin.

use std::io::{self, BufRead, IsTerminal, Write};

use beskar_core::{Error, Result};

/// True when both stdin and stdout are terminals, so prompts make sense.
pub fn interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn read_line() -> Result<Option<String>> {
    let mut line = String::new();
    let n = io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| Error::io("<stdin>", e))?;
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim().to_string()))
}

/// Ask a yes/no question. `assume_yes` skips the prompt. Without a terminal
/// and without `assume_yes`, this is an error that says how to proceed.
pub fn confirm(question: &str, default_yes: bool, assume_yes: bool) -> Result<bool> {
    if assume_yes {
        return Ok(true);
    }
    if !interactive() {
        return Err(Error::invalid(format!(
            "{question} — no terminal to ask on; pass --yes to confirm non-interactively"
        )));
    }
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    loop {
        print!("{question} {hint} ");
        io::stdout().flush().ok();
        let Some(answer) = read_line()? else {
            return Ok(false);
        };
        match answer.to_ascii_lowercase().as_str() {
            "" => return Ok(default_yes),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("please answer y or n"),
        }
    }
}

/// Offer single-letter choices until one is picked. Returns the letter.
pub fn choose(prompt: &str, options: &[(char, &str)]) -> Result<char> {
    loop {
        let menu: Vec<String> = options
            .iter()
            .map(|(c, text)| format!("[{c}] {text}"))
            .collect();
        print!("{prompt}\n  {}\n> ", menu.join("  "));
        io::stdout().flush().ok();
        let Some(answer) = read_line()? else {
            return Err(Error::invalid("input closed; aborting"));
        };
        let mut chars = answer.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            let c = c.to_ascii_lowercase();
            if options.iter().any(|(o, _)| *o == c) {
                return Ok(c);
            }
        }
        println!("please pick one of the letters shown");
    }
}

/// Left-aligned columns. Rows shorter than the widest row are padded.
pub fn table(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            if i + 1 == row.len() {
                line.push_str(cell);
            } else {
                line.push_str(&format!("{:<width$}  ", cell, width = widths[i]));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Shorten long descriptions for listings.
pub fn truncate(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        return text;
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

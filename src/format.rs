use std::io;

pub type Result<T> = std::result::Result<T, String>;

/// A deliberately small format: one `key = value` per line, optional repeated
/// keys, `#` comments, and quoted strings for paths containing whitespace.
pub fn fields(line: &str) -> Result<Option<(String, Vec<String>)>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let (key, rest) = line.split_once('=').ok_or("expected `key = value`")?;
    let key = key.trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
        return Err(format!("invalid key `{key}`"));
    }
    let values = words(rest)?;
    if values.is_empty() {
        return Err(format!("missing value for `{key}`"));
    }
    Ok(Some((key.to_owned(), values)))
}

pub fn words(input: &str) -> Result<Vec<String>> {
    let mut chars = input.chars().peekable();
    let mut out = Vec::new();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '#' {
            break;
        }
        let mut word = String::new();
        if c == '"' {
            chars.next();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some('\\') => match chars.next() {
                        Some('n') => word.push('\n'),
                        Some('"') => word.push('"'),
                        Some('\\') => word.push('\\'),
                        _ => return Err("invalid quoted-string escape".into()),
                    },
                    Some(c) => word.push(c),
                    None => return Err("unterminated quoted string".into()),
                }
            }
            if chars
                .peek()
                .is_some_and(|c| !c.is_whitespace() && *c != '#')
            {
                return Err("expected whitespace after quoted string".into());
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == '#' {
                    break;
                }
                if c == '"' || c == '=' || c == '\\' {
                    return Err("bare values cannot contain quotes, `=` or backslashes".into());
                }
                word.push(c);
                chars.next();
            }
        }
        if word.is_empty() {
            return Err("empty value".into());
        }
        out.push(word);
    }
    Ok(out)
}

pub fn quoted(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn one(values: Vec<String>, label: &str) -> Result<String> {
    if values.len() != 1 {
        return Err(format!("`{label}` needs exactly one value"));
    }
    Ok(values.into_iter().next().unwrap())
}

pub fn read(path: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn lines(input: &str) -> impl Iterator<Item = (usize, &str)> {
    input.lines().enumerate().map(|(i, line)| (i + 1, line))
}

pub fn at(path: &std::path::Path, line: usize, message: String) -> String {
    format!("{}:{line}: {message}", path.display())
}

pub fn atomic_write(path: &std::path::Path, contents: &str) -> io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing parent"))?;
    std::fs::create_dir_all(parent)?;
    for number in 0..100 {
        let temp = parent.join(format!(
            ".beskar-{}-{}-{number}.tmp",
            std::process::id(),
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(mut file) => {
                let result = (|| {
                    file.write_all(contents.as_bytes())?;
                    file.sync_all()?;
                    std::fs::rename(&temp, path)
                })();
                if result.is_err() {
                    let _ = std::fs::remove_file(&temp);
                }
                return result;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot reserve temporary file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_values_round_trip() {
        let s = "space # \\\" newline\n";
        assert_eq!(words(&quoted(s)).unwrap(), vec![s]);
        assert_eq!(
            fields("skill = \"a b\" # note").unwrap().unwrap().1,
            vec!["a b"]
        );
        assert!(fields("skill = \"unfinished").is_err());
    }
}

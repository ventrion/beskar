use std::fmt;

/// A problem with the text of a Beskar lines file.
///
/// Errors carry the line they belong to, the text of that line and, when there
/// is one, a hint that says what to change. [`Error::render`] turns all of that
/// into a message a person or an agent can act on without opening the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    line: usize,
    message: String,
    source_line: Option<String>,
    hint: Option<String>,
}

impl Error {
    /// An error that is not tied to a line.
    pub fn new(message: impl Into<String>) -> Self {
        Error { line: 0, message: message.into(), source_line: None, hint: None }
    }

    /// Pins the error to a 1-based line number and the text of that line.
    #[must_use]
    pub fn at_line(mut self, line: usize, text: impl Into<String>) -> Self {
        self.line = line;
        self.source_line = Some(text.into());
        self
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// The 1-based line number, or 0 when the error is not tied to a line.
    pub fn line(&self) -> usize {
        self.line
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// Formats the error for a terminal, naming `file` as the origin.
    ///
    /// ```text
    /// profiles/coding.bsk:3: unknown key `skils`, did you mean `skill`?
    ///    3 | skils git
    /// ```
    pub fn render(&self, file: &str) -> String {
        let mut out = if self.line > 0 {
            format!("{file}:{}: {}", self.line, self.message)
        } else {
            format!("{file}: {}", self.message)
        };
        if let Some(text) = &self.source_line {
            out.push_str(&format!("\n{:>4} | {}", self.line, text));
        }
        if let Some(hint) = &self.hint {
            out.push_str(&format!("\n   = hint: {hint}"));
        }
        out
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "line {}: {}", self.line, self.message)
        } else {
            f.write_str(&self.message)
        }
    }
}

impl std::error::Error for Error {}

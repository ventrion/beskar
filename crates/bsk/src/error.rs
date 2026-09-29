use std::fmt;

/// A problem in a BSK document.
///
/// Line and column numbers start at 1, and columns count characters rather
/// than bytes. A `line` of 0 means the problem concerns the document as a
/// whole, for example a required key that is missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// Line number, or 0 for problems that are not on a particular line.
    pub line: usize,
    /// Character column where the problem starts.
    pub column: usize,
    /// Number of characters to underline, at least 1.
    pub width: usize,
    /// What is wrong.
    pub message: String,
    /// How to fix it, when there is an obvious fix.
    pub help: Option<String>,
    /// Text of the offending line, for rendering a snippet.
    pub source_line: String,
}

impl Error {
    /// A problem that concerns the whole document.
    pub fn document(message: impl Into<String>) -> Self {
        Error {
            line: 0,
            column: 0,
            width: 0,
            message: message.into(),
            help: None,
            source_line: String::new(),
        }
    }

    /// A problem at a byte range of a line. `number` is the 1-based line
    /// number, or 0 if the line did not come from parsed text.
    pub(crate) fn at(
        text: &str,
        number: usize,
        start: usize,
        len: usize,
        message: impl Into<String>,
    ) -> Self {
        let start = floor_boundary(text, start.min(text.len()));
        let end = floor_boundary(text, (start + len).min(text.len()));
        if number == 0 {
            return Error::document(message);
        }
        Error {
            line: number,
            column: text[..start].chars().count() + 1,
            width: text[start..end].chars().count().max(1),
            message: message.into(),
            help: None,
            source_line: text.to_string(),
        }
    }

    /// Attach advice on how to fix the problem.
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "line {}, column {}: ", self.line, self.column)?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

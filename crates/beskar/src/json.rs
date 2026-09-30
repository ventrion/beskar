//! JSON output for `--json`: a value type and a writer. Beskar only ever
//! writes JSON, so there is no parser.

use std::path::Path;

use beskar_core::{Error, ErrorKind};

/// A JSON value. Objects keep their fields in the order they were added,
/// so output is stable and reads in a sensible order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// An object from `(key, value)` pairs.
    pub fn obj<K: Into<String>>(fields: impl IntoIterator<Item = (K, Json)>) -> Json {
        Json::Obj(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    pub fn arr(items: impl IntoIterator<Item = Json>) -> Json {
        Json::Arr(items.into_iter().collect())
    }

    /// An array of strings.
    pub fn strings<S: ToString>(items: impl IntoIterator<Item = S>) -> Json {
        Json::Arr(
            items
                .into_iter()
                .map(|s| Json::Str(s.to_string()))
                .collect(),
        )
    }

    pub fn path(path: &Path) -> Json {
        Json::Str(path.to_string_lossy().into_owned())
    }

    pub fn paths<'a>(paths: impl IntoIterator<Item = &'a std::path::PathBuf>) -> Json {
        Json::arr(paths.into_iter().map(|p| Json::path(p)))
    }

    pub fn count(n: usize) -> Json {
        Json::Int(i64::try_from(n).unwrap_or(i64::MAX))
    }

    /// Add a field to an object; anything else is left as it is.
    pub fn with(mut self, key: &str, value: Json) -> Json {
        if let Json::Obj(fields) = &mut self {
            fields.push((key.to_string(), value));
        }
        self
    }

    /// The value with two-space indentation and a final newline.
    pub fn pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(n) => out.push_str(&n.to_string()),
            Json::Str(s) => write_string(out, s),
            Json::Arr(items) if items.is_empty() => out.push_str("[]"),
            Json::Obj(fields) if fields.is_empty() => out.push_str("{}"),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    indent(out, depth + 1);
                    item.write(out, depth + 1);
                }
                out.push('\n');
                indent(out, depth);
                out.push(']');
            }
            Json::Obj(fields) => {
                out.push('{');
                for (i, (key, value)) in fields.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    indent(out, depth + 1);
                    write_string(out, key);
                    out.push_str(": ");
                    value.write(out, depth + 1);
                }
                out.push('\n');
                indent(out, depth);
                out.push('}');
            }
        }
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Json {
        Json::Str(s.to_string())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Json {
        Json::Str(s)
    }
}

impl From<bool> for Json {
    fn from(b: bool) -> Json {
        Json::Bool(b)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(value: Option<T>) -> Json {
        value.map_or(Json::Null, Into::into)
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The stable name of an error kind, for scripts to match on.
pub fn kind(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::NotInitialized => "not_initialized",
        ErrorKind::NotFound => "not_found",
        ErrorKind::AlreadyExists => "already_exists",
        ErrorKind::Invalid => "invalid",
        ErrorKind::Conflict => "conflict",
        ErrorKind::Locked => "locked",
        ErrorKind::Io => "io",
    }
}

/// An error as scripts see it: kind, message, hints and, for a problem in
/// one of Beskar's files, where it is.
pub fn error(error: &Error) -> Json {
    let mut json = Json::obj([
        ("kind", Json::from(kind(error.kind))),
        ("message", Json::from(error.message.as_str())),
        ("hints", Json::strings(&error.hints)),
    ]);
    if let Some(path) = &error.path {
        json = json.with("file", Json::path(path));
        if let Some(diagnostic) = error.diagnostic.as_deref().filter(|d| d.line > 0) {
            json = json
                .with("line", Json::count(diagnostic.line))
                .with("column", Json::count(diagnostic.column));
        }
    }
    json
}

/// A usage error: the command line itself was wrong.
pub fn usage_error(message: &str, hints: &[String]) -> Json {
    Json::obj([
        ("kind", Json::from("usage")),
        ("message", Json::from(message)),
        ("hints", Json::strings(hints)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_printing() {
        let value = Json::obj([
            ("name", Json::from("a \"quoted\"\nline\u{1b}")),
            ("n", Json::Int(-3)),
            ("empty", Json::arr([])),
            ("list", Json::strings(["x", "y"])),
            ("none", Json::Null),
            ("yes", Json::Bool(true)),
            ("nested", Json::obj([("k", Json::obj::<&str>([]))])),
        ]);
        assert_eq!(
            value.pretty(),
            "{\n  \"name\": \"a \\\"quoted\\\"\\nline\\u001b\",\n  \"n\": -3,\n  \"empty\": [],\n  \"list\": [\n    \"x\",\n    \"y\"\n  ],\n  \"none\": null,\n  \"yes\": true,\n  \"nested\": {\n    \"k\": {}\n  }\n}\n"
        );
    }

    #[test]
    fn errors_carry_their_location() {
        let diagnostic = bsk::Error {
            line: 4,
            column: 2,
            width: 1,
            message: "unknown key".into(),
            help: Some("did you mean `skill`?".into()),
            source_line: "skils: git".into(),
        };
        let json = error(&Error::bsk(Path::new("/p/coding.bsk"), diagnostic));
        let text = json.pretty();
        assert!(text.contains("\"kind\": \"invalid\""), "{text}");
        assert!(text.contains("\"file\": \"/p/coding.bsk\""), "{text}");
        assert!(text.contains("\"line\": 4"), "{text}");
        assert!(text.contains("\"column\": 2"), "{text}");
    }
}

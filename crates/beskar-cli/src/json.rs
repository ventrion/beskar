//! A JSON value and writer, for `--json`. Output only: Beskar never reads JSON.

use std::collections::BTreeMap;
use std::path::Path;

/// A JSON value. Objects keep the order in which fields were added.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    /// `null`
    Null,
    /// `true` or `false`
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A string.
    Str(String),
    /// An array.
    Arr(Vec<Json>),
    /// An object.
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// An object from `(name, value)` pairs.
    pub fn obj<const N: usize>(fields: [(&str, Json); N]) -> Json {
        Json::Obj(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    /// An array.
    pub fn arr<I: IntoIterator<Item = Json>>(items: I) -> Json {
        Json::Arr(items.into_iter().collect())
    }

    /// An array of strings.
    pub fn strings<I, S>(items: I) -> Json
    where
        I: IntoIterator<Item = S>,
        S: ToString,
    {
        Json::Arr(
            items
                .into_iter()
                .map(|s| Json::Str(s.to_string()))
                .collect(),
        )
    }

    /// A path as a string.
    pub fn path(path: &Path) -> Json {
        Json::Str(path.display().to_string())
    }

    /// A failure as scripts see it: `{"kind", "message", "hint"}`. The hint is `null` when there is none.
    pub fn error(error: &beskar_core::Error) -> Json {
        Json::obj([
            ("kind", error.kind().id().into()),
            ("message", error.message().into()),
            ("hint", error.hint().map(str::to_string).into()),
        ])
    }

    /// The value printed with two-space indentation and a trailing newline.
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
            Json::Arr(items) => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    indent(out, depth + 1);
                    item.write(out, depth + 1);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                indent(out, depth);
                out.push(']');
            }
            Json::Obj(fields) if fields.is_empty() => out.push_str("{}"),
            Json::Obj(fields) => {
                out.push_str("{\n");
                for (i, (key, value)) in fields.iter().enumerate() {
                    indent(out, depth + 1);
                    write_string(out, key);
                    out.push_str(": ");
                    value.write(out, depth + 1);
                    out.push_str(if i + 1 < fields.len() { ",\n" } else { "\n" });
                }
                indent(out, depth);
                out.push('}');
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl From<&str> for Json {
    fn from(value: &str) -> Json {
        Json::Str(value.to_string())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Json {
        Json::Str(value)
    }
}

impl From<bool> for Json {
    fn from(value: bool) -> Json {
        Json::Bool(value)
    }
}

impl From<usize> for Json {
    fn from(value: usize) -> Json {
        Json::Int(value as i64)
    }
}

impl From<i64> for Json {
    fn from(value: i64) -> Json {
        Json::Int(value)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(value: Option<T>) -> Json {
        value.map_or(Json::Null, Into::into)
    }
}

impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(value: Vec<T>) -> Json {
        Json::Arr(value.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Json>> From<BTreeMap<String, T>> for Json {
    fn from(value: BTreeMap<String, T>) -> Json {
        Json::Obj(value.into_iter().map(|(k, v)| (k, v.into())).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars() {
        assert_eq!(Json::Null.pretty(), "null\n");
        assert_eq!(Json::from(true).pretty(), "true\n");
        assert_eq!(Json::from(-5i64).pretty(), "-5\n");
        assert_eq!(Json::from(7usize).pretty(), "7\n");
        assert_eq!(Json::from("hi").pretty(), "\"hi\"\n");
    }

    #[test]
    fn strings_are_escaped() {
        let tricky = "quote\" backslash\\ newline\n tab\t bell\u{7} del\u{7f} unicode é ✓";
        assert_eq!(
            Json::from(tricky).pretty(),
            "\"quote\\\" backslash\\\\ newline\\n tab\\t bell\\u0007 del\\u007f unicode é ✓\"\n"
        );
    }

    #[test]
    fn nested_values_are_indented() {
        let value = Json::obj([
            ("name", "git".into()),
            ("tags", Json::strings(["a", "b"])),
            ("empty_list", Json::arr([])),
            ("empty_object", Json::Obj(vec![])),
            (
                "nested",
                Json::obj([
                    ("n", 1usize.into()),
                    ("missing", Option::<&str>::None.into()),
                ]),
            ),
        ]);
        assert_eq!(
            value.pretty(),
            "{\n  \"name\": \"git\",\n  \"tags\": [\n    \"a\",\n    \"b\"\n  ],\n  \"empty_list\": [],\n  \"empty_object\": {},\n  \"nested\": {\n    \"n\": 1,\n    \"missing\": null\n  }\n}\n"
        );
    }

    #[test]
    fn conversions() {
        assert_eq!(
            Json::from(vec!["a", "b"]),
            Json::Arr(vec![Json::Str("a".into()), Json::Str("b".into())])
        );
        assert_eq!(Json::from(Some(3usize)), Json::Int(3));
        assert_eq!(Json::path(Path::new("/a b")), Json::Str("/a b".into()));
        let map: BTreeMap<String, usize> = [("k".to_string(), 1)].into_iter().collect();
        assert_eq!(Json::from(map), Json::Obj(vec![("k".into(), Json::Int(1))]));
    }
}

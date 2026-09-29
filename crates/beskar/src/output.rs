use std::fmt::Write;

pub enum Json {
    Null,
    Bool(bool),
    Num(usize),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

pub fn s(value: impl std::fmt::Display) -> Json {
    Json::Str(value.to_string())
}
pub fn arr(values: impl IntoIterator<Item = Json>) -> Json {
    Json::Array(values.into_iter().collect())
}
pub fn obj<const N: usize>(fields: [(&str, Json); N]) -> Json {
    Json::Object(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

impl Json {
    pub fn render(&self) -> String {
        match self {
            Self::Null => "null".into(),
            Self::Bool(b) => b.to_string(),
            Self::Num(n) => n.to_string(),
            Self::Str(s) => quote(s),
            Self::Array(items) => format!(
                "[{}]",
                items.iter().map(Self::render).collect::<Vec<_>>().join(",")
            ),
            Self::Object(fields) => format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|(k, v)| format!("{}:{}", quote(k), v.render()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}

fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < '\u{20}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub struct Report {
    pub data: Json,
    pub human: String,
    pub code: i32,
}
impl Report {
    pub fn new(data: Json, human: impl Into<String>) -> Self {
        Self {
            data,
            human: human.into(),
            code: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escape_json_without_corrupting_unicode() {
        assert_eq!(s("a\n\"\\\t\0 é").render(), "\"a\\n\\\"\\\\\\t\\u0000 é\"");
    }
}

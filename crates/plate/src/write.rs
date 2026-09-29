//! Emit canonical Plate text. Every value is checked so that what is
//! written parses back to exactly the same data.

use crate::Error;
use crate::edit::{validate_key, validate_label, validate_value};

#[derive(Debug, Default)]
pub struct Writer {
    out: String,
}

impl Writer {
    pub fn new() -> Self {
        Writer::default()
    }

    /// One `# ...` line per line of `text`.
    pub fn comment(&mut self, text: &str) {
        for line in text.lines() {
            if line.is_empty() {
                self.out.push_str("#\n");
            } else {
                self.out.push_str("# ");
                self.out.push_str(line);
                self.out.push('\n');
            }
        }
    }

    pub fn blank(&mut self) {
        self.out.push('\n');
    }

    pub fn section(&mut self, kind: &str, label: &str) -> Result<(), Error> {
        validate_key(kind)?;
        validate_label(label)?;
        if label.is_empty() {
            self.out.push_str(&format!("[{kind}]\n"));
        } else {
            self.out.push_str(&format!("[{kind} {label}]\n"));
        }
        Ok(())
    }

    pub fn scalar(&mut self, key: &str, value: &str) -> Result<(), Error> {
        validate_key(key)?;
        validate_value(value, true)?;
        if value.is_empty() {
            self.out.push_str(&format!("{key} =\n"));
        } else {
            self.out.push_str(&format!("{key} = {value}\n"));
        }
        Ok(())
    }

    pub fn list<I, S>(&mut self, key: &str, items: I) -> Result<(), Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        validate_key(key)?;
        let mut block = format!("{key}:\n");
        for item in items {
            let item = item.as_ref();
            validate_value(item, false)?;
            block.push_str(&format!("  - {item}\n"));
        }
        self.out.push_str(&block);
        Ok(())
    }

    pub fn finish(self) -> String {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::Writer;
    use crate::Document;

    #[test]
    fn written_text_parses_back() {
        let mut w = Writer::new();
        w.comment("Header\n\nsecond");
        w.scalar("format", "1").unwrap();
        w.blank();
        w.section("repo", "/home/me/a b").unwrap();
        w.list("profiles", ["coding", "research"]).unwrap();
        w.list("empty", Vec::<String>::new()).unwrap();
        w.scalar("note", "a = b # not a comment").unwrap();
        let text = w.finish();
        let doc = Document::parse(&text).unwrap();
        assert_eq!(doc.root().scalar("format").unwrap(), Some("1"));
        let repo = doc.section("repo", "/home/me/a b").unwrap();
        assert_eq!(repo.list("profiles").unwrap().unwrap().len(), 2);
        assert_eq!(repo.scalar("note").unwrap(), Some("a = b # not a comment"));
        assert!(text.starts_with("# Header\n#\n# second\n"));
    }

    #[test]
    fn refuses_values_that_would_not_round_trip() {
        let mut w = Writer::new();
        assert!(w.scalar("k", "trailing ").is_err());
        assert!(w.list("k", ["ok", ""]).is_err());
        assert!(w.section("repo", "x\ny").is_err());
    }
}

//! Randomized checks of the properties the spec promises. No property-testing
//! dependency: a small xorshift generator with a fixed seed keeps runs
//! reproducible.

#![allow(clippy::unwrap_used)]

use beskar_lines::Document;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const KEYS: &[&str] = &["skill", "repo", "profile", "description", "a", "k9", "x-y"];
const WORDS: &[&str] = &[
    "git",
    "code-review",
    "/home/ana/my skills",
    "c#-review",
    "\"quoted\"",
    "ünïcode",
    "tab\tinside",
    "a: b",
    "- dash",
    "[bracket]",
    "fp1:0123456789abcdef",
    "~/x #1",
];
const GAPS: &[&str] = &[" ", "  ", "\t", "        ", " \t "];
const TRAILS: &[&str] = &["", "", "", " ", "\t", "   "];
const LEADS: &[&str] = &["  ", "    ", "\t", " \t"];
const COMMENTS: &[&str] = &["# note", "#", "   # indented", "#no space", "# a: b"];

fn random_text(rng: &mut Rng, crlf: bool, final_newline: bool) -> String {
    let count = rng.below(14);
    let mut lines: Vec<String> = Vec::new();
    let mut has_parent = false;
    for _ in 0..count {
        match rng.below(6) {
            0 => lines.push(String::new()),
            1 => lines.push(rng.pick(COMMENTS).to_string()),
            2 => lines.push(rng.pick(&["  ", "\t", " \t "]).to_string()),
            _ => {
                let indented = has_parent && rng.below(2) == 0;
                let lead = if indented { rng.pick(LEADS) } else { "" };
                let key = rng.pick(KEYS);
                let line = if rng.below(8) == 0 {
                    format!("{lead}{key}{}", rng.pick(TRAILS))
                } else {
                    format!("{lead}{key}{}{}{}", rng.pick(GAPS), rng.pick(WORDS), rng.pick(TRAILS))
                };
                has_parent |= !indented;
                lines.push(line);
            }
        }
    }
    let eol = if crlf { "\r\n" } else { "\n" };
    let mut text = lines.join(eol);
    if final_newline && !lines.is_empty() {
        text.push_str(eol);
    }
    text
}

#[test]
fn parse_then_render_is_the_identity() {
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    for round in 0..3000 {
        let text = random_text(&mut rng, round % 3 == 0, round % 5 != 0);
        let doc = Document::parse(&text)
            .unwrap_or_else(|e| panic!("generated text must parse: {e}\n{text:?}"));
        assert_eq!(doc.to_string(), text, "round {round}");
    }
}

#[test]
fn render_then_parse_preserves_entries() {
    let mut rng = Rng(0x0dd_ba11_cafe_f00d);
    for round in 0..1000 {
        let text = random_text(&mut rng, false, true);
        let doc = Document::parse(&text).unwrap();
        let again = Document::parse(&doc.to_string()).unwrap();
        let facts = |d: &Document| -> Vec<(String, String, bool)> {
            d.entries()
                .iter()
                .map(|e| (e.key().to_string(), e.value().to_string(), e.is_indented()))
                .collect()
        };
        assert_eq!(facts(&doc), facts(&again), "round {round}");
    }
}

#[test]
fn removing_and_readding_keeps_other_lines_untouched() {
    let mut rng = Rng(0xfeed_face_0bad_c0de);
    for round in 0..1000 {
        let text = random_text(&mut rng, false, true);
        let mut doc = Document::parse(&text).unwrap();
        doc.insert_grouped("skill", "zz-added").unwrap();
        assert_eq!(doc.remove("skill", "zz-added"), 1, "round {round}");
        assert_eq!(doc.to_string(), text, "round {round}");
    }
}

//! A sandbox for running the whole tool in-process against fake terminals.
#![allow(dead_code)]

use std::cell::RefCell;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use beskar_cli::context::Context;
use beskar_core::Env;
use beskar_core::testing::TempDir;

/// What one invocation produced.
#[derive(Debug)]
pub struct Output {
    pub status: i32,
    pub out: String,
    pub err: String,
}

impl Output {
    /// Fails the test with the full output unless the command succeeded.
    #[track_caller]
    pub fn ok(&self) -> &Output {
        assert_eq!(self.status, 0, "expected success\n{self:#?}");
        self
    }

    /// Fails the test with the full output unless the command failed with `status`.
    #[track_caller]
    pub fn fails(&self, status: i32) -> &Output {
        assert_eq!(
            self.status, status,
            "expected exit status {status}\n{self:#?}"
        );
        self
    }

    #[track_caller]
    pub fn stdout_has(&self, needle: &str) -> &Output {
        assert!(
            self.out.contains(needle),
            "stdout should contain {needle:?}\n{self:#?}"
        );
        self
    }

    #[track_caller]
    pub fn stdout_lacks(&self, needle: &str) -> &Output {
        assert!(
            !self.out.contains(needle),
            "stdout should not contain {needle:?}\n{self:#?}"
        );
        self
    }

    #[track_caller]
    pub fn stderr_has(&self, needle: &str) -> &Output {
        assert!(
            self.err.contains(needle),
            "stderr should contain {needle:?}\n{self:#?}"
        );
        self
    }
}

#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A user's machine: a home directory, a working directory, and somewhere to keep projects.
pub struct Sandbox {
    pub dir: TempDir,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let dir = TempDir::new("cli");
        dir.mkdir("home");
        dir.mkdir("work");
        Sandbox { dir }
    }

    /// A sandbox where `beskar init` has already run.
    pub fn initialised() -> Sandbox {
        let sandbox = Sandbox::new();
        sandbox.run("init").ok();
        sandbox
    }

    pub fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    pub fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    pub fn library(&self) -> PathBuf {
        self.home().join(".beskar/library")
    }

    /// Runs a command line given as words separated by spaces, from the working directory.
    pub fn run(&self, line: &str) -> Output {
        self.run_in(&self.work(), line)
    }

    /// Runs a command line from another directory.
    pub fn run_in(&self, cwd: &Path, line: &str) -> Output {
        let args: Vec<String> = line.split_whitespace().map(String::from).collect();
        self.execute(cwd, &args, None)
    }

    /// Runs with exact arguments, for values that contain spaces.
    pub fn run_args(&self, cwd: &Path, args: &[&str]) -> Output {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.execute(cwd, &args, None)
    }

    /// Runs as if a person were at the terminal typing `answers`.
    pub fn run_interactive(&self, cwd: &Path, line: &str, answers: &str) -> Output {
        let args: Vec<String> = line.split_whitespace().map(String::from).collect();
        self.execute(cwd, &args, Some(answers))
    }

    fn execute(&self, cwd: &Path, args: &[String], answers: Option<&str>) -> Output {
        let (out, err) = (Shared::default(), Shared::default());
        let mut ctx = Context {
            cwd: cwd.to_path_buf(),
            env: Env {
                user_home: Some(self.home()),
                beskar_home: None,
            },
            color: false,
            err_color: false,
            interactive: answers.is_some(),
            truncate: false,
            input: Box::new(Cursor::new(answers.unwrap_or("").as_bytes().to_vec())),
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
        };
        let status = beskar_cli::run(args, &mut ctx);
        let text = |buffer: &Shared| String::from_utf8_lossy(&buffer.0.borrow()).into_owned();
        Output {
            status,
            out: text(&out),
            err: text(&err),
        }
    }

    // ----- files -----

    /// Creates a skill folder under `where_` (relative to the sandbox) with a SKILL.md.
    pub fn skill_source(&self, where_: &str, name: &str, description: &str) -> PathBuf {
        let path = format!("{where_}/{name}/SKILL.md");
        self.dir.write(
            &path,
            &format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
        );
        self.dir.path().join(where_).join(name)
    }

    /// Imports `names` into the library as skills whose descriptions are "The <name> skill".
    pub fn library_with(&self, names: &[&str]) {
        for name in names {
            self.skill_source("incoming", name, &format!("The {name} skill"));
        }
        self.run(&format!(
            "library scan {} --yes",
            self.dir.path().join("incoming").display()
        ))
        .ok();
    }

    /// Makes a project folder, registers it and returns its path.
    pub fn project(&self, name: &str) -> PathBuf {
        let path = self.dir.mkdir(&format!("projects/{name}"));
        self.run_in(&path, "repo add .").ok();
        path
    }

    pub fn installed(&self, project: &Path) -> Vec<String> {
        let dir = project.join(".agents/skills");
        let mut names: Vec<String> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    }

    pub fn read(&self, path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// A project folder inside the fake home directory, so paths print as `~/projects/<name>`.
    pub fn home_project(&self, name: &str) -> PathBuf {
        let path = self.dir.mkdir(&format!("home/projects/{name}"));
        self.run_in(&path, "repo add .").ok();
        path
    }

    /// Writes a file relative to a project.
    pub fn write_in(&self, base: &Path, relative: &str, content: &str) {
        let path = base.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

/// Every file under `dir` with its content, for checking that nothing changed.
pub fn snapshot(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(base).unwrap().display().to_string(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

// ----- a small JSON reader, so tests can check what `--json` prints -----

#[derive(Debug, Clone, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    pub fn parse(text: &str) -> J {
        let mut parser = Parser {
            chars: text.chars().collect(),
            at: 0,
        };
        let value = parser.value();
        parser.space();
        assert_eq!(
            parser.at,
            parser.chars.len(),
            "trailing text after JSON value in {text:?}"
        );
        value
    }

    #[track_caller]
    pub fn get(&self, key: &str) -> &J {
        match self {
            J::Obj(fields) => fields
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v)
                .unwrap_or_else(|| panic!("no key {key:?} in {self:?}")),
            other => panic!("not an object: {other:?}"),
        }
    }

    #[track_caller]
    pub fn at(&self, index: usize) -> &J {
        match self {
            J::Arr(items) => &items[index],
            other => panic!("not an array: {other:?}"),
        }
    }

    #[track_caller]
    pub fn len(&self) -> usize {
        match self {
            J::Arr(items) => items.len(),
            J::Obj(fields) => fields.len(),
            other => panic!("no length: {other:?}"),
        }
    }

    #[track_caller]
    pub fn str(&self) -> &str {
        match self {
            J::Str(s) => s,
            other => panic!("not a string: {other:?}"),
        }
    }

    #[track_caller]
    pub fn strs(&self) -> Vec<&str> {
        match self {
            J::Arr(items) => items.iter().map(J::str).collect(),
            other => panic!("not an array: {other:?}"),
        }
    }

    #[track_caller]
    pub fn bool(&self) -> bool {
        match self {
            J::Bool(b) => *b,
            other => panic!("not a bool: {other:?}"),
        }
    }

    #[track_caller]
    pub fn num(&self) -> i64 {
        match self {
            J::Num(n) => *n as i64,
            other => panic!("not a number: {other:?}"),
        }
    }

    pub fn is_null(&self) -> bool {
        *self == J::Null
    }
}

struct Parser {
    chars: Vec<char>,
    at: usize,
}

impl Parser {
    fn space(&mut self) {
        while self.chars.get(self.at).is_some_and(|c| c.is_whitespace()) {
            self.at += 1;
        }
    }

    fn eat(&mut self, expected: char) {
        self.space();
        assert_eq!(
            self.chars.get(self.at),
            Some(&expected),
            "expected {expected:?} at {}",
            self.at
        );
        self.at += 1;
    }

    fn value(&mut self) -> J {
        self.space();
        match self.chars.get(self.at).copied() {
            Some('{') => {
                self.at += 1;
                let mut fields = Vec::new();
                self.space();
                if self.chars.get(self.at) == Some(&'}') {
                    self.at += 1;
                    return J::Obj(fields);
                }
                loop {
                    self.space();
                    let J::Str(key) = self.value() else {
                        panic!("object key must be a string")
                    };
                    self.eat(':');
                    fields.push((key, self.value()));
                    self.space();
                    match self.chars.get(self.at) {
                        Some(',') => self.at += 1,
                        Some('}') => {
                            self.at += 1;
                            return J::Obj(fields);
                        }
                        other => panic!("bad object separator {other:?}"),
                    }
                }
            }
            Some('[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.chars.get(self.at) == Some(&']') {
                    self.at += 1;
                    return J::Arr(items);
                }
                loop {
                    items.push(self.value());
                    self.space();
                    match self.chars.get(self.at) {
                        Some(',') => self.at += 1,
                        Some(']') => {
                            self.at += 1;
                            return J::Arr(items);
                        }
                        other => panic!("bad array separator {other:?}"),
                    }
                }
            }
            Some('"') => {
                self.at += 1;
                let mut out = String::new();
                loop {
                    let c = self.chars[self.at];
                    self.at += 1;
                    match c {
                        '"' => return J::Str(out),
                        '\\' => {
                            let e = self.chars[self.at];
                            self.at += 1;
                            match e {
                                'n' => out.push('\n'),
                                't' => out.push('\t'),
                                'r' => out.push('\r'),
                                '"' => out.push('"'),
                                '\\' => out.push('\\'),
                                '/' => out.push('/'),
                                'u' => {
                                    let hex: String =
                                        self.chars[self.at..self.at + 4].iter().collect();
                                    self.at += 4;
                                    out.push(
                                        char::from_u32(u32::from_str_radix(&hex, 16).unwrap())
                                            .unwrap(),
                                    );
                                }
                                other => panic!("bad escape \\{other}"),
                            }
                        }
                        c => {
                            assert!(c >= ' ', "raw control character in JSON string");
                            out.push(c);
                        }
                    }
                }
            }
            Some('t') => self.word("true", J::Bool(true)),
            Some('f') => self.word("false", J::Bool(false)),
            Some('n') => self.word("null", J::Null),
            Some(c) if c == '-' || c.is_ascii_digit() => {
                let start = self.at;
                while self
                    .chars
                    .get(self.at)
                    .is_some_and(|c| c.is_ascii_digit() || matches!(c, '-' | '.' | 'e' | 'E' | '+'))
                {
                    self.at += 1;
                }
                J::Num(
                    self.chars[start..self.at]
                        .iter()
                        .collect::<String>()
                        .parse()
                        .unwrap(),
                )
            }
            other => panic!("unexpected {other:?} at {}", self.at),
        }
    }

    fn word(&mut self, word: &str, value: J) -> J {
        let found: String = self.chars[self.at..].iter().take(word.len()).collect();
        assert_eq!(found, word);
        self.at += word.len();
        value
    }
}

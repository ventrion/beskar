//! Shared harness for the end-to-end tests: a scratch HOME to run the
//! `beskar` binary in, and a strict JSON parser to read what `--json`
//! prints (there is no JSON crate: Beskar has no dependencies).

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::Write;
use std::ops::Index;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Where Beskar keeps its library's skills, relative to the scratch HOME.
pub const LIB: &str = ".beskar/library/skills";

/// A pid no process can have: above Linux's maximum.
pub const DEAD_PID: u32 = 4_194_305;

// ----- Running the binary -----

/// A scratch HOME with Beskar's files, directories of skills to import and
/// room for workspaces. Removed when dropped.
pub struct World {
    pub root: PathBuf,
    /// Every `command` a `--json` run reported, for coverage checks.
    pub commands: RefCell<BTreeSet<String>>,
}

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn of(output: Output) -> Run {
        Run {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    #[track_caller]
    pub fn ok(self) -> Self {
        self.code(0)
    }

    #[track_caller]
    pub fn code(self, code: i32) -> Self {
        assert_eq!(
            self.code, code,
            "stdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        self
    }

    #[track_caller]
    pub fn out_has(self, text: &str) -> Self {
        assert!(
            self.stdout.contains(text),
            "stdout lacks {text:?}:\n{}\nstderr:\n{}",
            self.stdout,
            self.stderr
        );
        self
    }

    #[track_caller]
    pub fn err_has(self, text: &str) -> Self {
        assert!(
            self.stderr.contains(text),
            "stderr lacks {text:?}:\n{}\nstdout:\n{}",
            self.stderr,
            self.stdout
        );
        self
    }
}

impl World {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "beskar-e2e-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        World {
            root: fs::canonicalize(&root).unwrap(),
            commands: RefCell::default(),
        }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        if rel == "." {
            self.root.clone()
        } else {
            self.root.join(rel)
        }
    }

    /// An absolute path under the root as text, as `--json` prints paths.
    pub fn abs(&self, rel: &str) -> String {
        self.path(rel).to_str().unwrap().to_string()
    }

    pub fn write(&self, rel: &str, text: &str) {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    pub fn append(&self, rel: &str, text: &str) {
        let before = self.read(rel);
        fs::write(self.path(rel), before + text).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.path(rel).symlink_metadata().is_ok()
    }

    pub fn skill(&self, dir: &str, name: &str, description: &str) {
        self.write(
            &format!("{dir}/{name}/SKILL.md"),
            &format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
        );
    }

    /// `beskar` with `args` in `cwd` (relative to the root, created if
    /// missing), with HOME set to the root, no color, no lock timeout from
    /// the caller's environment and no standard input.
    pub fn command(&self, cwd: &str, args: &[&str]) -> Command {
        let cwd = self.path(cwd);
        fs::create_dir_all(&cwd).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_beskar"));
        command
            .args(args)
            .current_dir(&cwd)
            .env("HOME", &self.root)
            .env_remove("BESKAR_HOME")
            .env_remove("BESKAR_LOCK_TIMEOUT")
            .env_remove("COLUMNS")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Run `beskar` in `cwd` without a terminal.
    pub fn run(&self, cwd: &str, args: &[&str]) -> Run {
        Run::of(self.command(cwd, args).output().expect("run beskar"))
    }

    /// Run `beskar` with `input` piped to its standard input.
    pub fn run_input(&self, cwd: &str, args: &[&str], input: &str) -> Run {
        let mut child = self
            .command(cwd, args)
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn beskar");
        // The answers may not all be read; a closed pipe is fine.
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        Run::of(child.wait_with_output().unwrap())
    }

    /// Run `beskar --json args...` and check the document's envelope.
    pub fn json(&self, cwd: &str, args: &[&str]) -> Doc {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        self.doc(self.run(cwd, &all))
    }

    /// Check a run that was given `--json` somewhere and read its document.
    pub fn doc(&self, run: Run) -> Doc {
        let doc = Doc::from_run(run);
        if let Some(command) = doc.value.get("command").and_then(Value::str_opt) {
            self.commands.borrow_mut().insert(command.to_string());
        }
        doc
    }

    /// `beskar init`, a library with a few skills, and profiles `coding`
    /// (git, code-review, testing) and `research` (pdf, git).
    pub fn with_library() -> Self {
        let world = World::new();
        for (name, description) in [
            ("git", "Git workflows"),
            ("code-review", "Review code changes"),
            ("testing", "Write tests first"),
            ("pdf", "Read PDFs"),
            ("playwright", "Browser automation"),
        ] {
            world.skill("my-skills", name, description);
        }
        world.run(".", &["init"]).ok();
        world
            .run(".", &["library", "scan", "my-skills", "--yes"])
            .ok();
        world
            .run(
                ".",
                &[
                    "profile",
                    "create",
                    "coding",
                    "git",
                    "code-review",
                    "testing",
                ],
            )
            .ok();
        world
            .run(".", &["profile", "create", "research", "pdf", "git"])
            .ok();
        world
    }

    /// Register `name`, enable `profiles` and update it.
    pub fn workspace(&self, name: &str, profiles: &[&str]) {
        self.run(name, &["repo", "add", "."]).ok();
        if !profiles.is_empty() {
            let mut args = vec!["repo", "enable"];
            args.extend(profiles);
            self.run(name, &args).ok();
        }
        self.run(name, &["repo", "update"]).ok();
    }

    /// Every entry under `rel` (not following links) with its contents, to
    /// check that nothing changed.
    pub fn tree(&self, rel: &str) -> BTreeMap<String, String> {
        let mut found = BTreeMap::new();
        walk(&self.path(rel), Path::new(""), &mut found);
        found
    }

    /// Names in the directory `rel`, sorted; empty if it does not exist.
    pub fn list(&self, rel: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.path(rel))
            .map(|dir| {
                dir.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn walk(dir: &Path, rel: &Path, found: &mut BTreeMap<String, String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        let rel = rel.join(entry.file_name());
        let key = rel.to_string_lossy().into_owned();
        let meta = fs::symlink_metadata(&path).unwrap();
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).unwrap();
            found.insert(key, format!("-> {}", target.display()));
        } else if meta.is_dir() {
            found.insert(key, "<dir>".to_string());
            walk(&path, &rel, found);
        } else {
            let bytes = fs::read(&path).unwrap();
            found.insert(key, String::from_utf8_lossy(&bytes).into_owned());
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

// ----- The `--json` document -----

/// A `--json` run: its exit status, the parsed document and the streams.
pub struct Doc {
    pub code: i32,
    pub value: Value,
    pub stdout: String,
    pub stderr: String,
}

const ERROR_KINDS: &[&str] = &[
    "usage",
    "not_initialized",
    "not_found",
    "already_exists",
    "invalid",
    "conflict",
    "locked",
    "io",
];

impl Doc {
    /// Parse standard output as exactly one JSON value and check the
    /// envelope every document has.
    #[track_caller]
    pub fn from_run(run: Run) -> Doc {
        let context = format!(
            "exit status {}\nstdout:\n{}\nstderr:\n{}",
            run.code, run.stdout, run.stderr
        );
        let value = parse(&run.stdout)
            .unwrap_or_else(|e| panic!("stdout is not one JSON document: {e}\n{context}"));
        let check = |ok: bool, what: &str| assert!(ok, "{what}\n{context}");
        check(
            run.stdout.ends_with("}\n"),
            "the document ends in a newline",
        );
        check(matches!(value, Value::Obj(_)), "the document is an object");
        for key in value.keys() {
            check(
                ["ok", "command", "exit", "data", "error", "notices"].contains(&key),
                &format!("unexpected top-level key {key:?}"),
            );
        }
        for key in ["ok", "command", "exit"] {
            check(value.has(key), &format!("the document lacks {key:?}"));
        }
        let exit = value["exit"].as_i64();
        check(i64::from(run.code) == exit, "`exit` is the exit status");
        check(value["ok"].as_bool() == (exit == 0), "`ok` is `exit == 0`");
        check(
            matches!(value["command"], Value::Null | Value::Str(_)),
            "`command` is a string or null",
        );
        if exit == 2 {
            check(value.has("error"), "a usage error has an `error`");
        }
        if let Some(error) = value.get("error") {
            check(exit != 0, "a document with `error` has a non-zero exit");
            let kind = error["kind"].as_str();
            check(
                ERROR_KINDS.contains(&kind),
                &format!("unknown kind {kind:?}"),
            );
            check(
                (exit == 2) == (kind == "usage"),
                "exit 2 exactly for usage errors",
            );
            check(
                !error["message"].as_str().is_empty(),
                "an error has a message",
            );
            error["hints"].strs();
        }
        if let Some(notices) = value.get("notices") {
            check(!notices.as_array().is_empty(), "`notices` is never empty");
            for notice in notices.as_array() {
                notice["notice"].as_str();
            }
        }
        if exit != 0 {
            check(
                value.has("error") || value.has("data"),
                "a failure says why, in `error` or `data`",
            );
        }
        Doc {
            code: run.code,
            value,
            stdout: run.stdout,
            stderr: run.stderr,
        }
    }

    #[track_caller]
    pub fn ok(self) -> Self {
        self.code(0)
    }

    #[track_caller]
    pub fn code(self, code: i32) -> Self {
        assert_eq!(
            self.code, code,
            "stdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        self
    }

    /// No text on standard error: nothing was asked or explained.
    #[track_caller]
    pub fn quiet(self) -> Self {
        assert!(
            self.stderr.is_empty(),
            "stderr is not empty:\n{}\nstdout:\n{}",
            self.stderr,
            self.stdout
        );
        self
    }

    /// The kinds of the notices, in order.
    pub fn notices(&self) -> Vec<String> {
        self.value.get("notices").map_or(Vec::new(), |notices| {
            notices
                .as_array()
                .iter()
                .map(|n| n["notice"].as_str().to_string())
                .collect()
        })
    }
}

impl Index<&str> for Doc {
    type Output = Value;

    #[track_caller]
    fn index(&self, key: &str) -> &Value {
        match self.value.get(key) {
            Some(value) => value,
            None => panic!("the document has no {key:?}:\n{}", self.stdout),
        }
    }
}

// ----- A strict JSON parser -----

/// A JSON value. Numbers keep their text; objects keep their order.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

/// Parse `text` as exactly one JSON value (RFC 8259), with no duplicate
/// keys, surrounded by nothing but whitespace.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut parser = Parser {
        s: text.as_bytes(),
        pos: 0,
    };
    parser.ws();
    let value = parser.value(0)?;
    parser.ws();
    if parser.pos != parser.s.len() {
        return Err(parser.err("more text after the value"));
    }
    Ok(value)
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        format!("{what} at byte {}", self.pos)
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() != Some(byte) {
            return Err(self.err(&format!("expected `{}`", byte as char)));
        }
        self.pos += 1;
        Ok(())
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > 200 {
            return Err(self.err("nested too deeply"));
        }
        let word = |p: &mut Self, word: &str, value: Value| {
            if !p.s[p.pos..].starts_with(word.as_bytes()) {
                return Err(p.err("expected a value"));
            }
            p.pos += word.len();
            Ok(value)
        };
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Value::Str),
            Some(b't') => word(self, "true", Value::Bool(true)),
            Some(b'f') => word(self, "false", Value::Bool(false)),
            Some(b'n') => word(self, "null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.err("expected a value")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, String> {
        self.eat(b'{')?;
        let mut fields: Vec<(String, Value)> = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Obj(fields));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected a key"));
            }
            let key = self.string()?;
            if fields.iter().any(|(k, _)| *k == key) {
                return Err(self.err(&format!("duplicate key {key:?}")));
            }
            self.ws();
            self.eat(b':')?;
            self.ws();
            fields.push((key, self.value(depth + 1)?));
            self.ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Obj(fields));
                }
                _ => return Err(self.err("expected `,` or `}`")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, String> {
        self.eat(b'[')?;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Arr(items));
                }
                _ => return Err(self.err("expected `,` or `]`")),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            let start = self.pos;
            while self
                .peek()
                .is_some_and(|b| b != b'"' && b != b'\\' && b >= 0x20)
            {
                self.pos += 1;
            }
            // The input is a `str` and runs stop at ASCII bytes, so each run
            // is whole characters.
            out.push_str(std::str::from_utf8(&self.s[start..self.pos]).unwrap());
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    out.push(self.escape()?);
                }
                Some(_) => return Err(self.err("unescaped control character in a string")),
                None => return Err(self.err("unterminated string")),
            }
        }
    }

    fn escape(&mut self) -> Result<char, String> {
        let byte = self.peek().ok_or_else(|| self.err("unterminated escape"))?;
        self.pos += 1;
        Ok(match byte {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let high = self.hex4()?;
                let code = if (0xD800..0xDC00).contains(&high) {
                    if !self.s[self.pos..].starts_with(b"\\u") {
                        return Err(self.err("unpaired surrogate"));
                    }
                    self.pos += 2;
                    let low = self.hex4()?;
                    if !(0xDC00..0xE000).contains(&low) {
                        return Err(self.err("unpaired surrogate"));
                    }
                    0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
                } else {
                    high
                };
                char::from_u32(code).ok_or_else(|| self.err("unpaired surrogate"))?
            }
            _ => return Err(self.err("unknown escape")),
        })
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .s
            .get(self.pos..self.pos + 4)
            .filter(|d| d.iter().all(u8::is_ascii_hexdigit))
            .ok_or_else(|| self.err("expected four hexadecimal digits"))?;
        self.pos += 4;
        Ok(u32::from_str_radix(std::str::from_utf8(digits).unwrap(), 16).unwrap())
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => self.digits()?,
            _ => return Err(self.err("expected a digit")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            self.digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            self.digits()?;
        }
        let text = std::str::from_utf8(&self.s[start..self.pos]).unwrap();
        Ok(Value::Num(text.to_string()))
    }

    fn digits(&mut self) -> Result<(), String> {
        let start = self.pos;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.err("expected a digit"));
        }
        Ok(())
    }
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn keys(&self) -> Vec<&str> {
        match self {
            Value::Obj(fields) => fields.iter().map(|(k, _)| k.as_str()).collect(),
            _ => panic!("not an object: {self}"),
        }
    }

    pub fn is_null(&self) -> bool {
        *self == Value::Null
    }

    #[track_caller]
    pub fn as_str(&self) -> &str {
        self.str_opt()
            .unwrap_or_else(|| panic!("not a string: {self}"))
    }

    pub fn str_opt(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    #[track_caller]
    pub fn as_i64(&self) -> i64 {
        match self {
            Value::Num(n) => n.parse().unwrap_or_else(|_| panic!("not an integer: {n}")),
            _ => panic!("not a number: {self}"),
        }
    }

    #[track_caller]
    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            _ => panic!("not a boolean: {self}"),
        }
    }

    #[track_caller]
    pub fn as_array(&self) -> &[Value] {
        match self {
            Value::Arr(items) => items,
            _ => panic!("not an array: {self}"),
        }
    }

    /// An array of strings.
    #[track_caller]
    pub fn strs(&self) -> Vec<&str> {
        self.as_array().iter().map(Value::as_str).collect()
    }

    /// The element of an array of objects whose `key` is `value`.
    #[track_caller]
    pub fn find(&self, key: &str, value: &str) -> &Value {
        self.as_array()
            .iter()
            .find(|item| item.get(key).and_then(Value::str_opt) == Some(value))
            .unwrap_or_else(|| panic!("no element with {key} = {value:?} in {self}"))
    }
}

impl Index<&str> for Value {
    type Output = Value;

    #[track_caller]
    fn index(&self, key: &str) -> &Value {
        self.get(key)
            .unwrap_or_else(|| panic!("no key {key:?} in {self}"))
    }
}

impl Index<usize> for Value {
    type Output = Value;

    #[track_caller]
    fn index(&self, index: usize) -> &Value {
        self.as_array()
            .get(index)
            .unwrap_or_else(|| panic!("no element {index} in {self}"))
    }
}

/// Compact JSON, for failure messages.
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Num(n) => f.write_str(n),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Arr(items) => {
                f.write_str("[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str("]")
            }
            Value::Obj(fields) => {
                f.write_str("{")?;
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{key:?}:{value}")?;
                }
                f.write_str("}")
            }
        }
    }
}

/// Whether `text` is a fingerprint: 64 lowercase hexadecimal digits.
pub fn is_fingerprint(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `strs` sorted, for lists whose order is not the point.
pub fn sorted(mut items: Vec<&str>) -> Vec<&str> {
    items.sort_unstable();
    items
}

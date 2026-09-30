//! What every command handler gets: the process environment, output, and
//! the Beskar configuration (loaded on first use).

use std::env;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use beskar_core::config::{DEFAULT_LOCK_TIMEOUT, normalize, parse_seconds};
use beskar_core::ops::RepoRef;
use beskar_core::ops::setup::Home;
use beskar_core::{Beskar, Error, ErrorKind, Notice, Notifier};

use crate::json::Json;
use crate::output::{Output, Style};

pub const EXIT_OK: u8 = 0;
pub const EXIT_ERROR: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_CONFLICT: u8 = 3;

/// Why a command did not succeed.
#[derive(Debug)]
pub enum Failure {
    /// A domain error, rendered with its location and hints.
    Error(Error),
    /// The command line was wrong.
    Usage { message: String, hints: Vec<String> },
}

impl Failure {
    pub fn usage(message: impl Into<String>) -> Self {
        Failure::Usage {
            message: message.into(),
            hints: Vec::new(),
        }
    }

    pub fn hint(self, hint: impl Into<String>) -> Self {
        match self {
            Failure::Usage { message, mut hints } => {
                hints.push(hint.into());
                Failure::Usage { message, hints }
            }
            Failure::Error(error) => Failure::Error(error.hint(hint)),
        }
    }
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Failure::Error(error)
    }
}

pub type Outcome = Result<u8, Failure>;

/// Facts about the process that commands depend on.
#[derive(Clone, Debug)]
pub struct Env {
    pub cwd: PathBuf,
    /// The user's home directory, for `~`.
    pub user_home: Option<PathBuf>,
    /// Beskar's home directory: `$BESKAR_HOME`, or `~/.beskar`.
    pub beskar_home: Option<PathBuf>,
    /// Whether questions can be asked (standard input and error are
    /// terminals).
    pub interactive: bool,
    /// Whether standard output is a terminal.
    pub stdout_tty: bool,
    pub stdout_color: bool,
    pub stderr_color: bool,
    /// `$BESKAR_LOCK_TIMEOUT`, overriding the config's `lock-timeout`, or
    /// the text it holds if that is not a number of seconds.
    pub lock_timeout: Option<Result<Duration, String>>,
}

impl Env {
    pub fn from_process() -> Env {
        // An empty path means the working directory is gone; see
        // `App::cwd`.
        let cwd = env::current_dir()
            .map(|dir| fs::canonicalize(&dir).unwrap_or(dir))
            .unwrap_or_default();
        let var_path = |name: &str| {
            env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let user_home = var_path("HOME")
            .or_else(|| var_path("USERPROFILE"))
            .map(|home| fs::canonicalize(&home).unwrap_or(home));
        let beskar_home = var_path("BESKAR_HOME")
            .map(|home| {
                let home = normalize(&cwd.join(home));
                fs::canonicalize(&home).unwrap_or(home)
            })
            .or_else(|| user_home.as_ref().map(|home| home.join(".beskar")));
        let color = env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
            && env::var("TERM").map_or(true, |t| t != "dumb");
        let stdout_tty = io::stdout().is_terminal();
        Env {
            cwd,
            user_home,
            beskar_home,
            interactive: io::stdin().is_terminal() && io::stderr().is_terminal(),
            stdout_tty,
            stdout_color: color && stdout_tty,
            stderr_color: color && io::stderr().is_terminal(),
            lock_timeout: env::var("BESKAR_LOCK_TIMEOUT")
                .ok()
                .filter(|text| !text.trim().is_empty())
                .map(|text| parse_seconds(text.trim()).ok_or(text)),
        }
    }
}

pub struct App {
    pub env: Env,
    pub out: Output,
    /// Whether output is one JSON document (`--json`) instead of text.
    pub json: bool,
    /// The command's result for `--json`.
    data: Option<Json>,
    /// Notices collected for `--json`.
    notices: Arc<Mutex<Vec<Json>>>,
    /// The command being run, such as `repo update`, once it is known.
    pub command: Option<String>,
    loaded: Option<Beskar>,
}

impl App {
    pub fn new(env: Env) -> Self {
        let out = Output::new(env.stdout_color, env.stderr_color);
        App {
            env,
            out,
            json: false,
            data: None,
            notices: Arc::default(),
            command: None,
            loaded: None,
        }
    }

    /// Switch to JSON output: text on standard output is dropped, nothing
    /// is asked, and the result is printed as one JSON document at the end.
    pub fn set_json(&mut self) {
        self.json = true;
        self.env.interactive = false;
        self.env.stdout_color = false;
        self.out = Output::new(false, self.env.stderr_color);
        self.out.silence();
    }

    /// Show text such as help: as it is, or as `{"text": ...}` for `--json`.
    pub fn text(&mut self, text: String) {
        if self.json {
            self.data = Some(Json::obj([("text", Json::from(text))]));
        } else {
            self.out.line(text);
        }
    }

    /// `--no-color`: plain text on both streams.
    pub fn no_color(&mut self) {
        self.env.stdout_color = false;
        self.env.stderr_color = false;
        let silenced = self.json;
        self.out = Output::new(false, false);
        if silenced {
            self.out.silence();
        }
    }

    /// Record the command's result for `--json`. Built only in JSON mode.
    pub fn data(&mut self, json: impl FnOnce() -> Json) {
        if self.json {
            self.data = Some(json());
        }
    }

    /// The result and notices recorded for `--json`.
    pub fn take_json(&mut self) -> (Option<Json>, Vec<Json>) {
        let notices = std::mem::take(&mut *self.notices.lock().unwrap_or_else(|e| e.into_inner()));
        (self.data.take(), notices)
    }

    /// Where Beskar lives and how it waits, for commands that run before
    /// the configuration exists.
    pub fn home_setup(&self) -> Result<(PathBuf, Duration, Notifier), Failure> {
        let dir = self.home()?;
        Ok((
            dir,
            self.lock_timeout()?.unwrap_or(DEFAULT_LOCK_TIMEOUT),
            self.notifier(),
        ))
    }

    /// `$BESKAR_LOCK_TIMEOUT`, if it is set.
    fn lock_timeout(&self) -> Result<Option<Duration>, Failure> {
        match &self.env.lock_timeout {
            None => Ok(None),
            Some(Ok(timeout)) => Ok(Some(*timeout)),
            Some(Err(text)) => Err(Failure::Error(
                Error::invalid(format!(
                    "BESKAR_LOCK_TIMEOUT is `{}`, not a number of seconds",
                    crate::output::clean(text)
                ))
                .hint("set it to a whole number such as 300, or 0 not to wait"),
            )),
        }
    }

    /// Run `f` with the [`Home`] setup commands need.
    pub fn with_home<T>(&self, f: impl FnOnce(&Home) -> T) -> Result<T, Failure> {
        let (dir, lock_timeout, notifier) = self.home_setup()?;
        let home = Home {
            dir: &dir,
            user: self.env.user_home.as_deref(),
            lock_timeout,
            notifier,
        };
        Ok(f(&home))
    }

    /// Where core notices go: standard error for text, the JSON document's
    /// `notices` for `--json` (a wait is announced on standard error too,
    /// since someone may be watching).
    fn notifier(&self) -> Notifier {
        let style = self.out.err_style();
        let home = self.env.user_home.clone();
        let collect = self.json.then(|| Arc::clone(&self.notices));
        Notifier::new(move |notice| {
            if let Some(collected) = &collect {
                if let Ok(mut list) = collected.lock() {
                    list.push(crate::cmd::notice_json(notice));
                }
                if !matches!(notice, Notice::Waiting { .. }) {
                    return;
                }
            }
            for line in crate::cmd::notice_lines(notice, style, home.as_deref()) {
                eprintln!("{line}");
            }
        })
    }

    /// Beskar's home directory.
    pub fn home(&self) -> Result<PathBuf, Failure> {
        self.env.beskar_home.clone().ok_or_else(|| {
            Failure::Error(
                Error::new(
                    ErrorKind::NotInitialized,
                    "cannot tell where Beskar's files go: HOME is not set",
                )
                .hint("set BESKAR_HOME to a directory for Beskar's configuration"),
            )
        })
    }

    /// The configuration and library, loaded once.
    pub fn load(&mut self) -> Result<Beskar, Failure> {
        if self.loaded.is_none() {
            let home = self.home()?;
            let mut beskar =
                Beskar::load(&home, self.env.user_home.as_deref())?.with_notifier(self.notifier());
            if let Some(timeout) = self.lock_timeout()? {
                beskar.config.lock_timeout = timeout;
            }
            self.loaded = Some(beskar);
        }
        Ok(self.loaded.clone().expect("loaded above"))
    }

    /// A path from the command line: `~` expanded, made absolute against
    /// the working directory, and resolved through symlinks if it exists.
    pub fn path_arg(&self, arg: &str) -> Result<PathBuf, Failure> {
        let expanded = match (&self.env.user_home, arg) {
            (Some(home), "~") => home.clone(),
            (Some(home), _) if arg.starts_with("~/") => home.join(&arg[2..]),
            _ => PathBuf::from(arg),
        };
        let absolute = if expanded.is_absolute() {
            normalize(&expanded)
        } else {
            normalize(&self.cwd()?.join(expanded))
        };
        Ok(fs::canonicalize(&absolute).unwrap_or(absolute))
    }

    /// The working directory, which may have been deleted under us.
    pub fn cwd(&self) -> Result<PathBuf, Failure> {
        if self.env.cwd.as_os_str().is_empty() {
            return Err(Failure::Error(
                Error::not_found("the current directory does not exist any more")
                    .hint("`cd` to an existing directory, or pass the path with --repo"),
            ));
        }
        Ok(self.env.cwd.clone())
    }

    /// A path for display, with `~` for the home directory and control
    /// characters made visible.
    pub fn display(&self, path: &Path) -> String {
        crate::output::clean(&beskar_core::config::display_path(
            path,
            self.env.user_home.as_deref(),
        ))
    }

    /// A path as a shell argument in a suggested command.
    pub fn arg(&self, path: &Path) -> String {
        beskar_core::shell_quote(&self.display(path))
    }

    /// The workspace a command acts on: the one `--repo` names, or the one
    /// around the working directory.
    pub fn repo_ref(&self, flag: Option<&str>) -> Result<RepoRef, Failure> {
        match flag {
            Some("") => Err(Failure::usage("`--repo` needs a path")),
            Some(path) => Ok(RepoRef::named(self.path_arg(path)?)),
            None => Ok(RepoRef::here(self.cwd()?)),
        }
    }

    /// Style for questions and other text on standard error.
    pub fn err_style(&self) -> Style {
        self.out.err_style()
    }

    /// Ask a yes/no question on the terminal. `None` when there is no
    /// terminal to ask on.
    pub fn confirm(&self, question: &str, default: bool) -> Option<bool> {
        if !self.env.interactive {
            return None;
        }
        let choices = if default { "[Y/n]" } else { "[y/N]" };
        self.out.prompt(&format!("{question} {choices} "));
        let answer = read_answer()?;
        Some(match answer.as_str() {
            "" => default,
            "y" | "yes" => true,
            _ => false,
        })
    }

    /// Ask for one of several single-letter choices. `None` on end of input
    /// or without a terminal.
    pub fn choose(&self, question: &str, choices: &[char]) -> Option<char> {
        if !self.env.interactive {
            return None;
        }
        loop {
            self.out.prompt(question);
            let answer = read_answer()?;
            if let Some(c) = answer.chars().next()
                && answer.chars().count() == 1
                && choices.contains(&c)
            {
                return Some(c);
            }
            let list: Vec<String> = choices.iter().map(char::to_string).collect();
            self.out
                .err_line(format!("Please answer {}.", list.join(", ")));
        }
    }

    /// The width available for a line, if output goes to a terminal:
    /// `$COLUMNS`, else what the terminal reports, else 80.
    pub fn width(&self) -> Option<usize> {
        self.env.stdout_tty.then(|| {
            env::var("COLUMNS")
                .ok()
                .and_then(|c| c.parse().ok())
                .or_else(terminal_width)
                .unwrap_or(80)
        })
    }
}

/// Ask the terminal for its width with `stty size`, the one query that
/// needs no system call wrapper.
#[cfg(unix)]
fn terminal_width() -> Option<usize> {
    let tty = fs::File::open("/dev/tty").ok()?;
    let output = std::process::Command::new("stty")
        .arg("size")
        .stdin(tty)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    text.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(not(unix))]
fn terminal_width() -> Option<usize> {
    None
}

fn read_answer() -> Option<String> {
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => {
            let answer = line.trim().to_lowercase();
            // Answers piped in are not echoed by a terminal; echo them so
            // the conversation reads the same in a log.
            if !io::stdin().is_terminal() {
                eprintln!("{answer}");
            }
            Some(answer)
        }
    }
}

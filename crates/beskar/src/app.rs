//! What every command handler gets: the process environment, output, and
//! the Beskar configuration (loaded on first use).

use std::env;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use beskar_core::config::normalize;
use beskar_core::{Beskar, Error, ErrorKind, Registry};

use crate::output::Output;

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
}

impl Env {
    pub fn from_process() -> Env {
        let cwd = env::current_dir()
            .map(|dir| fs::canonicalize(&dir).unwrap_or(dir))
            .unwrap_or_else(|_| PathBuf::from("."));
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
        }
    }
}

pub struct App {
    pub env: Env,
    pub out: Output,
    loaded: Option<Beskar>,
}

impl App {
    pub fn new(env: Env) -> Self {
        let out = Output::new(env.stdout_color, env.stderr_color);
        App {
            env,
            out,
            loaded: None,
        }
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
            self.loaded = Some(Beskar::load(&home, self.env.user_home.as_deref())?);
        }
        Ok(self.loaded.clone().expect("loaded above"))
    }

    /// A path from the command line: `~` expanded, made absolute against
    /// the working directory, and resolved through symlinks if it exists.
    pub fn path_arg(&self, arg: &str) -> PathBuf {
        let expanded = match (&self.env.user_home, arg) {
            (Some(home), "~") => home.clone(),
            (Some(home), _) if arg.starts_with("~/") => home.join(&arg[2..]),
            _ => PathBuf::from(arg),
        };
        let absolute = normalize(&self.env.cwd.join(expanded));
        fs::canonicalize(&absolute).unwrap_or(absolute)
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

    /// The registered workspace that `--repo` names, or that contains the
    /// working directory.
    pub fn repo(&self, registry: &Registry, flag: Option<&str>) -> Result<PathBuf, Failure> {
        let dir = match flag {
            Some("") => return Err(Failure::usage("`--repo` needs a path")),
            Some(path) => self.path_arg(path),
            None => self.env.cwd.clone(),
        };
        if flag.is_some() && !dir.is_dir() {
            return Err(Failure::Error(Error::not_found(format!(
                "{} does not exist",
                self.display(&dir)
            ))));
        }
        match registry.containing(&dir) {
            Some(entry) => Ok(entry.path.clone()),
            None => {
                let error = Error::not_found(format!(
                    "{} is not inside a registered workspace",
                    self.display(&dir)
                ));
                Err(Failure::Error(if flag.is_some() {
                    error.hint(format!(
                        "register it with `beskar repo add {}`",
                        self.arg(&dir)
                    ))
                } else {
                    error
                        .hint("register this directory with `beskar repo add .`")
                        .hint("or point at a workspace with --repo <path>")
                }))
            }
        }
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
        Ok(_) => Some(line.trim().to_lowercase()),
    }
}

//! Where Beskar keeps its files, and the settings a person can change.
//!
//! Everything Beskar owns lives in one folder, `~/.beskar` unless `BESKAR_HOME`
//! says otherwise:
//!
//! ```text
//! ~/.beskar/
//!   config.bsk      settings (this module)
//!   registry.bsk    machine-local state: repositories and what is installed
//!   library/        skills and profiles, safe to keep in Git
//! ```
//!
//! The settings file can move the library and the registry elsewhere.

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use bsk::{Cardinality, Diagnostic, Diagnostics, Document, Schema};

use crate::error::{Error, ErrorKind, Result};
use crate::fsx::{self, FileLock};
use crate::library::Library;

/// Expands a leading `~` or `~/` to the user's home directory.
fn expand_tilde(path: &Path, user_home: Option<&Path>) -> PathBuf {
    let Some(home) = user_home else {
        return path.to_path_buf();
    };
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

/// The default place inside a repository where skills are installed.
pub const DEFAULT_AGENT_SKILLS: &str = ".agents/skills";

/// The parts of the process environment that decide where Beskar looks.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// The user's home directory.
    pub user_home: Option<PathBuf>,
    /// The `BESKAR_HOME` override.
    pub beskar_home: Option<PathBuf>,
}

impl Env {
    /// Reads `HOME` (or `USERPROFILE`) and `BESKAR_HOME` from the process environment.
    ///
    /// `BESKAR_HOME` may be relative or start with a `~` that the shell did not expand. Either way it
    /// is turned into an absolute path here, once, so that every later command finds the same folder
    /// no matter where it runs.
    pub fn from_process() -> Env {
        let get = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let user_home = get("HOME").or_else(|| get("USERPROFILE"));
        let beskar_home = get("BESKAR_HOME").map(|path| {
            let expanded = expand_tilde(&path, user_home.as_deref());
            if expanded.is_absolute() {
                expanded
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(&expanded))
                    .unwrap_or(expanded)
            }
        });
        Env {
            user_home,
            beskar_home,
        }
    }
}

/// The folder that holds everything Beskar owns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    /// A home at a known path.
    pub fn at(root: impl Into<PathBuf>) -> Home {
        Home { root: root.into() }
    }

    /// Finds the home: an explicit path wins, then `BESKAR_HOME`, then `~/.beskar`.
    /// Fails if the place it points at is a file.
    pub fn locate(explicit: Option<&Path>, env: &Env) -> Result<Home> {
        let root = match (explicit, &env.beskar_home, &env.user_home) {
            (Some(path), _, _) => path.to_path_buf(),
            (None, Some(path), _) => path.clone(),
            (None, None, Some(home)) => home.join(".beskar"),
            (None, None, None) => {
                return Err(Error::invalid(
                    "cannot find your home directory because HOME is not set",
                )
                .with_hint("set BESKAR_HOME to the folder Beskar should use"));
            }
        };
        if root.exists() && !root.is_dir() {
            return Err(
                Error::invalid(format!("'{}' is a file, not a folder", root.display()))
                    .with_hint("point BESKAR_HOME or --home at a folder"),
            );
        }
        Ok(Home::at(root))
    }

    /// The folder itself.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The settings file.
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.bsk")
    }

    /// Where the registry lives unless the settings say otherwise.
    pub fn default_registry_file(&self) -> PathBuf {
        self.root.join("registry.bsk")
    }

    /// Where the library lives unless the settings say otherwise.
    pub fn default_library_dir(&self) -> PathBuf {
        self.root.join("library")
    }
}

/// What to do when an installed skill has local changes that an update would overwrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Ask, when a person is at the terminal. Otherwise behave like `Fail`.
    Ask,
    /// Stop before changing anything in the repository.
    Fail,
    /// Leave the locally modified skill alone and update the rest.
    Keep,
    /// Overwrite (or remove) the local copy with the library's version.
    Replace,
}

impl ConflictPolicy {
    /// Every policy, in the order they are documented.
    pub const ALL: [ConflictPolicy; 4] = [
        ConflictPolicy::Ask,
        ConflictPolicy::Fail,
        ConflictPolicy::Keep,
        ConflictPolicy::Replace,
    ];

    /// The word used in settings and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            ConflictPolicy::Ask => "ask",
            ConflictPolicy::Fail => "fail",
            ConflictPolicy::Keep => "keep",
            ConflictPolicy::Replace => "replace",
        }
    }

    /// One line explaining the policy.
    pub fn describe(self) -> &'static str {
        match self {
            ConflictPolicy::Ask => "ask at the terminal; fail when nobody can answer",
            ConflictPolicy::Fail => "stop before changing the repository",
            ConflictPolicy::Keep => "keep the local copy and update the rest",
            ConflictPolicy::Replace => "overwrite local changes with the library version",
        }
    }
}

impl fmt::Display for ConflictPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for ConflictPolicy {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        ConflictPolicy::ALL
            .into_iter()
            .find(|p| p.name() == text)
            .ok_or_else(|| {
                let names: Vec<&str> = ConflictPolicy::ALL.iter().map(|p| p.name()).collect();
                let hint = match bsk::closest(text, names.iter().copied()) {
                    Some(near) => format!("did you mean '{near}'?"),
                    None => format!("choose one of: {}", names.join(", ")),
                };
                Error::invalid(format!("unknown conflict policy '{text}'")).with_hint(hint)
            })
    }
}

/// Beskar's settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The library folder.
    pub library: PathBuf,
    /// The registry file.
    pub registry: PathBuf,
    /// Where skills are installed inside a repository, relative to the repository root.
    pub agent_skills: PathBuf,
    /// What to do about locally modified skills.
    pub on_conflict: ConflictPolicy,
}

/// The names of the settings in `config.bsk`.
pub const KEYS: [&str; 4] = ["library", "registry", "agent-skills", "on-conflict"];

impl Config {
    /// The settings used when the file says nothing.
    pub fn defaults(home: &Home) -> Config {
        Config {
            library: home.default_library_dir(),
            registry: home.default_registry_file(),
            agent_skills: PathBuf::from(DEFAULT_AGENT_SKILLS),
            on_conflict: ConflictPolicy::Ask,
        }
    }

    fn schema() -> Schema {
        KEYS.iter().fold(Schema::new(), |schema, key| {
            schema.key(key, Cardinality::Optional)
        })
    }

    /// Reads the settings file. Fails with `NotInitialized` if it does not exist.
    pub fn load(home: &Home, env: &Env) -> Result<Config> {
        let file = home.config_file();
        let Some(text) = fsx::read_to_string_if_exists(&file)? else {
            return Err(Error::new(
                ErrorKind::NotInitialized,
                format!(
                    "Beskar is not set up yet: {} does not exist",
                    file.display()
                ),
            )
            .with_hint("run 'beskar init'"));
        };
        Config::parse(&text, home, env.user_home.as_deref())
            .map_err(|problems| Error::bsk(&file, &text, &problems))
    }

    /// Reads settings from text. Relative paths start at the home folder.
    pub fn parse(
        text: &str,
        home: &Home,
        user_home: Option<&Path>,
    ) -> std::result::Result<Config, Diagnostics> {
        let doc = Document::parse(text)?;
        Config::schema().check(&doc)?;
        let mut config = Config::defaults(home);
        let mut problems = Vec::new();
        for entry in doc.root().entries() {
            let value = entry.value();
            let outcome: std::result::Result<(), (String, Option<String>)> = match entry.key() {
                "library" => path_setting(value, home, user_home).map(|p| config.library = p),
                "registry" => path_setting(value, home, user_home).map(|p| config.registry = p),
                "agent-skills" => relative_setting(value).map(|p| config.agent_skills = p),
                "on-conflict" => value
                    .parse::<ConflictPolicy>()
                    .map(|p| config.on_conflict = p)
                    .map_err(|e| (e.message().to_string(), e.hint().map(str::to_string))),
                _ => Ok(()),
            };
            if let Err((message, hint)) = outcome {
                problems.push(value_problem(&entry, message, hint));
            }
        }
        match Diagnostics::from_vec(problems) {
            Some(problems) => Err(problems),
            None => Ok(config),
        }
    }

    /// The settings file as text, with comments that explain each setting.
    pub fn render(&self, user_home: Option<&Path>) -> Result<String> {
        let show = |path: &Path| shorten(path, user_home);
        let mut doc = Document::new();
        doc.push_comment("Beskar settings. Every line is 'key value'; lines that start with # are comments.\nRun 'beskar help format' for the file format.");
        doc.push_blank();
        doc.push_comment("Where your skills and profiles live. Point this at a Git checkout to sync it between machines.\nRelative paths start at the folder that holds this file.");
        push(&mut doc, "library", &show(&self.library))?;
        doc.push_blank();
        doc.push_comment("Machine-local record of your repositories and what is installed in them. Keep it out of the library.");
        push(&mut doc, "registry", &show(&self.registry))?;
        doc.push_blank();
        doc.push_comment(
            "Where skills are installed inside each repository, relative to the repository root.",
        );
        push(
            &mut doc,
            "agent-skills",
            &self.agent_skills.to_string_lossy(),
        )?;
        doc.push_blank();
        doc.push_comment(&format!(
            "What to do when an installed skill has local changes that an update would overwrite.\n{}",
            ConflictPolicy::ALL.map(|p| format!("{}: {}", p.name(), p.describe())).join("\n")
        ));
        push(&mut doc, "on-conflict", self.on_conflict.name())?;
        Ok(doc.to_string())
    }

    /// Changes one setting in the settings file, leaving comments and layout alone.
    pub fn set(home: &Home, env: &Env, key: &str, value: &str) -> Result<Config> {
        if !KEYS.contains(&key) {
            let hint = match bsk::closest(key, KEYS) {
                Some(near) => format!("did you mean '{near}'?"),
                None => format!("settings: {}", KEYS.join(", ")),
            };
            return Err(Error::invalid(format!("unknown setting '{key}'")).with_hint(hint));
        }
        let file = home.config_file();
        let _lock = FileLock::acquire(
            &home.root().join("locks/config.lock"),
            Duration::from_secs(10),
        )?;
        let text = fsx::read_to_string(&file).map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                Error::new(ErrorKind::NotInitialized, "Beskar is not set up yet")
                    .with_hint("run 'beskar init'")
            } else {
                e
            }
        })?;
        // The file has to be valid before the change, or the change would be blamed for an old mistake.
        Config::parse(&text, home, env.user_home.as_deref())
            .map_err(|problems| Error::bsk(&file, &text, &problems))?;
        let mut doc = Document::parse(&text).map_err(|p| Error::bsk(&file, &text, &p))?;
        doc.root_mut()
            .set(key, value)
            .map_err(|e| Error::invalid(format!("cannot store that value: {e}")))?;
        let updated = doc.to_string();
        let config =
            Config::parse(&updated, home, env.user_home.as_deref()).map_err(|problems| {
                let first = problems.first();
                Error::invalid(format!("invalid value for '{key}': {}", first.message())).with_hint(
                    first
                        .hint()
                        .unwrap_or("see 'beskar config' for the current settings")
                        .to_string(),
                )
            })?;
        if key == "library"
            && let Some(problem) = Library::location_problem(&config.library)
        {
            return Err(
                Error::invalid(format!("the library cannot live here: {problem}"))
                    .with_hint("pick a folder that agents do not scan, such as ~/.beskar/library"),
            );
        }
        fsx::write_atomic(&file, &updated)?;
        Ok(config)
    }
}

fn push(doc: &mut Document, key: &str, value: &str) -> Result<()> {
    doc.push_entry(key, value).map_err(|e| {
        Error::invalid(format!(
            "'{value}' cannot be stored in the settings file: {e}"
        ))
        .with_hint("choose a path without line breaks or spaces at either end")
    })
}

fn value_problem(entry: &bsk::Entry<'_>, message: String, hint: Option<String>) -> Diagnostic {
    let diagnostic = entry.diagnostic(message);
    match hint {
        Some(hint) if diagnostic.hint().is_none() => diagnostic.with_hint(hint),
        _ => diagnostic,
    }
}

type SettingError = (String, Option<String>);

/// True if the value contains a comment introduced by a space and a `#`, as in `value # note`.
/// bsk has no trailing comments, so such a value would silently become part of the path.
pub(crate) fn has_trailing_comment(value: &str) -> bool {
    value.match_indices('#').any(|(at, _)| {
        at > 0
            && value[..at].ends_with([' ', '\t'])
            && value[at + 1..]
                .chars()
                .next()
                .is_none_or(|c| c == ' ' || c == '\t')
    })
}

/// Catches the usual ways people write `key value` wrongly, before they turn into odd folder names.
fn suspicious_path(value: &str) -> Option<SettingError> {
    if value.starts_with(['=', ':']) {
        return Some((
            format!("'{value}' is not a path"),
            Some("write the path directly after the key, with no '=' or ':'".to_string()),
        ));
    }
    if value.starts_with(['"', '\'']) {
        return Some((
            format!("'{value}' is quoted"),
            Some("bsk has no quoting; write the path as it is, spaces included".to_string()),
        ));
    }
    if has_trailing_comment(value) {
        return Some((
            format!("'{value}' has a comment after the path"),
            Some("bsk has no trailing comments; put the comment on its own line".to_string()),
        ));
    }
    None
}

fn path_setting(
    value: &str,
    home: &Home,
    user_home: Option<&Path>,
) -> std::result::Result<PathBuf, SettingError> {
    if value.is_empty() {
        return Err(("this setting needs a path".to_string(), None));
    }
    if let Some(problem) = suspicious_path(value) {
        return Err(problem);
    }
    let expanded = match value.strip_prefix('~') {
        Some("") => user_home.map(Path::to_path_buf),
        Some(rest) if rest.starts_with('/') || rest.starts_with(std::path::MAIN_SEPARATOR) => {
            user_home.map(|h| h.join(rest.trim_start_matches(['/', std::path::MAIN_SEPARATOR])))
        }
        Some(_) => {
            return Err((
                format!("'{value}' uses ~user, which is not supported"),
                Some("write the full path instead".to_string()),
            ));
        }
        None => Some(PathBuf::from(value)),
    };
    let Some(path) = expanded else {
        return Err((
            "cannot expand '~' because HOME is not set".to_string(),
            None,
        ));
    };
    Ok(if path.is_absolute() {
        path
    } else {
        home.root().join(path)
    })
}

fn relative_setting(value: &str) -> std::result::Result<PathBuf, SettingError> {
    if let Some(problem) = suspicious_path(value) {
        return Err(problem);
    }
    let path = Path::new(value);
    let ok = !value.is_empty()
        && path.components().all(|c| matches!(c, Component::Normal(_)))
        && !path.is_absolute();
    if ok {
        Ok(path.components().collect())
    } else {
        Err((
            format!("'{value}' is not a folder inside the repository"),
            Some("use a relative path without '.' or '..', for example .agents/skills".to_string()),
        ))
    }
}

/// Writes a path with `~` for the user's home directory, the way a person would type it.
pub fn shorten(path: &Path, user_home: Option<&Path>) -> String {
    if let Some(home) = user_home {
        if path == home {
            return "~".to_string();
        }
        if let Ok(rest) = path.strip_prefix(home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn home() -> Home {
        Home::at("/data/beskar")
    }

    fn parse(text: &str) -> std::result::Result<Config, Diagnostics> {
        Config::parse(text, &home(), Some(Path::new("/home/me")))
    }

    fn first_problem(text: &str) -> String {
        let problems = parse(text).unwrap_err();
        let d = problems.first();
        format!(
            "{}:{} {} [{}]",
            d.line(),
            d.column(),
            d.message(),
            d.hint().unwrap_or("")
        )
    }

    #[test]
    fn an_empty_file_gives_the_defaults() {
        let config = parse("# nothing here\n").unwrap();
        assert_eq!(config, Config::defaults(&home()));
        assert_eq!(config.library, Path::new("/data/beskar/library"));
        assert_eq!(config.registry, Path::new("/data/beskar/registry.bsk"));
        assert_eq!(config.agent_skills, Path::new(".agents/skills"));
        assert_eq!(config.on_conflict, ConflictPolicy::Ask);
    }

    #[test]
    fn reads_every_setting() {
        let config = parse("library ~/skills lib\nregistry /var/lib/beskar/reg.bsk\nagent-skills .claude/skills\non-conflict keep\n").unwrap();
        assert_eq!(config.library, Path::new("/home/me/skills lib"));
        assert_eq!(config.registry, Path::new("/var/lib/beskar/reg.bsk"));
        assert_eq!(config.agent_skills, Path::new(".claude/skills"));
        assert_eq!(config.on_conflict, ConflictPolicy::Keep);
    }

    #[test]
    fn relative_paths_start_at_the_home_folder() {
        let config = parse("library ../shared/skills\nregistry state/registry.bsk\n").unwrap();
        assert_eq!(config.library, Path::new("/data/beskar/../shared/skills"));
        assert_eq!(
            config.registry,
            Path::new("/data/beskar/state/registry.bsk")
        );
    }

    #[test]
    fn a_lone_tilde_is_the_home_directory() {
        assert_eq!(parse("library ~\n").unwrap().library, Path::new("/home/me"));
        assert_eq!(
            parse("library ~/\n").unwrap().library,
            Path::new("/home/me")
        );
    }

    #[test]
    fn tilde_needs_a_home_directory() {
        let problems = Config::parse("library ~/x\n", &home(), None).unwrap_err();
        assert_eq!(
            problems.first().message(),
            "cannot expand '~' because HOME is not set"
        );
        assert_eq!(
            first_problem("library ~other/x\n"),
            "1:9 '~other/x' uses ~user, which is not supported [write the full path instead]"
        );
    }

    #[test]
    fn unknown_keys_get_suggestions() {
        assert_eq!(
            first_problem("libary /x\n"),
            "1:1 unknown key 'libary' [did you mean 'library'?]"
        );
        assert_eq!(
            first_problem("on_conflict keep\n"),
            "1:1 unknown key 'on_conflict' [did you mean 'on-conflict'?]"
        );
    }

    #[test]
    fn a_setting_may_appear_only_once() {
        assert_eq!(
            first_problem("library /a\nlibrary /b\n"),
            "2:1 'library' appears more than once [the first one is on line 1; keep only one]"
        );
    }

    #[test]
    fn bad_policies_are_explained() {
        assert_eq!(
            first_problem("on-conflict kep\n"),
            "1:13 unknown conflict policy 'kep' [did you mean 'keep'?]"
        );
        assert!(
            first_problem("on-conflict zzzzzzzz\n")
                .contains("choose one of: ask, fail, keep, replace")
        );
    }

    #[test]
    fn equals_sign_mistakes_get_the_specific_hint() {
        assert!(first_problem("on-conflict = keep\n").contains("no '=' or ':'"));
    }

    #[test]
    fn agent_skills_must_stay_inside_the_repository() {
        for bad in ["/etc/skills", "../skills", "a/../b", "./skills", ""] {
            let text = format!("agent-skills {bad}\n");
            assert!(parse(&text).is_err(), "{bad:?} should be rejected");
        }
        assert_eq!(
            parse("agent-skills skills\n").unwrap().agent_skills,
            Path::new("skills")
        );
        assert_eq!(
            parse("agent-skills a//b/\n").unwrap().agent_skills,
            Path::new("a/b")
        );
    }

    #[test]
    fn rendered_settings_parse_back_to_the_same_config() {
        let config = Config {
            library: PathBuf::from("/home/me/dotfiles/beskar library"),
            registry: PathBuf::from("/data/beskar/registry.bsk"),
            agent_skills: PathBuf::from(".claude/skills"),
            on_conflict: ConflictPolicy::Replace,
        };
        let text = config.render(Some(Path::new("/home/me"))).unwrap();
        assert!(
            text.contains("library ~/dotfiles/beskar library\n"),
            "{text}"
        );
        assert_eq!(parse(&text).unwrap(), config);
        assert!(text.contains("# Beskar settings."));
    }

    #[test]
    fn shorten_uses_tilde_only_inside_the_home_directory() {
        let home = Some(Path::new("/home/me"));
        assert_eq!(shorten(Path::new("/home/me"), home), "~");
        assert_eq!(shorten(Path::new("/home/me/a/b"), home), "~/a/b");
        assert_eq!(shorten(Path::new("/home/mearch/a"), home), "/home/mearch/a");
        assert_eq!(shorten(Path::new("/x"), None), "/x");
    }

    #[test]
    fn locate_prefers_explicit_then_env_then_default() {
        let env = Env {
            user_home: Some(PathBuf::from("/home/me")),
            beskar_home: Some(PathBuf::from("/opt/beskar")),
        };
        assert_eq!(
            Home::locate(Some(Path::new("/x")), &env).unwrap().root(),
            Path::new("/x")
        );
        assert_eq!(
            Home::locate(None, &env).unwrap().root(),
            Path::new("/opt/beskar")
        );
        let env = Env {
            user_home: Some(PathBuf::from("/home/me")),
            beskar_home: None,
        };
        assert_eq!(
            Home::locate(None, &env).unwrap().root(),
            Path::new("/home/me/.beskar")
        );
        let e = Home::locate(None, &Env::default()).unwrap_err();
        assert_eq!(
            e.hint(),
            Some("set BESKAR_HOME to the folder Beskar should use")
        );
    }

    #[test]
    fn policy_names_round_trip() {
        for policy in ConflictPolicy::ALL {
            assert_eq!(policy.name().parse::<ConflictPolicy>().unwrap(), policy);
            assert!(!policy.describe().is_empty());
        }
    }

    #[test]
    fn load_reports_a_missing_file_as_not_initialised() {
        let dir = TempDir::new("cfg");
        let e = Config::load(&Home::at(dir.path()), &Env::default()).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotInitialized);
        assert_eq!(e.hint(), Some("run 'beskar init'"));
    }

    #[test]
    fn load_quotes_the_offending_line() {
        let dir = TempDir::new("cfg");
        dir.write("config.bsk", "library /x\non-conflict: keep\n");
        let e = Config::load(&Home::at(dir.path()), &Env::default()).unwrap_err();
        assert!(
            e.message().contains("2 | on-conflict: keep"),
            "{}",
            e.message()
        );
        assert!(e.message().contains("config.bsk is not valid"));
    }

    #[test]
    fn set_edits_in_place_and_keeps_comments() {
        let dir = TempDir::new("cfg");
        dir.write(
            "config.bsk",
            "# my notes\nlibrary /x\n\n# policy\non-conflict ask\n",
        );
        let home = Home::at(dir.path());
        let config = Config::set(&home, &Env::default(), "on-conflict", "keep").unwrap();
        assert_eq!(config.on_conflict, ConflictPolicy::Keep);
        assert_eq!(
            dir.read("config.bsk"),
            "# my notes\nlibrary /x\n\n# policy\non-conflict keep\n"
        );
    }

    #[test]
    fn set_appends_a_missing_setting() {
        let dir = TempDir::new("cfg");
        dir.write("config.bsk", "library /x\n");
        Config::set(
            &Home::at(dir.path()),
            &Env::default(),
            "agent-skills",
            ".claude/skills",
        )
        .unwrap();
        assert_eq!(
            dir.read("config.bsk"),
            "library /x\nagent-skills .claude/skills\n"
        );
    }

    #[test]
    fn set_refuses_bad_input_and_leaves_the_file_alone() {
        let dir = TempDir::new("cfg");
        dir.write("config.bsk", "library /x\n");
        let home = Home::at(dir.path());
        let e = Config::set(&home, &Env::default(), "on-conflict", "sometimes").unwrap_err();
        assert!(
            e.message().starts_with("invalid value for 'on-conflict'"),
            "{}",
            e.message()
        );
        let e = Config::set(&home, &Env::default(), "libary", "/y").unwrap_err();
        assert_eq!(e.hint(), Some("did you mean 'library'?"));
        let e = Config::set(&home, &Env::default(), "library", "two\nlines").unwrap_err();
        assert!(e.message().contains("line break"));
        assert_eq!(dir.read("config.bsk"), "library /x\n");
    }
}

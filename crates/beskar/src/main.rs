mod output;

use beskar::app::{App, filename};
use beskar::fs;
use beskar::model::validate_name;
use beskar::reconcile::{self, Action, ConflictPolicy, Plan};
use beskar::{Error, Result};
use output::{Json, Report, arr, obj, s};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

const HELP: &str = r#"Beskar: manage skills once, deploy profiles to local workspaces.

Usage: beskar [--home PATH] [--json] COMMAND

  init [--library PATH]              Initialize config, library, and registry
  doctor                            Check references, copies, and pending updates
  library init [--library PATH]      Same bootstrap as init
  library add PATH [--name NAME]     Copy one directory into the library
  library scan PATH [--yes]          Discover SKILL.md directories and confirm import
               [--dry-run]          List discoveries without importing
  library list | show SKILL | remove SKILL
  profile create NAME | delete NAME | list | show NAME
  profile add NAME SKILL...          Add skills to a profile
  profile remove NAME SKILL...       Remove profile membership
  repo add [PATH]                    Register a workspace, default current directory
  repo remove [PATH]                 Forget it; leave its files in place
  repo list
  repo enable PROFILE...            Change desired profiles, without copying files
  repo disable PROFILE... | toggle PROFILE...
  repo status [--all]                Compare desired, installed, and local contents
  repo update [--all] [--dry-run] [--conflict fail|keep|replace]
  registry list [--profile NAME] [--skill NAME]
  registry status | stats
  registry update --all [--dry-run] [--conflict fail|keep|replace]
  registry prune [--dry-run]         Forget missing workspaces; keep existing ones
  status [--all] | update [--all]    Short forms of repo status and repo update

Options:
  --repo PATH      Select an exact registered workspace for repo operations
  --home PATH      State directory; otherwise BESKAR_HOME, then ~/.beskar
  --json           One JSON document on stdout, including errors; no prompts
  --help, -h       Show this help
  --version, -V    Show version
  --              End options; following arguments are literal

Update defaults to refusing local changes. 'keep' leaves their original baseline
and reports them again next time. 'replace' explicitly discards managed local
changes. Unmanaged paths are never overwritten by any policy.

Names: 1 to 64 lowercase letters, digits, '-' or '_'; begin with a letter or digit.
Format: .bsk files start with 'beskar 1'. Each record is 'key literal value'.
Only full-line # comments exist. No quoting, escaping, or environment expansion.

Exit codes: 0 success; 1 operational error; 2 invalid CLI syntax;
            3 status needs attention, or a dry-run found conflicts.
"#;

#[derive(Default)]
struct Args {
    words: Vec<String>,
    options: BTreeMap<String, String>,
}

impl Args {
    fn parse(raw: Vec<String>) -> Result<Self> {
        let mut args = Self::default();
        let mut iter = raw.into_iter();
        let mut literal = false;
        while let Some(arg) = iter.next() {
            if literal {
                args.words.push(arg);
                continue;
            }
            if arg == "--" {
                literal = true;
                continue;
            }
            if !arg.starts_with('-') || arg == "-" {
                args.words.push(arg);
                continue;
            }
            let (key, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(k, v)| (k, Some(v.to_owned())));
            let key = match key {
                "-h" => "--help",
                "-V" => "--version",
                other => other,
            };
            let value = match key {
                "--home" | "--repo" | "--library" | "--name" | "--conflict" | "--profile"
                | "--skill" => {
                    let value = inline
                        .or_else(|| iter.next())
                        .ok_or_else(|| Error(format!("{key} needs a value")))?;
                    if value.is_empty() || value.starts_with("--") {
                        return Err(Error(format!("{key} needs a value")));
                    }
                    value
                }
                "--all" | "--dry-run" | "--yes" | "--json" | "--help" | "--version" => {
                    if inline.is_some() {
                        return Err(Error(format!("{key} does not take a value")));
                    }
                    String::new()
                }
                _ => return Err(Error(format!("unknown option {key}; use --help"))),
            };
            if args.options.insert(key.into(), value).is_some() {
                return Err(Error(format!("duplicate option {key}")));
            }
        }
        Ok(args)
    }
    fn has(&self, key: &str) -> bool {
        self.options.contains_key(key)
    }
    fn get(&self, key: &str) -> Option<&str> {
        self.options.get(key).map(String::as_str)
    }
    fn allow(&self, extra: &[&str]) -> Result<()> {
        for key in self.options.keys() {
            if !["--home", "--json", "--help", "--version"].contains(&key.as_str())
                && !extra.contains(&key.as_str())
            {
                return Err(Error(format!("option {key} is not valid for this command")));
            }
        }
        Ok(())
    }
    fn count(&self, min: usize, max: usize, usage: &str) -> Result<()> {
        if self.words.len() < min || self.words.len() > max {
            return Err(Error(format!("usage: beskar {usage}")));
        }
        Ok(())
    }
    fn home(&self) -> Result<PathBuf> {
        if let Some(path) = self.get("--home") {
            return Ok(path.into());
        }
        if let Some(path) = std::env::var_os("BESKAR_HOME") {
            if path.is_empty() {
                return Err(Error("BESKAR_HOME cannot be empty".into()));
            }
            return Ok(path.into());
        }
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|p| PathBuf::from(p).join(".beskar"))
            .ok_or_else(|| Error("set BESKAR_HOME or use --home PATH".into()))
    }
    fn repo(&self, app: &App, positional: Option<&str>) -> Result<PathBuf> {
        if positional.is_some() && self.has("--repo") {
            return Err(Error(
                "use either a workspace argument or --repo, not both".into(),
            ));
        }
        if let Some(path) = positional.or(self.get("--repo")) {
            let path = fs::absolute(Path::new(path))?;
            if app.registry.repos.contains_key(&path) {
                return Ok(path);
            }
            return Err(Error(format!(
                "workspace is not registered: {}",
                path.display()
            )));
        }
        app.repository(&std::env::current_dir()?)
    }
}

fn main() {
    let raw: Result<Vec<_>> = std::env::args_os()
        .skip(1)
        .map(|s| {
            s.into_string()
                .map_err(|_| Error("arguments must be UTF-8".into()))
        })
        .collect();
    let wants_json = raw
        .as_ref()
        .is_ok_and(|v| v.iter().take_while(|s| *s != "--").any(|s| s == "--json"));
    let args = match raw.and_then(Args::parse) {
        Ok(args) => args,
        Err(e) => {
            emit_error(&e, wants_json, 2);
            return;
        }
    };
    match run(&args) {
        Ok(report) => {
            let text = if args.has("--json") {
                report.data.render()
            } else {
                report.human
            };
            let result = writeln!(std::io::stdout().lock(), "{text}");
            if result.is_err_and(|e| e.kind() != std::io::ErrorKind::BrokenPipe) {
                std::process::exit(1);
            }
            std::process::exit(report.code);
        }
        Err(e) => emit_error(
            &e,
            args.has("--json"),
            if e.0.starts_with("usage:")
                || e.0.starts_with("option ")
                || e.0.starts_with("unknown command")
            {
                2
            } else {
                1
            },
        ),
    }
}

fn emit_error(error: &Error, json: bool, code: i32) {
    if json {
        let _ = writeln!(
            std::io::stdout().lock(),
            "{}",
            obj([("ok", Json::Bool(false)), ("error", s(&error.0))]).render()
        );
    } else {
        let _ = writeln!(std::io::stderr().lock(), "error: {error}");
    }
    std::process::exit(code);
}

fn success(message: impl Into<String>) -> Report {
    let human = message.into();
    Report::new(
        obj([("ok", Json::Bool(true)), ("message", s(&human))]),
        human,
    )
}

fn run(args: &Args) -> Result<Report> {
    if args.has("--help")
        || (args.words.is_empty() && !args.has("--version"))
        || args.words.first().is_some_and(|w| w == "help")
    {
        return Ok(Report::new(obj([("help", s(HELP))]), HELP));
    }
    if args.has("--version") {
        return Ok(Report::new(
            obj([("version", s(env!("CARGO_PKG_VERSION")))]),
            format!("beskar {}", env!("CARGO_PKG_VERSION")),
        ));
    }
    let words = &args.words;
    let group = words[0].as_str();
    let sub = words.get(1).map(String::as_str).unwrap_or("");
    if group == "init" || (group == "library" && sub == "init") {
        let n = if group == "init" { 1 } else { 2 };
        args.count(n, n, "init [--library PATH]")?;
        args.allow(&["--library"])?;
        let app = App::initialize(&args.home()?, args.get("--library").map(Path::new))?;
        return Ok(Report::new(
            obj([
                ("home", s(app.home.display())),
                ("library", s(app.config.library.display())),
                ("registry", s(app.config.registry.display())),
            ]),
            format!(
                "Initialized Beskar at {}\nLibrary: {}\nRegistry: {}",
                app.home.display(),
                app.config.library.display(),
                app.config.registry.display()
            ),
        ));
    }
    if ![
        "doctor", "library", "profile", "repo", "registry", "status", "update",
    ]
    .contains(&group)
    {
        return Err(Error(format!("unknown command {group:?}; use --help")));
    }
    let mut app = App::open(&args.home()?)?;
    match group {
        "doctor" => {
            args.count(1, 1, "doctor")?;
            args.allow(&[])?;
            doctor(&app)
        }
        "library" => library(args, &app, sub),
        "profile" => profile(args, &app, sub),
        "repo" => repo(args, &mut app, sub, 2),
        "registry" => registry(args, &mut app, sub),
        "status" | "update" => repo(args, &mut app, group, 1),
        _ => unreachable!(),
    }
}

fn library(args: &Args, app: &App, sub: &str) -> Result<Report> {
    match sub {
        "list" => {
            args.count(2, 2, "library list")?;
            args.allow(&[])?;
            let skills = app.skills()?;
            Ok(Report::new(
                obj([("skills", arr(skills.iter().map(s)))]),
                if skills.is_empty() {
                    "No library skills.".into()
                } else {
                    skills.join("\n")
                },
            ))
        }
        "add" => {
            args.count(3, 3, "library add PATH [--name NAME]")?;
            args.allow(&["--name"])?;
            if fs::metadata(Path::new(&args.words[2]))?.is_some_and(|m| m.file_type().is_symlink())
            {
                return Err(Error("source must be a directory, not a symlink".into()));
            }
            let source = fs::absolute(Path::new(&args.words[2]))?;
            let name = args
                .get("--name")
                .map(str::to_owned)
                .unwrap_or(filename(&source)?);
            app.import(&[(name.clone(), source)])?;
            Ok(success(format!("Imported {name}.")))
        }
        "scan" => {
            args.count(3, 3, "library scan PATH [--yes] [--dry-run]")?;
            args.allow(&["--yes", "--dry-run"])?;
            if fs::metadata(Path::new(&args.words[2]))?.is_some_and(|m| m.file_type().is_symlink())
            {
                return Err(Error("source must be a directory, not a symlink".into()));
            }
            let found = app.discover(&fs::absolute(Path::new(&args.words[2]))?)?;
            let mut imported = false;
            if !args.has("--dry-run") && !found.is_empty() {
                let approve = if args.has("--yes") {
                    true
                } else if !args.has("--json") && std::io::stdin().is_terminal() {
                    eprintln!(
                        "Found {} skills:\n{}",
                        found.len(),
                        found
                            .iter()
                            .map(|(n, p)| format!("  {n}  {}", p.display()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    eprint!("Import {} skills? [y/N] ", found.len());
                    std::io::stderr().flush()?;
                    let mut answer = String::new();
                    std::io::stdin().read_line(&mut answer)?;
                    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
                } else {
                    return Err(Error("scan needs --yes to import without a terminal; use --dry-run to inspect discoveries".into()));
                };
                if approve {
                    app.import(&found)?;
                    imported = true;
                }
            }
            let skills = arr(found
                .iter()
                .map(|(n, p)| obj([("name", s(n)), ("path", s(p.display()))])));
            Ok(Report::new(
                obj([
                    ("skills", skills),
                    ("imported", Json::Bool(imported)),
                    ("dry_run", Json::Bool(args.has("--dry-run"))),
                ]),
                format!(
                    "Found {} skills. {}\n{}",
                    found.len(),
                    if imported {
                        "Imported."
                    } else {
                        "No files changed."
                    },
                    found
                        .iter()
                        .map(|(n, p)| format!("{n}  {}", p.display()))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            ))
        }
        "show" => {
            args.count(3, 3, "library show SKILL")?;
            args.allow(&[])?;
            let name = &args.words[2];
            let path = app.skill_path(name)?;
            let fingerprint = fs::fingerprint(&path)?;
            let profiles: Vec<_> = app
                .profiles()?
                .into_iter()
                .filter_map(|(n, p)| p.skills.contains(name).then_some(n))
                .collect();
            let repositories: Vec<_> = app
                .registry
                .repos
                .iter()
                .filter_map(|(p, r)| {
                    r.installed
                        .contains_key(name)
                        .then_some(p.display().to_string())
                })
                .collect();
            let description = if fs::metadata(&path.join("SKILL.md"))?.is_some() {
                fs::read_text(&path.join("SKILL.md"))?
            } else {
                String::new()
            };
            Ok(Report::new(
                obj([
                    ("name", s(name)),
                    ("path", s(path.display())),
                    ("fingerprint", s(&fingerprint)),
                    ("profiles", arr(profiles.iter().map(s))),
                    ("repositories", arr(repositories.iter().map(s))),
                    ("skill_md", s(&description)),
                ]),
                format!(
                    "{name}\nPath: {}\nSHA-256: {fingerprint}\nProfiles: {}\nInstalled in: {}\n\n{description}",
                    path.display(),
                    profiles.join(", "),
                    repositories.join(", ")
                ),
            ))
        }
        "remove" => {
            args.count(3, 3, "library remove SKILL")?;
            args.allow(&[])?;
            app.remove_skill(&args.words[2])?;
            Ok(success(format!(
                "Removed {} from the library.",
                args.words[2]
            )))
        }
        _ => Err(Error(
            "unknown command; use 'beskar --help' for library commands".into(),
        )),
    }
}

fn profile(args: &Args, app: &App, sub: &str) -> Result<Report> {
    args.allow(&[])?;
    match sub {
        "list" => {
            args.count(2, 2, "profile list")?;
            let profiles = app.profiles()?;
            let data = arr(profiles
                .iter()
                .map(|(n, p)| obj([("name", s(n)), ("skills", arr(p.skills.iter().map(s)))])));
            Ok(Report::new(
                obj([("profiles", data)]),
                if profiles.is_empty() {
                    "No profiles.".into()
                } else {
                    profiles
                        .iter()
                        .map(|(n, p)| {
                            format!(
                                "{n}  {}",
                                p.skills.iter().cloned().collect::<Vec<_>>().join(", ")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                },
            ))
        }
        "create" => {
            args.count(3, 3, "profile create NAME")?;
            app.create_profile(&args.words[2])?;
            Ok(success(format!("Created profile {}.", args.words[2])))
        }
        "delete" => {
            args.count(3, 3, "profile delete NAME")?;
            app.delete_profile(&args.words[2])?;
            Ok(success(format!("Deleted profile {}.", args.words[2])))
        }
        "show" => {
            args.count(3, 3, "profile show NAME")?;
            let name = &args.words[2];
            let profile = app.profile(name)?;
            let repos: Vec<_> = app
                .registry
                .repos
                .iter()
                .filter_map(|(p, r)| r.profiles.contains(name).then_some(p.display().to_string()))
                .collect();
            Ok(Report::new(
                obj([
                    ("name", s(name)),
                    ("skills", arr(profile.skills.iter().map(s))),
                    ("repositories", arr(repos.iter().map(s))),
                ]),
                format!(
                    "{name}\nSkills:\n{}\nEnabled in:\n{}",
                    profile
                        .skills
                        .iter()
                        .map(|s| format!("  {s}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    repos
                        .iter()
                        .map(|p| format!("  {p}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            ))
        }
        "add" | "remove" => {
            args.count(4, usize::MAX, "profile add|remove NAME SKILL...")?;
            app.edit_profile(&args.words[2], &args.words[3..], sub == "add")?;
            Ok(success(format!(
                "Updated profile {}. Run 'beskar update --all' to reconcile workspaces.",
                args.words[2]
            )))
        }
        _ => Err(Error(
            "unknown command; use 'beskar --help' for profile commands".into(),
        )),
    }
}

fn repo(args: &Args, app: &mut App, sub: &str, prefix: usize) -> Result<Report> {
    match sub {
        "add" => {
            args.count(prefix, prefix + 1, "repo add [PATH]")?;
            args.allow(&[])?;
            let path = app.register(Path::new(
                args.words.get(prefix).map(String::as_str).unwrap_or("."),
            ))?;
            Ok(success(format!("Registered {}.", path.display())))
        }
        "remove" => {
            args.count(prefix, prefix + 1, "repo remove [PATH]")?;
            args.allow(&["--repo"])?;
            let path = args.repo(app, args.words.get(prefix).map(String::as_str))?;
            app.unregister(&path)?;
            Ok(success(format!(
                "Unregistered {}. Installed files remain in place.",
                path.display()
            )))
        }
        "list" => {
            args.count(prefix, prefix, "repo list")?;
            args.allow(&[])?;
            registry_list(args, app)
        }
        "enable" | "disable" | "toggle" => {
            args.count(
                prefix + 1,
                usize::MAX,
                "repo enable|disable|toggle PROFILE... [--repo PATH]",
            )?;
            args.allow(&["--repo"])?;
            let path = args.repo(app, None)?;
            app.set_profiles(&path, &args.words[prefix..], sub)?;
            Ok(success(format!(
                "Updated desired profiles for {}. Run 'beskar repo update' to reconcile files.",
                path.display()
            )))
        }
        "status" | "update" => {
            args.count(prefix, prefix, "repo status|update [--all] [--repo PATH]")?;
            args.allow(if sub == "update" {
                &["--repo", "--all", "--dry-run", "--conflict"]
            } else {
                &["--repo", "--all"]
            })?;
            if args.has("--all") && args.has("--repo") {
                return Err(Error("option --all cannot be combined with --repo".into()));
            }
            let paths = if args.has("--all") {
                app.registry.repos.keys().cloned().collect()
            } else {
                vec![args.repo(app, None)?]
            };
            if sub == "status" {
                return status(app, &paths);
            }
            update(args, app, &paths)
        }
        _ => Err(Error(
            "unknown command; use 'beskar --help' for repo commands".into(),
        )),
    }
}

fn registry(args: &Args, app: &mut App, sub: &str) -> Result<Report> {
    args.count(2, 2, "registry list|status|stats|update|prune")?;
    match sub {
        "list" => {
            args.allow(&["--profile", "--skill"])?;
            registry_list(args, app)
        }
        "status" => {
            args.allow(&[])?;
            status(app, &app.registry.repos.keys().cloned().collect::<Vec<_>>())
        }
        "stats" => {
            args.allow(&[])?;
            let skills = app.skills()?;
            let profiles = app.profiles()?;
            let mut used = BTreeSet::new();
            for repo in app.registry.repos.values() {
                used.extend(repo.installed.keys().cloned());
                for p in &repo.profiles {
                    used.extend(app.profile(p)?.skills);
                }
            }
            let installed: usize = app.registry.repos.values().map(|r| r.installed.len()).sum();
            let unused: Vec<_> = skills.iter().filter(|n| !used.contains(*n)).collect();
            Ok(Report::new(
                obj([
                    ("repositories", Json::Num(app.registry.repos.len())),
                    ("profiles", Json::Num(profiles.len())),
                    ("library_skills", Json::Num(skills.len())),
                    ("installed_skills", Json::Num(installed)),
                    ("unused_skills", arr(unused.iter().map(s))),
                ]),
                format!(
                    "Repositories      {}\nProfiles          {}\nLibrary skills    {}\nInstalled skills  {installed}\nUnused skills     {}",
                    app.registry.repos.len(),
                    profiles.len(),
                    skills.len(),
                    unused.len()
                ),
            ))
        }
        "update" => {
            args.allow(&["--all", "--dry-run", "--conflict"])?;
            if !args.has("--all") {
                return Err(Error("usage: beskar registry update --all [--dry-run] [--conflict fail|keep|replace]".into()));
            }
            update(
                args,
                app,
                &app.registry.repos.keys().cloned().collect::<Vec<_>>(),
            )
        }
        "prune" => {
            args.allow(&["--dry-run"])?;
            app.check_pending()?;
            let mut missing = Vec::new();
            for path in app.registry.repos.keys() {
                if fs::metadata(path)?.is_none() {
                    missing.push(path.clone());
                }
            }
            if !args.has("--dry-run") && !missing.is_empty() {
                for path in &missing {
                    app.registry.repos.remove(path);
                }
                app.save_registry()?;
            }
            Ok(Report::new(
                obj([
                    ("repositories", arr(missing.iter().map(|p| s(p.display())))),
                    ("dry_run", Json::Bool(args.has("--dry-run"))),
                ]),
                format!(
                    "{} {} missing workspaces.\n{}",
                    if args.has("--dry-run") {
                        "Would prune"
                    } else {
                        "Pruned"
                    },
                    missing.len(),
                    missing
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            ))
        }
        _ => Err(Error(
            "unknown command; use 'beskar --help' for registry commands".into(),
        )),
    }
}

fn registry_list(args: &Args, app: &App) -> Result<Report> {
    for name in [args.get("--profile"), args.get("--skill")]
        .into_iter()
        .flatten()
    {
        validate_name(name)?;
    }
    let mut rows = Vec::new();
    let mut lines = Vec::new();
    for (path, repo) in &app.registry.repos {
        if args
            .get("--profile")
            .is_some_and(|p| !repo.profiles.contains(p))
            || args
                .get("--skill")
                .is_some_and(|s| !repo.installed.contains_key(s))
        {
            continue;
        }
        let mut installed = Vec::new();
        for (skill, hash) in &repo.installed {
            let mut origins = Vec::new();
            for profile in &repo.profiles {
                if app.profile(profile)?.skills.contains(skill) {
                    origins.push(profile);
                }
            }
            installed.push(obj([
                ("skill", s(skill)),
                ("fingerprint", s(hash)),
                ("profiles", arr(origins.iter().map(s))),
            ]));
        }
        rows.push(obj([
            ("path", s(path.display())),
            ("profiles", arr(repo.profiles.iter().map(s))),
            ("installed", arr(installed)),
            ("last_sync", repo.last_sync.map_or(Json::Null, s)),
        ]));
        lines.push(format!(
            "{}  profiles: {}  installed: {}",
            path.display(),
            repo.profiles.iter().cloned().collect::<Vec<_>>().join(", "),
            repo.installed
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(Report::new(
        obj([("repositories", arr(rows))]),
        if lines.is_empty() {
            "No matching workspaces.".into()
        } else {
            lines.join("\n")
        },
    ))
}

fn plan_json(plan: &Plan) -> Json {
    obj([
        ("repository", s(plan.repository.display())),
        ("clean", Json::Bool(plan.clean())),
        (
            "entries",
            arr(plan.entries.iter().map(|entry| {
                obj([
                    ("skill", s(&entry.skill)),
                    ("action", s(entry.action.as_str())),
                    ("baseline", entry.baseline.as_ref().map_or(Json::Null, s)),
                    ("current", entry.current.as_ref().map_or(Json::Null, s)),
                    ("desired", entry.desired.as_ref().map_or(Json::Null, s)),
                ])
            })),
        ),
        ("unmanaged", arr(plan.unmanaged.iter().map(s))),
    ])
}

fn plan_text(plan: &Plan) -> String {
    let mut lines = vec![format!("Repository: {}", plan.repository.display())];
    for entry in &plan.entries {
        let symbol = match entry.action {
            Action::Add => "+",
            Action::Update => "~",
            Action::Remove | Action::Forget => "-",
            Action::LocalChanges | Action::Unmanaged => "!",
            _ => "=",
        };
        lines.push(format!(
            "{symbol} {}  {}",
            entry.skill,
            entry.action.as_str()
        ));
    }
    for name in &plan.unmanaged {
        lines.push(format!("  {name}  unmanaged, left in place"));
    }
    if plan.entries.is_empty() {
        lines.push("No managed skills.".into());
    }
    lines.join("\n")
}

fn update(args: &Args, app: &mut App, paths: &[PathBuf]) -> Result<Report> {
    app.check_pending()?;
    let policy = match args.get("--conflict").unwrap_or("fail") {
        "fail" => ConflictPolicy::Fail,
        "keep" => ConflictPolicy::Keep,
        "replace" => ConflictPolicy::Replace,
        _ => {
            return Err(Error(
                "option --conflict must be fail, keep, or replace".into(),
            ));
        }
    };
    let plans = reconcile::plan_all(app, paths)?;
    let mut warnings = Vec::new();
    let dry = args.has("--dry-run");
    if !dry {
        warnings = reconcile::apply(app, &plans, policy)?;
    }
    let conflicts = plans.iter().any(Plan::conflicts);
    let mut text = plans.iter().map(plan_text).collect::<Vec<_>>().join("\n\n");
    text.push_str(if dry {
        "\nNo files changed."
    } else {
        "\nUpdate complete."
    });
    if policy == ConflictPolicy::Keep && conflicts {
        text.push_str(" Local changes kept; they will remain visible in status.");
    }
    for warning in &warnings {
        text.push_str(&format!("\n{warning}"));
    }
    let mut report = Report::new(
        obj([
            ("repositories", arr(plans.iter().map(plan_json))),
            ("dry_run", Json::Bool(dry)),
            (
                "conflict_policy",
                s(args.get("--conflict").unwrap_or("fail")),
            ),
            ("warnings", arr(warnings.iter().map(s))),
        ]),
        text,
    );
    if dry && conflicts {
        report.code = 3;
    }
    Ok(report)
}

fn status(app: &App, paths: &[PathBuf]) -> Result<Report> {
    let mut data = Vec::new();
    let mut text = Vec::new();
    let mut clean = true;
    if let Err(e) = app.check_pending() {
        clean = false;
        text.push(e.to_string());
        data.push(obj([("error", s(e))]));
    }
    for path in paths {
        match reconcile::plan(app, path) {
            Ok(plan) => {
                clean &= plan.clean();
                text.push(plan_text(&plan));
                data.push(plan_json(&plan));
            }
            Err(e) => {
                clean = false;
                text.push(format!("{}: {e}", path.display()));
                data.push(obj([("repository", s(path.display())), ("error", s(e))]));
            }
        }
    }
    if text.is_empty() {
        text.push("No registered workspaces.".into());
    }
    let mut report = Report::new(
        obj([("clean", Json::Bool(clean)), ("repositories", arr(data))]),
        text.join("\n\n"),
    );
    if !clean {
        report.code = 3;
    }
    Ok(report)
}

fn doctor(app: &App) -> Result<Report> {
    let mut issues = Vec::new();
    if let Err(e) = app.check_pending() {
        issues.push(e.to_string());
    }
    match app.skills() {
        Ok(skills) => {
            for skill in skills {
                if let Err(e) = fs::fingerprint(&app.skill_path(&skill)?) {
                    issues.push(format!("library {skill}: {e}"));
                }
            }
        }
        Err(e) => issues.push(e.to_string()),
    }
    match app.profiles() {
        Ok(profiles) => {
            for (name, profile) in profiles {
                for skill in profile.skills {
                    if let Err(e) = fs::require_dir(&app.skill_path(&skill)?) {
                        issues.push(format!("profile {name}: {e}"));
                    }
                }
            }
        }
        Err(e) => issues.push(e.to_string()),
    }
    for path in app.registry.repos.keys() {
        match reconcile::plan(app, path) {
            Ok(plan) => {
                for entry in plan.entries {
                    if entry.action != Action::Unchanged {
                        issues.push(format!(
                            "{}: {} {}",
                            path.display(),
                            entry.skill,
                            entry.action.as_str()
                        ));
                    }
                }
            }
            Err(e) => issues.push(format!("{}: {e}", path.display())),
        }
    }
    let healthy = issues.is_empty();
    let mut report = Report::new(
        obj([
            ("healthy", Json::Bool(healthy)),
            ("issues", arr(issues.iter().map(s))),
        ]),
        if healthy {
            "Beskar is healthy. Library, profiles, registry, and installed copies agree.".into()
        } else {
            format!("Found {} issues:\n{}", issues.len(), issues.join("\n"))
        },
    );
    if !healthy {
        report.code = 1;
    }
    Ok(report)
}

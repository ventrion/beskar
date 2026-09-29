//! Hand-rolled argument parsing. The command tree is small and regular;
//! this keeps the binary dependency-free and the error messages ours.

use std::path::PathBuf;

use crate::config::ConflictPolicy;
use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct Global {
    pub home: Option<PathBuf>,
    pub yes: bool,
    pub color: bool,
}

impl Default for Global {
    fn default() -> Self {
        Global { home: None, yes: false, color: true }
    }
}

#[derive(Debug, Clone)]
pub enum Cmd {
    Init,
    Doctor,
    Library(LibCmd),
    Profile(ProfileCmd),
    Repo(RepoCmd),
    Registry(RegCmd),
    /// `beskar update` — repo update in cwd, or registry update with --all.
    Update { all: bool, dry_run: bool, conflict: Option<ConflictPolicy> },
    /// `beskar status` — repo status in cwd, else registry status.
    Status { path: Option<PathBuf> },
}

#[derive(Debug, Clone)]
pub enum LibCmd {
    Init { path: Option<PathBuf> },
    Add { path: PathBuf, name: Option<String>, force: bool },
    Scan { path: PathBuf, force: bool },
    List,
    Show { skill: String },
    Remove { skill: String, force: bool },
}

#[derive(Debug, Clone)]
pub enum ProfileCmd {
    Create { name: String, description: Option<String> },
    Delete { name: String, force: bool },
    List,
    Show { name: String },
    Add { name: String, skills: Vec<String> },
    Remove { name: String, skills: Vec<String> },
}

#[derive(Debug, Clone)]
pub enum RepoCmd {
    Add { path: Option<PathBuf> },
    Remove { path: Option<PathBuf>, purge: bool },
    List,
    Status { path: Option<PathBuf> },
    Enable { profile: String, path: Option<PathBuf> },
    Disable { profile: String, path: Option<PathBuf> },
    Toggle { profile: String, path: Option<PathBuf> },
    Update { path: Option<PathBuf>, all: bool, dry_run: bool, conflict: Option<ConflictPolicy> },
}

#[derive(Debug, Clone)]
pub enum RegCmd {
    List,
    Status,
    Stats,
    Update { path: Option<PathBuf>, dry_run: bool, conflict: Option<ConflictPolicy> },
    Prune { assumed_yes: bool },
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn help_text() -> String {
    format!(
        "beskar {VERSION} — better skill arrangement

Manage a global library of agent skills, organize them into profiles,
and materialize the right profiles into each repository's .agents/skills/.

USAGE:
    beskar [--home DIR] [--no-color] [--yes] <command> [args]

COMMANDS:
    init                        create the beskar home (config, registry, library)
    doctor                      verify library, registry, repos and installed state

    library init [--path DIR]   create the library skeleton (or point config at DIR)
    library add PATH            import one skill directory [--name NAME] [--force]
    library scan PATH           discover skills under PATH and offer to import
    library list                list library skills
    library show SKILL          show one skill's details
    library remove SKILL        remove a skill from the library [--force]

    profile create NAME         start a new profile [--description TEXT]
    profile delete NAME         delete a profile [--force]
    profile list                list profiles
    profile show NAME           show a profile's skills and usage
    profile add NAME SKILL...   add skills to a profile
    profile remove NAME SKILL.. remove skills from a profile

    repo add [PATH]             register a workspace (default: current directory)
    repo remove [PATH]          unregister a workspace [--purge]
    repo list                   list registered workspaces
    repo status [PATH]          show per-skill state for a workspace
    repo enable PROFILE         add a profile to the workspace's desired state
    repo disable PROFILE        remove a profile from the desired state
    repo toggle PROFILE         enable if disabled, disable if enabled
    repo update                 materialize desired skills into .agents/skills/
                                [--dry-run] [--conflict POLICY] [--all]

    registry list               registered workspaces and their profiles
    registry status             one-line health per workspace
    registry stats              counts: repos, profiles, skills, installs
    registry update             reconcile every (or one PATH) [--dry-run]
    registry prune              forget workspaces whose path no longer exists

    update                      alias: `repo update` here, `registry update --all`
    status                      alias: `repo status` here, else `registry status`
    help [COMMAND]              this text

CONFLICT POLICY (updates that would clobber locally modified skills):
    ask       prompt per conflict (interactive default)
    skip      keep local, change nothing (non-interactive default, announced)
    replace   overwrite the workspace copy with the library version
    promote   copy the workspace version into the library, then update from it
    abort     stop at the first conflict, touching nothing

ENVIRONMENT:
    BESKAR_HOME   beskar home directory (default ~/.beskar)

Exit codes: 0 success, 1 something did not fully succeed, 2 usage error.
"
    )
}

/// Parse a full argv (without program name).
pub fn parse(args: &[String]) -> Result<(Global, Cmd)> {
    let mut global = Global::default();
    let mut rest: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--home" => {
                let v = args.get(i + 1).ok_or_else(|| usage("--home needs a directory"))?;
                global.home = Some(PathBuf::from(v));
                i += 2;
            }
            _ if args[i].starts_with("--home=") => {
                global.home = Some(PathBuf::from(&args[i]["--home=".len()..]));
                i += 1;
            }
            "--yes" | "-y" => {
                global.yes = true;
                i += 1;
            }
            "--no-color" => {
                global.color = false;
                i += 1;
            }
            "--color" => {
                global.color = true;
                i += 1;
            }
            "--version" | "-V" => {
                println!("beskar {VERSION}");
                std::process::exit(0);
            }
            "--help" | "-h" => {
                print!("{}", help_text());
                std::process::exit(0);
            }
            other => {
                rest.push(other.to_string());
                i += 1;
            }
        }
    }

    let mut p = Parser { toks: rest };
    let Some(cmd) = p.next() else {
        print!("{}", help_text());
        std::process::exit(0);
    };
    let cmd = match cmd.as_str() {
        "init" => Cmd::Init,
        "doctor" => Cmd::Doctor,
        "update" => {
            let (all, dry_run, conflict) = parse_update_flags(&mut p)?;
            Cmd::Update { all, dry_run, conflict }
        }
        "status" => {
            let path = one_optional_path(&mut p)?;
            Cmd::Status { path }
        }
        "library" | "lib" => Cmd::Library(parse_library(&mut p)?),
        "profile" | "profiles" => Cmd::Profile(parse_profile(&mut p)?),
        "repo" | "repos" => Cmd::Repo(parse_repo(&mut p)?),
        "registry" | "reg" => Cmd::Registry(parse_registry(&mut p)?),
        "help" => {
            // `beskar help <anything>` shows the full text; simple and complete.
            print!("{}", help_text());
            std::process::exit(0);
        }
        other => {
            return Err(usage(&format!(
                "unknown command `{other}` — run `beskar help` for the command list"
            )));
        }
    };
    p.expect_empty()?;
    Ok((global, cmd))
}

fn usage(msg: &str) -> Error {
    Error::Usage(msg.to_string())
}

pub struct Parser {
    toks: Vec<String>,
}

impl Parser {
    pub fn next(&mut self) -> Option<String> {
        if self.toks.is_empty() {
            None
        } else {
            Some(self.toks.remove(0))
        }
    }

    pub fn expect_empty(&self) -> Result<()> {
        if let Some(t) = self.toks.first() {
            return Err(usage(&format!("unexpected argument `{t}`")));
        }
        Ok(())
    }

    fn need_positional(&mut self, what: &str) -> Result<String> {
        self.next().ok_or_else(|| usage(&format!("missing <{what}>")))
    }

    fn collect_positionals(&mut self, what: &str, min: usize) -> Result<Vec<String>> {
        let mut out = Vec::new();
        while let Some(t) = self.next() {
            out.push(t);
        }
        if out.len() < min {
            return Err(usage(&format!(
                "missing <{what}> (need at least {min}, got {})",
                out.len()
            )));
        }
        Ok(out)
    }
}

fn parse_policy(s: &str) -> Result<ConflictPolicy> {
    ConflictPolicy::parse(s).ok_or_else(|| {
        usage(&format!(
            "unknown conflict policy `{s}` (ask | skip | replace | promote | abort)"
        ))
    })
}

/// (--all, --dry-run, --conflict P)
fn parse_update_flags(p: &mut Parser) -> Result<(bool, bool, Option<ConflictPolicy>)> {
    let mut all = false;
    let mut dry = false;
    let mut conflict = None;
    while let Some(tok) = p.next() {
        match tok.as_str() {
            "--all" => all = true,
            "--dry-run" | "-n" => dry = true,
            "--conflict" => {
                let v = p.next().ok_or_else(|| usage("--conflict needs a policy"))?;
                conflict = Some(parse_policy(&v)?);
            }
            _ if tok.starts_with("--conflict=") => {
                conflict = Some(parse_policy(&tok["--conflict=".len()..])?);
            }
            _ => return Err(usage(&format!("unexpected argument `{tok}`"))),
        }
    }
    Ok((all, dry, conflict))
}

fn one_optional_path(p: &mut Parser) -> Result<Option<PathBuf>> {
    let mut path = None;
    while let Some(tok) = p.next() {
        if tok.starts_with('-') {
            return Err(usage(&format!("unexpected flag `{tok}`")));
        }
        if path.is_some() {
            return Err(usage(&format!("unexpected argument `{tok}`")));
        }
        path = Some(PathBuf::from(tok));
    }
    Ok(path)
}

fn parse_library(p: &mut Parser) -> Result<LibCmd> {
    let sub = p
        .next()
        .ok_or_else(|| usage("library needs a subcommand (add, scan, list, show, remove, init)"))?;
    let mut name = None;
    let mut force = false;
    match sub.as_str() {
        "init" => {
            let mut path = None;
            while let Some(tok) = p.next() {
                match tok.as_str() {
                    "--path" => {
                        let v = p.next().ok_or_else(|| usage("--path needs a directory"))?;
                        path = Some(PathBuf::from(v));
                    }
                    _ if tok.starts_with("--path=") => {
                        path = Some(PathBuf::from(&tok["--path=".len()..]));
                    }
                    _ => return Err(usage(&format!("unexpected argument `{tok}`"))),
                }
            }
            Ok(LibCmd::Init { path })
        }
        "add" => {
            let path = PathBuf::from(p.need_positional("path")?);
            while let Some(tok) = p.next() {
                match tok.as_str() {
                    "--name" => {
                        let v = p.next().ok_or_else(|| usage("--name needs a value"))?;
                        name = Some(v);
                    }
                    _ if tok.starts_with("--name=") => {
                        name = Some(tok["--name=".len()..].to_string());
                    }
                    "--force" | "-f" => force = true,
                    _ => return Err(usage(&format!("unexpected argument `{tok}`"))),
                }
            }
            Ok(LibCmd::Add { path, name, force })
        }
        "scan" => {
            let path = PathBuf::from(p.need_positional("path")?);
            while let Some(tok) = p.next() {
                match tok.as_str() {
                    "--force" | "-f" => force = true,
                    "--yes" | "-y" => {} // global --yes also works; accepted here for convenience
                    _ => return Err(usage(&format!("unexpected argument `{tok}`"))),
                }
            }
            Ok(LibCmd::Scan { path, force })
        }
        "list" | "ls" => Ok(LibCmd::List),
        "show" => Ok(LibCmd::Show { skill: p.need_positional("skill")? }),
        "remove" | "rm" => {
            let skill = p.need_positional("skill")?;
            while let Some(tok) = p.next() {
                match tok.as_str() {
                    "--force" | "-f" => force = true,
                    _ => return Err(usage(&format!("unexpected argument `{tok}`"))),
                }
            }
            Ok(LibCmd::Remove { skill, force })
        }
        other => Err(usage(&format!(
            "unknown library subcommand `{other}` (add, scan, list, show, remove, init)"
        ))),
    }
}

fn parse_profile(p: &mut Parser) -> Result<ProfileCmd> {
    let sub = p.next().ok_or_else(|| {
        usage("profile needs a subcommand (create, delete, list, show, add, remove)")
    })?;
    match sub.as_str() {
        "create" | "new" => {
            let name = p.need_positional("name")?;
            let mut description = None;
            while let Some(tok) = p.next() {
                match tok.as_str() {
                    "--description" | "-d" => {
                        let v = p.next().ok_or_else(|| usage("--description needs text"))?;
                        description = Some(v);
                    }
                    _ if tok.starts_with("--description=") => {
                        description = Some(tok["--description=".len()..].to_string());
                    }
                    _ => return Err(usage(&format!("unexpected argument `{tok}`"))),
                }
            }
            Ok(ProfileCmd::Create { name, description })
        }
        "delete" | "rm" => {
            let name = p.need_positional("name")?;
            let mut force = false;
            while let Some(tok) = p.next() {
                match tok.as_str() {
                    "--force" | "-f" => force = true,
                    _ => return Err(usage(&format!("unexpected argument `{tok}`"))),
                }
            }
            Ok(ProfileCmd::Delete { name, force })
        }
        "list" | "ls" => Ok(ProfileCmd::List),
        "show" => Ok(ProfileCmd::Show { name: p.need_positional("name")? }),
        "add" | "remove" => {
            let name = p.need_positional("name")?;
            let skills = p.collect_positionals("skill", 1)?;
            if sub == "add" {
                Ok(ProfileCmd::Add { name, skills })
            } else {
                Ok(ProfileCmd::Remove { name, skills })
            }
        }
        other => Err(usage(&format!(
            "unknown profile subcommand `{other}` (create, delete, list, show, add, remove)"
        ))),
    }
}

fn parse_repo(p: &mut Parser) -> Result<RepoCmd> {
    let sub = p.next().ok_or_else(|| {
        usage("repo needs a subcommand (add, remove, list, status, enable, disable, toggle, update)")
    })?;
    match sub.as_str() {
        "add" => Ok(RepoCmd::Add { path: one_optional_path(p)? }),
        "remove" | "rm" => {
            let mut path = None;
            let mut purge = false;
            while let Some(tok) = p.next() {
                match tok.as_str() {
                    "--purge" => purge = true,
                    _ if tok.starts_with('-') => {
                        return Err(usage(&format!("unexpected flag `{tok}`")))
                    }
                    _ => {
                        if path.is_some() {
                            return Err(usage(&format!("unexpected argument `{tok}`")));
                        }
                        path = Some(PathBuf::from(tok));
                    }
                }
            }
            Ok(RepoCmd::Remove { path, purge })
        }
        "list" | "ls" => Ok(RepoCmd::List),
        "status" => Ok(RepoCmd::Status { path: one_optional_path(p)? }),
        "enable" | "disable" | "toggle" => {
            let profile = p.need_positional("profile")?;
            let mut path = None;
            while let Some(tok) = p.next() {
                match tok.as_str() {
                    "--repo" => {
                        let v = p.next().ok_or_else(|| usage("--repo needs a path"))?;
                        path = Some(PathBuf::from(v));
                    }
                    _ if tok.starts_with("--repo=") => {
                        path = Some(PathBuf::from(&tok["--repo=".len()..]));
                    }
                    _ => return Err(usage(&format!("unexpected argument `{tok}`"))),
                }
            }
            Ok(match sub.as_str() {
                "enable" => RepoCmd::Enable { profile, path },
                "disable" => RepoCmd::Disable { profile, path },
                _ => RepoCmd::Toggle { profile, path },
            })
        }
        "update" | "sync" => {
            let mut path = None;
            let mut all = false;
            let mut dry_run = false;
            let mut conflict = None;
            while let Some(tok) = p.next() {
                match tok.as_str() {
                    "--all" => all = true,
                    "--dry-run" | "-n" => dry_run = true,
                    "--conflict" => {
                        let v = p.next().ok_or_else(|| usage("--conflict needs a policy"))?;
                        conflict = Some(parse_policy(&v)?);
                    }
                    _ if tok.starts_with("--conflict=") => {
                        conflict = Some(parse_policy(&tok["--conflict=".len()..])?);
                    }
                    _ if tok.starts_with('-') => {
                        return Err(usage(&format!("unexpected flag `{tok}`")))
                    }
                    _ => {
                        if path.is_some() {
                            return Err(usage(&format!("unexpected argument `{tok}`")));
                        }
                        path = Some(PathBuf::from(tok));
                    }
                }
            }
            Ok(RepoCmd::Update { path, all, dry_run, conflict })
        }
        other => Err(usage(&format!(
            "unknown repo subcommand `{other}` (add, remove, list, status, enable, disable, toggle, update)"
        ))),
    }
}

fn parse_registry(p: &mut Parser) -> Result<RegCmd> {
    let sub = p.next().ok_or_else(|| {
        usage("registry needs a subcommand (list, status, stats, update, prune)")
    })?;
    match sub.as_str() {
        "list" | "ls" => Ok(RegCmd::List),
        "status" => Ok(RegCmd::Status),
        "stats" => Ok(RegCmd::Stats),
        "update" => {
            let mut path = None;
            let mut dry_run = false;
            let mut conflict = None;
            while let Some(tok) = p.next() {
                match tok.as_str() {
                    "--dry-run" | "-n" => dry_run = true,
                    "--conflict" => {
                        let v = p.next().ok_or_else(|| usage("--conflict needs a policy"))?;
                        conflict = Some(parse_policy(&v)?);
                    }
                    _ if tok.starts_with("--conflict=") => {
                        conflict = Some(parse_policy(&tok["--conflict=".len()..])?);
                    }
                    _ if tok.starts_with('-') => {
                        return Err(usage(&format!("unexpected flag `{tok}`")))
                    }
                    _ => {
                        if path.is_some() {
                            return Err(usage(&format!("unexpected argument `{tok}`")));
                        }
                        path = Some(PathBuf::from(tok));
                    }
                }
            }
            Ok(RegCmd::Update { path, dry_run, conflict })
        }
        "prune" => {
            let mut assumed_yes = false;
            while let Some(tok) = p.next() {
                match tok.as_str() {
                    "--yes" | "-y" => assumed_yes = true,
                    _ => return Err(usage(&format!("unexpected argument `{tok}`"))),
                }
            }
            Ok(RegCmd::Prune { assumed_yes })
        }
        other => Err(usage(&format!(
            "unknown registry subcommand `{other}` (list, status, stats, update, prune)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_simple_commands() {
        let (g, c) = parse(&args(&["init"])).unwrap();
        assert!(matches!(c, Cmd::Init));
        assert!(g.home.is_none());

        let (_, c) = parse(&args(&["library", "list"])).unwrap();
        assert!(matches!(c, Cmd::Library(LibCmd::List)));

        let (_, c) = parse(&args(&["profile", "add", "coding", "git", "testing"])).unwrap();
        match c {
            Cmd::Profile(ProfileCmd::Add { name, skills }) => {
                assert_eq!(name, "coding");
                assert_eq!(skills, vec!["git", "testing"]);
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn parses_flags_anywhere() {
        let (g, c) =
            parse(&args(&["repo", "update", "--dry-run", "--conflict", "replace", "--all"])).unwrap();
        match c {
            Cmd::Repo(RepoCmd::Update { all, dry_run, conflict, path }) => {
                assert!(all && dry_run);
                assert_eq!(conflict, Some(ConflictPolicy::Replace));
                assert!(path.is_none());
            }
            _ => panic!("wrong command"),
        }
        assert!(g.yes == false);

        let (g, _) = parse(&args(&["--yes", "--home", "/tmp/bh", "init"])).unwrap();
        assert!(g.yes);
        assert_eq!(g.home, Some(PathBuf::from("/tmp/bh")));
    }

    #[test]
    fn equals_form_flags() {
        let (_, c) = parse(&args(&["repo", "update", "--conflict=promote"])).unwrap();
        match c {
            Cmd::Repo(RepoCmd::Update { conflict, .. }) => {
                assert_eq!(conflict, Some(ConflictPolicy::Promote));
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(&args(&["frobnicate"])).is_err());
        assert!(parse(&args(&["repo", "enable"])).is_err()); // missing profile
        assert!(parse(&args(&["profile", "add", "p"])).is_err()); // missing skill
        assert!(parse(&args(&["library", "add", "p", "--conflict", "x"])).is_err());
        assert!(parse(&args(&["repo", "update", "--conflict", "nonsense"])).is_err());
        let err = parse(&args(&["library", "show"])).unwrap_err();
        assert!(matches!(err, Error::Usage(_)));
    }

    #[test]
    fn aliases_accept() {
        let (_, c) = parse(&args(&["lib", "ls"])).unwrap();
        assert!(matches!(c, Cmd::Library(LibCmd::List)));
        let (_, c) = parse(&args(&["reg", "stats"])).unwrap();
        assert!(matches!(c, Cmd::Registry(RegCmd::Stats)));
        let (_, c) = parse(&args(&["update", "--all"])).unwrap();
        assert!(matches!(c, Cmd::Update { all: true, .. }));
    }
}

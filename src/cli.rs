use beskar::format::Result;
use beskar::store::{self, Config, Registry, Repository};
use beskar::sync::{self, Policy};
use beskar::tree;
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

pub fn run(args: Vec<String>) -> Result<()> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    if !matches!(words.as_slice(), [] | ["help"] | ["--help"] | ["-h"]) {
        // File locks release automatically, including after an error or panic.
        let home = store::home()?;
        fs::create_dir_all(&home).map_err(|e| format!("{}: {e}", home.display()))?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(home.join(".lock"))
            .map_err(|e| format!("cannot open Beskar lock: {e}"))?;
        lock.lock()
            .map_err(|e| format!("cannot lock Beskar state: {e}"))?;
        return dispatch(&words);
    }
    dispatch(&words)
}

fn dispatch(words: &[&str]) -> Result<()> {
    match words {
        [] | ["help"] | ["--help"] | ["-h"] => {
            help();
            Ok(())
        }
        ["init", rest @ ..] => init(rest),
        ["doctor"] => doctor(),
        ["library", rest @ ..] => library(rest),
        ["profile", rest @ ..] => profile(rest),
        ["repo", rest @ ..] => repo(rest),
        ["registry", rest @ ..] => registry(rest),
        _ => Err("unknown command; run `beskar help`".into()),
    }
}

fn help() {
    println!(
        r#"Beskar manages agent skills across local workspaces.

Usage:
  beskar init [--library PATH] [--registry PATH] [--skills-dir RELATIVE_PATH]
  beskar doctor
  beskar library init|list|add PATH|scan PATH [--yes]|show NAME|remove NAME
  beskar profile create|delete|list|show|add|remove ...
  beskar repo add [PATH]|remove [PATH]|list|status [--repo PATH]
  beskar repo enable|disable|toggle PROFILE [--repo PATH]
  beskar repo update [--repo PATH] [--dry-run] [--on-conflict keep|replace]
  beskar registry list|status|stats|where profile|skill NAME
  beskar registry update --all [--dry-run] [--on-conflict keep|replace]
  beskar registry prune [--dry-run]

Use BESKAR_HOME to choose the state directory (default: ~/.beskar).
Configuration, profiles and registry use Beskar's line-based .bsk format."#
    );
}

fn print_plan(plan: &sync::Plan) {
    println!("Repository: {}", plan.path.display());
    for change in &plan.changes {
        let sign = match change.action {
            sync::Action::Add => "+",
            sync::Action::Replace => "~",
            sync::Action::Remove | sync::Action::Forget => "-",
            sync::Action::Record => "=",
            sync::Action::Keep | sync::Action::Conflict => "!",
            sync::Action::Unchanged => continue,
        };
        println!("  {sign} {}: {}", change.skill, change.reason);
    }
    if plan
        .changes
        .iter()
        .all(|c| c.action == sync::Action::Unchanged)
    {
        println!("  up to date");
    }
}

fn need<'a>(args: &'a [&str], position: usize, what: &str) -> Result<&'a str> {
    args.get(position)
        .copied()
        .ok_or_else(|| format!("missing {what}"))
}

fn init(args: &[&str]) -> Result<()> {
    let home = store::home()?;
    let path = Config::path()?;
    if path.exists() {
        return Err(format!("already initialized: {}", path.display()));
    }
    let mut config = Config::default_at(&home);
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--library" => {
                config.library = store::absolute(Path::new(need(args, i + 1, "library path")?))?
            }
            "--registry" => {
                config.registry = store::absolute(Path::new(need(args, i + 1, "registry path")?))?
            }
            "--skills-dir" => {
                config.skills_dir = PathBuf::from(need(args, i + 1, "skills directory")?)
            }
            value => return Err(format!("unknown init option `{value}`")),
        }
        i += 2;
    }
    config.validate()?;
    fs::create_dir_all(config.library.join("skills")).map_err(|e| e.to_string())?;
    fs::create_dir_all(config.library.join("profiles")).map_err(|e| e.to_string())?;
    if config.registry.exists() {
        return Err(format!(
            "registry already exists: {}",
            config.registry.display()
        ));
    }
    Registry::default().save(&config)?;
    if let Err(error) = config.save() {
        let _ = fs::remove_file(&config.registry);
        return Err(error);
    }
    println!(
        "Initialized Beskar\n  library: {}\n  registry: {}\n  config: {}",
        config.library.display(),
        config.registry.display(),
        path.display()
    );
    Ok(())
}

fn doctor() -> Result<()> {
    let config = Config::load()?;
    let mut issues = Vec::new();
    for dir in [
        config.library.join("skills"),
        config.library.join("profiles"),
    ] {
        if !dir.is_dir() {
            issues.push(format!("missing directory: {}", dir.display()));
        }
    }
    let registry = Registry::load(&config)?;
    for skill in store::skills(&config)? {
        if let Err(e) = tree::fingerprint(&store::skill_path(&config, &skill)?) {
            issues.push(e);
        }
    }
    for profile in store::profiles(&config)? {
        match store::load_profile(&config, &profile) {
            Ok(skills) => {
                for skill in skills {
                    if !store::skill_path(&config, &skill)?.is_dir() {
                        issues.push(format!(
                            "profile `{profile}` references missing skill `{skill}`"
                        ));
                    }
                }
            }
            Err(e) => issues.push(e),
        }
    }
    for (path, repo) in &registry.repos {
        match sync::plan(&config, path, repo, Policy::Stop) {
            Ok(plan) => {
                for change in plan.changes {
                    if !matches!(change.action, sync::Action::Unchanged) {
                        issues.push(format!(
                            "{}: {}: {}",
                            path.display(),
                            change.skill,
                            change.reason
                        ));
                    }
                }
            }
            Err(e) => issues.push(e),
        }
    }
    if issues.is_empty() {
        println!("Beskar is healthy ({} repositories).", registry.repos.len());
        Ok(())
    } else {
        for issue in &issues {
            eprintln!("- {issue}");
        }
        Err(format!("{} issue(s) found", issues.len()))
    }
}

fn library(args: &[&str]) -> Result<()> {
    let config = Config::load()?;
    match args {
        ["init"] => {
            fs::create_dir_all(config.library.join("skills")).map_err(|e| e.to_string())?;
            fs::create_dir_all(config.library.join("profiles")).map_err(|e| e.to_string())?;
            println!("Library ready: {}", config.library.display());
            Ok(())
        }
        ["list"] => {
            for skill in store::skills(&config)? {
                println!("{skill}");
            }
            Ok(())
        }
        ["add", path] => import(&config, Path::new(path)),
        ["scan", path] | ["scan", path, "--yes"] => {
            let yes = args.contains(&"--yes");
            let path = Path::new(path);
            let mut found = Vec::new();
            if path.join("SKILL.md").is_file() {
                found.push(path.to_path_buf());
            } else {
                for child in tree::entries(path)? {
                    if child.is_dir() && child.join("SKILL.md").is_file() {
                        found.push(child);
                    }
                }
            }
            println!("Found {} skill(s):", found.len());
            for child in &found {
                println!("  {}", child.file_name().unwrap().to_string_lossy());
            }
            if found.is_empty() {
                return Ok(());
            }
            if !yes {
                if !io::stdin().is_terminal() {
                    return Err("scan needs `--yes` when stdin is not a terminal".into());
                }
                print!("Import all? [y/N] ");
                io::stdout().flush().map_err(|e| e.to_string())?;
                let mut answer = String::new();
                io::stdin()
                    .read_line(&mut answer)
                    .map_err(|e| e.to_string())?;
                if !matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
                    println!("No skills imported.");
                    return Ok(());
                }
            }
            // Check every destination before importing any of them.
            for child in &found {
                let skill = child
                    .file_name()
                    .unwrap()
                    .to_str()
                    .ok_or("non-UTF-8 skill name")?;
                if store::skill_path(&config, skill)?.exists() {
                    return Err(format!("already in library: `{skill}`"));
                }
                tree::fingerprint(child)?;
            }
            for child in &found {
                import(&config, child)?;
            }
            Ok(())
        }
        ["show", skill] => {
            let path = store::skill_path(&config, skill)?;
            println!(
                "Skill: {skill}\nPath: {}\nSHA-256: {}",
                path.display(),
                tree::fingerprint(&path)?
            );
            let metadata = path.join("SKILL.md");
            if metadata.is_file() {
                for line in fs::read_to_string(metadata)
                    .map_err(|e| e.to_string())?
                    .lines()
                    .take(15)
                {
                    if line.starts_with("name:") || line.starts_with("description:") {
                        println!("{}", line.trim());
                    }
                }
            }
            Ok(())
        }
        ["remove", skill] => {
            let path = store::skill_path(&config, skill)?;
            if !path.is_dir() {
                return Err(format!("unknown skill `{skill}`"));
            }
            for profile in store::profiles(&config)? {
                if store::load_profile(&config, &profile)?.contains(*skill) {
                    return Err(format!("skill `{skill}` is used by profile `{profile}`"));
                }
            }
            fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("Removed `{skill}` from library.");
            Ok(())
        }
        _ => Err(
            "usage: beskar library init|list|add PATH|scan PATH [--yes]|show NAME|remove NAME"
                .into(),
        ),
    }
}

fn import(config: &Config, source: &Path) -> Result<()> {
    let source = fs::canonicalize(source).map_err(|e| format!("{}: {e}", source.display()))?;
    if !source.is_dir() {
        return Err(format!("not a directory: {}", source.display()));
    }
    let skill = source
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("non-UTF-8 skill name")?;
    let target = store::skill_path(config, skill)?;
    if target.exists() {
        return Err(format!("already in library: `{skill}`"));
    }
    if target.starts_with(&source) {
        return Err("cannot import library into itself".into());
    }
    let hash = tree::fingerprint(&source)?;
    let stage = tree::temporary_dir(&config.library, "import")?;
    let fresh = stage.join("new");
    if let Err(e) = tree::copy_dir(&source, &fresh) {
        let _ = fs::remove_dir_all(&stage);
        return Err(e);
    }
    if tree::fingerprint(&fresh)? != hash {
        let _ = fs::remove_dir_all(&stage);
        return Err("source changed during import".into());
    }
    fs::rename(&fresh, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    let _ = fs::remove_dir_all(stage);
    println!("Added `{skill}` to library.");
    Ok(())
}

fn profile(args: &[&str]) -> Result<()> {
    let config = Config::load()?;
    match args {
        ["list"] => {
            for name in store::profiles(&config)? {
                println!("{name}");
            }
            Ok(())
        }
        ["create", name] => {
            let path = store::profile_path(&config, name)?;
            if path.exists() {
                return Err(format!("profile `{name}` already exists"));
            }
            store::save_profile(&config, name, &BTreeSet::new())?;
            println!("Created profile `{name}`.");
            Ok(())
        }
        ["delete", name] => {
            let path = store::profile_path(&config, name)?;
            if !path.is_file() {
                return Err(format!("unknown profile `{name}`"));
            }
            let registry = Registry::load(&config)?;
            for (repo, state) in registry.repos {
                if state.profiles.contains(*name) {
                    return Err(format!("profile `{name}` is enabled in {}", repo.display()));
                }
            }
            fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("Deleted profile `{name}`.");
            Ok(())
        }
        ["show", name] => {
            println!("Profile: {name}");
            for skill in store::load_profile(&config, name)? {
                println!("  {skill}");
            }
            Ok(())
        }
        ["add", profile, skill] => {
            if !store::skill_path(&config, skill)?.is_dir() {
                return Err(format!("unknown skill `{skill}`"));
            }
            let mut skills = store::load_profile(&config, profile)?;
            if !skills.insert((*skill).into()) {
                return Err(format!("`{skill}` is already in profile `{profile}`"));
            }
            store::save_profile(&config, profile, &skills)?;
            println!("Added `{skill}` to `{profile}`.");
            Ok(())
        }
        ["remove", profile, skill] => {
            let mut skills = store::load_profile(&config, profile)?;
            if !skills.remove(*skill) {
                return Err(format!("`{skill}` is not in profile `{profile}`"));
            }
            store::save_profile(&config, profile, &skills)?;
            println!("Removed `{skill}` from `{profile}`.");
            Ok(())
        }
        _ => Err(
            "usage: beskar profile create|delete|list|show NAME, or add|remove PROFILE SKILL"
                .into(),
        ),
    }
}

fn repo_path(args: &[&str]) -> Result<PathBuf> {
    let path = if args.is_empty() {
        Path::new(".")
    } else if args.len() == 2 && args[0] == "--repo" {
        Path::new(args[1])
    } else {
        return Err("expected `--repo PATH`".into());
    };
    fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn registered<'a>(registry: &'a Registry, path: &Path) -> Result<&'a Repository> {
    registry.repos.get(path).ok_or_else(|| {
        format!(
            "repository is not registered: {}; run `beskar repo add`",
            path.display()
        )
    })
}

fn repo(args: &[&str]) -> Result<()> {
    let config = Config::load()?;
    let mut registry = Registry::load(&config)?;
    match args {
        ["add"] | ["add", _] => {
            let path =
                fs::canonicalize(args.get(1).copied().unwrap_or(".")).map_err(|e| e.to_string())?;
            if !path.is_dir() {
                return Err("repository path is not a directory".into());
            }
            if registry.repos.contains_key(&path) {
                return Err(format!("already registered: {}", path.display()));
            }
            registry.repos.insert(path.clone(), Repository::default());
            registry.save(&config)?;
            println!("Registered {}", path.display());
            Ok(())
        }
        ["remove"] | ["remove", _] => {
            let path = repo_path(&["--repo", args.get(1).copied().unwrap_or(".")])?;
            let state = registered(&registry, &path)?;
            if !state.installed.is_empty() {
                return Err(
                    "repository still has managed skills; disable profiles and update first".into(),
                );
            }
            registry.repos.remove(&path);
            registry.save(&config)?;
            println!("Unregistered {}", path.display());
            Ok(())
        }
        ["list"] => {
            for (path, state) in registry.repos {
                println!(
                    "{}  [{}]",
                    path.display(),
                    state
                        .profiles
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            Ok(())
        }
        ["status", tail @ ..] => {
            let path = repo_path(tail)?;
            let state = registered(&registry, &path)?;
            println!(
                "Profiles: {}",
                if state.profiles.is_empty() {
                    "(none)".into()
                } else {
                    state
                        .profiles
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            );
            print_plan(&sync::plan(&config, &path, state, Policy::Stop)?);
            Ok(())
        }
        ["enable" | "disable" | "toggle", name, tail @ ..] => {
            let path = repo_path(tail)?;
            if !store::profile_path(&config, name)?.is_file() {
                return Err(format!("unknown profile `{name}`"));
            }
            let state = registry
                .repos
                .get_mut(&path)
                .ok_or_else(|| format!("repository is not registered: {}", path.display()))?;
            match args[0] {
                "enable" => {
                    state.profiles.insert((*name).into());
                }
                "disable" => {
                    state.profiles.remove(*name);
                }
                _ => {
                    if !state.profiles.remove(*name) {
                        state.profiles.insert((*name).into());
                    }
                }
            }
            registry.save(&config)?;
            println!(
                "Profiles for {}: {}",
                path.display(),
                registry.repos[&path]
                    .profiles
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            Ok(())
        }
        ["update", tail @ ..] => {
            let options = update_options(tail, false)?;
            let path = repo_path(&options.repo_args)?;
            let plan = sync::plan(
                &config,
                &path,
                registered(&registry, &path)?,
                options.policy,
            )?;
            print_plan(&plan);
            if options.dry_run {
                println!("No files changed.");
                return Ok(());
            }
            sync::apply(&config, &mut registry, &plan)
        }
        _ => Err("usage: beskar repo add|remove|list|status|enable|disable|toggle|update".into()),
    }
}

struct UpdateOptions<'a> {
    repo_args: Vec<&'a str>,
    dry_run: bool,
    policy: Policy,
}
fn update_options<'a>(args: &'a [&'a str], all: bool) -> Result<UpdateOptions<'a>> {
    let mut options = UpdateOptions {
        repo_args: Vec::new(),
        dry_run: false,
        policy: Policy::Stop,
    };
    let mut seen_all = false;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--dry-run" => options.dry_run = true,
            "--all" if all => seen_all = true,
            "--repo" if !all => {
                options
                    .repo_args
                    .extend(["--repo", need(args, i + 1, "repository path")?]);
                i += 1;
            }
            "--on-conflict" => {
                options.policy = match need(args, i + 1, "conflict policy")? {
                    "keep" => Policy::Keep,
                    "replace" => Policy::Replace,
                    value => {
                        return Err(format!(
                            "unknown conflict policy `{value}`; use keep or replace"
                        ));
                    }
                };
                i += 1;
            }
            value => return Err(format!("unknown update option `{value}`")),
        }
        i += 1;
    }
    if all && !seen_all {
        return Err("global update requires `--all`".into());
    }
    Ok(options)
}

fn registry(args: &[&str]) -> Result<()> {
    let config = Config::load()?;
    let mut registry = Registry::load(&config)?;
    match args {
        ["list"] => {
            for path in registry.repos.keys() {
                println!("{}", path.display());
            }
            Ok(())
        }
        ["status"] => {
            let mut errors = Vec::new();
            for (path, state) in &registry.repos {
                match sync::plan(&config, path, state, Policy::Stop) {
                    Ok(plan) => print_plan(&plan),
                    Err(e) => {
                        println!("Repository: {}\n  error: {e}", path.display());
                        errors.push(e);
                    }
                }
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join("\n"))
            }
        }
        ["stats"] => {
            let skills = store::skills(&config)?;
            let mut used = BTreeSet::new();
            for profile in store::profiles(&config)? {
                used.extend(store::load_profile(&config, &profile)?);
            }
            println!(
                "Repositories      {}\nProfiles          {}\nLibrary skills    {}\nInstalled skills  {}\nUnused skills     {}",
                registry.repos.len(),
                store::profiles(&config)?.len(),
                skills.len(),
                registry
                    .repos
                    .values()
                    .map(|r| r.installed.len())
                    .sum::<usize>(),
                skills.iter().filter(|s| !used.contains(*s)).count()
            );
            Ok(())
        }
        ["where", "profile", name] => {
            store::name(name)?;
            println!("Profile `{name}`:");
            for (path, state) in &registry.repos {
                if state.profiles.contains(*name) {
                    println!("  {}", path.display());
                }
            }
            Ok(())
        }
        ["where", "skill", name] => {
            store::name(name)?;
            println!("Skill `{name}`:");
            for (path, state) in &registry.repos {
                if state.installed.contains_key(*name) {
                    let mut via = Vec::new();
                    for profile in &state.profiles {
                        if store::load_profile(&config, profile)?.contains(*name) {
                            via.push(profile.clone());
                        }
                    }
                    println!("  {}  [{}]", path.display(), via.join(", "));
                }
            }
            Ok(())
        }
        ["update", tail @ ..] => {
            let options = update_options(tail, true)?;
            let mut plans = Vec::new();
            let mut errors = Vec::new();
            for (path, state) in &registry.repos {
                match sync::plan(&config, path, state, options.policy) {
                    Ok(plan) => plans.push(plan),
                    Err(e) => errors.push(e),
                }
            }
            for plan in &plans {
                print_plan(plan);
            }
            if options.dry_run {
                println!("No files changed.");
                return if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors.join("\n"))
                };
            }
            if !errors.is_empty() {
                return Err(errors.join("\n"));
            }
            if plans.iter().any(|p| p.conflicts() > 0) {
                return Err(
                    "conflicts found; no repositories changed; use `--on-conflict keep|replace`"
                        .into(),
                );
            }
            for plan in &plans {
                sync::apply(&config, &mut registry, plan)?;
            }
            Ok(())
        }
        ["prune"] | ["prune", "--dry-run"] => {
            let dry = args.len() == 2;
            let missing = registry
                .repos
                .keys()
                .filter(|path| !path.is_dir())
                .cloned()
                .collect::<Vec<_>>();
            for path in &missing {
                println!("{}", path.display());
            }
            if dry {
                println!("No registry entries changed.");
            } else {
                for path in &missing {
                    registry.repos.remove(path);
                }
                registry.save(&config)?;
            }
            Ok(())
        }
        _ => Err(
            "usage: beskar registry list|status|stats|where profile|skill NAME|update --all|prune"
                .into(),
        ),
    }
}

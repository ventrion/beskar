use crate::{
    Result, format, io,
    model::{Config, Profile, Registry, disjoint},
    reconcile::{self, Action, Plan, Policy},
    store::{Lock, Store},
    transaction::Transaction,
    tree,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
};

const HELP: &str = "Beskar 0.1.0: reusable skills for local workspaces

Usage: beskar [--home PATH] <command>

  init [--library PATH] [--registry PATH] [--agent-skills RELATIVE_PATH]
  doctor [--recover]
  library init | list | add PATH [--name NAME] | scan PATH [--yes]
  library show NAME | remove NAME
  profile create NAME | delete NAME | list | show NAME
  profile add NAME SKILL... | remove NAME SKILL...
  repo add [PATH] | remove [PATH] | list | status
  repo enable PROFILE... | disable PROFILE... | toggle PROFILE...
  repo update [--all] [--dry-run] [--conflict abort|keep|replace]
  registry list | status | stats | prune [--dry-run]
  registry where --profile NAME | --skill NAME
  registry update --all [--dry-run] [--conflict abort|keep|replace]
  skill promote NAME [--repo PATH] [--dry-run] [--conflict abort|replace]
  status [--all] | update [--all] [--dry-run] [--conflict abort|keep|replace]

Repository commands accept --repo PATH. Otherwise use the nearest registered
workspace containing the current directory. Enable/disable change intent;
update copies files. Local drift blocks updates unless explicitly resolved.
Unmanaged skills are preserved. Imports and removals support --dry-run.

State defaults to $BESKAR_HOME or $HOME/.beskar. No Git or network required.
See docs/FORMAT.md for the editable .bsk format.
";

#[derive(Default)]
struct Args {
    words: Vec<String>,
    flags: BTreeSet<String>,
    values: BTreeMap<String, String>,
}
impl Args {
    fn parse(arguments: impl Iterator<Item = OsString>) -> Result<Self> {
        let mut result = Self::default();
        let mut args = arguments.map(|v| {
            v.into_string()
                .map_err(|_| "arguments must be UTF-8".to_string())
        });
        let mut literal = false;
        while let Some(arg) = args.next() {
            let arg = arg?;
            if literal {
                result.words.push(arg);
                continue;
            }
            if arg == "--" {
                literal = true;
                continue;
            }
            if matches!(arg.as_str(), "-h" | "-V") {
                result
                    .flags
                    .insert(if arg == "-h" { "help" } else { "version" }.into());
                continue;
            }
            if let Some(option) = arg.strip_prefix("--") {
                let (key, inline) = option
                    .split_once('=')
                    .map_or((option, None), |(k, v)| (k, Some(v)));
                if [
                    "home",
                    "library",
                    "registry",
                    "agent-skills",
                    "name",
                    "repo",
                    "conflict",
                    "profile",
                    "skill",
                ]
                .contains(&key)
                {
                    let value = match inline {
                        Some(v) => v.into(),
                        None => args
                            .next()
                            .ok_or_else(|| format!("--{key} needs a value"))??,
                    };
                    if value.is_empty() || value.starts_with("--") {
                        return Err(format!("--{key} needs a value"));
                    }
                    if result.values.insert(key.into(), value).is_some() {
                        return Err(format!("duplicate --{key}"));
                    }
                } else if ["all", "dry-run", "yes", "recover", "help", "version"].contains(&key)
                    && inline.is_none()
                {
                    if !result.flags.insert(key.into()) {
                        return Err(format!("duplicate --{key}"));
                    }
                } else {
                    return Err(format!("unknown option {arg}"));
                }
            } else if arg.starts_with('-') {
                return Err(format!(
                    "unknown option {arg}; use -- before a path starting with -"
                ));
            } else {
                result.words.push(arg);
            }
        }
        Ok(result)
    }
    fn flag(&self, key: &str) -> bool {
        self.flags.contains(key)
    }
    fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
    fn check(&self, flags: &[&str], values: &[&str]) -> Result<()> {
        for key in &self.flags {
            if !flags.contains(&key.as_str()) {
                return Err(format!("--{key} is not valid for this command"));
            }
        }
        for key in self.values.keys() {
            if key != "home" && !values.contains(&key.as_str()) {
                return Err(format!("--{key} is not valid for this command"));
            }
        }
        Ok(())
    }
    fn count(&self, min: usize, max: usize, usage: &str) -> Result<()> {
        if (min..=max).contains(&self.words.len()) {
            Ok(())
        } else {
            Err(format!("usage: beskar {usage}"))
        }
    }
    fn policy(&self) -> Result<Policy> {
        match self.value("conflict").unwrap_or("abort") {
            "abort" => Ok(Policy::Abort),
            "keep" => Ok(Policy::Keep),
            "replace" => Ok(Policy::Replace),
            other => Err(format!(
                "unknown conflict policy {other:?}; use abort, keep or replace"
            )),
        }
    }
}

pub fn run(arguments: impl Iterator<Item = OsString>) -> Result<()> {
    let args = Args::parse(arguments)?;
    if args.flag("version") {
        println!("beskar {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.flag("help") || args.words.is_empty() {
        print!("{HELP}");
        return Ok(());
    }
    let home = home(&args)?;
    let command = args.words[0].as_str();
    if command == "init" {
        return init(&home, &args);
    }
    // Validate dispatch before opening state, so typos get useful errors before initialization.
    if ![
        "doctor", "library", "profile", "repo", "registry", "skill", "status", "update",
    ]
    .contains(&command)
    {
        return Err(format!("unknown command {command:?}; use beskar --help"));
    }
    if !tree::exists(&home.join("config.bsk"))? {
        return Err("Beskar is not initialized; run beskar init".into());
    }
    let recovering = command == "doctor" && args.flag("recover");
    if command == "doctor" {
        args.check(&["recover"], &[])?;
        args.count(1, 1, "doctor [--recover]")?;
    }
    let mut store = Store::open(home, recovering)?;
    match command {
        "doctor" => doctor(&store),
        "library" => library(&mut store, &args),
        "profile" => profile(&mut store, &args),
        "repo" => repo(&mut store, &args),
        "registry" => registry(&mut store, &args),
        "skill" => promote(&mut store, &args),
        "status" => {
            args.count(1, 1, "status [--all] [--repo PATH]")?;
            args.check(&["all"], &["repo"])?;
            status(&store, &targets(&store, &args, false)?)
        }
        "update" => {
            args.count(1, 1, "update [--all] [--dry-run] [--conflict POLICY]")?;
            update(&mut store, &args, false)
        }
        _ => unreachable!(),
    }
}

fn home(args: &Args) -> Result<PathBuf> {
    let path = if let Some(value) = args.value("home") {
        PathBuf::from(value)
    } else if let Some(value) = std::env::var_os("BESKAR_HOME") {
        PathBuf::from(value)
    } else {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|v| PathBuf::from(v).join(".beskar"))
            .ok_or("set BESKAR_HOME or pass --home PATH")?
    };
    tree::absolute(&path)
}

fn initialize_library(config: &Config) -> Result<()> {
    for path in [
        config.library.join("skills"),
        config.library.join("profiles"),
    ] {
        tree::safe_path(&path)?;
        io(path.display(), fs::create_dir_all(&path))?;
    }
    Ok(())
}

fn init(home: &Path, args: &Args) -> Result<()> {
    args.count(
        1,
        1,
        "init [--library PATH] [--registry PATH] [--agent-skills PATH]",
    )?;
    args.check(&[], &["library", "registry", "agent-skills"])?;
    let path = home.join("config.bsk");
    let config = if tree::exists(&path)? {
        if ["library", "registry", "agent-skills"]
            .iter()
            .any(|key| args.value(key).is_some())
        {
            return Err("already initialized; edit config.bsk to change configuration".into());
        }
        Config::decode(&format::read(&path)?)?
    } else {
        Config {
            library: tree::absolute(
                &args
                    .value("library")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join("library")),
            )?,
            registry: tree::absolute(
                &args
                    .value("registry")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join("registry.bsk")),
            )?,
            agent_skills: PathBuf::from(args.value("agent-skills").unwrap_or(".agents/skills")),
        }
    };
    config.validate()?;
    for reserved in ["config.bsk", ".lock", "transaction.bsk"] {
        let reserved = home.join(reserved);
        if !disjoint(&config.library, &reserved) || !disjoint(&config.registry, &reserved) {
            return Err(format!(
                "state path overlaps reserved file {}",
                reserved.display()
            ));
        }
    }
    tree::safe_path(home)?;
    io(home.display(), fs::create_dir_all(home))?;
    let _lock = Lock::acquire(home)?;
    if tree::exists(&home.join("transaction.bsk"))? {
        return Err("unfinished transaction; run beskar doctor --recover".into());
    }
    initialize_library(&config)?;
    tree::safe_path(&config.registry)?;
    io(
        config.registry.display(),
        fs::create_dir_all(config.registry.parent().ok_or("registry needs a parent")?),
    )?;
    if tree::exists(&config.registry)? {
        Registry::decode(&format::read(&config.registry)?)?;
    } else {
        tree::write_new(&config.registry, &Registry::default().encode()?)?;
    }
    if !tree::exists(&path)? {
        tree::write_new(&path, &config.encode()?)?;
    }
    println!(
        "Initialized {}\nLibrary: {}\nRegistry: {}",
        home.display(),
        config.library.display(),
        config.registry.display()
    );
    Ok(())
}

fn subcommand(args: &Args) -> Result<&str> {
    args.words
        .get(1)
        .map(String::as_str)
        .ok_or("missing subcommand; use beskar --help".into())
}

fn library(store: &mut Store, args: &Args) -> Result<()> {
    match subcommand(args)? {
        "init" => {
            args.count(2, 2, "library init")?;
            args.check(&[], &[])?;
            initialize_library(&store.config)?;
            println!("Library: {}", store.config.library.display());
        }
        "list" => {
            args.count(2, 2, "library list")?;
            args.check(&[], &[])?;
            for name in store.skills()? {
                println!("{name}");
            }
        }
        "show" => {
            args.count(3, 3, "library show NAME")?;
            args.check(&[], &[])?;
            let name = &args.words[2];
            let path = store.skill_path(name)?;
            println!(
                "Skill: {name}\nPath: {}\nFingerprint: {}",
                path.display(),
                tree::fingerprint(&path)?
            );
            for profile in store.profiles()? {
                if store.profile(&profile)?.skills.contains(name) {
                    println!("Profile: {profile}");
                }
            }
            print_usage(store, name)?;
            let metadata = path.join("SKILL.md");
            tree::safe_path(&metadata)?;
            if metadata.is_file() {
                println!(
                    "\n{}",
                    io(metadata.display(), fs::read_to_string(&metadata))?
                );
            }
        }
        "add" => {
            args.count(3, 3, "library add PATH [--name NAME] [--dry-run]")?;
            args.check(&["dry-run"], &["name"])?;
            let source = workspace_path(Path::new(&args.words[2]))?;
            let name = args.value("name").map(String::from).unwrap_or(
                format::path_text(Path::new(source.file_name().ok_or("source needs a name")?))?
                    .into(),
            );
            import(store, &[(name, source)], args.flag("dry-run"))?;
        }
        "scan" => {
            args.count(3, 3, "library scan PATH [--yes] [--dry-run]")?;
            args.check(&["yes", "dry-run"], &[])?;
            let source = workspace_path(Path::new(&args.words[2]))?;
            let mut found = Vec::new();
            scan(&source, &mut found)?;
            println!("Found {} skills.", found.len());
            // Preflight before asking a person to approve the batch.
            validate_imports(store, &found)?;
            for (name, path) in &found {
                println!("  {name}  {}", path.display());
            }
            if found.is_empty() {
                return Ok(());
            }
            if args.flag("dry-run") {
                println!("No files changed.");
                return Ok(());
            }
            if !args.flag("yes") {
                if !std::io::stdin().is_terminal() {
                    return Err(
                        "scan needs --yes in non-interactive environments; preview with --dry-run"
                            .into(),
                    );
                }
                print!("Import {} skills? [y/N] ", found.len());
                io("stdout", std::io::stdout().flush())?;
                let mut answer = String::new();
                io("stdin", std::io::stdin().read_line(&mut answer))?;
                if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    println!("No files changed.");
                    return Ok(());
                }
            }
            import(store, &found, false)?;
        }
        "remove" => {
            args.count(3, 3, "library remove NAME [--dry-run]")?;
            args.check(&["dry-run"], &[])?;
            let name = &args.words[2];
            let target = store.skill_path(name)?;
            if !target.is_dir() {
                return Err(format!("unknown skill {name}"));
            }
            for profile in store.profiles()? {
                if store.profile(&profile)?.skills.contains(name) {
                    return Err(format!(
                        "skill {name} is referenced by profile {profile}; remove that reference first"
                    ));
                }
            }
            let hash = tree::fingerprint(&target)?;
            if args.flag("dry-run") {
                println!("Would remove {name}. No files changed.");
            } else {
                let mut transaction = Transaction::new(&store.home);
                transaction.remove(&target, &store.config.library, Some(hash))?;
                transaction.commit()?;
                println!("Removed {name}");
            }
        }
        other => return Err(format!("unknown library command {other:?}")),
    }
    Ok(())
}

fn scan(path: &Path, found: &mut Vec<(String, PathBuf)>) -> Result<()> {
    tree::safe_path(path)?;
    let skill_file = path.join("SKILL.md");
    tree::safe_path(&skill_file)?;
    if skill_file.is_file() {
        let name = format::path_text(Path::new(
            path.file_name().ok_or("skill needs a directory name")?,
        ))?
        .to_string();
        found.push((name, path.into()));
    } else {
        for child in tree::children(path)? {
            tree::safe_path(&child)?;
            if child.is_dir() && child.file_name().is_none_or(|name| name != ".git") {
                scan(&child, found)?;
            }
        }
    }
    Ok(())
}

fn validate_imports(store: &Store, imports: &[(String, PathBuf)]) -> Result<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut hashes = Vec::new();
    for (name, source) in imports {
        format::name(name)?;
        if !seen.insert(name) {
            return Err(format!(
                "multiple sources named {name}; import them individually with --name"
            ));
        }
        let destination = store.skill_path(name)?;
        if tree::exists(&destination)? {
            return Err(format!(
                "skill {name} already exists; import under a different --name"
            ));
        }
        if !disjoint(source, &store.config.library) {
            return Err("cannot import from inside the library or an ancestor of it".into());
        }
        if !source.is_dir() {
            return Err(format!("{}: skill must be a directory", source.display()));
        }
        hashes.push(tree::fingerprint(source)?);
    }
    Ok(hashes)
}

fn import(store: &Store, imports: &[(String, PathBuf)], dry: bool) -> Result<()> {
    let hashes = validate_imports(store, imports)?;
    if dry {
        for (name, source) in imports {
            println!("+ {name}  {}", source.display());
        }
        println!("No files changed.");
        return Ok(());
    }
    let mut transaction = Transaction::new(&store.home);
    for ((name, source), hash) in imports.iter().zip(hashes) {
        transaction.copy(
            &store.skill_path(name)?,
            source,
            &store.config.library,
            None,
            &hash,
        )?;
    }
    transaction.commit()?;
    for (name, _) in imports {
        println!("Imported {name}");
    }
    Ok(())
}

fn profile(store: &mut Store, args: &Args) -> Result<()> {
    args.check(&[], &[])?;
    match subcommand(args)? {
        "list" => {
            args.count(2, 2, "profile list")?;
            for name in store.profiles()? {
                println!("{name}  {} skills", store.profile(&name)?.skills.len());
            }
        }
        "create" => {
            args.count(3, 3, "profile create NAME")?;
            let name = &args.words[2];
            let path = store.profile_path(name)?;
            tree::write_new(&path, &Profile::default().encode())?;
            println!("Created profile {name}");
        }
        "show" => {
            args.count(3, 3, "profile show NAME")?;
            let name = &args.words[2];
            let profile = store.profile(name)?;
            println!(
                "Profile: {name}\nPath: {}",
                store.profile_path(name)?.display()
            );
            for skill in profile.skills {
                println!("  {skill}");
            }
            for (path, repo) in &store.registry.repos {
                if repo.profiles.contains(name) {
                    println!("Used by: {}", path.display());
                }
            }
        }
        "delete" => {
            args.count(3, 3, "profile delete NAME")?;
            let name = &args.words[2];
            store.profile(name)?;
            for (path, repo) in &store.registry.repos {
                if repo.profiles.contains(name) {
                    return Err(format!(
                        "profile {name} is enabled in {}; disable it first",
                        path.display()
                    ));
                }
            }
            tree::remove(&store.profile_path(name)?)?;
            println!("Deleted profile {name}");
        }
        action @ ("add" | "remove") => {
            args.count(4, usize::MAX, "profile add|remove NAME SKILL...")?;
            let name = &args.words[2];
            let mut profile = store.profile(name)?;
            for skill in &args.words[3..] {
                format::name(skill)?;
                if action == "add" {
                    let path = store.skill_path(skill)?;
                    tree::safe_path(&path)?;
                    if !path.is_dir() {
                        return Err(format!("unknown skill {skill}"));
                    }
                    profile.skills.insert(skill.clone());
                } else {
                    profile.skills.remove(skill);
                }
            }
            store.save_profile(name, &profile)?;
            println!("Saved profile {name}");
        }
        other => return Err(format!("unknown profile command {other:?}")),
    }
    Ok(())
}

fn workspace_path(path: &Path) -> Result<PathBuf> {
    let path = tree::absolute(path)?;
    if !path.is_dir() {
        return Err(format!(
            "{}: expected an existing workspace directory",
            path.display()
        ));
    }
    io(path.display(), fs::canonicalize(&path))
}

fn current_repo(store: &Store, args: &Args) -> Result<PathBuf> {
    if let Some(path) = args.value("repo") {
        let path = tree::absolute(Path::new(path))?;
        if store.registry.repos.contains_key(&path) {
            return Ok(path);
        }
        return Err(format!("{}: repository is not registered", path.display()));
    }
    let mut path = tree::absolute(&io("current directory", std::env::current_dir())?)?;
    loop {
        if store.registry.repos.contains_key(&path) {
            return Ok(path);
        }
        if !path.pop() {
            break;
        }
    }
    Err("no registered repository contains the current directory; run beskar repo add . or pass --repo PATH".into())
}

fn targets(store: &Store, args: &Args, global: bool) -> Result<Vec<PathBuf>> {
    if args.flag("all") || global {
        if args.value("repo").is_some() {
            return Err("--all and --repo cannot be combined".into());
        }
        Ok(store.registry.repos.keys().cloned().collect())
    } else {
        Ok(vec![current_repo(store, args)?])
    }
}

fn repo(store: &mut Store, args: &Args) -> Result<()> {
    match subcommand(args)? {
        action @ ("add" | "remove") => {
            args.count(2, 3, "repo add|remove [PATH]")?;
            args.check(&[], &["repo"])?;
            if args.words.len() == 3 && args.value("repo").is_some() {
                return Err("choose a positional path or --repo PATH".into());
            }
            let path = if action == "add" {
                workspace_path(Path::new(
                    args.words
                        .get(2)
                        .map(String::as_str)
                        .or(args.value("repo"))
                        .unwrap_or("."),
                ))?
            } else if let Some(path) = args.words.get(2) {
                tree::absolute(Path::new(path))?
            } else {
                current_repo(store, args)?
            };
            if action == "add" {
                store.validate_repo(&path)?;
                tree::safe_path(&path.join(&store.config.agent_skills))?;
                store.registry.repos.entry(path.clone()).or_default();
                println!("Registered {}", path.display());
            } else {
                store
                    .registry
                    .repos
                    .remove(&path)
                    .ok_or("repository is not registered")?;
                println!(
                    "Unregistered {}. Workspace skills are preserved.",
                    path.display()
                );
            }
            store.save_registry()?;
        }
        "list" => {
            args.count(2, 2, "repo list")?;
            args.check(&[], &[])?;
            list(store);
        }
        "status" => {
            args.count(2, 2, "repo status [--all] [--repo PATH]")?;
            args.check(&["all"], &["repo"])?;
            status(store, &targets(store, args, false)?)?;
        }
        action @ ("enable" | "disable" | "toggle") => {
            args.count(
                3,
                usize::MAX,
                "repo enable|disable|toggle PROFILE... [--repo PATH]",
            )?;
            args.check(&[], &["repo"])?;
            let path = current_repo(store, args)?;
            let mut next = store.registry.repos.get(&path).unwrap().clone();
            let mut seen = BTreeSet::new();
            for name in &args.words[2..] {
                format::name(name)?;
                if !seen.insert(name) {
                    return Err(format!("duplicate profile {name}"));
                }
                if action == "disable" || action == "toggle" && next.profiles.contains(name) {
                    next.profiles.remove(name);
                } else {
                    store.desired(&BTreeSet::from([name.clone()]))?;
                    next.profiles.insert(name.clone());
                }
            }
            store.registry.repos.insert(path.clone(), next);
            store.save_registry()?;
            println!(
                "Saved profiles for {}. Run beskar repo update to apply.",
                path.display()
            );
        }
        "update" => {
            args.count(2, 2, "repo update [--all] [--dry-run] [--conflict POLICY]")?;
            update(store, args, false)?;
        }
        other => return Err(format!("unknown repo command {other:?}")),
    }
    Ok(())
}

fn list(store: &Store) {
    for (path, repo) in &store.registry.repos {
        println!(
            "{}  [profiles: {}]  {} installed",
            path.display(),
            repo.profiles.iter().cloned().collect::<Vec<_>>().join(", "),
            repo.installed.len()
        );
    }
}

fn print_plan(plan: &Plan, unchanged: bool) {
    println!("Repository: {}", plan.path.display());
    for skill in &plan.skills {
        if !unchanged && skill.action == Action::Unchanged {
            continue;
        }
        let symbol = match skill.action {
            Action::Add => "+",
            Action::Update => "~",
            Action::Remove => "-",
            Action::Unchanged => "=",
            Action::Keep => "k",
            Action::Conflict => "!",
        };
        let profiles = if skill.profiles.is_empty() {
            String::new()
        } else {
            format!(
                " [profiles: {}]",
                skill
                    .profiles
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        println!("  {symbol} {}  {}{profiles}", skill.name, skill.reason);
    }
    for name in &plan.unmanaged {
        println!("  ? {name}  unmanaged, preserved");
    }
    if plan.pending() == 0 {
        println!("  Up to date.");
    }
}

fn update(store: &mut Store, args: &Args, global: bool) -> Result<()> {
    args.check(&["all", "dry-run"], &["repo", "conflict"])?;
    if global && !args.flag("all") {
        return Err("usage: beskar registry update --all".into());
    }
    let paths = targets(store, args, global)?;
    let plans = reconcile::plan_all(store, &paths, args.policy()?)?;
    for plan in &plans {
        print_plan(plan, false);
    }
    if args.flag("dry-run") {
        println!("No files changed.");
        if plans.iter().any(Plan::conflicts) {
            return Err("dry run found conflicts".into());
        }
        return Ok(());
    }
    reconcile::apply(store, &plans)?;
    println!("Updated {} repositories.", plans.len());
    Ok(())
}

fn status(store: &Store, paths: &[PathBuf]) -> Result<()> {
    let mut errors = Vec::new();
    for path in paths {
        match reconcile::plan(store, path, Policy::Abort) {
            Ok(plan) => print_plan(&plan, true),
            Err(error) => {
                println!("! {error}");
                errors.push(error);
            }
        }
    }
    if !errors.is_empty() {
        return Err(format!(
            "{} repositories could not be inspected",
            errors.len()
        ));
    }
    Ok(())
}

fn registry(store: &mut Store, args: &Args) -> Result<()> {
    match subcommand(args)? {
        "list" => {
            args.count(2, 2, "registry list")?;
            args.check(&[], &[])?;
            list(store);
        }
        "status" => {
            args.count(2, 2, "registry status")?;
            args.check(&[], &[])?;
            status(
                store,
                &store.registry.repos.keys().cloned().collect::<Vec<_>>(),
            )?;
        }
        "update" => {
            args.count(
                2,
                2,
                "registry update --all [--dry-run] [--conflict POLICY]",
            )?;
            update(store, args, true)?;
        }
        "stats" => {
            args.count(2, 2, "registry stats")?;
            args.check(&[], &[])?;
            let skills = store.skills()?;
            let profiles = store.profiles()?;
            let mut used = BTreeSet::new();
            for profile in &profiles {
                used.extend(store.profile(profile)?.skills);
            }
            for repo in store.registry.repos.values() {
                used.extend(repo.installed.keys().cloned());
            }
            println!(
                "Repositories      {}\nProfiles          {}\nLibrary skills    {}\nInstalled skills  {}\nUnused skills     {}",
                store.registry.repos.len(),
                profiles.len(),
                skills.len(),
                store
                    .registry
                    .repos
                    .values()
                    .map(|r| r.installed.len())
                    .sum::<usize>(),
                skills.iter().filter(|s| !used.contains(*s)).count()
            );
        }
        "where" => {
            args.count(2, 2, "registry where --profile NAME | --skill NAME")?;
            args.check(&[], &["profile", "skill"])?;
            match (args.value("profile"), args.value("skill")) {
                (Some(name), None) => {
                    store.profile(name)?;
                    println!("Profile: {name}");
                    for (path, repo) in &store.registry.repos {
                        if repo.profiles.contains(name) {
                            println!("  {}", path.display());
                        }
                    }
                }
                (None, Some(name)) => {
                    format::name(name)?;
                    print_usage(store, name)?;
                }
                _ => return Err("choose exactly one of --profile NAME and --skill NAME".into()),
            }
        }
        "prune" => {
            args.count(2, 2, "registry prune [--dry-run]")?;
            args.check(&["dry-run"], &[])?;
            let mut missing = Vec::new();
            for path in store.registry.repos.keys() {
                if !tree::exists(path)? {
                    missing.push(path.clone());
                }
            }
            for path in &missing {
                println!("- {}", path.display());
            }
            if args.flag("dry-run") {
                println!("No files changed.");
            } else {
                for path in &missing {
                    store.registry.repos.remove(path);
                }
                if !missing.is_empty() {
                    store.save_registry()?;
                }
                println!(
                    "Pruned {} missing repositories. Existing workspaces are preserved.",
                    missing.len()
                );
            }
        }
        other => return Err(format!("unknown registry command {other:?}")),
    }
    Ok(())
}

fn print_usage(store: &Store, name: &str) -> Result<()> {
    println!("Skill: {name}");
    for (path, (installed, profiles)) in reconcile::skill_usage(store, name)? {
        let presence = if installed {
            "tracked installation"
        } else {
            "desired, not installed"
        };
        println!(
            "  {}  {presence} [profiles: {}]",
            path.display(),
            profiles.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    Ok(())
}

fn promote(store: &mut Store, args: &Args) -> Result<()> {
    if subcommand(args)? != "promote" {
        return Err("usage: beskar skill promote NAME".into());
    }
    args.count(
        3,
        3,
        "skill promote NAME [--repo PATH] [--dry-run] [--conflict abort|replace]",
    )?;
    args.check(&["dry-run"], &["repo", "conflict"])?;
    let policy = args.policy()?;
    if policy == Policy::Keep {
        return Err("promotion accepts --conflict abort or --conflict replace".into());
    }
    let name = &args.words[2];
    format::name(name)?;
    let path = current_repo(store, args)?;
    let repo = store.registry.repos.get(&path).unwrap();
    store.validate_deployment(&path)?;
    let baseline = repo
        .installed
        .get(name)
        .ok_or_else(|| format!("{name}: only tracked installed skills can be promoted"))?;
    let local = path.join(&store.config.agent_skills).join(name);
    tree::safe_path(&local)?;
    if !local.is_dir() {
        return Err(format!(
            "{}: no skill directory to promote",
            local.display()
        ));
    }
    let current = tree::fingerprint(&local)?;
    let canonical = store.skill_path(name)?;
    let library = tree::optional_hash(&canonical)?;
    if library
        .as_ref()
        .is_some_and(|hash| hash != baseline && hash != &current)
        && policy != Policy::Replace
    {
        return Err("library changed since installation; review it before using --conflict replace to promote over it".into());
    }
    println!(
        "Promote {name}: {} -> {}",
        local.display(),
        canonical.display()
    );
    if args.flag("dry-run") {
        println!("No files changed.");
        return Ok(());
    }
    let mut registry = store.registry.clone();
    registry
        .repos
        .get_mut(&path)
        .unwrap()
        .installed
        .insert(name.clone(), current.clone());
    let mut transaction = Transaction::new(&store.home);
    if library.as_ref() != Some(&current) {
        transaction.copy(&canonical, &local, &store.config.library, library, &current)?;
    }
    if registry.encode()? != store.registry.encode()? {
        transaction.text(
            &store.config.registry,
            &registry.encode()?,
            tree::optional_hash(&store.config.registry)?,
        )?;
    }
    transaction.commit()?;
    store.registry = registry;
    println!("Promoted {name}. Run beskar update --all to distribute it.");
    Ok(())
}

fn doctor(store: &Store) -> Result<()> {
    let mut problems = Vec::new();
    for skill in store.skills()? {
        if let Err(e) = tree::fingerprint(&store.skill_path(&skill)?) {
            problems.push(e);
        }
    }
    for name in store.profiles()? {
        if let Err(e) = store.desired(&BTreeSet::from([name])) {
            problems.push(e);
        }
    }
    for path in store.registry.repos.keys() {
        match reconcile::plan(store, path, Policy::Abort) {
            Ok(plan) => {
                for skill in &plan.skills {
                    if skill.action == Action::Conflict {
                        problems.push(format!(
                            "{} / {}: {}",
                            path.display(),
                            skill.name,
                            skill.reason
                        ));
                    }
                }
                println!("{}: {} pending changes", path.display(), plan.pending());
            }
            Err(e) => problems.push(e),
        }
    }
    for problem in &problems {
        println!("! {problem}");
    }
    if !problems.is_empty() {
        return Err(format!("doctor found {} problems", problems.len()));
    }
    println!(
        "Beskar is healthy. Library: {}",
        store.config.library.display()
    );
    Ok(())
}

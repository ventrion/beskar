use crate::{
    args::Args,
    json::Json,
    output::{self, Report},
    prompt,
};
use beskar_core::{self as core, Beskar, Config, Plan, Policy, ProfileSelection, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn run(args: &Args, command: &str, words: &[String]) -> Result<Report> {
    let home = home(args)?;
    if command == "init" {
        let custom = ["library", "registry", "agent-skills"]
            .iter()
            .any(|key| args.value(key).is_some());
        let config = if custom {
            Some(Config {
                library: core::absolute_path(Path::new(
                    args.value("library")
                        .unwrap_or(home.join("library").to_str().ok_or("invalid home path")?),
                ))?,
                registry: core::absolute_path(Path::new(
                    args.value("registry").unwrap_or(
                        home.join("registry.bsk")
                            .to_str()
                            .ok_or("invalid home path")?,
                    ),
                ))?,
                agent_skills: args
                    .value("agent-skills")
                    .unwrap_or(".agents/skills")
                    .into(),
            })
        } else {
            None
        };
        let app = Beskar::init(&home, config)?;
        return Ok(Report::new(
            format!(
                "Initialized {}\nLibrary: {}\nRegistry: {}\n",
                home.display(),
                app.config().library.display(),
                app.config().registry.display()
            ),
            config_json(&app),
        ));
    }
    let mut app = Beskar::open(&home, command == "doctor" && args.flag("recover"))?;
    match command {
        "doctor" => {
            let health = app.doctor();
            let mut text = String::new();
            for (path, pending) in &health.repositories {
                text.push_str(&format!("{}: {pending} pending changes\n", path.display()));
            }
            for problem in &health.problems {
                text.push_str(&format!("! {problem}\n"));
            }
            let data = Json::obj([
                ("healthy", health.problems.is_empty().into()),
                ("problems", Json::strings(&health.problems)),
            ]);
            let report = Report::new(text, data);
            if health.problems.is_empty() {
                Ok(Report::new(
                    format!(
                        "{}Beskar is healthy. Library: {}\n",
                        report.text,
                        app.config().library.display()
                    ),
                    report.data,
                ))
            } else {
                Ok(report.fail(format!("doctor found {} problems", health.problems.len())))
            }
        }
        "config show" => Ok(Report::new(
            fs::read_to_string(home.join("config.bsk")).map_err(|e| e.to_string())?,
            config_json(&app),
        )),
        "config path" => Ok(Report::new(
            format!("{}\n", home.join("config.bsk").display()),
            Json::path(&home.join("config.bsk")),
        )),
        "config set" => {
            app.set_config(&words[0], &words[1])?;
            Ok(Report::message(format!("Saved {}", words[0])))
        }
        "library init" => {
            app.initialize_library()?;
            Ok(Report::message(format!(
                "Library: {}",
                app.config().library.display()
            )))
        }
        "library list" => {
            let names = app.skills()?;
            Ok(Report::new(lines(&names), Json::strings(names)))
        }
        "library show" => {
            let skill = app.skill(&words[0])?;
            let mut text = format!(
                "Skill: {}\nPath: {}\nFingerprint: {}\n",
                skill.name,
                skill.path.display(),
                skill.fingerprint
            );
            for profile in &skill.profiles {
                text.push_str(&format!("Profile: {profile}\n"));
            }
            if !skill.requires.is_empty() {
                text.push_str(&format!("Requires: {}\n", output::join(&skill.requires)));
            }
            if !skill.required_by.is_empty() {
                text.push_str(&format!(
                    "Required by: {}\n",
                    output::join(&skill.required_by)
                ));
            }
            text.push_str(&usage_text(&skill.usage));
            if let Some(content) = &skill.content {
                text.push_str(&format!("\n{content}\n"));
            }
            Ok(Report::new(
                text,
                Json::obj([
                    ("name", skill.name.into()),
                    ("path", Json::path(&skill.path)),
                    ("fingerprint", skill.fingerprint.into()),
                    ("profiles", Json::strings(skill.profiles)),
                    ("requires", Json::strings(skill.requires)),
                    ("required_by", Json::strings(skill.required_by)),
                    ("usage", output::usage(&skill.usage)),
                    ("content", skill.content.into()),
                ]),
            ))
        }
        "library add" | "library scan" => {
            let source = core::absolute_path(Path::new(&words[0]))?;
            let imports = if command == "library scan" {
                app.scan(&source)?
            } else {
                let name = match args.value("name") {
                    Some(name) => name.to_string(),
                    None => core::skill_name(&source)?,
                };
                // Names cannot contain commas, so the list needs no quoting.
                let requires: Vec<String> = args
                    .value("requires")
                    .map(|list| list.split(',').map(String::from).collect())
                    .unwrap_or_default();
                vec![app.prepare_import(&name, &source, &requires)?]
            };
            let mut text = format!("Found {} skills.\n", imports.len());
            for item in &imports {
                text.push_str(&format!("  {}  {}\n", item.name, item.source.display()));
                if !item.requires.is_empty() {
                    text.push_str(&format!("    requires {}\n", output::join(&item.requires)));
                }
            }
            let data = Json::arr(imports.iter().map(|i| {
                Json::obj([
                    ("name", i.name.clone().into()),
                    ("source", Json::path(&i.source)),
                    ("fingerprint", i.fingerprint.clone().into()),
                    ("requires", Json::strings(&i.requires)),
                ])
            }));
            if args.flag("dry-run") {
                text.push_str("No files changed.\n");
                return Ok(Report::new(text, data));
            }
            if !imports.is_empty() && command == "library scan" && !args.flag("yes") {
                if !prompt::interactive(args.flag("json")) {
                    return Ok(Report::new(text, data).fail(
                        "scan needs --yes in non-interactive environments; preview with --dry-run",
                    ));
                }
                eprint!("{text}");
                if !prompt::confirm(&format!("Import {} skills? [y/N] ", imports.len()))? {
                    return Ok(Report::message("No files changed."));
                }
            }
            app.import(&imports)?;
            for item in &imports {
                text.push_str(&format!("Imported {}\n", item.name));
            }
            Ok(Report::new(text, data))
        }
        "library remove" => {
            app.remove_skill(&words[0], args.flag("dry-run"))?;
            Ok(Report::message(format!(
                "{} {}{}",
                if args.flag("dry-run") {
                    "Would remove"
                } else {
                    "Removed"
                },
                words[0],
                if args.flag("dry-run") {
                    ". No files changed."
                } else {
                    ""
                }
            )))
        }
        "library require" | "library unrequire" => {
            app.edit_requirements(&words[0], &words[1..], command == "library require")?;
            Ok(Report::message(format!(
                "Saved metadata for {}. Run beskar update --all to apply.",
                words[0]
            )))
        }
        "profile list" => {
            let mut text = String::new();
            let mut data = Vec::new();
            for name in app.profiles()? {
                let profile = app.profile(&name)?;
                text.push_str(&format!("{name}  {} skills\n", profile.skills.len()));
                data.push(Json::obj([
                    ("name", name.into()),
                    ("skills", Json::strings(profile.skills)),
                ]));
            }
            Ok(Report::new(text, Json::arr(data)))
        }
        "profile show" => {
            let name = &words[0];
            let profile = app.profile(name)?;
            let repos: Vec<_> = app
                .registry()
                .repos
                .iter()
                .filter(|(_, r)| r.profiles.contains(name))
                .map(|(p, _)| p)
                .collect();
            // Show a broken profile anyway: this command is how a user inspects it.
            let (dependencies, problem) = match app.profile_dependencies(name) {
                Ok(dependencies) => (dependencies, None),
                Err(error) => (Default::default(), Some(error)),
            };
            let mut text = format!(
                "Profile: {name}\nPath: {}\n{}",
                app.profile_path(name)?.display(),
                lines(&profile.skills.iter().cloned().collect::<Vec<_>>())
            );
            for (skill, required_by) in &dependencies {
                text.push_str(&format!(
                    "Dependency: {skill}{}\n",
                    output::required_by(required_by)
                ));
            }
            for repo in &repos {
                text.push_str(&format!("Used by: {}\n", repo.display()));
            }
            if let Some(problem) = &problem {
                text.push_str(&format!("! {problem}\n"));
            }
            let report = Report::new(
                text,
                Json::obj([
                    ("name", name.clone().into()),
                    ("skills", Json::strings(profile.skills)),
                    (
                        "dependencies",
                        Json::arr(dependencies.iter().map(|(skill, required_by)| {
                            Json::obj([
                                ("name", skill.clone().into()),
                                ("required_by", Json::strings(required_by)),
                            ])
                        })),
                    ),
                    (
                        "repositories",
                        Json::arr(repos.into_iter().map(|p| Json::path(p))),
                    ),
                ]),
            );
            Ok(match problem {
                Some(problem) => report.fail(problem),
                None => report,
            })
        }
        "profile create" => {
            app.create_profile(&words[0], &words[1..])?;
            Ok(Report::message(format!("Created profile {}", words[0])))
        }
        "profile delete" => {
            app.delete_profile(&words[0])?;
            Ok(Report::message(format!("Deleted profile {}", words[0])))
        }
        "profile add" | "profile remove" => {
            let add = command == "profile add";
            // Resolve before saving, so broken metadata fails the command without an edit.
            let mut notes = String::new();
            if add {
                for skill in &words[1..] {
                    let requirements = app.requirements(skill)?;
                    if !requirements.is_empty() {
                        notes.push_str(&format!(
                            "\n{skill} also installs: {}",
                            output::join(&requirements)
                        ));
                    }
                }
            }
            app.edit_profile(&words[0], &words[1..], add)?;
            Ok(Report::message(format!(
                "Saved profile {}{notes}",
                words[0]
            )))
        }
        "repo add" | "repo remove" => {
            if !words.is_empty() && args.value("repo").is_some() {
                return Err("choose a positional path or --repo PATH".into());
            }
            if command == "repo add" {
                let path = app.register(Path::new(
                    words
                        .first()
                        .map(String::as_str)
                        .or(args.value("repo"))
                        .unwrap_or("."),
                ))?;
                Ok(Report::message(format!("Registered {}", path.display())))
            } else {
                let path = if let Some(word) = words.first() {
                    core::absolute_path(Path::new(word))?
                } else {
                    repository(&app, args)?
                };
                app.unregister(&path)?;
                Ok(Report::message(format!(
                    "Unregistered {}. Workspace skills are preserved.",
                    path.display()
                )))
            }
        }
        "repo enable" | "repo disable" | "repo toggle" => {
            let path = repository(&app, args)?;
            let selection = match command {
                "repo enable" => ProfileSelection::Enable,
                "repo disable" => ProfileSelection::Disable,
                _ => ProfileSelection::Toggle,
            };
            app.select_profiles(&path, words, selection)?;
            Ok(Report::message(format!(
                "Saved profiles for {}. Run beskar repo update to apply.",
                path.display()
            )))
        }
        "repo list" | "registry list" | "registry where" => {
            list(&app, args, command == "registry where")
        }
        "repo status" | "status" | "registry status" => {
            let paths = targets(&app, args, command == "registry status")?;
            let mut plans = Vec::new();
            let mut errors = Vec::new();
            for path in paths {
                match app.plan(&[path], Policy::Abort) {
                    Ok(mut p) => plans.append(&mut p),
                    Err(e) => errors.push(e),
                }
            }
            let mut text: String = plans.iter().map(|p| output::plan_text(p, true)).collect();
            for error in &errors {
                text.push_str(&format!("! {error}\n"));
            }
            let report = Report::new(
                text,
                Json::obj([
                    ("repositories", output::plans(&plans)),
                    ("errors", Json::strings(&errors)),
                ]),
            );
            if errors.is_empty() {
                Ok(report)
            } else {
                Ok(report.fail(format!(
                    "{} repositories could not be inspected",
                    errors.len()
                )))
            }
        }
        "repo update" | "update" | "registry update" => {
            update(&mut app, args, command == "registry update")
        }
        "registry stats" => {
            let stats = app.stats()?;
            Ok(Report::new(
                format!(
                    "Repositories      {}\nProfiles          {}\nLibrary skills    {}\nInstalled skills  {}\nUnused skills     {}\n",
                    stats.repositories,
                    stats.profiles,
                    stats.library_skills,
                    stats.installed_skills,
                    stats.unused_skills
                ),
                Json::obj([
                    ("repositories", stats.repositories.into()),
                    ("profiles", stats.profiles.into()),
                    ("library_skills", stats.library_skills.into()),
                    ("installed_skills", stats.installed_skills.into()),
                    ("unused_skills", stats.unused_skills.into()),
                ]),
            ))
        }
        "registry prune" => {
            let missing = app.prune(args.flag("dry-run"))?;
            let mut text: String = missing
                .iter()
                .map(|p| format!("- {}\n", p.display()))
                .collect();
            text.push_str(&if args.flag("dry-run") {
                "No files changed.\n".into()
            } else {
                format!(
                    "Pruned {} missing repositories. Existing workspaces are preserved.\n",
                    missing.len()
                )
            });
            Ok(Report::new(
                text,
                Json::arr(missing.iter().map(|p| Json::path(p))),
            ))
        }
        "skill diff" => {
            let differences = app.differences(&repository(&app, args)?, &words[0])?;
            let text: String = if differences.is_empty() {
                "No differences.\n".into()
            } else {
                differences.iter().map(|d| d.text.as_str()).collect()
            };
            Ok(Report::new(
                text,
                Json::arr(differences.iter().map(|d| {
                    Json::obj([
                        ("path", Json::path(&d.path)),
                        ("change", d.change.as_str().into()),
                        ("diff", d.text.clone().into()),
                    ])
                })),
            ))
        }
        "skill promote" => {
            if args.policy()? == Policy::Keep {
                return Err("promotion accepts --conflict abort or --conflict replace".into());
            }
            let path = repository(&app, args)?;
            let promotion = app.promote(
                &path,
                &words[0],
                args.policy()? == Policy::Replace,
                args.flag("dry-run"),
            )?;
            let text = format!(
                "Promote {}: {} -> {}\n{}\n",
                promotion.name,
                promotion.source.display(),
                promotion.destination.display(),
                if args.flag("dry-run") {
                    "No files changed."
                } else {
                    "Promoted. Run beskar update --all to distribute it."
                }
            );
            Ok(Report::new(
                text,
                Json::obj([
                    ("name", promotion.name.into()),
                    ("source", Json::path(&promotion.source)),
                    ("destination", Json::path(&promotion.destination)),
                    ("fingerprint", promotion.fingerprint.into()),
                ]),
            ))
        }
        _ => unreachable!("validated command"),
    }
}
fn home(args: &Args) -> Result<PathBuf> {
    let path = if let Some(path) = args.value("home") {
        PathBuf::from(path)
    } else if let Some(path) = std::env::var_os("BESKAR_HOME") {
        PathBuf::from(path)
    } else {
        PathBuf::from(
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .ok_or("set BESKAR_HOME or pass --home PATH")?,
        )
        .join(".beskar")
    };
    core::absolute_path(&path)
}
fn repository(app: &Beskar, args: &Args) -> Result<PathBuf> {
    app.repository(
        args.value("repo").map(Path::new),
        &std::env::current_dir().map_err(|e| e.to_string())?,
    )
}
fn targets(app: &Beskar, args: &Args, global: bool) -> Result<Vec<PathBuf>> {
    if global || args.flag("all") {
        Ok(app.registry().repos.keys().cloned().collect())
    } else {
        Ok(vec![repository(app, args)?])
    }
}
fn config_json(app: &Beskar) -> Json {
    Json::obj([
        ("home", Json::path(app.home())),
        ("library", Json::path(&app.config().library)),
        ("registry", Json::path(&app.config().registry)),
        ("agent_skills", Json::path(&app.config().agent_skills)),
    ])
}
fn lines(names: &[String]) -> String {
    names.iter().map(|s| format!("{s}\n")).collect()
}
fn usage_text(items: &[core::Usage]) -> String {
    items
        .iter()
        .map(|u| {
            format!(
                "  {}  {}{}\n",
                u.path.display(),
                if u.installed {
                    "tracked installation"
                } else {
                    "desired, not installed"
                },
                output::reasons(&u.profiles, &u.required_by)
            )
        })
        .collect()
}
fn list(app: &Beskar, args: &Args, require_filter: bool) -> Result<Report> {
    if let Some(skill) = args.value("skill") {
        let usage = app.skill_usage(skill)?;
        return Ok(Report::new(
            format!("Skill: {skill}\n{}", usage_text(&usage)),
            output::usage(&usage),
        ));
    }
    let profile = args.value("profile");
    if let Some(profile) = profile {
        app.profile(profile)?;
    } else if require_filter {
        return Err("choose exactly one of --profile NAME and --skill NAME".into());
    }
    let mut text = String::new();
    let mut data = Vec::new();
    for (path, repo) in &app.registry().repos {
        if profile.is_some_and(|p| !repo.profiles.contains(p)) {
            continue;
        }
        text.push_str(&format!(
            "{}  [profiles: {}]  {} installed\n",
            path.display(),
            repo.profiles.iter().cloned().collect::<Vec<_>>().join(", "),
            repo.installed.len()
        ));
        data.push(Json::obj([
            ("path", Json::path(path)),
            ("profiles", Json::strings(&repo.profiles)),
            ("installed", Json::strings(repo.installed.keys())),
            (
                "last_sync",
                repo.last_sync
                    .map(|n| Json::Int(n as i64))
                    .unwrap_or(Json::Null),
            ),
        ]));
    }
    Ok(Report::new(text, Json::arr(data)))
}
fn update(app: &mut Beskar, args: &Args, global: bool) -> Result<Report> {
    let paths = targets(app, args, global)?;
    let mut plans = app.plan(&paths, args.policy()?)?;
    let mut resolution_error = None;
    if plans.iter().any(Plan::conflicts)
        && !args.flag("dry-run")
        && matches!(args.value("conflict"), None | Some("ask"))
        && prompt::interactive(args.flag("json"))
    {
        resolution_error = prompt::resolve(app, &mut plans).err();
    }
    let mut text: String = plans.iter().map(|p| output::plan_text(p, false)).collect();
    let blocked = plans.iter().any(Plan::conflicts) || resolution_error.is_some();
    if args.flag("dry-run") {
        text.push_str("No files changed.\n");
    } else if !blocked {
        app.apply(&plans)?;
        text.push_str(&format!("Updated {} repositories.\n", plans.len()));
    }
    let report = Report::new(
        text,
        Json::obj([
            ("dry_run", args.flag("dry-run").into()),
            ("applied", (!blocked && !args.flag("dry-run")).into()),
            ("repositories", output::plans(&plans)),
        ]),
    );
    if blocked {
        Ok(report.fail(resolution_error.unwrap_or_else(|| {
            if args.flag("dry-run") {
                "dry run found conflicts".into()
            } else {
                "update blocked by conflicts; no files changed".into()
            }
        })))
    } else {
        Ok(report)
    }
}

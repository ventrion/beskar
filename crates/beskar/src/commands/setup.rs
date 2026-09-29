//! `init`, `doctor` and `config`.

use beskar_core::config::Config;
use beskar_core::doctor::{self, Level};
use beskar_core::fsutil::display_path;
use beskar_core::{Beskar, Home, Result};

use super::{done, parse_cmd, unknown_sub};
use crate::args;
use crate::{help, Outcome};

pub fn init(home: &Home, argv: &[String]) -> Result<Outcome> {
    let parsed = match parse_cmd(argv, &[args::LIBRARY], 1, help::INIT) {
        Ok(p) => p,
        Err(o) => return Ok(o),
    };
    if !parsed.positional.is_empty() {
        return crate::usage("init takes no positional arguments", help::INIT);
    }
    let library = parsed.value(&args::LIBRARY).map(std::path::Path::new);
    let report = Beskar::init(home, library)?;
    let state = |created: bool| if created { "created" } else { "exists " };
    println!(
        "{} config    {}",
        state(report.created_config),
        display_path(&report.config_path)
    );
    println!(
        "{} library   {}",
        state(report.created_library),
        display_path(&report.library_path)
    );
    println!(
        "{} registry  {}",
        state(report.created_registry),
        display_path(&report.registry_path)
    );
    if report.created_config {
        println!(
            "\nNext:\n  beskar library scan <dir-with-skills>\n  beskar profile create <name>\n  cd <project> && beskar repo add . && beskar repo enable <name>\n  beskar repo update"
        );
    }
    done()
}

pub fn doctor(home: &Home, argv: &[String]) -> Result<Outcome> {
    if let Err(o) = parse_cmd(argv, &[], 1, help::DOCTOR) {
        return Ok(o);
    }
    let (findings, failed) = doctor::run_all(home)?;
    for f in &findings {
        println!("[{:<4}] {}", f.level, f.message);
    }
    let warns = findings.iter().filter(|f| f.level == Level::Warn).count();
    let fails = findings.iter().filter(|f| f.level == Level::Fail).count();
    println!(
        "\n{} check(s), {warns} warning(s), {fails} failure(s)",
        findings.len()
    );
    Ok(if failed { Outcome::Failed } else { Outcome::Ok })
}

pub fn config(home: &Home, sub: Option<&str>, argv: &[String]) -> Result<Outcome> {
    match sub {
        Some("show") => {
            if let Err(o) = parse_cmd(argv, &[], 2, help::CONFIG) {
                return Ok(o);
            }
            let config = Config::load(home)?;
            println!("# {}", display_path(&config.path));
            println!("library = {}", display_path(&config.library));
            println!("registry = {}", display_path(&config.registry));
            println!("skills_dir = {}", config.skills_dir.display());
            println!("on_conflict = {}", config.on_conflict.as_str());
            done()
        }
        Some("set") => {
            let parsed = match parse_cmd(argv, &[], 2, help::CONFIG) {
                Ok(p) => p,
                Err(o) => return Ok(o),
            };
            let [key, value] = parsed.positional.as_slice() else {
                return crate::usage("config set needs <key> <value>", help::CONFIG);
            };
            let mut config = Config::load(home)?;
            config.set(home, key, value)?;
            println!("{key} = {value}");
            done()
        }
        other => unknown_sub("config", other, help::CONFIG),
    }
}

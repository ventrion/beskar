//! `beskar init` — bootstrap the beskar environment.

use std::fs;

use crate::error::{Exit, Result};
use crate::library::Library;
use crate::registry::Registry;

use super::Ctx;

pub fn run(ctx: &Ctx) -> Result<Exit> {
    let home = crate::config::Config::resolve_home(ctx.global.home.as_deref())?;
    fs::create_dir_all(&home).map_err(|e| crate::error::Error::io(&home, &e))?;

    // Config.
    let config_path = crate::config::Config::config_file(&home);
    let cfg = if config_path.exists() {
        let cfg = crate::config::Config::load(&home)?;
        ctx.ui.note(&format!(
            "config      {} (kept)",
            crate::util::display_path(&config_path)
        ));
        cfg
    } else {
        let c = crate::config::Config::default_for(&home);
        c.save()?;
        ctx.ui.ok(&format!(
            "config      {}",
            crate::util::display_path(&config_path)
        ));
        c
    };

    // Registry.
    if cfg.registry_path.exists() {
        ctx.ui.note(&format!(
            "registry    {} (kept)",
            crate::util::display_path(&cfg.registry_path)
        ));
    } else {
        Registry::load(&cfg.registry_path)?.save()?;
        ctx.ui.ok(&format!(
            "registry    {}",
            crate::util::display_path(&cfg.registry_path)
        ));
    }

    // Library skeleton.
    let lib = Library::new(&cfg.library_path);
    lib.init_dirs()?;
    let n_skills = lib.list_skills()?.len();
    let n_profiles = lib.list_profiles()?.len();
    ctx.ui.ok(&format!(
        "library     {} ({} skills, {} profiles)",
        crate::util::display_path(&cfg.library_path),
        n_skills,
        n_profiles
    ));

    println!();
    ctx.ui.note("Repos materialize skills into <repo>/.agents/skills by default.");
    println!();
    println!("Next steps:");
    println!("  beskar library scan <dir>     # import existing skills");
    println!("  beskar profile create <name>  # group skills into profiles");
    println!("  beskar repo add .             # register a workspace");
    println!("  beskar repo enable <profile>  # select what the repo should have");
    println!("  beskar repo update            # materialize it");
    Ok(Exit::Ok)
}

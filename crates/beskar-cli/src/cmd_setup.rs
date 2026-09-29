//! `init`, `doctor` and `library init`.

use std::path::Path;

use beskar_core::doctor::{self, Level};
use beskar_core::{Config, Library, Result, paths};

use crate::args::Args;
use crate::ui::{self, Style, show};
use crate::{Ctx, Exit};

pub fn init(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let report = beskar_core::init(&ctx.home, args.value("library").map(Path::new))?;
    let c = &report.config;
    let state = |created: bool| if created { ctx.ui.paint("created", Style::Green) } else { ctx.ui.dim("exists") };
    let anything = report.created_config || report.created_library || report.created_registry;
    if anything {
        println!("Initialized Beskar in {}", show(&c.home));
    } else {
        println!("Beskar is already initialized in {}", show(&c.home));
    }
    print!(
        "{}",
        ui::table(&[
            vec!["  config".into(), show(&c.file()), state(report.created_config)],
            vec!["  library".into(), show(&c.library), state(report.created_library)],
            vec!["  registry".into(), show(&c.registry), state(report.created_registry)],
        ])
    );
    if anything {
        println!("\nNext:");
        print!(
            "{}",
            ui::table(&[
                vec!["  beskar library scan <dir>".into(), "import skills you already have".into()],
                vec!["  beskar profile create <name> <skill>...".into(), "group skills into a profile".into()],
                vec!["  beskar repo add . --enable <profile>".into(), "use the profile in a workspace".into()],
            ])
        );
    }
    Ok(Exit::Ok)
}

pub fn doctor(ctx: &Ctx, _args: &Args) -> Result<Exit> {
    let findings = doctor::run(&ctx.home);
    let (mut warnings, mut errors) = (0, 0);
    for f in &findings {
        let tag = match f.level {
            Level::Ok => ctx.ui.paint("ok   ", Style::Green),
            Level::Warning => {
                warnings += 1;
                ctx.ui.paint("warn ", Style::Yellow)
            }
            Level::Error => {
                errors += 1;
                ctx.ui.paint("error", Style::Red)
            }
        };
        println!("{tag}  {}", f.message);
        if let Some(hint) = &f.hint {
            println!("       {}", ctx.ui.dim(&format!("hint: {hint}")));
        }
    }
    println!("\n{}, {}", ui::plural(warnings, "warning", "warnings"), ui::plural(errors, "error", "errors"));
    Ok(if errors > 0 { Exit::Failure } else { Exit::Ok })
}

pub fn library_init(ctx: &Ctx, args: &Args) -> Result<Exit> {
    let configured = Config::load(&ctx.home).ok().map(|c| c.library);
    let path = match args.positional.first() {
        Some(p) => paths::absolute(Path::new(p))?,
        None => configured.clone().unwrap_or_else(|| Config::defaults(&ctx.home).library),
    };
    let was_library = Library::is_library(&path);
    Library::init(&path)?;
    if was_library {
        println!("{} is already a Beskar library", show(&path));
    } else {
        println!("Initialized a Beskar library in {}", show(&path));
    }
    match configured {
        Some(c) if c == path => {}
        Some(_) => println!(
            "{}",
            ctx.ui.dim(&format!(
                "hint: Beskar uses {}; set `library = {}` in {} to switch",
                show(&Config::load(&ctx.home)?.library),
                paths::tilde(&path),
                show(&ctx.home.join(beskar_core::config::CONFIG_FILE))
            ))
        ),
        None => println!("{}", ctx.ui.dim(&format!("hint: run `beskar init --library {}` to use it", path.display()))),
    }
    Ok(Exit::Ok)
}

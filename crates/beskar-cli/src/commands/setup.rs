use beskar_core::Beskar;
use beskar_core::doctor::{self, Severity};

use crate::app::{Ctx, Outcome};
use crate::args::Parsed;
use crate::render::{columns, plural};

pub fn init(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let library = parsed.value("library").map(std::path::Path::new);
    let (_, report) = Beskar::init(&ctx.home, library, &ctx.cwd)?;
    let state = |created: bool| if created { "(created)" } else { "(already there)" };
    let rows = vec![
        vec![
            "config".to_string(),
            ctx.show(&report.config_file),
            state(report.config_created).to_string(),
        ],
        vec![
            "library".to_string(),
            ctx.show(&report.library),
            state(report.library_created).to_string(),
        ],
        vec![
            "registry".to_string(),
            ctx.show(&report.registry),
            state(report.registry_created).to_string(),
        ],
    ];
    say!("Beskar is ready.\n");
    put!("{}", columns(&rows, "  "));
    say!(
        "\nNext: import skills with `beskar library scan <dir>` or `beskar library add <dir>`,\n\
         then `beskar help` for the rest."
    );
    Ok(0)
}

pub fn doctor(ctx: &Ctx, _parsed: &Parsed) -> Outcome {
    let report = doctor::run(&ctx.home);
    let mut errors = 0;
    let mut warnings = 0;
    let mut rows = Vec::new();
    let mut details: Vec<Vec<String>> = Vec::new();
    for check in &report.checks {
        let status = match check.worst() {
            None | Some(Severity::Info) => "ok",
            Some(Severity::Warning) => "warn",
            Some(Severity::Error) => "ERROR",
        };
        rows.push(vec![
            status.to_string(),
            shorten(ctx, &check.name),
            shorten(ctx, &check.summary),
        ]);
        let mut lines = Vec::new();
        for finding in &check.findings {
            let label = match finding.severity {
                Severity::Info => "note",
                Severity::Warning => {
                    warnings += 1;
                    "warning"
                }
                Severity::Error => {
                    errors += 1;
                    "error"
                }
            };
            let text = format!(
                "{label}: {}: {}",
                shorten(ctx, &finding.subject),
                shorten(ctx, &finding.message)
            );
            lines.push(indent_continuation(&text, "         "));
            if let Some(hint) = &finding.hint {
                lines.push(format!("  hint: {hint}"));
            }
        }
        details.push(lines);
    }
    let table = columns(&rows, "");
    for (line, lines) in table.lines().zip(&details) {
        say!("{line}");
        for detail in lines {
            say!("    {detail}");
        }
    }
    say!();
    if errors == 0 && warnings == 0 {
        say!("Everything looks fine.");
    } else {
        say!("{}, {}.", plural(errors, "error"), plural(warnings, "warning"));
    }
    Ok(i32::from(errors > 0))
}

/// Indents every line after the first, so a multi-line message stays under its label.
fn indent_continuation(text: &str, indent: &str) -> String {
    let mut lines = text.lines();
    let mut out = lines.next().unwrap_or_default().to_string();
    for line in lines {
        out.push('\n');
        out.push_str(indent);
        out.push_str(line.trim_start_matches("   ").trim_start());
    }
    out
}

/// Abbreviates the home directory wherever it appears in free text.
fn shorten(ctx: &Ctx, text: &str) -> String {
    match ctx.home.user_home() {
        Some(user) => text.replace(&user.display().to_string(), "~"),
        None => text.to_string(),
    }
}

mod args;
mod commands;
mod json;
mod output;
mod prompt;
mod spec;
use json::Json;
use output::Report;
use std::{
    ffi::OsString,
    io::{self, Write},
};

fn main() {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let json = arguments
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--json");
    let (report, code) = match run(arguments) {
        Ok(report) => {
            let code = u8::from(report.error.is_some());
            (report, code)
        }
        Err((code, error)) => (Report::new("", Json::Null).fail(error), code),
    };
    if let Some(error) = &report.error {
        let _ = writeln!(io::stderr().lock(), "beskar: {error}");
    }
    let text = if json {
        Json::obj([
            ("ok", (code == 0).into()),
            ("data", report.data),
            (
                "error",
                report
                    .error
                    .map(|message| {
                        Json::obj([
                            (
                                "kind",
                                if code == 2 {
                                    "usage".into()
                                } else {
                                    "operation".into()
                                },
                            ),
                            ("message", message.into()),
                        ])
                    })
                    .into(),
            ),
        ])
        .pretty()
    } else {
        report.text
    };
    if let Err(error) = io::stdout().lock().write_all(text.as_bytes()) {
        if error.kind() == io::ErrorKind::BrokenPipe {
            std::process::exit(i32::from(code));
        }
        let _ = writeln!(io::stderr().lock(), "beskar: stdout: {error}");
        std::process::exit(1);
    }
    std::process::exit(i32::from(code));
}
fn run(arguments: Vec<OsString>) -> Result<Report, (u8, String)> {
    let mut args = args::Args::parse(arguments.into_iter()).map_err(|e| (2, e))?;
    let topic = usize::from(args.words.first().is_some_and(|s| s == "help"));
    normalize_alias(&mut args.words[topic..]);
    if args.flag("version") {
        return Ok(Report::message(format!(
            "beskar {}",
            env!("CARGO_PKG_VERSION")
        )));
    }
    if args.words.first().is_some_and(|s| s == "help") {
        let text = spec::help(&args.words[1..]).map_err(|e| (2, e))?;
        return Ok(Report::new(text.clone(), text.into()));
    }
    if args.flag("help") || args.words.is_empty() {
        let text = spec::help(&args.words).map_err(|e| (2, e))?;
        return Ok(Report::new(text.clone(), text.into()));
    }
    let (command, size) = spec::find(&args).map_err(|e| (2, e))?;
    commands::run(&args, command.path, &args.words[size..]).map_err(|e| (1, e))
}

fn normalize_alias(words: &mut [String]) {
    if words.first().is_some_and(|s| s == "repo")
        && words
            .get(1)
            .is_some_and(|s| matches!(s.as_str(), "diff" | "promote"))
    {
        words[0] = "skill".into();
    }
}

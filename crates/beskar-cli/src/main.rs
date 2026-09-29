//! The `beskar` binary: reads the process's arguments and environment and hands them to the library.

use std::io::Write;
use std::process::ExitCode;

use beskar_cli::context::Context;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let mut ctx = match Context::from_process() {
        Ok(ctx) => ctx,
        Err(error) => {
            eprintln!("error: cannot read the current directory: {error}");
            return ExitCode::from(1);
        }
    };
    let status = beskar_cli::run(&args, &mut ctx);
    let _ = ctx.out.flush();
    let _ = ctx.err.flush();
    ExitCode::from(u8::try_from(status).unwrap_or(1))
}

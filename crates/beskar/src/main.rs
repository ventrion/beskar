fn main() {
    if let Err(error) = beskar::cli::run(std::env::args_os().skip(1)) {
        eprintln!("beskar: {error}");
        std::process::exit(1);
    }
}

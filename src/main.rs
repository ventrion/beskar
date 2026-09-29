mod cli;

fn main() {
    if let Err(error) = cli::run(std::env::args().skip(1).collect()) {
        eprintln!("beskar: {error}");
        std::process::exit(1);
    }
}

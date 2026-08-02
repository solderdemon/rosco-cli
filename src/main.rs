use clap::Parser;

fn main() {
    if let Err(error) = rosco_cli::run(rosco_cli::cli::Cli::parse()) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

use clap::Parser;
use sshm::{
    cli::{self, Cli},
    model::clean_text,
    store::Store,
};

fn main() {
    let cli = Cli::parse();
    let result = if let Some(cli::Commands::Update { check }) = &cli.command {
        sshm::update::run(*check)
    } else {
        Store::default_location().and_then(|store| cli::run(cli, store))
    };
    let code = match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("sshm: {}", clean_text(&format!("{error:#}")));
            1
        }
    };
    std::process::exit(code);
}

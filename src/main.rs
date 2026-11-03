use clap::Parser;
use sshm::{
    cli::{self, Cli},
    model::clean_text,
    store::Store,
};

fn main() {
    #[cfg(windows)]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("__finish-update")) {
        let code = match sshm::update::windows::finish_from_args() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("sshm update: {error:#}");
                1
            }
        };
        std::process::exit(code);
    }
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

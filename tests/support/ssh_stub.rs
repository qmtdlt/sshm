//! Local-only SSH stand-in. Never connects to a network or reads credentials.
use std::io::{self, IsTerminal, Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "StrictHostKeyChecking=yes") {
        assert!(io::stdout().is_terminal(), "stdout is not a terminal");
    }
    std::fs::write(std::env::var_os("SSHM_TEST_ARGS").unwrap(), args.join("\n")).unwrap();
    if args.last().is_some_and(|a| a == "sh -s") || args.iter().any(|a| a == "BatchMode=yes") {
        if args.last().is_some_and(|a| a == "sh -s") {
            let mut script = String::new();
            io::stdin().read_to_string(&mut script).unwrap();
            let log = std::path::PathBuf::from(std::env::var_os("SSHM_TEST_ARGS").unwrap());
            std::fs::write(log.with_extension("script"), script).unwrap();
            let mut count = std::fs::OpenOptions::new().create(true).append(true).open(log.with_extension("installs")).unwrap();
            writeln!(count, "install").unwrap();
            println!("SSHM_KEY_INSTALLED");
            return;
        }
        assert_eq!(args.last().map(String::as_str), Some("true"));
        let log = std::path::PathBuf::from(std::env::var_os("SSHM_TEST_ARGS").unwrap());
        if args.iter().any(|a| a == "StrictHostKeyChecking=yes")
            && (!log.with_extension("script").exists() || !args.iter().any(|a| a == "-i")) {
            std::process::exit(23);
        }
        let code = std::env::var("SSHM_TEST_EXIT").unwrap().parse::<i32>().unwrap();
        std::process::exit(code);
    }
    assert!(io::stdin().is_terminal(), "stdin is not a terminal");
    println!("SSHM_STUB_READY");
    io::stdout().flush().unwrap();
    if std::env::var_os("SSHM_TEST_WAIT").is_some() {
        let mut line = String::new();
        io::stdin().read_line(&mut line).unwrap();
        println!("SSHM_STUB_RECEIVED:{}", line.trim());
    }
    let code = std::env::var("SSHM_TEST_EXIT").unwrap_or_else(|_| "0".into()).parse::<i32>().unwrap();
    std::process::exit(code);
}

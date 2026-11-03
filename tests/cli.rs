use std::process::{Command, Output};

fn run(args: &[&str], dir: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sshm"))
        .args(args)
        .env("SSHM_CONFIG_DIR", dir)
        .output()
        .unwrap()
}

#[test]
fn all_commands_have_help_without_reading_or_creating_user_data() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("config");
    for args in [
        vec!["--help"],
        vec!["list", "--help"],
        vec!["connect", "--help"],
        vec!["add", "--help"],
        vec!["edit", "--help"],
        vec!["remove", "--help"],
        vec!["setup-key", "--help"],
        vec!["import", "--help"],
        vec!["--version"],
    ] {
        let output = run(&args, &dir);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(!dir.exists());
}

#[test]
fn noninteractive_list_works_and_interactive_commands_fail_clearly() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("config");
    let output = run(&["list"], &dir);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("还没有连接"));
    let output = run(&["add"], &dir);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("交互式终端"));
    assert!(!dir.exists());
}

#[test]
fn unknown_name_and_invalid_arguments_do_not_launch_ssh() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        run(&["connect", "not-found"], temp.path()).status.code(),
        Some(1)
    );
    for args in [
        vec!["connect"],
        vec!["unknown"],
        vec!["add", "--bad-flag"],
        vec!["list", "extra"],
    ] {
        assert_eq!(run(&args, temp.path()).status.code(), Some(2));
    }
}

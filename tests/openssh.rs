use sshm::{import, ssh};
use std::{fs, process::Command};

#[test]
fn native_openssh_accepts_imported_reference_without_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config with spaces");
    let content =
        "Host sample\n HostName 127.0.0.1\n User sshm-test\n Port 2222\n ProxyJump jump-box\n";
    fs::write(&path, content).unwrap();
    let preview = import::preview(&path).unwrap();
    let output = Command::new("ssh")
        .arg("-G")
        .args(ssh::arguments(&preview.profiles[0]).unwrap())
        .output();
    let output = match output {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("OpenSSH is not installed; skipping local parser verification");
            return;
        }
        other => other.unwrap(),
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "hostname 127.0.0.1",
        "user sshm-test",
        "port 2222",
        "proxyjump jump-box",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert_eq!(fs::read_to_string(path).unwrap(), content);
}

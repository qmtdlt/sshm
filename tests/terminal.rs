use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use sshm::{
    model::{Connection, Profile},
    store::Store,
};
use std::{
    io::{Read, Write},
    path::Path,
    process::Command,
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

fn ssh_stub() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/ssh_stub.rs");
        let path = dir
            .path()
            .join(if cfg!(windows) { "ssh.exe" } else { "ssh" });
        let output = Command::new("rustc")
            .arg("--edition=2024")
            .arg(source)
            .arg("-o")
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        dir
    })
    .path()
}

// Ignore CSI positioning/style sequences when matching visible Chinese text.
fn terminal_text(bytes: &[u8]) -> String {
    let mut plain = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at..].starts_with(b"\x1b[") {
            at += 2;
            while at < bytes.len() {
                let end = (0x40..=0x7e).contains(&bytes[at]);
                at += 1;
                if end {
                    break;
                }
            }
        } else {
            plain.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&plain).into_owned()
}

struct Session {
    _master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    rx: Receiver<Vec<u8>>,
    output: Vec<u8>,
}

impl Session {
    fn start(store: &Store, args: &[&str], exit: i32, wait: bool, log: &Path) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_sshm"));
        command.args(args);
        command.env("SSHM_CONFIG_DIR", &store.directory);
        command.env("SSHM_SSH_DIR", store.directory.join(".ssh"));
        // Never inherit a developer's real OAuth client in terminal tests.
        command.env(
            "SSHM_GOOGLE_OAUTH_CONFIG",
            store.directory.join("oauth-not-configured.json"),
        );
        command.env("SSHM_TEST_EXIT", exit.to_string());
        command.env("SSHM_TEST_ARGS", log);
        if args.first() == Some(&"setup-key") {
            command.env("SSHM_TEST_SETUP", "1");
        }
        if wait {
            command.env("SSHM_TEST_WAIT", "1");
        }
        let mut paths = vec![ssh_stub().to_path_buf()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        command.env("PATH", std::env::join_paths(paths).unwrap());
        command.env("TERM", "xterm-256color");
        command.env("SSHM_UPDATE_CHECK", "0");
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        // ConPTY requests the starting cursor position before CreateProcess
        // returns. Supply it before spawning to avoid a handshake deadlock.
        #[cfg(windows)]
        let writer = {
            let mut writer = writer;
            writer.write_all(b"\x1b[1;1R").unwrap();
            writer
        };
        let writer = Arc::new(Mutex::new(writer));
        let responder = Arc::clone(&writer);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0_u8; 8192];
            let mut pending = Vec::new();
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                // A PTY is not a terminal emulator: answer cursor-position queries
                // issued by Crossterm on macOS, including split escape sequences.
                pending.extend_from_slice(&buf[..n]);
                let requests = pending.windows(4).filter(|s| *s == b"\x1b[6n").count();
                if requests > 0 {
                    let mut writer = responder.lock().unwrap();
                    for _ in 0..requests {
                        let _ = writer.write_all(b"\x1b[1;1R");
                    }
                    let _ = writer.flush();
                }
                let retain = pending.len().saturating_sub(3);
                pending.drain(..retain);
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        Self {
            _master: pair.master,
            child,
            writer,
            rx,
            output: vec![],
        }
    }

    fn send(&mut self, text: &str) {
        self.output.clear();
        while self.rx.try_recv().is_ok() {}
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(text.as_bytes()).unwrap();
        writer.flush().unwrap();
    }

    fn expect(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(12);
        while !terminal_text(&self.output).contains(text) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {text:?}: {:?}",
                String::from_utf8_lossy(&self.output)
            );
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(bytes) => self.output.extend(bytes),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(e) => panic!(
                    "{e}: waiting for {text:?}: {:?}",
                    String::from_utf8_lossy(&self.output)
                ),
            }
        }
    }

    fn exit_code(&mut self) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.exit_code();
            }
            assert!(Instant::now() < deadline, "process did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn setup() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store {
        directory: temp.path().join("config"),
    };
    let mut profile = Profile::new(
        "dev".into(),
        Connection::Manual {
            host: "localhost".into(),
            user: "tester".into(),
            port: 2222,
            identity_file: None,
        },
    );
    profile.key_login_configured = true;
    store.save(profile, None).unwrap();
    (temp, store)
}

#[test]
fn sync_menu_reports_missing_client_and_returns_to_working_ssh() {
    let (temp, store) = setup();
    let mut session = Session::start(&store, &[], 0, true, &temp.path().join("args"));
    session.expect("登录 Google Drive 以同步");
    session.send("g");
    session.expect("无法读取 Google 桌面客户端配置文件");
    session.send("\x1b");
    session.expect("SSH Manager");
    session.send("\r");
    session.expect("SSHM_STUB_READY");
    session.send("exit\r");
    session.expect("SSH Manager");
    session.send("q");
    assert_eq!(session.exit_code(), 0);
}

#[test]
fn ssh_inherits_terminal_and_nonzero_status_reaches_the_shell() {
    let (temp, store) = setup();
    let log = temp.path().join("args");
    let mut session = Session::start(&store, &["connect", "dev"], 23, false, &log);
    session.expect("SSHM_STUB_READY");
    assert_eq!(session.exit_code(), 23);
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "-p\n2222\n-l\ntester\n--\nlocalhost"
    );
    assert!(store.find("dev").unwrap().last_used.is_some());
}

#[test]
fn tui_yields_keyboard_to_ssh_and_restores_list_after_failure() {
    let (temp, store) = setup();
    let mut session = Session::start(&store, &[], 23, true, &temp.path().join("args"));
    session.expect("SSH Manager");
    session.send("/dev\r");
    session.expect("dev");
    session.send("\r");
    session.expect("SSHM_STUB_READY");
    session.send("q\r");
    session.expect("SSHM_STUB_RECEIVED:q");
    session.expect("23");
    session.send("\r");
    session.expect("SSH Manager");
    session.expect("dev");
    session.send("q");
    assert_eq!(session.exit_code(), 0);
}

#[test]
fn new_connection_can_be_entered_and_saved_in_a_real_terminal() {
    let (temp, store) = setup();
    let mut session = Session::start(&store, &["add"], 0, false, &temp.path().join("args"));
    session.expect("Tab");
    session.send("新连接\t127.0.0.1\ttester\t\t\tlocal\t\r");
    assert_eq!(session.exit_code(), 0);
    let profile = store.find("新连接").unwrap();
    assert_eq!(profile.tags, ["local"]);
    assert!(profile.last_used.is_none());
}

#[test]
fn ctrl_c_in_child_does_not_kill_the_manager() {
    let (temp, store) = setup();
    let mut session = Session::start(&store, &[], 0, true, &temp.path().join("args"));
    session.expect("SSH Manager");
    session.send("\r");
    session.expect("SSHM_STUB_READY");
    session.send("\x03");
    session.expect("Enter");
    session.send("\r");
    session.expect("SSH Manager");
    session.send("q");
    assert_eq!(session.exit_code(), 0);
}

#[test]
fn normal_session_returns_to_the_list_without_acknowledgment() {
    let (temp, store) = setup();
    let mut session = Session::start(&store, &[], 0, true, &temp.path().join("args"));
    session.expect("SSH Manager");
    session.send("\r");
    session.expect("SSHM_STUB_READY");
    session.send("exit\r");
    session.expect("SSH Manager");
    session.send("q");
    assert_eq!(session.exit_code(), 0);
}

#[test]
fn setup_key_requires_confirmation_then_installs_verifies_and_reuses_key() {
    let (temp, store) = setup();
    let log = temp.path().join("args");
    let mut session = Session::start(&store, &["setup-key", "dev"], 0, false, &log);
    session.expect("authorized_keys");
    assert!(!store.directory.join("keys").exists());
    session.send("\t\r");
    assert_eq!(session.exit_code(), 0);
    let profile = store.find("dev").unwrap();
    let path = profile.identity_file().unwrap();
    assert!(path.is_file());
    let first_key = std::fs::read(path).unwrap();
    let script = std::fs::read_to_string(log.with_extension("script")).unwrap();
    assert!(script.contains("ssh-ed25519 "));
    assert!(!script.contains("PRIVATE KEY"));
    let arguments = std::fs::read_to_string(&log).unwrap();
    assert!(arguments.contains("BatchMode=yes"));
    assert!(arguments.contains("ControlPath=none"));
    assert!(arguments.contains("PasswordAuthentication=no"));
    assert!(arguments.contains(&path.display().to_string()));
    let mut session = Session::start(&store, &["setup-key", "dev"], 0, false, &log);
    session.expect("authorized_keys");
    session.send("\t\r");
    assert_eq!(session.exit_code(), 0);
    assert!(first_key == std::fs::read(path).unwrap());
}

#[test]
fn failed_key_verification_leaves_connection_unchanged_and_can_retry() {
    let (temp, store) = setup();
    let original = store.find("dev").unwrap();
    let log = temp.path().join("args");
    let mut session = Session::start(&store, &["setup-key", "dev"], 23, false, &log);
    session.expect("authorized_keys");
    session.send("\t\r");
    assert_eq!(session.exit_code(), 1);
    assert_eq!(store.find("dev").unwrap(), original);
    let mut retry = Session::start(&store, &["setup-key", "dev"], 0, false, &log);
    retry.expect("authorized_keys");
    retry.send("\t\r");
    assert_eq!(retry.exit_code(), 0);
    assert!(store.find("dev").unwrap().identity_file().is_some());
}

#[test]
fn cancelling_key_setup_creates_no_key_and_runs_no_ssh() {
    let (temp, store) = setup();
    let log = temp.path().join("args");
    let mut session = Session::start(&store, &["setup-key", "dev"], 0, false, &log);
    session.expect("authorized_keys");
    session.send("\r");
    assert_eq!(session.exit_code(), 0);
    assert!(!store.directory.join("keys").exists());
    assert!(!log.exists());
}

#[test]
fn imported_connection_gets_a_local_key_without_rewriting_original_config() {
    let (temp, store) = setup();
    let config = temp.path().join("source config");
    let content = "Host imported\n HostName localhost\n User tester\n";
    std::fs::write(&config, content).unwrap();
    let profile = Profile::new(
        "imported".into(),
        Connection::SshConfig {
            path: config.clone(),
            alias: "imported".into(),
        },
    );
    store.import(vec![profile]).unwrap();
    let mut session = Session::start(
        &store,
        &["setup-key", "imported"],
        0,
        false,
        &temp.path().join("args"),
    );
    session.expect("authorized_keys");
    session.send("\t\r");
    assert_eq!(session.exit_code(), 0);
    assert!(
        store
            .find("imported")
            .unwrap()
            .local_identity_file
            .is_some()
    );
    assert_eq!(std::fs::read_to_string(config).unwrap(), content);
}

fn existing_key_fixture(store: &Store) -> std::path::PathBuf {
    let directory = store.directory.join(".ssh");
    std::fs::create_dir_all(&directory).unwrap();
    let key = directory.join("existing key");
    std::fs::write(&key, "existing-private-file-must-not-change").unwrap();
    std::fs::write(
        directory.join("existing key.pub"),
        "ssh-ed25519 AAAAtest original-comment\n",
    )
    .unwrap();
    std::fs::write(directory.join("config"), "Host local-ubuntu\n HostName localhost\n User tester\n Port 2222\n IdentityFile \"~/.ssh/existing key\"\n").unwrap();
    dunce::canonicalize(key).unwrap()
}

#[test]
fn connecting_old_record_automatically_uses_the_reverse_matched_key() {
    let (temp, store) = setup();
    let key = existing_key_fixture(&store);
    let log = temp.path().join("args");
    let mut session = Session::start(&store, &["connect", "dev"], 0, false, &log);
    session.expect("SSHM_STUB_READY");
    assert_eq!(session.exit_code(), 0);
    assert!(
        std::fs::read_to_string(log)
            .unwrap()
            .contains(&key.display().to_string())
    );
    assert!(store.find("dev").unwrap().identity_file().is_none()); // Discovery alone does not rewrite records.
}

#[test]
fn setup_reuses_an_authorized_existing_key_without_generating_or_installing() {
    let (temp, store) = setup();
    let key = existing_key_fixture(&store);
    let log = temp.path().join("args");
    std::fs::write(log.with_extension("script"), "already-authorized").unwrap();
    let mut session = Session::start(&store, &["setup-key", "dev"], 0, false, &log);
    session.expect("authorized_keys");
    session.send("\t\r");
    assert_eq!(session.exit_code(), 0);
    assert_eq!(store.find("dev").unwrap().identity_file(), Some(&key));
    assert!(!store.directory.join("keys").exists());
    assert_eq!(
        std::fs::read_to_string(log.with_extension("script")).unwrap(),
        "already-authorized"
    );
    assert_eq!(
        std::fs::read_to_string(key).unwrap(),
        "existing-private-file-must-not-change"
    );
}

#[test]
fn setup_installs_only_the_existing_public_key_when_not_yet_authorized() {
    let (temp, store) = setup();
    let key = existing_key_fixture(&store);
    let log = temp.path().join("args");
    let mut session = Session::start(&store, &["setup-key", "dev"], 0, false, &log);
    session.expect("authorized_keys");
    session.send("\t\r");
    assert_eq!(session.exit_code(), 0);
    assert_eq!(store.find("dev").unwrap().identity_file(), Some(&key));
    assert!(!store.directory.join("keys").exists());
    assert!(
        std::fs::read_to_string(log.with_extension("script"))
            .unwrap()
            .contains("ssh-ed25519 AAAAtest")
    );
    assert_eq!(
        std::fs::read_to_string(key).unwrap(),
        "existing-private-file-must-not-change"
    );
}

#[test]
fn real_form_fills_a_matching_key_without_the_user_entering_a_path() {
    let (temp, store) = setup();
    let key = existing_key_fixture(&store);
    let mut session = Session::start(&store, &["add"], 0, false, &temp.path().join("args"));
    session.expect("Tab");
    session.send("autofill\tlocalhost\ttester\t\x152222\t\tlocal\t\r");
    assert_eq!(session.exit_code(), 0);
    assert_eq!(store.find("autofill").unwrap().identity_file(), Some(&key));
    assert_eq!(store.find("autofill").unwrap().tags, ["local"]);
}

fn require_first_setup(store: &Store) {
    let mut profile = store.find("dev").unwrap();
    profile.key_login_configured = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match store.save(profile.clone(), Some(profile.revision)) {
            Ok(_) => break,
            Err(error)
                if error.chain().any(|cause| {
                    cause
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::WouldBlock)
                }) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => panic!("cannot prepare first-connection fixture: {error:#}"),
        }
    }
}

#[test]
fn first_connect_sets_up_once_then_second_connection_goes_straight_to_ssh() {
    let (temp, store) = setup();
    require_first_setup(&store);
    let log = temp.path().join("args");
    let mut session = Session::start(&store, &[], 0, true, &log);
    session.expect("SSH Manager");
    session.send("\r");
    session.expect("仅本次");
    assert!(!log.exists());
    session.send("\r");
    session.expect("SSHM_STUB_READY");
    assert!(store.find("dev").unwrap().key_login_configured);
    assert!(
        std::fs::read_to_string(&log)
            .unwrap()
            .contains("IdentitiesOnly=yes")
    );
    assert!(
        store
            .find("dev")
            .unwrap()
            .identity_file()
            .unwrap()
            .is_file()
    );
    session.send("exit\r");
    session.expect("SSH Manager");
    session.send("\r");
    session.expect("SSHM_STUB_READY");
    session.send("exit\r");
    session.expect("SSH Manager");
    session.send("q");
    assert_eq!(session.exit_code(), 0);
    assert_eq!(
        std::fs::read_to_string(log.with_extension("installs"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn direct_connect_has_the_same_first_time_setup_and_preserves_exit_behavior() {
    let (temp, store) = setup();
    require_first_setup(&store);
    let log = temp.path().join("args");
    let mut session = Session::start(&store, &["connect", "dev"], 0, false, &log);
    session.expect("仅本次");
    session.send("\r");
    session.expect("SSHM_STUB_READY");
    assert_eq!(session.exit_code(), 0);
    let mut second = Session::start(&store, &["connect", "dev"], 0, false, &log);
    second.expect("SSHM_STUB_READY");
    assert_eq!(second.exit_code(), 0);
    assert_eq!(
        std::fs::read_to_string(log.with_extension("installs"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn temporary_login_does_not_install_keys_or_claim_to_remember_passwords() {
    let (temp, store) = setup();
    require_first_setup(&store);
    let log = temp.path().join("args");
    let mut session = Session::start(&store, &["connect", "dev"], 0, false, &log);
    session.expect("仅本次");
    session.send("\t\r");
    session.expect("SSHM_STUB_READY");
    assert_eq!(session.exit_code(), 0);
    assert!(!store.find("dev").unwrap().key_login_configured);
    assert!(!store.directory.join("keys").exists());
    assert!(!log.with_extension("installs").exists());
}

#[test]
fn failed_first_time_verification_never_opens_a_password_session_or_marks_ready() {
    let (temp, store) = setup();
    require_first_setup(&store);
    let log = temp.path().join("args");
    let mut session = Session::start(&store, &["connect", "dev"], 23, false, &log);
    session.expect("仅本次");
    session.send("\r");
    assert_eq!(session.exit_code(), 1);
    assert!(!store.find("dev").unwrap().key_login_configured);
    assert!(store.find("dev").unwrap().identity_file().is_none());
    assert!(std::fs::read_to_string(log).unwrap().ends_with("true"));
}

#[test]
fn add_and_connect_finishes_setup_before_opening_the_session() {
    let (temp, store) = setup();
    let log = temp.path().join("args");
    let mut session = Session::start(&store, &["add", "--connect"], 0, false, &log);
    session.expect("Tab");
    session.send("fresh\tlocalhost\ttester\t\t\t\t\r");
    session.expect("仅本次");
    session.send("\r");
    session.expect("SSHM_STUB_READY");
    assert_eq!(session.exit_code(), 0);
    assert!(store.find("fresh").unwrap().key_login_configured);
}

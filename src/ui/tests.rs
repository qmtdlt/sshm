use super::{
    app::{Action, App, Confirmation, Entry, Screen},
    form::Form,
    render,
};
use crate::{
    model::{Connection, Profile},
    store::Store,
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

fn setup() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store {
        directory: dir.path().join("config"),
    };
    let mut app = App::new(store, Entry::List).unwrap();
    app.remote_environment = false;
    app.discovery = crate::discovery::Discovery::in_directory(dir.path().join(".ssh"));
    (dir, app)
}
fn key(app: &mut App, code: KeyCode) -> Action {
    app.event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

#[test]
fn google_footer_opens_login_and_never_intercepts_search_typing() {
    let (_dir, mut app) = setup();
    let text = snapshot(&mut app, 80, 24);
    assert!(text.contains("登录 Google Drive 以同步"));
    assert!(matches!(
        key(&mut app, KeyCode::Char('g')),
        Action::Sync(crate::sync::worker::Command::Login)
    ));
    assert!(matches!(app.screen, Screen::Sync { .. }));
    app.sync_status.authorizing = true;
    assert!(matches!(key(&mut app, KeyCode::Esc), Action::CancelSync));
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Char('g'));
    assert_eq!(app.query.value, "g");
}

#[test]
fn google_account_switch_requires_explicit_yes_and_can_be_cancelled() {
    let (_dir, mut app) = setup();
    app.screen = Screen::Sync { offset: 0 };
    app.sync_status.confirm_account = true;
    assert!(matches!(key(&mut app, KeyCode::Enter), Action::None));
    assert!(matches!(
        key(&mut app, KeyCode::Char('y')),
        Action::Sync(crate::sync::worker::Command::ConfirmAccount(true))
    ));
    assert!(matches!(
        key(&mut app, KeyCode::Esc),
        Action::Sync(crate::sync::worker::Command::ConfirmAccount(false))
    ));
}

#[test]
fn p_pulls_only_on_explicit_key_and_remains_text_in_search() {
    let (_dir, mut app) = setup();
    assert!(matches!(key(&mut app, KeyCode::Char('p')), Action::None));
    assert!(app.message.contains("登录"));
    app.sync_status.logged_in = true;
    assert!(matches!(
        key(&mut app, KeyCode::Char('p')),
        Action::Sync(crate::sync::worker::Command::Now)
    ));
    app.screen = Screen::Sync { offset: 0 };
    assert!(matches!(
        key(&mut app, KeyCode::Char('p')),
        Action::Sync(crate::sync::worker::Command::Now)
    ));
    app.screen = Screen::List;
    key(&mut app, KeyCode::Char('/'));
    assert!(matches!(key(&mut app, KeyCode::Char('p')), Action::None));
    assert_eq!(app.query.value, "p");
}

#[test]
fn remote_reload_does_not_replace_unsaved_form() {
    let (_dir, mut app) = setup();
    let original = add_profile(&mut app, "original");
    key(&mut app, KeyCode::Char('e'));
    let Screen::Form(form) = &mut app.screen else {
        panic!("form");
    };
    form.fields[0].input = super::input::Input::new("unsaved");
    let mut remote = original.clone();
    remote.tags.push("remote".into());
    app.store.save(remote, Some(original.revision)).unwrap();
    app.reload(Some(original.id)).unwrap();
    let Screen::Form(form) = &app.screen else {
        panic!("form");
    };
    assert_eq!(form.fields[0].input.value, "unsaved");
    let candidate = form.profile(&app.profiles).unwrap();
    assert!(app.store.save(candidate, Some(original.revision)).is_err());
}

#[test]
fn unbound_import_opens_binding_instead_of_ssh_or_key_installation() {
    let (_dir, mut app) = setup();
    let mut p = Profile::new(
        "imported".into(),
        Connection::SshConfig {
            path: Default::default(),
            alias: "server".into(),
        },
    );
    p.import_source = Some(uuid::Uuid::new_v4());
    app.store.save(p, None).unwrap();
    app.reload(None).unwrap();
    for code in [KeyCode::Enter, KeyCode::Char('s'), KeyCode::Char('e')] {
        app.screen = Screen::List;
        assert!(matches!(key(&mut app, code), Action::None));
        assert!(matches!(app.screen, Screen::BindConfig { .. }));
    }
}

#[test]
fn sync_screens_render_in_narrow_terminal_and_scroll_authorization_link() {
    let (_dir, mut app) = setup();
    app.screen = Screen::Sync { offset: 2 };
    app.sync_status.authorizing = true;
    app.sync_status.url = Some(format!(
        "https://example.test/authorize?{}",
        "x".repeat(600)
    ));
    key(&mut app, KeyCode::Down);
    assert!(matches!(app.screen, Screen::Sync { offset: 3 }));
    for (w, h) in [(38, 12), (80, 24)] {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| render::draw(f, &mut app)).unwrap();
    }
}
fn fill_form(app: &mut App) {
    let Screen::Form(form) = &mut app.screen else {
        panic!("not a form")
    };
    for (field, value) in
        form.fields
            .iter_mut()
            .zip(["开发机", "::1", "tester", "2222", "", "开发, API"])
    {
        field.input = super::input::Input::new(value);
    }
}
fn add_profile(app: &mut App, name: &str) -> Profile {
    let p = app
        .store
        .save(
            Profile::new(
                name.into(),
                Connection::Manual {
                    host: "localhost".into(),
                    user: "tester".into(),
                    port: 22,
                    identity_file: None,
                },
            ),
            None,
        )
        .unwrap();
    app.reload(None).unwrap();
    p
}

#[test]
fn typed_shortcut_letters_never_trigger_list_actions() {
    let (_dir, mut app) = setup();
    key(&mut app, KeyCode::Char('/'));
    for ch in "adeiq".chars() {
        key(&mut app, KeyCode::Char(ch));
    }
    assert_eq!(app.query.value, "adeiq");
    assert!(matches!(app.screen, Screen::List));
    key(&mut app, KeyCode::Enter);
    assert!(!app.searching);
    assert_eq!(app.query.value, "adeiq");
    key(&mut app, KeyCode::Esc);
    assert!(app.query.value.is_empty());
}

#[test]
fn form_validation_preserves_values_and_focuses_invalid_field() {
    let (_dir, mut app) = setup();
    key(&mut app, KeyCode::Char('a'));
    fill_form(&mut app);
    if let Screen::Form(form) = &mut app.screen {
        form.fields[3].input.value = "65536".into();
        form.focus = form.fields.len();
    }
    key(&mut app, KeyCode::Enter);
    let Screen::Form(form) = &app.screen else {
        panic!()
    };
    assert_eq!(form.focus, 3);
    assert_eq!(form.fields[0].input.value, "开发机");
    assert!(app.store.load().unwrap().is_empty());
}

#[test]
fn save_and_connect_persists_before_handing_terminal_over() {
    let (_dir, mut app) = setup();
    key(&mut app, KeyCode::Char('a'));
    fill_form(&mut app);
    if let Screen::Form(form) = &mut app.screen {
        form.focus = form.fields.len() + 1;
    }
    assert!(matches!(key(&mut app, KeyCode::Enter), Action::None));
    let Screen::FirstConnect { profile, choice } = &app.screen else {
        panic!()
    };
    assert_eq!(app.store.find("开发机").unwrap(), *profile);
    assert_eq!(profile.tags, ["开发", "API"]);
    assert_eq!(*choice, 0);
}

#[test]
fn dirty_cancel_defaults_to_keep_editing() {
    let (_dir, mut app) = setup();
    key(&mut app, KeyCode::Char('a'));
    fill_form(&mut app);
    key(&mut app, KeyCode::Esc);
    assert!(matches!(
        app.screen,
        Screen::Confirm {
            yes: false,
            action: Confirmation::Discard(_)
        }
    ));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.screen, Screen::Form(_)));
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.screen, Screen::List));
    assert!(app.store.load().unwrap().is_empty());
}

#[test]
fn delete_defaults_to_cancel_and_needs_explicit_confirmation() {
    let (_dir, mut app) = setup();
    add_profile(&mut app, "delete me");
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.store.load().unwrap().len(), 1);
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Enter);
    assert!(app.store.load().unwrap().is_empty());
}

#[test]
fn direct_add_exits_after_save_and_connect_flag_selects_connect() {
    let (dir, app) = setup();
    let mut app = App::new(app.store, Entry::Add(true)).unwrap();
    fill_form(&mut app);
    if let Screen::Form(form) = &mut app.screen {
        form.focus = form.fields.len();
    }
    assert!(matches!(key(&mut app, KeyCode::Enter), Action::None));
    assert!(matches!(app.screen, Screen::FirstConnect { choice: 0, .. }));
    assert!(app.standalone);
    drop(dir);
}

#[test]
fn imported_edit_changes_labels_only() {
    let (_dir, app) = setup();
    let connection = Connection::SshConfig {
        path: app.store.directory.join("source"),
        alias: "real-alias".into(),
    };
    let original = Profile::new("label".into(), connection.clone());
    let mut form = Form::new(Some(original), false);
    assert_eq!(form.fields.len(), 2);
    form.fields[0].input.value = "new label".into();
    let edited = form.profile(&[]).unwrap();
    assert_eq!(edited.connection, connection);
}

#[test]
fn import_selection_and_report_are_keyboard_driven() {
    let (dir, mut app) = setup();
    let path = dir.path().join("ssh_config");
    std::fs::write(&path, "Host dev prod\nHost *\n").unwrap();
    let before = std::fs::read(&path).unwrap();
    let preview = crate::import::preview(&path).unwrap();
    app.screen = Screen::Import {
        checked: vec![false; preview.profiles.len()],
        preview,
        cursor: 0,
    };
    key(&mut app, KeyCode::Char(' '));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.store.load().unwrap().len(), 1);
    assert!(matches!(app.screen, Screen::Report { .. }));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

fn snapshot(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| render::draw(frame, app)).unwrap();
    let buf = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..height {
        let mut x = 0;
        while x < width {
            let symbol = buf[(x, y)].symbol();
            text.push_str(symbol);
            x += unicode_width::UnicodeWidthStr::width(symbol).max(1) as u16;
        }
        text.push('\n');
    }
    text
}

#[test]
fn responsive_columns_hide_in_the_agreed_order() {
    let (_dir, mut app) = setup();
    add_profile(&mut app, "服务器");
    assert!(snapshot(&mut app, 120, 30).contains("最近使用"));
    let medium = snapshot(&mut app, 90, 24);
    assert!(!medium.contains("最近使用"));
    assert!(medium.contains("标签"));
    let narrow = snapshot(&mut app, 70, 20);
    assert!(!narrow.contains("标签"));
    assert!(narrow.contains("用户"));
}

#[test]
fn every_screen_renders_at_small_and_large_terminal_sizes() {
    let (_dir, mut app) = setup();
    for (width, height) in [(1, 1), (37, 11), (38, 12), (60, 16), (80, 24), (160, 50)] {
        app.screen = Screen::List;
        snapshot(&mut app, width, height);
        app.screen = Screen::Help;
        snapshot(&mut app, width, height);
        app.screen = Screen::Form(Form::new(None, false));
        fill_form(&mut app);
        for focus in 0..9 {
            if let Screen::Form(form) = &mut app.screen {
                form.focus = focus;
            }
            snapshot(&mut app, width, height);
        }
        key(&mut app, KeyCode::Esc);
        snapshot(&mut app, width, height);
    }
}

#[test]
fn setup_shortcut_needs_visible_confirmation_and_defaults_to_cancel() {
    let (_dir, mut app) = setup();
    add_profile(&mut app, "dev");
    key(&mut app, KeyCode::Char('s'));
    assert!(matches!(
        app.screen,
        Screen::Confirm {
            action: Confirmation::SetupKey(_),
            yes: false
        }
    ));
    key(&mut app, KeyCode::Tab);
    assert!(matches!(key(&mut app, KeyCode::Enter), Action::None));
    assert!(!app.store.directory.join("keys").exists());
    assert!(snapshot(&mut app, 120, 30).contains("authorized_keys"));
    key(&mut app, KeyCode::Tab);
    assert!(matches!(key(&mut app, KeyCode::Enter), Action::SetupKey(_)));
}

fn discovery_fixture(app: &App) -> std::path::PathBuf {
    std::fs::create_dir_all(&app.discovery.directory).unwrap();
    let path = app.discovery.directory.join("existing");
    std::fs::write(&path, "private contents not read").unwrap();
    std::fs::write(
        app.discovery.directory.join("existing.pub"),
        "ssh-ed25519 AAAAtest",
    )
    .unwrap();
    std::fs::write(
        app.discovery.directory.join("config"),
        "Host ubuntu\n HostName ::1\n User tester\n Port 2222\n IdentityFile ~/.ssh/existing\n",
    )
    .unwrap();
    dunce::canonicalize(path).unwrap()
}

#[test]
fn form_autofills_key_and_replaces_only_its_own_suggestion_on_target_change() {
    let (_temp, mut app) = setup();
    let path = discovery_fixture(&app);
    key(&mut app, KeyCode::Char('a'));
    fill_form(&mut app);
    let Screen::Form(form) = &mut app.screen else {
        panic!()
    };
    form.detect_identity(&app.discovery);
    assert_eq!(form.fields[4].input.value, path.display().to_string());
    form.fields[1].input.value = "other-host".into();
    form.detect_identity(&app.discovery);
    assert!(form.fields[4].input.value.is_empty());
    form.fields[4].input.value = "manually-chosen".into();
    form.fields[1].input.value = "::1".into();
    form.detect_identity(&app.discovery);
    assert_eq!(form.fields[4].input.value, "manually-chosen");
}

#[test]
fn key_picker_sets_the_path_without_typing_it() {
    let (_temp, mut app) = setup();
    let path = discovery_fixture(&app);
    key(&mut app, KeyCode::Char('a'));
    fill_form(&mut app);
    key(&mut app, KeyCode::F(2));
    assert!(snapshot(&mut app, 120, 30).contains("已有密钥"));
    key(&mut app, KeyCode::Enter);
    let Screen::Form(form) = &app.screen else {
        panic!()
    };
    assert_eq!(form.fields[4].input.value, path.display().to_string());
}

#[test]
fn first_connect_defaults_to_setup_but_requires_visible_consent() {
    let (_dir, mut app) = setup();
    add_profile(&mut app, "first");
    assert!(matches!(key(&mut app, KeyCode::Enter), Action::None));
    assert!(matches!(app.screen, Screen::FirstConnect { choice: 0, .. }));
    assert!(matches!(key(&mut app, KeyCode::Enter), Action::None));
    assert!(snapshot(&mut app, 120, 30).contains("仅本次"));
    assert!(matches!(
        key(&mut app, KeyCode::Enter),
        Action::SetupAndConnect(_)
    ));
}

#[test]
fn editing_authentication_invalidates_verified_state_but_renaming_does_not() {
    let (_dir, mut app) = setup();
    let mut profile = add_profile(&mut app, "verified");
    profile.key_login_configured = true;
    let mut form = Form::new(Some(profile.clone()), false);
    form.fields[0].input.value = "renamed".into();
    assert!(form.profile(&[]).unwrap().key_login_configured);
    form.fields[1].input.value = "different-host".into();
    assert!(!form.profile(&[]).unwrap().key_login_configured);
    let mut form = Form::new(Some(profile), false);
    form.fields[3].input.value = "2222".into();
    assert!(!form.profile(&[]).unwrap().key_login_configured);
}

#[test]
fn verified_connections_skip_the_first_connect_screen() {
    let (_dir, mut app) = setup();
    let mut profile = add_profile(&mut app, "verified");
    profile.key_login_configured = true;
    app.store
        .save(profile.clone(), Some(profile.revision))
        .unwrap();
    assert!(matches!(key(&mut app, KeyCode::Enter), Action::Connect(_)));
}

#[test]
fn remote_mode_is_explicit_and_ssh_sessions_select_it() {
    use crate::sync::worker::Command;
    let (_dir, mut app) = setup();
    app.remote_environment = true;
    let action = key(&mut app, KeyCode::Char('g'));
    if cfg!(target_os = "linux") {
        assert!(matches!(action, Action::None));
        assert!(matches!(app.screen, Screen::RemoteLogin { remember: true }));
    } else {
        assert!(matches!(
            action,
            Action::Sync(Command::LoginRemote { remember: true })
        ));
    }
    app.screen = Screen::RemoteLogin { remember: true };
    snapshot(&mut app, 38, 12);
    assert!(matches!(key(&mut app, KeyCode::Enter), Action::None));
    assert!(matches!(app.screen, Screen::RemoteLogin { .. }));
    let text = snapshot(&mut app, 60, 20);
    assert!(text.contains("仅本次运行"));
    assert!(text.contains("令牌不加密"));
    assert!(text.contains("root"));
    key(&mut app, KeyCode::Tab);
    assert!(matches!(
        key(&mut app, KeyCode::Enter),
        Action::Sync(Command::LoginRemote { remember: false })
    ));
    app.sync_status.authorizing = true;
    app.sync_status.remote_login = true;
    app.sync_status.callback_port = Some(41234);
    app.sync_status.auth_deadline =
        Some(std::time::Instant::now() + std::time::Duration::from_secs(600));
    let text = snapshot(&mut app, 120, 40);
    assert!(text.contains("远程登录模式"));
    assert!(text.contains("127.0.0.1:41234:127.0.0.1:41234"));
    assert!(matches!(key(&mut app, KeyCode::Esc), Action::CancelSync));
}

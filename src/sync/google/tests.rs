use super::*;
use std::{net::TcpStream, sync::Arc, thread};
use tiny_http::{Header, Response as Reply, Server};

fn mock() -> (Arc<Server>, Google) {
    let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
    let root = format!("http://{}", server.server_addr());
    let mut google = Google::new().unwrap();
    google.http = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    google.token = format!("{root}/token");
    google.drive = format!("{root}/files");
    google.upload = format!("{root}/upload");
    (server, google)
}
fn session() -> Session {
    Session {
        credential: Credential {
            account: "account".into(),
            email: "test@example.test".into(),
            refresh_token: "SECRET_REFRESH".into(),
            client_id: "test".into(),
        },
        access: "SECRET_ACCESS".into(),
        expires: Instant::now(),
    }
}

#[test]
fn full_desktop_oauth_uses_pkce_loopback_and_verified_account_without_browser() {
    use std::sync::Mutex;
    let (server, mut google) = mock();
    google.userinfo = format!("http://{}/userinfo", server.server_addr());
    let challenge = Arc::new(Mutex::new(String::new()));
    let observed = challenge.clone();
    let task = thread::spawn(move || {
        let mut request = next(&server);
        let mut body = String::new();
        request.as_reader().read_to_string(&mut body).unwrap();
        let parsed = reqwest::Url::parse(&format!("http://local/?{body}")).unwrap();
        let fields: std::collections::BTreeMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(fields["code"], "authorization-code");
        assert_eq!(fields["grant_type"], "authorization_code");
        assert_eq!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(fields["code_verifier"].as_bytes())),
            *observed.lock().unwrap()
        );
        assert!(fields["redirect_uri"].starts_with("http://127.0.0.1:"));
        request.respond(Reply::from_string(r#"{"access_token":"access","refresh_token":"refresh","expires_in":3600,"scope":"https://www.googleapis.com/auth/drive.appdata openid email"}"#)).unwrap();
        let request = next(&server);
        assert_eq!(request.url(), "/userinfo");
        request
            .respond(Reply::from_string(
                r#"{"sub":"verified-account","email":"test@example.test","email_verified":true}"#,
            ))
            .unwrap();
    });
    let session = google
        .authorize_using(
            &OAuthConfig {
                client_id: "test.apps.googleusercontent.com".into(),
                client_secret: "desktop-app-value".into(),
            },
            &AtomicBool::new(false),
            |_| {},
            |url| {
                let url = reqwest::Url::parse(url).unwrap();
                let fields: std::collections::BTreeMap<_, _> =
                    url.query_pairs().into_owned().collect();
                assert_eq!(fields["code_challenge_method"], "S256");
                *challenge.lock().unwrap() = fields["code_challenge"].clone();
                let mut redirect = reqwest::Url::parse(&fields["redirect_uri"]).unwrap();
                redirect
                    .query_pairs_mut()
                    .append_pair("state", &fields["state"])
                    .append_pair("code", "authorization-code");
                thread::spawn(move || {
                    Client::builder()
                        .no_proxy()
                        .build()
                        .unwrap()
                        .get(redirect)
                        .send()
                        .unwrap();
                });
            },
        )
        .unwrap();
    assert_eq!(session.credential.account, "verified-account");
    assert_eq!(session.credential.refresh_token, "refresh");
    task.join().unwrap();
}
fn next(server: &Server) -> tiny_http::Request {
    server
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .expect("expected request")
}

#[test]
fn refresh_uses_refresh_token_and_does_not_leak_error_response() {
    let (server, google) = mock();
    let task = thread::spawn(move || {
        let mut request = next(&server);
        assert_eq!(request.url(), "/token");
        let mut body = String::new();
        request.as_reader().read_to_string(&mut body).unwrap();
        assert!(body.contains("refresh_token=SECRET_REFRESH"));
        request
            .respond(Reply::from_string(
                r#"{"access_token":"new-access","expires_in":3600}"#,
            ))
            .unwrap();
        let request = next(&server);
        request
            .respond(Reply::from_string("SECRET_SERVER_RESPONSE").with_status_code(400))
            .unwrap();
    });
    let mut session = session();
    let config = OAuthConfig {
        client_id: "test".into(),
        client_secret: String::new(),
    };
    google.refresh(&mut session, &config).unwrap();
    assert_eq!(session.access, "new-access");
    session.expires = Instant::now();
    let error = google.refresh(&mut session, &config).unwrap_err();
    assert!(error.downcast_ref::<ServiceError>().unwrap().reauthorize);
    assert!(!error.to_string().contains("SECRET"));
    task.join().unwrap();
}

#[test]
fn drive_follows_pages_skips_seen_files_and_downloads_only_appdata() {
    let (server, google) = mock();
    let task = thread::spawn(move || {
        let request = next(&server);
        assert!(request.url().contains("appDataFolder"));
        assert!(request.url().contains("sshm_sync"));
        request
            .respond(Reply::from_string(
                r#"{"files":[{"id":"seen"}],"nextPageToken":"second"}"#,
            ))
            .unwrap();
        let request = next(&server);
        assert!(request.url().contains("pageToken=second"));
        request
            .respond(Reply::from_string(r#"{"files":[{"id":"fresh"}]}"#))
            .unwrap();
        let request = next(&server);
        assert!(request.url().starts_with("/files/fresh?"));
        request
            .respond(Reply::from_string(r#"{"version":1,"changes":[]}"#))
            .unwrap();
    });
    let (batches, ids) = google
        .download(&session(), &["seen".into()].into(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(batches.len(), 1);
    assert_eq!(ids, ["fresh"]);
    task.join().unwrap();
}

#[test]
fn rate_limits_honor_retry_after_and_do_not_include_response_secrets() {
    let (server, google) = mock();
    let task = thread::spawn(move || {
        next(&server)
            .respond(
                Reply::from_string("SECRET")
                    .with_status_code(429)
                    .with_header(Header::from_bytes("Retry-After", "120").unwrap()),
            )
            .unwrap();
    });
    let error = google
        .download(&session(), &Default::default(), &AtomicBool::new(false))
        .unwrap_err();
    let service = error.downcast_ref::<ServiceError>().unwrap();
    assert_eq!(service.retry_after, 120);
    assert_eq!(
        crate::sync::worker::retry_delay(1, service.retry_after),
        Duration::from_secs(120)
    );
    assert!(!error.to_string().contains("SECRET"));
    task.join().unwrap();
}

#[test]
fn upload_has_appdata_parent_and_no_secrets_in_content() {
    let (server, google) = mock();
    let task = thread::spawn(move || {
        let mut request = next(&server);
        assert!(request.url().starts_with("/upload?"));
        let mut body = String::new();
        request.as_reader().read_to_string(&mut body).unwrap();
        assert!(body.contains("appDataFolder"));
        assert!(body.contains("sshm_sync"));
        assert!(!body.contains("SECRET"));
        request
            .respond(Reply::from_string(r#"{"id":"uploaded"}"#))
            .unwrap();
    });
    assert_eq!(
        google
            .upload(
                &session(),
                &Batch {
                    version: 1,
                    changes: vec![]
                }
            )
            .unwrap(),
        "uploaded"
    );
    task.join().unwrap();
}

#[test]
fn local_push_never_lists_or_downloads_cloud_and_idle_push_has_no_requests() {
    use crate::{
        model::{Connection, Profile},
        store::Store,
        sync::worker::push_pending,
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store {
        directory: dir.path().join("data"),
    };
    store
        .save(
            Profile::new(
                "server".into(),
                Connection::Manual {
                    host: "host".into(),
                    user: "user".into(),
                    port: 22,
                    identity_file: None,
                },
            ),
            None,
        )
        .unwrap();
    store
        .sync_account("account", "test@example.test", false)
        .unwrap();
    let (server, google) = mock();
    let observer = server.clone();
    let task = thread::spawn(move || {
        let request = next(&server);
        assert_eq!(request.method(), &tiny_http::Method::Post);
        assert!(request.url().starts_with("/upload?"));
        request
            .respond(Reply::from_string(r#"{"id":"local-upload"}"#))
            .unwrap();
    });
    push_pending(&store, &google, &session(), &AtomicBool::new(false)).unwrap();
    task.join().unwrap();
    assert!(store.sync_capture().unwrap().pending.is_empty());
    push_pending(&store, &google, &session(), &AtomicBool::new(false)).unwrap();
    assert!(observer.try_recv().unwrap().is_none());
}

#[test]
fn invalid_json_and_pagination_cycles_fail_without_remote_writes() {
    let (server, google) = mock();
    let task = thread::spawn(move || {
        next(&server)
            .respond(Reply::from_string("{broken"))
            .unwrap();
        for _ in 0..2 {
            next(&server)
                .respond(Reply::from_string(r#"{"files":[],"nextPageToken":"loop"}"#))
                .unwrap();
        }
    });
    assert!(
        google
            .download(&session(), &Default::default(), &AtomicBool::new(false))
            .is_err()
    );
    assert!(
        google
            .download(&session(), &Default::default(), &AtomicBool::new(false))
            .unwrap_err()
            .to_string()
            .contains("分页循环")
    );
    task.join().unwrap();
}

#[test]
fn callback_rejects_wrong_state_and_duplicate_parameters() {
    assert!(
        parse_callback(
            "GET /oauth/callback?state=wrong&code=SECRET HTTP/1.1\r\n",
            "expected"
        )
        .is_err()
    );
    assert!(
        parse_callback(
            "GET /oauth/callback?state=expected&state=other&code=x HTTP/1.1\r\n",
            "expected"
        )
        .is_err()
    );
    assert!(
        parse_callback(
            "POST /oauth/callback?state=expected&code=x HTTP/1.1\r\n",
            "expected"
        )
        .is_err()
    );
    assert_eq!(
        parse_callback(
            "GET /oauth/callback?state=expected&error=access_denied HTTP/1.1\r\n",
            "expected"
        )
        .unwrap(),
        None
    );
}

#[test]
fn loopback_callback_ignores_invalid_request_then_accepts_valid_code() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let task = thread::spawn(move || {
        for state in ["wrong", "expected"] {
            let mut stream = TcpStream::connect(addr).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            write!(stream, "GET /oauth/callback?state={state}&code=accepted HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(response.contains(if state == "wrong" {
                "400 Bad Request"
            } else {
                "200 OK"
            }));
        }
    });
    assert_eq!(
        wait_callback(
            &listener,
            "expected",
            &AtomicBool::new(false),
            Duration::from_secs(5)
        )
        .unwrap(),
        "accepted"
    );
    task.join().unwrap();
}

#[test]
fn authorization_cancel_and_timeout_do_not_wait_for_a_browser() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    assert!(
        wait_callback(
            &listener,
            "state",
            &AtomicBool::new(true),
            Duration::from_secs(5)
        )
        .unwrap_err()
        .to_string()
        .contains("取消")
    );
    assert!(
        wait_callback(&listener, "state", &AtomicBool::new(false), Duration::ZERO)
            .unwrap_err()
            .to_string()
            .contains("超时")
    );
}

#[test]
fn uncertain_upload_keeps_durable_outbox_and_retry_deduplicates_events() {
    use crate::{
        model::{Connection, Profile},
        store::Store,
        sync::worker::synchronize,
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store {
        directory: dir.path().join("data"),
    };
    store
        .save(
            Profile::new(
                "server".into(),
                Connection::Manual {
                    host: "host".into(),
                    user: "user".into(),
                    port: 22,
                    identity_file: None,
                },
            ),
            None,
        )
        .unwrap();
    store
        .sync_account("account", "test@example.test", false)
        .unwrap();
    let original = store.sync_capture().unwrap().batch();
    let remote = serde_json::to_string(&original).unwrap();
    let (server, google) = mock();
    let task = thread::spawn(move || {
        next(&server)
            .respond(Reply::from_string(r#"{"files":[]}"#))
            .unwrap();
        // Server accepted bytes but the client cannot establish success.
        next(&server)
            .respond(Reply::from_string("uncertain").with_status_code(503))
            .unwrap();
        next(&server)
            .respond(Reply::from_string(r#"{"files":[{"id":"first-upload"}]}"#))
            .unwrap();
        next(&server).respond(Reply::from_string(remote)).unwrap();
        next(&server)
            .respond(Reply::from_string(r#"{"id":"retry-upload"}"#))
            .unwrap();
    });
    assert!(synchronize(&store, &google, &session(), &AtomicBool::new(false)).is_err());
    assert_eq!(
        store.sync_capture().unwrap().batch().changes,
        original.changes
    );
    synchronize(&store, &google, &session(), &AtomicBool::new(false)).unwrap();
    assert!(store.sync_capture().unwrap().pending.is_empty());
    assert_eq!(store.sync_capture().unwrap().events.len(), 1);
    task.join().unwrap();
}

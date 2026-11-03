use super::{
    google::{
        Credential, Google, OAuthConfig, ServiceError, Session, authorization_timeout,
        callback_port,
    },
    model::Batch,
};
use crate::store::Store;
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct Status {
    pub message: String,
    pub email: String,
    pub logged_in: bool,
    pub authorizing: bool,
    pub confirm_account: bool,
    pub url: Option<String>,
    pub session_only: bool,
    pub remote_login: bool,
    pub callback_port: Option<u16>,
    pub auth_deadline: Option<Instant>,
    pub file_credentials: bool,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            message: "登录 Google Drive 以同步".into(),
            email: String::new(),
            logged_in: false,
            authorizing: false,
            confirm_account: false,
            url: None,
            session_only: false,
            remote_login: false,
            callback_port: None,
            auth_deadline: None,
            file_credentials: false,
        }
    }
}

pub enum Command {
    Login,
    LoginRemote { remember: bool },
    Now,
    Logout,
    ConfirmAccount(bool),
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Work {
    Push,
    Pull,
}

#[derive(Default)]
struct Schedule {
    upload_due: Option<Instant>,
    pull_requested: bool,
    pending: std::collections::BTreeSet<Uuid>,
    failures: u32,
}

impl Schedule {
    fn observe(&mut self, pending: std::collections::BTreeSet<Uuid>, now: Instant) -> bool {
        if self.pending == pending {
            return false;
        }
        self.pending = pending;
        if self.pending.is_empty() {
            self.upload_due = None;
        } else if self.failures == 0 || self.upload_due.is_none() {
            self.failures = 0;
            self.upload_due = Some(now + Duration::from_secs(1));
        }
        true
    }

    fn request_pull(&mut self) {
        self.pull_requested = true;
    }

    fn take_work(&mut self, now: Instant) -> Option<Work> {
        if self.pull_requested {
            self.pull_requested = false;
            return Some(Work::Pull);
        }
        if self.upload_due.is_some_and(|due| due <= now) {
            self.upload_due = None;
            return Some(Work::Push);
        }
        None
    }

    fn complete(&mut self, pending: std::collections::BTreeSet<Uuid>, now: Instant) {
        self.failures = 0;
        self.pending = pending;
        self.upload_due = (!self.pending.is_empty()).then_some(now + Duration::from_secs(1));
    }

    fn failed(&mut self, server_seconds: u64, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        // Only retry actual pending uploads. A failed manual pull never
        // starts a background cloud polling loop.
        self.upload_due =
            (!self.pending.is_empty()).then_some(now + retry_delay(self.failures, server_seconds));
    }
}

pub struct Worker {
    sender: Sender<Command>,
    pub events: Receiver<Status>,
    cancel: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
}
impl Worker {
    pub fn start(store: Store) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (notify, events) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let (cancel_thread, stopped_thread) = (cancel.clone(), stopped.clone());
        thread::spawn(move || {
            if let Err(error) = run(store, receiver, &notify, &cancel_thread, &stopped_thread) {
                let _ = notify.send(Status {
                    message: format!("同步不可用：{error}"),
                    ..Default::default()
                });
            }
        });
        Self {
            sender,
            events,
            cancel,
            stopped,
        }
    }
    pub fn send(&self, command: Command) {
        if matches!(
            command,
            Command::Login | Command::LoginRemote { .. } | Command::Now
        ) {
            self.cancel.store(false, Ordering::Relaxed);
        }
        if matches!(command, Command::Logout | Command::Stop) {
            self.cancel.store(true, Ordering::Relaxed);
        }
        let _ = self.sender.send(command);
    }
    pub fn cancel_login(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.sender.send(Command::Stop);
    }
}

fn credential_entry(store: &Store) -> Result<keyring::Entry> {
    let path = dunce::canonicalize(&store.directory)?;
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, path.to_string_lossy().as_bytes());
    keyring::Entry::new("sshm.google-drive", &id.to_string()).context("无法访问系统凭据存储")
}

fn save_credential(store: &Store, credential: &Credential) -> Result<()> {
    credential_entry(store)?
        .set_password(&serde_json::to_string(credential)?)
        .map_err(|_| anyhow::anyhow!("无法保存系统凭据"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Storage {
    System,
    Session,
    #[cfg(target_os = "linux")]
    File,
}

fn remote_storage(remember: bool) -> Storage {
    #[cfg(target_os = "linux")]
    {
        if remember {
            Storage::File
        } else {
            Storage::Session
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = remember;
        Storage::System
    }
}

fn save_selected(store: &Store, credential: &Credential, storage: Storage) -> Result<()> {
    match storage {
        Storage::System => {
            save_credential(store, credential)?;
            #[cfg(target_os = "linux")]
            super::credentials::remove(store)?;
            Ok(())
        }
        Storage::Session => anyhow::bail!("本次新授权未保存"),
        #[cfg(target_os = "linux")]
        Storage::File => super::credentials::save(store, credential),
    }
}

fn restore_selected(store: &Store, account: Option<&str>) -> Result<Option<(Credential, Storage)>> {
    #[cfg(target_os = "linux")]
    if let Some(credential) = super::credentials::load(store)?
        && Some(credential.account.as_str()) == account
    {
        return Ok(Some((credential, Storage::File)));
    }
    Ok(credential_entry(store)
        .ok()
        .and_then(|entry| entry.get_password().ok())
        .and_then(|text| serde_json::from_str::<Credential>(&text).ok())
        .filter(|credential| Some(credential.account.as_str()) == account)
        .map(|credential| (credential, Storage::System)))
}

fn remove_credentials(store: &Store, storage: Storage) -> Result<()> {
    #[cfg(target_os = "linux")]
    let local = super::credentials::remove(store);
    let system = credential_entry(store).and_then(|entry| match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => anyhow::bail!("系统凭据未能删除，请在系统凭据管理器移除 sshm.google-drive"),
    });
    #[cfg(target_os = "linux")]
    local?;
    if storage == Storage::System {
        system
    } else {
        Ok(())
    }
}

fn install_session(
    store: &Store,
    session: &Session,
    confirmed: bool,
    status: &mut Status,
    storage: Storage,
) -> Result<()> {
    let mut reason = None;
    install_with(store, session, confirmed, status, |credential| {
        let result = if storage == Storage::Session {
            remove_credentials(store, storage)
                .and_then(|()| save_selected(store, credential, storage))
        } else {
            save_selected(store, credential, storage)
        };
        if let Err(error) = &result {
            reason = Some(error.to_string());
        }
        result
    })?;
    status.file_credentials = false;
    #[cfg(target_os = "linux")]
    if storage == Storage::File && !status.session_only {
        status.file_credentials = true;
    }
    if status.session_only && storage != Storage::System {
        status.message = format!(
            "已登录；{}。按 p 拉取云端",
            reason.unwrap_or_else(|| "本次新授权未保存".into())
        );
    } else if status.file_credentials {
        status.message = "已记住远程登录；按 p 拉取云端，令牌仅保存在本机权限受限文件".into();
    }
    Ok(())
}

fn install_with(
    store: &Store,
    session: &Session,
    confirmed: bool,
    status: &mut Status,
    persist: impl FnOnce(&Credential) -> Result<()>,
) -> Result<()> {
    store.sync_account(
        &session.credential.account,
        &session.credential.email,
        confirmed,
    )?;
    status.session_only = persist(&session.credential).is_err();
    status.logged_in = true;
    status.email = session.credential.email.clone();
    status.confirm_account = false;
    status.message = if status.session_only {
        "仅本次登录：系统凭据存储不可用，下次需重新登录"
    } else {
        "已登录；本机修改自动推送，按 p 拉取云端"
    }
    .into();
    Ok(())
}

fn lock(store: &Store) -> Result<File> {
    if !store.directory.exists() {
        std::fs::create_dir_all(&store.directory)?;
        crate::permissions::owner_only(&store.directory, true)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(store.directory.join("sync.lock"))?;
    file.try_lock_exclusive()
        .context("另一个 sshm 窗口正在负责同步")?;
    Ok(file)
}

fn run(
    store: Store,
    commands: Receiver<Command>,
    notify: &Sender<Status>,
    cancel: &AtomicBool,
    stopped: &AtomicBool,
) -> Result<()> {
    let google = Google::new()?;
    let mut status = Status::default();
    let mut session: Option<Session> = None;
    let mut candidate: Option<(Session, Storage)> = None;
    let mut active_storage = Storage::System;
    let mut lease = None;
    let mut restored = false;
    let mut next_check = Instant::now();
    let mut schedule = Schedule::default();
    let mut observed = crate::store::CHANGES.load(Ordering::Relaxed);
    let mut local_valid = true;
    loop {
        if stopped.load(Ordering::Relaxed) {
            break;
        }
        let command = match commands.recv_timeout(Duration::from_millis(100)) {
            Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(c) => Some(c),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
        };
        if lease.is_none() {
            if command.is_none() && Instant::now() < next_check {
                continue;
            }
            let enabled = store.read_data().map(|d| d.sync.enabled).unwrap_or(false);
            next_check = Instant::now() + Duration::from_secs(1);
            if command.is_none() && !enabled {
                continue;
            }
            match lock(&store) {
                Ok(file) => {
                    lease = Some(file);
                    restored = false;
                }
                Err(_) => {
                    status.message = "另一个 sshm 窗口正在负责同步；请在该窗口管理登录".into();
                    if let Ok(data) = store.read_data() {
                        status.logged_in = data.sync.enabled;
                        status.email = data.sync.email;
                    }
                    let _ = notify.send(status.clone());
                    next_check = Instant::now() + Duration::from_secs(1);
                    continue;
                }
            }
        }
        if !restored {
            restored = true;
            if let Ok(data) = store.read_data()
                && data.sync.enabled
            {
                match restore_selected(&store, data.sync.account.as_deref()) {
                    Ok(Some((credential, storage))) => {
                        status.email = credential.email.clone();
                        status.logged_in = true;
                        active_storage = storage;
                        #[cfg(target_os = "linux")]
                        {
                            status.file_credentials = storage == Storage::File;
                        }
                        session = Some(google.restore(credential));
                        status.message = "已恢复登录；本机修改自动推送，按 p 拉取云端".into();
                    }
                    Ok(None) => status.message = "需要重新登录 Google Drive".into(),
                    Err(error) => {
                        status.message = format!("本地凭据不可用：{error}；按 r 重新远程登录")
                    }
                }
                let _ = notify.send(status.clone());
            }
        }
        if let Some(command) = command {
            let result: Result<()> = match command {
                Command::Login | Command::LoginRemote { .. } => {
                    let remote = matches!(command, Command::LoginRemote { .. });
                    let storage = match command {
                        Command::LoginRemote { remember } => remote_storage(remember),
                        _ => Storage::System,
                    };
                    status.remote_login = remote;
                    status.callback_port = None;
                    status.auth_deadline = Some(Instant::now() + authorization_timeout(remote));
                    status.authorizing = true;
                    status.url = None;
                    status.confirm_account = false;
                    status.message = if remote {
                        "远程登录：先在浏览器所在电脑建立 SSH 转发，再打开授权链接；Esc 取消"
                    } else {
                        "正在等待本机浏览器授权；Esc 取消"
                    }
                    .into();
                    let _ = notify.send(status.clone());
                    let result = OAuthConfig::load().and_then(|config| {
                        let show = |url: &str| {
                            let mut shown = status.clone();
                            shown.url = Some(url.into());
                            shown.callback_port = callback_port(url);
                            let _ = notify.send(shown);
                        };
                        if remote {
                            google.authorize_remote(&config, cancel, show)
                        } else {
                            google.authorize(&config, cancel, show)
                        }
                    });
                    status.authorizing = false;
                    status.url = None;
                    status.callback_port = None;
                    status.auth_deadline = None;
                    match result {
                        Ok(new_session) => {
                            let old = store.read_data()?.sync.account;
                            if old
                                .as_ref()
                                .is_some_and(|a| a != &new_session.credential.account)
                            {
                                status.confirm_account = true;
                                status.message = format!(
                                    "切换到 {}？确认后会将现有本机连接合并到此账号",
                                    new_session.credential.email
                                );
                                candidate = Some((new_session, storage));
                                Ok(())
                            } else {
                                install_session(&store, &new_session, false, &mut status, storage)
                                    .map(|()| {
                                        session = Some(new_session);
                                        active_storage = storage;
                                        schedule = Schedule::default();
                                    })
                            }
                        }
                        Err(e) => Err(e),
                    }
                }
                Command::ConfirmAccount(yes) => {
                    status.confirm_account = false;
                    if let Some((new_session, storage)) = candidate.take().filter(|_| yes) {
                        install_session(&store, &new_session, true, &mut status, storage).map(
                            |()| {
                                session = Some(new_session);
                                active_storage = storage;
                                schedule = Schedule::default();
                            },
                        )
                    } else {
                        status.message = "已取消切换账号".into();
                        Ok(())
                    }
                }
                Command::Logout => {
                    schedule = Schedule::default();
                    candidate = None;
                    session = None;
                    store.update_data(|data| {
                        data.sync.enabled = false;
                        Ok(())
                    })?;
                    status = Status::default();
                    let deleted = remove_credentials(&store, active_storage);
                    active_storage = Storage::System;
                    cancel.store(false, Ordering::Relaxed);
                    deleted
                }
                Command::Now => {
                    cancel.store(false, Ordering::Relaxed);
                    if session.is_some() {
                        schedule.request_pull();
                    } else {
                        status.message = "请先登录 Google Drive，再按 p 拉取云端".into();
                    }
                    Ok(())
                }
                Command::Stop => break,
            };
            if let Err(e) = result {
                status.message = e.to_string();
            }
            let _ = notify.send(status.clone());
        }
        if candidate.is_some() {
            continue;
        }
        if session.is_none() && store.read_data().is_ok_and(|d| !d.sync.enabled) {
            lease = None;
            continue;
        }
        let change_counter = crate::store::CHANGES.load(Ordering::Relaxed);
        if session.is_some() && (Instant::now() >= next_check || observed != change_counter) {
            observed = change_counter;
            next_check = Instant::now() + Duration::from_secs(1);
            match store.sync_capture() {
                Ok(state) => {
                    local_valid = true;
                    if !state.enabled {
                        session = None;
                        status = Status::default();
                        let _ = notify.send(status.clone());
                        continue;
                    }
                    if schedule.observe(state.pending, Instant::now())
                        && !schedule.pending.is_empty()
                        && schedule.failures == 0
                    {
                        status.message = "本机修改待推送".into();
                        let _ = notify.send(status.clone());
                    }
                }
                Err(e) => {
                    local_valid = false;
                    status.message = format!("同步已暂停：{e}");
                    let _ = notify.send(status.clone());
                }
            }
        }
        if !local_valid {
            continue;
        }
        let Some(active) = session.as_mut() else {
            continue;
        };
        let Some(work) = schedule.take_work(Instant::now()) else {
            continue;
        };
        cancel.store(false, Ordering::Relaxed);
        status.message = match work {
            Work::Push => "正在推送本机修改",
            Work::Pull => "正在拉取并合并云端",
        }
        .into();
        let _ = notify.send(status.clone());
        let result = OAuthConfig::load().and_then(|config| {
            google.refresh(active, &config)?;
            if !status.session_only
                && save_selected(&store, &active.credential, active_storage).is_err()
            {
                status.session_only = true;
            }
            match work {
                Work::Push => push_pending(&store, &google, active, cancel),
                Work::Pull => synchronize(&store, &google, active, cancel),
            }
        });
        if stopped.load(Ordering::Relaxed) {
            break;
        }
        match result {
            Ok(()) => {
                schedule.complete(store.read_data()?.sync.pending, Instant::now());
                let conflicts = store.load()?.iter().filter(|p| p.sync_conflict).count();
                status.message = if conflicts > 0 {
                    format!("已同步，有 {conflicts} 个冲突版本待处理")
                } else {
                    match work {
                        Work::Push => "本机修改已推送；按 p 拉取云端",
                        Work::Pull => "已拉取并合并；本机修改自动推送",
                    }
                    .into()
                };
            }
            Err(error) => {
                let service = error.downcast_ref::<ServiceError>();
                schedule.failed(service.map_or(0, |e| e.retry_after), Instant::now());
                status.message = match work {
                    Work::Push => format!("本机修改待推送：{error}"),
                    Work::Pull => format!("拉取失败，按 p 重试：{error}"),
                };
                if service.is_some_and(|e| e.reauthorize) {
                    status.logged_in = false;
                    session = None;
                    status.message = "授权已失效，请按 g 重新登录".into();
                }
            }
        }
        let _ = notify.send(status.clone());
    }
    drop(lease);
    Ok(())
}

pub fn retry_delay(failures: u32, server_seconds: u64) -> Duration {
    Duration::from_secs(
        (5u64.saturating_mul(1u64 << failures.min(6)))
            .min(300)
            .max(server_seconds.min(86400)),
    )
}

pub fn synchronize(
    store: &Store,
    google: &Google,
    session: &Session,
    cancel: &AtomicBool,
) -> Result<()> {
    let state = store.sync_capture()?;
    ensure!(
        state.account.as_deref() == Some(&session.credential.account) && state.enabled,
        "同步账号不匹配"
    );
    let (batches, files) = google.download(session, &state.seen_files, cancel)?;
    ensure!(!cancel.load(Ordering::Relaxed), "同步已取消");
    store.sync_merge(&session.credential.account, &batches, &files, &[])?;
    push_pending(store, google, session, cancel)
}

/// Upload immutable local changes without listing or downloading cloud data.
pub fn push_pending(
    store: &Store,
    google: &Google,
    session: &Session,
    cancel: &AtomicBool,
) -> Result<()> {
    let state = store.sync_capture()?;
    ensure!(
        state.account.as_deref() == Some(&session.credential.account) && state.enabled,
        "同步账号不匹配"
    );
    let batch = state.batch();
    // Bound each request; successful chunks are acknowledged independently.
    for changes in batch.changes.chunks(32) {
        ensure!(!cancel.load(Ordering::Relaxed), "同步已取消");
        let batch = Batch {
            version: 1,
            changes: changes.to_vec(),
        };
        let file = google.upload(session, &batch)?;
        ensure!(!cancel.load(Ordering::Relaxed), "同步已取消");
        store.sync_merge(
            &session.credential.account,
            &[],
            &[file],
            &changes.iter().map(|c| c.id).collect::<Vec<_>>(),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_and_successful_pull_never_schedule_periodic_requests() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        assert_eq!(schedule.take_work(now + Duration::from_secs(86400)), None);
        schedule.request_pull();
        assert_eq!(schedule.take_work(now), Some(Work::Pull));
        schedule.complete(Default::default(), now);
        assert_eq!(schedule.take_work(now + Duration::from_secs(86400)), None);
    }

    #[test]
    fn only_pending_changes_trigger_debounced_push_and_retry() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        let mut pending = std::collections::BTreeSet::from([Uuid::new_v4()]);
        schedule.observe(pending.clone(), now);
        assert_eq!(schedule.take_work(now), None);
        pending.insert(Uuid::new_v4());
        schedule.observe(pending, now + Duration::from_millis(500));
        assert_eq!(schedule.take_work(now + Duration::from_secs(1)), None);
        assert_eq!(
            schedule.take_work(now + Duration::from_millis(1500)),
            Some(Work::Push)
        );
        schedule.failed(120, now);
        assert_eq!(schedule.take_work(now + Duration::from_secs(119)), None);
        assert_eq!(
            schedule.take_work(now + Duration::from_secs(120)),
            Some(Work::Push)
        );
        schedule.complete(Default::default(), now);
        assert_eq!(schedule.take_work(now + Duration::from_secs(86400)), None);
    }

    #[test]
    fn failed_manual_pull_is_not_automatically_repeated() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        schedule.request_pull();
        assert_eq!(schedule.take_work(now), Some(Work::Pull));
        schedule.failed(0, now);
        assert_eq!(schedule.take_work(now + Duration::from_secs(86400)), None);
        schedule.request_pull();
        assert_eq!(schedule.take_work(now), Some(Work::Pull));
        schedule.failed(0, now);
        schedule.observe([Uuid::new_v4()].into(), now);
        assert_eq!(
            schedule.take_work(now + Duration::from_secs(1)),
            Some(Work::Push)
        );
    }

    #[test]
    fn unavailable_credential_store_is_session_only_and_tokens_never_reach_json() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            directory: dir.path().join("data"),
        };
        let session = Google::new().unwrap().restore(Credential {
            account: "account".into(),
            email: "email@test".into(),
            refresh_token: "PRIVATE_TOKEN".into(),
            client_id: "test".into(),
        });
        let mut status = Status::default();
        install_with(&store, &session, false, &mut status, |_| {
            anyhow::bail!("locked keystore")
        })
        .unwrap();
        assert!(status.logged_in && status.session_only);
        assert!(
            !std::fs::read_to_string(store.path())
                .unwrap()
                .contains("PRIVATE_TOKEN")
        );
        assert!(store.read_data().unwrap().sync.enabled);
    }

    #[test]
    fn unconfirmed_account_switch_does_not_change_credentials_or_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            directory: dir.path().join("data"),
        };
        store.sync_account("old", "old@test", false).unwrap();
        let session = Google::new().unwrap().restore(Credential {
            account: "new".into(),
            email: "new@test".into(),
            refresh_token: "PRIVATE_TOKEN".into(),
            client_id: "test".into(),
        });
        let before = std::fs::read(store.path()).unwrap();
        assert!(
            install_with(&store, &session, false, &mut Status::default(), |_| panic!(
                "must not persist unconfirmed account"
            ))
            .is_err()
        );
        assert_eq!(std::fs::read(store.path()).unwrap(), before);
    }

    #[test]
    fn sync_lease_is_exclusive_and_available_after_owner_exits() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            directory: dir.path().join("data"),
        };
        let first = lock(&store).unwrap();
        assert!(lock(&store).is_err());
        drop(first);
        assert!(lock(&store).is_ok());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod remote_storage_tests {
    use super::*;
    #[test]
    fn file_login_restores_without_keyring_and_session_only_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            directory: dir.path().to_path_buf(),
        };
        let credential = Credential {
            account: "remote-test".into(),
            email: "remote@example.test".into(),
            refresh_token: "fake-refresh".into(),
            client_id: "fake-client".into(),
        };
        let session = Google::new().unwrap().restore(credential);
        let mut status = Status::default();
        install_session(&store, &session, false, &mut status, Storage::File).unwrap();
        assert!(status.logged_in && status.file_credentials && !status.session_only);
        // A fresh worker restores the saved login before any OAuth configuration or API request.
        let worker = Worker::start(Store {
            directory: store.directory.clone(),
        });
        let restored = worker.events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(restored.logged_in && restored.file_credentials && !restored.session_only);
        drop(worker);
        // Wait for the worker's lease to be released before proceeding.
        for _ in 0..100 {
            if let Ok(lease) = lock(&store) {
                drop(lease);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (saved, backend) = restore_selected(&store, Some("remote-test"))
            .unwrap()
            .unwrap();
        assert!(backend == Storage::File);
        assert_eq!(saved.refresh_token, "fake-refresh");
        assert!(
            !std::fs::read_to_string(store.path())
                .unwrap()
                .contains("fake-refresh")
        );
        install_session(&store, &session, false, &mut status, Storage::Session).unwrap();
        assert!(status.session_only && !status.file_credentials);
        assert!(super::super::credentials::load(&store).unwrap().is_none());
        install_session(&store, &session, false, &mut status, Storage::File).unwrap();
        remove_credentials(&store, Storage::File).unwrap();
        assert!(super::super::credentials::load(&store).unwrap().is_none());
    }
}

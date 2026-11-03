use super::model::Batch;
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use uuid::Uuid;

const LIMIT: u64 = 4 * 1024 * 1024;
const SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata openid email";

#[derive(Clone, Deserialize)]
pub struct OAuthConfig {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
}

impl OAuthConfig {
    pub fn load() -> Result<Self> {
        #[derive(Deserialize)]
        struct File {
            installed: OAuthConfig,
        }
        let config = if let Some(path) = std::env::var_os("SSHM_GOOGLE_OAUTH_CONFIG") {
            let bytes = fs::read(path).context("无法读取 Google 桌面客户端配置文件")?;
            ensure!(bytes.len() < 64 * 1024, "Google 客户端配置文件过大");
            serde_json::from_slice::<File>(&bytes)
                .context("请使用 Google 下载的桌面应用客户端 JSON")?
                .installed
        } else {
            Self {
                client_id: option_env!("SSHM_GOOGLE_CLIENT_ID")
                    .unwrap_or_default()
                    .into(),
                client_secret: option_env!("SSHM_GOOGLE_CLIENT_SECRET")
                    .unwrap_or_default()
                    .into(),
            }
        };
        ensure!(
            !config.client_id.is_empty(),
            "此版本尚未配置 Google 登录。开发者需设置 SSHM_GOOGLE_OAUTH_CONFIG，参见 Google Drive 接入说明"
        );
        ensure!(
            config.client_id.ends_with(".apps.googleusercontent.com"),
            "Google 客户端 ID 格式无效"
        );
        Ok(config)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Credential {
    pub account: String,
    pub email: String,
    pub refresh_token: String,
    pub client_id: String,
}

pub struct Session {
    pub credential: Credential,
    access: String,
    expires: Instant,
}

#[derive(Debug)]
pub struct ServiceError {
    pub status: u16,
    pub retry_after: u64,
    pub reauthorize: bool,
}
impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.reauthorize {
            write!(f, "Google 授权已失效，请重新登录")
        } else {
            write!(f, "Google 服务请求失败（HTTP {}）", self.status)
        }
    }
}
impl std::error::Error for ServiceError {}

pub struct Google {
    http: Client,
    auth: String,
    token: String,
    userinfo: String,
    drive: String,
    upload: String,
}

impl Google {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: Client::builder()
                .timeout(Duration::from_secs(15))
                .connect_timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("sshm/", env!("CARGO_PKG_VERSION")))
                .build()?,
            auth: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            userinfo: "https://openidconnect.googleapis.com/v1/userinfo".into(),
            drive: "https://www.googleapis.com/drive/v3/files".into(),
            upload: "https://www.googleapis.com/upload/drive/v3/files".into(),
        })
    }

    pub fn authorize(
        &self,
        config: &OAuthConfig,
        cancel: &AtomicBool,
        show: impl Fn(&str),
    ) -> Result<Session> {
        self.authorize_using(config, cancel, show, |url| {
            let _ = webbrowser::open(url);
        })
    }

    fn authorize_using(
        &self,
        config: &OAuthConfig,
        cancel: &AtomicBool,
        show: impl Fn(&str),
        open: impl Fn(&str),
    ) -> Result<Session> {
        self.authorize_using_mode(config, cancel, false, show, open)
    }

    /// Remote mode keeps the listener on loopback and never opens a server browser.
    pub fn authorize_remote(
        &self,
        config: &OAuthConfig,
        cancel: &AtomicBool,
        show: impl Fn(&str),
    ) -> Result<Session> {
        self.authorize_using_mode(config, cancel, true, show, |_| {})
    }

    fn authorize_using_mode(
        &self,
        config: &OAuthConfig,
        cancel: &AtomicBool,
        remote: bool,
        show: impl Fn(&str),
        open: impl Fn(&str),
    ) -> Result<Session> {
        ensure!(!cancel.load(Ordering::Relaxed), "已取消 Google 登录");
        let listener = TcpListener::bind(("127.0.0.1", 0)).context("无法启动本机授权回调")?;
        listener.set_nonblocking(true)?;
        let redirect = format!(
            "http://127.0.0.1:{}/oauth/callback",
            listener.local_addr()?.port()
        );
        let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = Uuid::new_v4().to_string();
        let mut url = reqwest::Url::parse(&self.auth)?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", config.client_id.as_str()),
            ("redirect_uri", &redirect),
            ("response_type", "code"),
            ("scope", SCOPE),
            ("access_type", "offline"),
            ("prompt", "consent select_account"),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ]);
        show(url.as_str());
        if !remote {
            open(url.as_str());
        }
        let code = wait_callback(&listener, &state, cancel, authorization_timeout(remote))?;
        let token: Token = self.token_request(
            config,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &redirect),
                ("code_verifier", &verifier),
            ],
        )?;
        if let Some(scope) = &token.scope {
            ensure!(
                scope
                    .split_whitespace()
                    .any(|s| s == "https://www.googleapis.com/auth/drive.appdata"),
                "未授予配置同步权限，请重新登录并勾选权限"
            );
        }
        #[derive(Deserialize)]
        struct User {
            sub: String,
            email: String,
            #[serde(default)]
            email_verified: bool,
        }
        let user: User = decode(
            self.http
                .get(&self.userinfo)
                .bearer_auth(&token.access_token)
                .send()
                .map_err(|e| e.without_url())?,
        )?;
        ensure!(
            !user.sub.is_empty() && user.email_verified,
            "无法确认 Google 账号"
        );
        ensure!(!cancel.load(Ordering::Relaxed), "已取消 Google 登录");
        Ok(Session {
            credential: Credential {
                account: user.sub,
                email: user.email,
                refresh_token: token
                    .refresh_token
                    .context("Google 未返回离线授权，请重新登录")?,
                client_id: config.client_id.clone(),
            },
            access: token.access_token,
            expires: expiry(token.expires_in),
        })
    }

    fn token_request(&self, config: &OAuthConfig, fields: &[(&str, &str)]) -> Result<Token> {
        let mut fields = fields.to_vec();
        fields.push(("client_id", &config.client_id));
        if !config.client_secret.is_empty() {
            fields.push(("client_secret", &config.client_secret));
        }
        let response = self
            .http
            .post(&self.token)
            .form(&fields)
            .send()
            .map_err(|e| e.without_url())?;
        if response.status().is_client_error() && response.status().as_u16() != 429 {
            return Err(ServiceError {
                status: response.status().as_u16(),
                retry_after: 0,
                reauthorize: true,
            }
            .into());
        }
        decode(response)
    }

    pub fn restore(&self, credential: Credential) -> Session {
        Session {
            credential,
            access: String::new(),
            expires: Instant::now(),
        }
    }

    pub fn refresh(&self, session: &mut Session, config: &OAuthConfig) -> Result<()> {
        ensure!(
            config.client_id == session.credential.client_id,
            "Google 客户端配置已改变，请重新登录"
        );
        if session.expires > Instant::now() {
            return Ok(());
        }
        let token = self.token_request(
            config,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &session.credential.refresh_token),
            ],
        )?;
        session.access = token.access_token;
        session.expires = expiry(token.expires_in);
        if let Some(refresh) = token.refresh_token {
            session.credential.refresh_token = refresh;
        }
        Ok(())
    }

    pub fn download(
        &self,
        session: &Session,
        seen: &std::collections::BTreeSet<String>,
        cancel: &AtomicBool,
    ) -> Result<(Vec<Batch>, Vec<String>)> {
        #[derive(Deserialize)]
        struct File {
            id: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct List {
            #[serde(default)]
            files: Vec<File>,
            next_page_token: Option<String>,
        }
        let mut page = String::new();
        let mut batches = vec![];
        let mut ids = vec![];
        let mut pages = std::collections::BTreeSet::new();
        let mut total = 0;
        loop {
            ensure!(!cancel.load(Ordering::Relaxed), "同步已取消");
            let mut request = self
                .http
                .get(&self.drive)
                .bearer_auth(&session.access)
                .query(&[
                    ("spaces", "appDataFolder"),
                    ("fields", "nextPageToken,files(id)"),
                    ("pageSize", "1000"),
                    ("q", "appProperties has { key='sshm_sync' and value='1' }"),
                ]);
            if !page.is_empty() {
                request = request.query(&[("pageToken", &page)]);
            }
            let list: List = decode(request.send().map_err(|e| e.without_url())?)?;
            total += list.files.len();
            ensure!(total <= 100_000, "云端历史文件过多，已停止同步");
            for file in list.files {
                if seen.contains(&file.id) || ids.contains(&file.id) {
                    continue;
                }
                ensure!(!cancel.load(Ordering::Relaxed), "同步已取消");
                let batch: Batch = decode(
                    self.http
                        .get(format!("{}/{}", self.drive, encode_id(&file.id)?))
                        .bearer_auth(&session.access)
                        .query(&[("alt", "media")])
                        .send()
                        .map_err(|e| e.without_url())?,
                )?;
                batches.push(batch);
                ids.push(file.id);
                ensure!(
                    batches.iter().map(|b| b.changes.len()).sum::<usize>() <= 100_000,
                    "云端历史过大"
                );
            }
            match list.next_page_token {
                Some(next) if !next.is_empty() => {
                    ensure!(pages.insert(next.clone()), "云端分页循环");
                    page = next;
                }
                _ => break,
            }
        }
        Ok((batches, ids))
    }

    pub fn upload(&self, session: &Session, batch: &Batch) -> Result<String> {
        let bytes = serde_json::to_vec(batch)?;
        ensure!(bytes.len() as u64 <= LIMIT, "同步批次超过 4 MiB");
        let name = format!(
            "sshm-{}.json",
            URL_SAFE_NO_PAD.encode(Sha256::digest(&bytes))
        );
        let metadata = serde_json::json!({"name": name, "parents": ["appDataFolder"], "appProperties": {"sshm_sync": "1"}});
        let boundary = format!("sshm{}", Uuid::new_v4().simple());
        let mut body = format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: application/json\r\n\r\n").into_bytes();
        body.extend(bytes);
        body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
        #[derive(Deserialize)]
        struct Created {
            id: String,
        }
        let created: Created = decode(
            self.http
                .post(&self.upload)
                .bearer_auth(&session.access)
                .query(&[("uploadType", "multipart"), ("fields", "id")])
                .header(
                    "Content-Type",
                    format!("multipart/related; boundary={boundary}"),
                )
                .body(body)
                .send()
                .map_err(|e| e.without_url())?,
        )?;
        encode_id(&created.id)?;
        Ok(created.id)
    }
}

fn encode_id(id: &str) -> Result<&str> {
    ensure!(
        !id.is_empty()
            && id.len() <= 256
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c)),
        "Google 文件 ID 无效"
    );
    Ok(id)
}

#[derive(Deserialize)]
struct Token {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
    scope: Option<String>,
}
fn expiry(seconds: u64) -> Instant {
    Instant::now() + Duration::from_secs(seconds.clamp(1, 86400).saturating_sub(30))
}

fn decode<T: DeserializeOwned>(response: Response) -> Result<T> {
    let status = response.status();
    if !status.is_success() {
        let retry_after = response
            .headers()
            .get("Retry-After")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        return Err(ServiceError {
            status: status.as_u16(),
            retry_after,
            reauthorize: status.as_u16() == 401,
        }
        .into());
    }
    let mut bytes = vec![];
    response
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .context("读取 Google 响应失败")?;
    ensure!(bytes.len() as u64 <= LIMIT, "Google 响应超过大小限制");
    serde_json::from_slice(&bytes).context("Google 返回的数据格式无效")
}

pub fn authorization_timeout(remote: bool) -> Duration {
    Duration::from_secs(if remote { 600 } else { 180 })
}

pub fn callback_port(authorization_url: &str) -> Option<u16> {
    let url = reqwest::Url::parse(authorization_url).ok()?;
    let (_, redirect) = url.query_pairs().find(|(name, _)| name == "redirect_uri")?;
    let redirect = reqwest::Url::parse(&redirect).ok()?;
    (redirect.host_str() == Some("127.0.0.1") && redirect.path() == "/oauth/callback")
        .then(|| redirect.port())
        .flatten()
}

fn wait_callback(
    listener: &TcpListener,
    state: &str,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        ensure!(!cancel.load(Ordering::Relaxed), "已取消 Google 登录");
        match listener.accept() {
            Ok((mut stream, address)) => {
                if !address.ip().is_loopback() {
                    continue;
                }
                stream.set_read_timeout(Some(Duration::from_millis(400)))?;
                stream.set_write_timeout(Some(Duration::from_millis(400)))?;
                let mut request = vec![];
                let mut byte = [0u8; 1];
                let read_deadline = Instant::now() + Duration::from_secs(2);
                while request.len() < 8192
                    && !request.ends_with(b"\r\n\r\n")
                    && Instant::now() < read_deadline
                    && !cancel.load(Ordering::Relaxed)
                {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                let request = String::from_utf8_lossy(&request);
                let parsed = parse_callback(&request, state);
                let (status, text) = if parsed.is_ok() {
                    ("200 OK", "Authorization received. Return to sshm.")
                } else {
                    ("400 Bad Request", "Invalid authorization callback.")
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
                if let Ok(code) = parsed {
                    return code.context("用户取消了 Google 授权");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(e.into()),
        }
    }
    bail!("Google 登录已超时，请重试")
}

fn parse_callback(request: &str, state: &str) -> Result<Option<String>> {
    let mut parts = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    ensure!(parts.next() == Some("GET"), "回调方法无效");
    let target = parts.next().unwrap_or_default();
    ensure!(target.starts_with("/oauth/callback?"), "回调路径无效");
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}"))?;
    let pairs: Vec<_> = url.query_pairs().collect();
    ensure!(
        pairs.iter().filter(|(k, _)| k == "state").count() == 1
            && pairs.iter().any(|(k, v)| k == "state" && v == state),
        "回调 state 无效"
    );
    if pairs.iter().any(|(k, _)| k == "error") {
        return Ok(None);
    }
    ensure!(
        pairs.iter().filter(|(k, _)| k == "code").count() == 1,
        "回调 code 无效"
    );
    let code = pairs
        .iter()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .to_string();
    ensure!(!code.is_empty(), "空授权码");
    Ok(Some(code))
}

#[cfg(test)]
mod tests;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    net::Ipv6Addr,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: Uuid,
    pub revision: u64,
    pub name: String,
    pub tags: Vec<String>,
    pub last_used: Option<u64>,
    pub connection: Connection,
    /// Machine-local key for a referenced SSH config; the source stays intact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_identity_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub key_login_configured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_source: Option<Uuid>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sync_conflict: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Connection {
    Manual {
        host: String,
        user: String,
        port: u16,
        identity_file: Option<PathBuf>,
    },
    SshConfig {
        path: PathBuf,
        alias: String,
    },
}

impl Profile {
    pub fn new(name: String, connection: Connection) -> Self {
        Self {
            id: Uuid::new_v4(),
            revision: 0,
            name,
            tags: vec![],
            last_used: None,
            connection,
            local_identity_file: None,
            key_login_configured: false,
            import_source: None,
            sync_conflict: false,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(path) = &self.local_identity_file {
            ensure!(path.is_absolute(), "本机密钥必须使用绝对路径");
        }
        ensure!(!self.name.trim().is_empty(), "名称不能为空");
        ensure!(self.name == self.name.trim(), "名称前后不能包含空格");
        ensure!(
            !self.name.chars().any(char::is_control),
            "名称不能包含控制字符"
        );
        ensure!(
            self.tags.iter().all(|t| !t.chars().any(char::is_control)),
            "标签不能包含控制字符"
        );
        match &self.connection {
            Connection::Manual {
                host,
                user,
                port,
                identity_file,
            } => {
                validate_host(host)?;
                ensure!(
                    !user.is_empty() && !user.chars().any(|c| c.is_control() || c.is_whitespace()),
                    "用户名不能为空或包含空白字符"
                );
                ensure!(*port > 0, "端口必须在 1～65535 之间");
                if let Some(path) = identity_file {
                    ensure!(!path.as_os_str().is_empty(), "私钥路径不能为空");
                }
            }
            Connection::SshConfig { path, alias } => {
                ensure!(
                    path.is_absolute()
                        || (path.as_os_str().is_empty() && self.import_source.is_some()),
                    "导入配置必须使用绝对路径"
                );
                ensure!(valid_alias(alias), "SSH 别名无效");
            }
        }
        Ok(())
    }

    pub fn identity_file(&self) -> Option<&PathBuf> {
        self.local_identity_file
            .as_ref()
            .or(match &self.connection {
                Connection::Manual { identity_file, .. } => identity_file.as_ref(),
                Connection::SshConfig { .. } => None,
            })
    }

    pub fn needs_binding(&self) -> bool {
        matches!(&self.connection, Connection::SshConfig { path, .. } if path.as_os_str().is_empty())
    }

    pub fn set_identity_file(&mut self, path: PathBuf) {
        match &mut self.connection {
            Connection::Manual { identity_file, .. } => {
                *identity_file = Some(path);
                self.local_identity_file = None;
            }
            Connection::SshConfig { .. } => self.local_identity_file = Some(path),
        }
    }

    pub fn address(&self) -> String {
        match &self.connection {
            Connection::Manual { host, port, .. } => {
                if host.contains(':') {
                    format!("[{host}]:{port}")
                } else {
                    format!("{host}:{port}")
                }
            }
            Connection::SshConfig { alias, .. } => format!(
                "SSH: {alias}{}",
                if self.needs_binding() {
                    "（待绑定配置）"
                } else {
                    ""
                }
            ),
        }
    }

    pub fn user(&self) -> &str {
        match &self.connection {
            Connection::Manual { user, .. } => user,
            Connection::SshConfig { .. } => "配置文件",
        }
    }

    pub fn matches(&self, query: &str) -> bool {
        let haystack = format!(
            "{} {} {} {}",
            self.name,
            self.address(),
            self.user(),
            self.tags.join(" ")
        )
        .to_lowercase();
        query
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }
}

pub fn validate_host(host: &str) -> Result<()> {
    ensure!(
        !host.is_empty() && !host.starts_with('-'),
        "主机地址不能为空或以 - 开头"
    );
    ensure!(
        !host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "@/\\[]".contains(c)),
        "主机地址不能包含用户名、空格或路径；IPv6 不需要方括号"
    );
    if host.contains(':') {
        host.parse::<Ipv6Addr>()
            .context("地址格式无效：端口请在端口字段填写")?;
    }
    Ok(())
}

pub fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && !alias.starts_with('-')
        && !alias
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "*?![]@".contains(c))
}

pub fn normalized_name(name: &str) -> String {
    name.trim().to_lowercase()
}

pub fn sort_profiles(profiles: &mut [Profile]) {
    profiles.sort_by(|a, b| {
        b.last_used
            .cmp(&a.last_used)
            .then_with(|| normalized_name(&a.name).cmp(&normalized_name(&b.name)))
    });
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn last_used_label(last: Option<u64>, now: u64) -> String {
    let Some(last) = last else {
        return "未使用".into();
    };
    match now.saturating_sub(last) {
        0..60 => "刚刚".into(),
        seconds @ 60..3600 => format!("{} 分钟前", seconds / 60),
        seconds @ 3600..86400 => format!("{} 小时前", seconds / 3600),
        seconds => format!("{} 天前", seconds / 86400),
    }
}

pub fn expand_path(value: &str) -> Result<PathBuf> {
    let value = value.trim().trim_matches('"');
    if value == "~" || value.starts_with("~/") || value.starts_with("~\\") {
        let home = directories::BaseDirs::new().context("无法确定用户主目录")?;
        return Ok(home.home_dir().join(value.get(2..).unwrap_or("")));
    }
    if value.starts_with('~') {
        bail!("路径仅支持 ~ 或 ~/，不支持 ~其他用户");
    }
    let path = PathBuf::from(value);
    Ok(if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    })
}

pub fn clean_text(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}

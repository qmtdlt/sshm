use super::input::Input;
use crate::discovery::Discovery;
use crate::model::{Connection, Profile, expand_path, validate_host};

#[derive(Clone)]
pub struct Field {
    pub label: &'static str,
    pub input: Input,
}

#[derive(Clone)]
pub struct Form {
    pub fields: Vec<Field>,
    pub original: Option<Profile>,
    pub focus: usize,
    pub connect_default: bool,
    initial: Vec<String>,
    pub discovery_note: String,
    detected_identity: Option<String>,
}

impl Form {
    pub fn new(original: Option<Profile>, connect_default: bool) -> Self {
        let name = original.as_ref().map_or("", |p| p.name.as_str()).to_owned();
        let tags = original
            .as_ref()
            .map_or_else(String::new, |p| p.tags.join(", "));
        let mut fields = vec![Field {
            label: "名称 *",
            input: Input::new(name),
        }];
        match original.as_ref().map(|p| &p.connection) {
            Some(Connection::SshConfig { .. }) => {}
            connection => {
                let (host, user, port, identity) = match connection {
                    Some(Connection::Manual {
                        host,
                        user,
                        port,
                        identity_file,
                    }) => (
                        host.clone(),
                        user.clone(),
                        port.to_string(),
                        identity_file
                            .as_ref()
                            .map_or_else(String::new, |p| p.display().to_string()),
                    ),
                    _ => (String::new(), String::new(), "22".into(), String::new()),
                };
                for (label, value) in [
                    ("主机地址 *", host),
                    ("用户名 *", user),
                    ("端口 *", port),
                    ("私钥路径（可选）", identity),
                ] {
                    fields.push(Field {
                        label,
                        input: Input::new(value),
                    });
                }
            }
        }
        fields.push(Field {
            label: "标签（逗号分隔）",
            input: Input::new(tags),
        });
        let initial = fields.iter().map(|f| f.input.value.clone()).collect();
        Self {
            fields,
            original,
            focus: 0,
            connect_default,
            initial,
            discovery_note: String::new(),
            detected_identity: None,
        }
    }

    pub fn dirty(&self) -> bool {
        self.fields
            .iter()
            .zip(&self.initial)
            .any(|(f, old)| f.input.value != *old)
    }
    pub fn detect_identity(&mut self, discovery: &Discovery) {
        if self.imported() {
            return;
        }
        let identity = self.fields[4].input.value.clone();
        if !identity.trim().is_empty() && self.detected_identity.as_ref() != Some(&identity) {
            return;
        }
        if self.detected_identity.take().is_some() {
            self.fields[4].input = Input::default();
        }
        let host = self.fields[1].input.value.trim();
        let user = self.fields[2].input.value.trim();
        let Ok(port) = self.fields[3].input.value.trim().parse::<u16>() else {
            self.discovery_note.clear();
            return;
        };
        if host.is_empty() || user.is_empty() || port == 0 {
            self.discovery_note.clear();
            return;
        }
        let profile = Profile::new(
            "detect".into(),
            Connection::Manual {
                host: host.into(),
                user: user.into(),
                port,
                identity_file: None,
            },
        );
        let found = discovery.find(&profile);
        self.discovery_note = found.note;
        if let Some(candidate) = found.automatic {
            let value = candidate.path.display().to_string();
            self.fields[4].input = Input::new(value.clone());
            self.detected_identity = Some(value);
        }
    }

    pub fn key_candidates(&self, discovery: &Discovery) -> crate::discovery::Found {
        let profile = Profile::new(
            "detect".into(),
            Connection::Manual {
                host: self.fields[1].input.value.trim().into(),
                user: self.fields[2].input.value.trim().into(),
                port: self.fields[3].input.value.trim().parse().unwrap_or(22),
                identity_file: None,
            },
        );
        discovery.find(&profile)
    }

    pub fn choose_identity(&mut self, path: Option<std::path::PathBuf>) {
        self.fields[4].input =
            Input::new(path.map_or_else(String::new, |p| p.display().to_string()));
        self.detected_identity = None;
        self.discovery_note = "已选择本机密钥".into();
        self.focus = 4;
    }
    pub fn imported(&self) -> bool {
        self.original
            .as_ref()
            .is_some_and(|p| matches!(p.connection, Connection::SshConfig { .. }))
    }
    pub fn focus_next(&mut self, backwards: bool) {
        let count = self.fields.len() + 3;
        self.focus = if backwards {
            (self.focus + count - 1) % count
        } else {
            (self.focus + 1) % count
        };
    }

    pub fn profile(&self, existing: &[Profile]) -> Result<Profile, (usize, String)> {
        let name = self.fields[0].input.value.trim().to_owned();
        if name.is_empty() {
            return Err((0, "请输入名称".into()));
        }
        if existing.iter().any(|p| {
            p.name.to_lowercase() == name.to_lowercase()
                && self.original.as_ref().is_none_or(|old| old.id != p.id)
        }) {
            return Err((0, "名称已存在，请换一个名称".into()));
        }
        let connection = if self.imported() {
            self.original.as_ref().unwrap().connection.clone()
        } else {
            let host = self.fields[1].input.value.trim().to_owned();
            validate_host(&host).map_err(|e| (1, e.to_string()))?;
            let user = self.fields[2].input.value.trim().to_owned();
            if user.is_empty() || user.chars().any(|c| c.is_control() || c.is_whitespace()) {
                return Err((2, "请输入不含空格的用户名".into()));
            }
            let port = self.fields[3]
                .input
                .value
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or((3, "端口必须在 1～65535 之间".into()))?;
            let identity = self.fields[4].input.value.trim();
            let identity_file = if identity.is_empty() {
                None
            } else {
                let path = expand_path(identity).map_err(|e| (4, e.to_string()))?;
                if !path.is_file() {
                    return Err((4, "找不到私钥文件，请检查路径".into()));
                }
                Some(path)
            };
            Connection::Manual {
                host,
                user,
                port,
                identity_file,
            }
        };
        let mut profile = self
            .original
            .clone()
            .unwrap_or_else(|| Profile::new(name.clone(), connection.clone()));
        profile.name = name;
        if profile.connection != connection {
            profile.key_login_configured = false;
        }
        profile.connection = connection;
        let mut tags = vec![];
        for tag in self
            .fields
            .last()
            .unwrap()
            .input
            .value
            .split([',', '，'])
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            if !tags.iter().any(|t: &String| t.eq_ignore_ascii_case(tag)) {
                tags.push(tag.to_owned());
            }
        }
        profile.tags = tags;
        profile.validate().map_err(|e| (0, e.to_string()))?;
        Ok(profile)
    }
}

//! Read-only local identity discovery. Never reads private key contents, runs
//! `ssh -G` / Match exec, resolves DNS, or connects to a server.
use crate::{
    import::split_words,
    model::{Connection, Profile, valid_alias},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashSet,
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
};

const DEFAULT_NAMES: &[&str] = &[
    "id_ed25519",
    "id_rsa",
    "id_ecdsa",
    "id_ed25519_sk",
    "id_ecdsa_sk",
    "id_mldsa44_ed25519",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub source: String,
}

#[derive(Default, Debug)]
pub struct Found {
    pub automatic: Option<Candidate>,
    pub target_matched: bool,
    pub candidates: Vec<Candidate>,
    pub note: String,
}

#[derive(Clone)]
pub struct Discovery {
    pub directory: PathBuf,
    home: PathBuf,
    system_config: Option<PathBuf>,
}

#[derive(Clone)]
enum Directive {
    Host(Vec<String>),
    Value(String, String),
    Include(Vec<Directive>),
    Unsupported,
}

#[derive(Default)]
struct Effective {
    host: Option<String>,
    user: Option<String>,
    port: Option<String>,
    identities: Vec<String>,
    specific_identities: Vec<String>,
    uncertain: bool,
}

impl Discovery {
    pub fn standard() -> Result<Self> {
        if let Some(directory) = std::env::var_os("SSHM_SSH_DIR") {
            ensure!(!directory.is_empty(), "SSHM_SSH_DIR 不能为空");
            return Ok(Self::in_directory(PathBuf::from(directory)));
        }
        let home = directories::BaseDirs::new()
            .context("无法确定 SSH 主目录")?
            .home_dir()
            .to_path_buf();
        #[cfg(windows)]
        let system_config =
            std::env::var_os("PROGRAMDATA").map(|p| PathBuf::from(p).join("ssh/ssh_config"));
        #[cfg(unix)]
        let system_config = Some(PathBuf::from("/etc/ssh/ssh_config"));
        Ok(Self {
            directory: home.join(".ssh"),
            home,
            system_config,
        })
    }

    pub fn in_directory(directory: PathBuf) -> Self {
        let home = directory.parent().unwrap_or(&directory).to_path_buf();
        Self {
            directory,
            home,
            system_config: None,
        }
    }

    pub fn find(&self, profile: &Profile) -> Found {
        let mut found = Found::default();
        if let Some(path) = profile.identity_file() {
            if path.is_file() {
                let candidate = Candidate {
                    path: path.clone(),
                    source: "连接已指定的密钥".into(),
                };
                found.automatic = Some(candidate.clone());
                found.candidates.push(candidate);
            } else {
                found.note = "已指定的密钥文件不存在，请修正路径".into();
            }
            return found;
        }
        let (config, requested, user, port, imported) = match &profile.connection {
            Connection::Manual {
                host, user, port, ..
            } => (
                self.directory.join("config"),
                host.as_str(),
                Some(user.as_str()),
                *port,
                false,
            ),
            Connection::SshConfig { path, alias } => (path.clone(), alias.as_str(), None, 22, true),
        };
        let mut directives = vec![];
        let mut errors = vec![];
        for config in std::iter::once(Some(config))
            .chain(std::iter::once(if imported {
                None
            } else {
                self.system_config.clone()
            }))
            .flatten()
        {
            if !config.exists() {
                continue;
            }
            let include_base = if Some(&config) == self.system_config.as_ref() {
                config.parent().unwrap_or(&self.directory)
            } else {
                &self.directory
            };
            match self.parse(&config, include_base, &mut HashSet::new(), &mut 0) {
                Ok(parsed) => directives.push(Directive::Include(parsed)),
                Err(error) => errors.push(error.to_string()),
            }
        }
        let mut direct = Effective::default();
        evaluate(&directives, requested, &mut direct);
        let uncertain = direct.uncertain || !errors.is_empty();
        let mut matched = vec![];
        let resolved = direct.host.as_deref().unwrap_or(requested);
        let mut configured = !direct.identities.is_empty();
        if !uncertain {
            let direct_compatible = imported
                || (direct.user.as_deref().is_none_or(|u| Some(u) == user)
                    && direct
                        .port
                        .as_deref()
                        .is_none_or(|p| p.parse::<u16>().ok() == Some(port)));
            if direct_compatible && (imported || !direct.specific_identities.is_empty()) {
                self.identities(&direct, resolved, user, port, "SSH 配置匹配", &mut matched);
            }
            // A manual IP does not match `Host my-alias`. Reverse-match literal
            // aliases by their effective HostName, user and port.
            if !imported && (direct.specific_identities.is_empty() || !direct_compatible) {
                let mut aliases = vec![];
                collect_aliases(&directives, &mut aliases);
                for alias in aliases {
                    let mut options = Effective::default();
                    evaluate(&directives, &alias, &mut options);
                    if options.uncertain {
                        continue;
                    }
                    let host = options.host.as_deref().unwrap_or(&alias);
                    let port_matches =
                        options.port.as_deref().unwrap_or("22").parse::<u16>().ok() == Some(port);
                    let user_matches = options.user.as_deref().is_none_or(|u| Some(u) == user);
                    if same_host(host, requested) && port_matches && user_matches {
                        configured |= !options.identities.is_empty();
                        self.identities(
                            &options,
                            host,
                            user,
                            port,
                            &format!("SSH 别名 {alias}"),
                            &mut matched,
                        );
                    }
                }
            }
            if matched.is_empty() && direct_compatible {
                self.identities(&direct, resolved, user, port, "SSH 配置匹配", &mut matched);
            }
        }
        if matched.len() == 1 {
            found.automatic = matched.first().cloned();
        }
        found.target_matched = !matched.is_empty();
        found.candidates = matched;
        let mut defaults = vec![];
        for name in DEFAULT_NAMES {
            push_candidate(&mut defaults, self.directory.join(name), "默认 SSH 密钥");
        }
        if found.candidates.is_empty() && !configured && !uncertain && defaults.len() == 1 {
            found.automatic = defaults.first().cloned();
        }
        for candidate in defaults {
            push_candidate(&mut found.candidates, candidate.path, &candidate.source);
        }
        // A paired public key is a safe way to list custom filenames without
        // opening unknown files or treating known_hosts as a login identity.
        if let Ok(entries) = fs::read_dir(&self.directory) {
            let mut paths: Vec<_> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|s| s == "pub"))
                .take(512)
                .collect();
            paths.sort();
            for public in paths {
                let private = public.with_extension("");
                if !private.is_file() || !valid_public_file(&public) {
                    continue;
                }
                push_candidate(
                    &mut found.candidates,
                    private,
                    "本机已有密钥（尚未匹配目标）",
                );
            }
        }
        found.note = if uncertain {
            "SSH 配置含动态条件或无法完整读取，未猜测密钥；可从已有密钥列表选择".into()
        } else if let Some(candidate) = &found.automatic {
            format!("自动识别：{}", candidate.source)
        } else if !found.candidates.is_empty() {
            format!(
                "发现 {} 把已有密钥；F2 选择，无需输入路径",
                found.candidates.len()
            )
        } else {
            "未发现可匹配的本机密钥；也可使用 SSH agent".into()
        };
        found
    }

    fn identities(
        &self,
        options: &Effective,
        host: &str,
        user: Option<&str>,
        port: u16,
        source: &str,
        out: &mut Vec<Candidate>,
    ) {
        let identities = if options.specific_identities.is_empty() {
            &options.identities
        } else {
            &options.specific_identities
        };
        let port = options
            .port
            .as_deref()
            .and_then(|p| p.parse().ok())
            .unwrap_or(port);
        for value in identities {
            if value.eq_ignore_ascii_case("none") {
                continue;
            }
            if let Some(path) = self.expand(value, host, user.or(options.user.as_deref()), port) {
                push_candidate(out, path, source);
            }
        }
    }

    fn expand(&self, value: &str, host: &str, user: Option<&str>, port: u16) -> Option<PathBuf> {
        if value.contains("%r") && user.is_none() {
            return None;
        }
        let mut value = value.to_owned();
        for (token, replacement) in [
            ("%d", self.home.to_str()?),
            ("%h", host),
            ("%r", user.unwrap_or("")),
            ("%p", &port.to_string()),
        ] {
            value = value.replace(token, replacement);
        }
        if value.contains(['%', '$']) {
            return None;
        }
        if value == "~" {
            return Some(self.home.clone());
        }
        if value.starts_with("~/") || value.starts_with("~\\") {
            return Some(self.home.join(&value[2..]));
        }
        let path = PathBuf::from(value);
        // OpenSSH interprets relative IdentityFile paths relative to the cwd,
        // not relative to the file containing the directive.
        Some(if path.is_absolute() {
            path
        } else {
            std::env::current_dir().ok()?.join(path)
        })
    }

    fn parse(
        &self,
        path: &Path,
        include_base: &Path,
        stack: &mut HashSet<PathBuf>,
        count: &mut usize,
    ) -> Result<Vec<Directive>> {
        *count += 1;
        ensure!(*count <= 256 && stack.len() < 16, "SSH Include 数量过多");
        let path = dunce::canonicalize(path)?;
        ensure!(stack.insert(path.clone()), "SSH Include 包含循环");
        ensure!(fs::metadata(&path)?.len() <= 1024 * 1024, "SSH 配置过大");
        let text = fs::read_to_string(&path)?;
        let mut result = vec![];
        for line in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let end = line
                .find(|c: char| c.is_whitespace() || c == '=')
                .unwrap_or(line.len());
            let key = line[..end].to_ascii_lowercase();
            if ![
                "host",
                "hostname",
                "user",
                "port",
                "identityfile",
                "include",
                "match",
                "canonicalizehostname",
            ]
            .contains(&key.as_str())
            {
                continue;
            }
            if key == "match" {
                result.push(Directive::Unsupported);
                continue;
            }
            let rest = line[end..].trim_start();
            let words = split_words(rest.strip_prefix('=').unwrap_or(rest).trim_start())?;
            if key == "host" {
                result.push(Directive::Host(words));
            } else if key == "include" {
                for pattern in words {
                    if pattern.contains(['%', '$']) {
                        result.push(Directive::Unsupported);
                        continue;
                    }
                    let path = if pattern.starts_with("~/") || pattern.starts_with("~\\") {
                        self.home.join(&pattern[2..])
                    } else {
                        let path = PathBuf::from(&pattern);
                        if path.is_absolute() {
                            path
                        } else {
                            include_base.join(path)
                        }
                    };
                    let pattern = path.to_string_lossy().replace('\\', "/");
                    for file in glob::glob(&pattern)? {
                        result.push(Directive::Include(self.parse(
                            &file?,
                            include_base,
                            stack,
                            count,
                        )?));
                    }
                }
            } else if key == "canonicalizehostname" {
                if words.first().is_some_and(|v| v != "no") {
                    result.push(Directive::Unsupported);
                }
            } else {
                ensure!(words.len() == 1, "SSH 配置字段格式不支持");
                result.push(Directive::Value(key, words[0].clone()));
            }
        }
        stack.remove(&path);
        Ok(result)
    }
}

fn evaluate(directives: &[Directive], requested: &str, options: &mut Effective) {
    evaluate_scope(directives, requested, options, false);
}

fn evaluate_scope(
    directives: &[Directive],
    requested: &str,
    options: &mut Effective,
    inherited_specific: bool,
) {
    let mut active = true;
    let mut specific = inherited_specific;
    for directive in directives {
        match directive {
            Directive::Host(patterns) => {
                active = host_matches(patterns, requested);
                specific =
                    inherited_specific || patterns.iter().any(|p| p != "*" && !p.starts_with('!'));
            }
            Directive::Unsupported => {
                options.uncertain = true;
                active = false;
            }
            Directive::Include(children) if active => {
                evaluate_scope(children, requested, options, specific)
            }
            Directive::Value(key, value) if active => match key.as_str() {
                "hostname" if options.host.is_none() => {
                    options.host = Some(value.replace("%h", requested))
                }
                "user" if options.user.is_none() => options.user = Some(value.clone()),
                "port" if options.port.is_none() => options.port = Some(value.clone()),
                "identityfile" => {
                    options.identities.push(value.clone());
                    if specific {
                        options.specific_identities.push(value.clone());
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn collect_aliases(directives: &[Directive], aliases: &mut Vec<String>) {
    for directive in directives {
        match directive {
            Directive::Host(patterns) => {
                for p in patterns {
                    if valid_alias(p) && !aliases.contains(p) {
                        aliases.push(p.clone());
                    }
                }
            }
            Directive::Include(children) => collect_aliases(children, aliases),
            _ => {}
        }
    }
}

fn host_matches(patterns: &[String], host: &str) -> bool {
    let mut positive = false;
    for pattern in patterns {
        if let Some(negative) = pattern.strip_prefix('!') {
            if wildcard(negative, host) {
                return false;
            }
        } else if wildcard(pattern, host) {
            positive = true;
        }
    }
    positive
}

fn wildcard(pattern: &str, host: &str) -> bool {
    let text: Vec<_> = host.to_lowercase().chars().collect();
    let mut matches = vec![false; text.len() + 1];
    matches[0] = true;
    for ch in pattern.to_lowercase().chars() {
        if ch == '*' {
            for i in 1..=text.len() {
                matches[i] |= matches[i - 1];
            }
        } else {
            for i in (1..=text.len()).rev() {
                matches[i] = matches[i - 1] && (ch == '?' || ch == text[i - 1]);
            }
            matches[0] = false;
        }
    }
    matches[text.len()]
}

fn same_host(a: &str, b: &str) -> bool {
    match (a.parse::<IpAddr>(), b.parse::<IpAddr>()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a
            .trim_end_matches('.')
            .eq_ignore_ascii_case(b.trim_end_matches('.')),
    }
}

fn push_candidate(out: &mut Vec<Candidate>, path: PathBuf, source: &str) {
    let Ok(path) = dunce::canonicalize(path) else {
        return;
    };
    if !path.is_file()
        || path.extension().is_some_and(|e| e == "pub")
        || out.iter().any(|c| c.path == path)
    {
        return;
    }
    out.push(Candidate {
        path,
        source: source.into(),
    });
}

fn valid_public_file(path: &Path) -> bool {
    if !fs::metadata(path).is_ok_and(|m| m.len() <= 16384) {
        return false;
    }
    fs::read_to_string(path).is_ok_and(|s| {
        s.starts_with("ssh-ed25519 ")
            || s.starts_with("ssh-rsa ")
            || s.starts_with("ecdsa-sha2-")
            || s.starts_with("sk-")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup(config: &str) -> (tempfile::TempDir, Discovery) {
        let dir = tempfile::tempdir().unwrap();
        let ssh = dir.path().join(".ssh");
        fs::create_dir(&ssh).unwrap();
        fs::write(ssh.join("config"), config).unwrap();
        (dir, Discovery::in_directory(ssh))
    }
    fn key(discovery: &Discovery, name: &str) -> PathBuf {
        let path = discovery.directory.join(name);
        fs::write(&path, [0, 255, 1]).unwrap(); // Not UTF-8: private contents must never be parsed.
        fs::write(
            discovery.directory.join(format!("{name}.pub")),
            "ssh-ed25519 AAAAtest comment",
        )
        .unwrap();
        dunce::canonicalize(path).unwrap()
    }
    fn profile(host: &str, user: &str, port: u16) -> Profile {
        Profile::new(
            "sample".into(),
            Connection::Manual {
                host: host.into(),
                user: user.into(),
                port,
                identity_file: None,
            },
        )
    }

    #[test]
    fn reverse_matches_ip_to_alias_with_user_and_port() {
        let (_temp, discovery) = setup(
            "Host my-ubuntu\n HostName 192.168.1.99\n User ubuntu\n Port 2222\n IdentityFile ~/.ssh/custom_key\n",
        );
        let path = key(&discovery, "custom_key");
        let found = discovery.find(&profile("192.168.1.99", "ubuntu", 2222));
        assert_eq!(found.automatic.unwrap().path, path);
        assert!(
            discovery
                .find(&profile("192.168.1.99", "root", 2222))
                .automatic
                .is_none()
        );
        assert!(
            discovery
                .find(&profile("192.168.1.99", "ubuntu", 22))
                .automatic
                .is_none()
        );
    }

    #[test]
    fn host_specific_alias_wins_over_global_identity() {
        let (_temp, discovery) = setup(
            "Host prod\n HostName 192.168.1.99\n User ubuntu\n IdentityFile ~/.ssh/production\nHost *\n IdentityFile ~/.ssh/id_ed25519\n",
        );
        let expected = key(&discovery, "production");
        key(&discovery, "id_ed25519");
        assert_eq!(
            discovery
                .find(&profile("192.168.1.99", "ubuntu", 22))
                .automatic
                .unwrap()
                .path,
            expected
        );
    }

    #[test]
    fn conflicting_aliases_do_not_silently_pick_a_key() {
        let (_temp, discovery) = setup(
            "Host one\n HostName 10.0.0.1\n IdentityFile ~/.ssh/one\nHost two\n HostName 10.0.0.1\n IdentityFile ~/.ssh/two\n",
        );
        key(&discovery, "one");
        key(&discovery, "two");
        let found = discovery.find(&profile("10.0.0.1", "user", 22));
        assert!(found.automatic.is_none());
        assert_eq!(found.candidates.len(), 2);
    }

    #[test]
    fn include_inherits_host_scope_and_scalar_first_value_wins() {
        let (_temp, discovery) = setup(
            "Host alias\n HostName 10.0.0.1\n Include keys.conf\n HostName 10.0.0.2\nHost other\n HostName 10.0.0.3\n",
        );
        fs::write(
            discovery.directory.join("keys.conf"),
            "IdentityFile \"~/.ssh/key with spaces\"\n",
        )
        .unwrap();
        let expected = key(&discovery, "key with spaces");
        assert_eq!(
            discovery
                .find(&profile("10.0.0.1", "user", 22))
                .automatic
                .unwrap()
                .path,
            expected
        );
        assert!(
            discovery
                .find(&profile("10.0.0.2", "user", 22))
                .automatic
                .is_none()
        );
        assert!(
            discovery
                .find(&profile("10.0.0.3", "user", 22))
                .automatic
                .is_none()
        );
    }

    #[test]
    fn match_exec_and_dynamic_configuration_are_never_evaluated() {
        let (_temp, discovery) = setup(
            "Match exec \"do-not-run\"\n IdentityFile ~/.ssh/custom\nHost *\n IdentityFile ~/.ssh/id_ed25519\n",
        );
        key(&discovery, "custom");
        key(&discovery, "id_ed25519");
        let found = discovery.find(&profile("host", "user", 22));
        assert!(found.automatic.is_none());
        assert!(found.note.contains("动态条件"));
    }

    #[test]
    fn sole_standard_key_is_used_but_custom_filenames_are_not_ip_proof() {
        let (_temp, discovery) = setup("");
        key(&discovery, "ubuntu_192_168_1_99");
        assert!(
            discovery
                .find(&profile("192.168.1.99", "user", 22))
                .automatic
                .is_none()
        );
        let default = key(&discovery, "id_ed25519");
        assert_eq!(
            discovery
                .find(&profile("192.168.1.99", "user", 22))
                .automatic
                .unwrap()
                .path,
            default
        );
        key(&discovery, "id_rsa");
        assert!(
            discovery
                .find(&profile("192.168.1.99", "user", 22))
                .automatic
                .is_none()
        );
    }

    #[test]
    fn identity_none_and_missing_mapped_keys_do_not_fall_back_to_a_guess() {
        let (_temp, discovery) = setup("Host *\n IdentityFile none\n");
        key(&discovery, "id_ed25519");
        assert!(
            discovery
                .find(&profile("host", "user", 22))
                .automatic
                .is_none()
        );
        fs::write(
            discovery.directory.join("config"),
            "Host alias\n HostName 10.0.0.1\n IdentityFile ~/.ssh/missing\n",
        )
        .unwrap();
        assert!(
            discovery
                .find(&profile("10.0.0.1", "user", 22))
                .automatic
                .is_none()
        );
    }

    #[test]
    fn known_hosts_and_authorized_keys_are_not_private_key_candidates() {
        let (_temp, discovery) = setup("");
        fs::write(
            discovery.directory.join("known_hosts"),
            "192.168.1.99 ssh-ed25519 AAAA",
        )
        .unwrap();
        fs::write(
            discovery.directory.join("authorized_keys"),
            "ssh-ed25519 AAAA",
        )
        .unwrap();
        fs::write(discovery.directory.join("orphan.pub"), "ssh-ed25519 AAAA").unwrap();
        assert!(
            discovery
                .find(&profile("192.168.1.99", "user", 22))
                .candidates
                .is_empty()
        );
    }

    #[test]
    fn explicit_choice_is_never_replaced() {
        let (_temp, discovery) = setup("Host *\n IdentityFile ~/.ssh/id_ed25519\n");
        key(&discovery, "id_ed25519");
        let chosen = key(&discovery, "chosen");
        let mut p = profile("host", "user", 22);
        p.set_identity_file(chosen.clone());
        assert_eq!(discovery.find(&p).automatic.unwrap().path, chosen);
    }

    #[test]
    fn matches_ipv6_and_expands_identity_tokens() {
        let (_temp, discovery) = setup(
            "Host alias\n HostName 2001:db8::1\n User user\n Port 2222\n IdentityFile ~/.ssh/%r_%p\n",
        );
        let expected = key(&discovery, "user_2222");
        assert_eq!(
            discovery
                .find(&profile("2001:db8:0:0:0:0:0:1", "user", 2222))
                .automatic
                .unwrap()
                .path,
            expected
        );
    }

    #[test]
    fn supports_negative_host_patterns_without_matching_excluded_hosts() {
        let (_temp, discovery) =
            setup("Host *.example.com !blocked.example.com\n IdentityFile ~/.ssh/domain\n");
        key(&discovery, "domain");
        assert!(
            discovery
                .find(&profile("web.example.com", "user", 22))
                .automatic
                .is_some()
        );
        assert!(
            discovery
                .find(&profile("blocked.example.com", "user", 22))
                .automatic
                .is_none()
        );
    }
}

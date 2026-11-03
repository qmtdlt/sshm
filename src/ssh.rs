use crate::{
    model::{Connection, Profile, now},
    store::Store,
};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    process::{Command, Stdio},
};

pub fn arguments(profile: &Profile) -> Result<Vec<OsString>> {
    profile.validate()?;
    ensure!(
        !profile.needs_binding(),
        "此连接需要先绑定本机 SSH 配置，请运行 sshm edit 编辑"
    );
    let mut args: Vec<OsString> = vec![];
    if let Some(path) = &profile.local_identity_file {
        args.extend(["-i".into(), path.as_os_str().to_owned()]);
    }
    match &profile.connection {
        Connection::Manual {
            host,
            user,
            port,
            identity_file,
        } => {
            args.extend([
                "-p".into(),
                port.to_string().into(),
                "-l".into(),
                user.into(),
            ]);
            if let Some(path) = identity_file {
                args.extend(["-i".into(), path.as_os_str().to_owned()]);
            }
            args.extend(["--".into(), host.into()]);
        }
        Connection::SshConfig { path, alias } => {
            args.extend([
                "-F".into(),
                path.as_os_str().to_owned(),
                "--".into(),
                alias.into(),
            ]);
        }
    }
    Ok(args)
}

pub fn run(profile: &Profile, store: &Store) -> Result<i32> {
    let verified_identity = profile.key_login_configured && profile.identity_file().is_some();
    let mut resolved = profile.clone();
    if resolved.identity_file().is_none() {
        let found = crate::discovery::Discovery::standard()?.find(profile);
        if let Some(candidate) = found.automatic {
            println!(
                "使用已有密钥：{}",
                crate::model::clean_text(&candidate.path.display().to_string())
            );
            resolved.set_identity_file(candidate.path);
        }
    }
    let profile = &resolved;
    if let Some(path) = profile.identity_file() {
        ensure!(path.is_file(), "私钥文件不存在：{}", path.display());
    }
    match &profile.connection {
        Connection::SshConfig { path, .. } => ensure!(
            path.is_file(),
            "引用的 SSH 配置已不存在：{}",
            path.display()
        ),
        Connection::Manual {
            identity_file: Some(path),
            ..
        } => ensure!(path.is_file(), "私钥文件不存在：{}", path.display()),
        _ => {}
    }
    let mut args = arguments(profile)?;
    if verified_identity {
        // Match the successful setup verification: unrelated agent keys must
        // not exhaust the server's authentication attempts before this key.
        args.splice(
            0..0,
            [OsString::from("-o"), OsString::from("IdentitiesOnly=yes")],
        );
    }
    // Inherit the terminal directly; no shell, PTY proxy, password capture, or
    // rewritten host-key policy. The child receives Ctrl+C as usual.
    let _signals = ParentSignals::new()?;
    let mut child = Command::new("ssh")
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("无法启动系统 ssh，请确认 OpenSSH 客户端已安装且位于 PATH")?;
    if let Err(error) = store.mark_used(profile.id, now()) {
        eprintln!("\n最近使用时间未保存：{error:#}");
    }
    let status = child.wait().context("等待 SSH 进程时出错")?;
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        Ok(status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
    }
    #[cfg(not(unix))]
    {
        Ok(status.code().unwrap_or(1))
    }
}

// Do not let the waiting manager die when the foreground SSH session receives
// Ctrl+C. A registered handler is reset on exec on Unix, unlike SIG_IGN.
#[cfg(unix)]
pub(crate) struct ParentSignals(Vec<signal_hook::SigId>);
#[cfg(unix)]
impl ParentSignals {
    pub(crate) fn new() -> Result<Self> {
        use std::sync::{Arc, atomic::AtomicBool};
        let mut guard = Self(vec![]);
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGQUIT] {
            guard.0.push(signal_hook::flag::register(
                signal,
                Arc::new(AtomicBool::new(false)),
            )?);
        }
        Ok(guard)
    }
}
#[cfg(unix)]
impl Drop for ParentSignals {
    fn drop(&mut self) {
        for id in &self.0 {
            signal_hook::low_level::unregister(*id);
        }
    }
}

#[cfg(windows)]
pub(crate) struct ParentSignals;
#[cfg(windows)]
unsafe extern "system" fn handle_control(event: u32) -> i32 {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
    i32::from(event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT)
}
#[cfg(windows)]
impl ParentSignals {
    pub(crate) fn new() -> Result<Self> {
        // A process-local handler is not inherited by the child (unlike the
        // ignore-Ctrl+C attribute set with a null handler).
        let ok = unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(handle_control), 1)
        };
        ensure!(
            ok != 0,
            "无法设置终端信号处理：{}",
            std::io::Error::last_os_error()
        );
        Ok(Self)
    }
}
#[cfg(windows)]
impl Drop for ParentSignals {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(handle_control), 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_separate_arguments_and_ipv6() {
        let p = Profile::new(
            "开发".into(),
            Connection::Manual {
                host: "::1".into(),
                user: "me".into(),
                port: 2222,
                identity_file: Some("C:/key with spaces".into()),
            },
        );
        assert_eq!(
            arguments(&p).unwrap(),
            [
                "-p",
                "2222",
                "-l",
                "me",
                "-i",
                "C:/key with spaces",
                "--",
                "::1"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn imported_alias_uses_original_file_not_copied_parameters() {
        let path = std::env::current_dir().unwrap().join("my config");
        let p = Profile::new(
            "label".into(),
            Connection::SshConfig {
                path: path.clone(),
                alias: "actual-alias".into(),
            },
        );
        assert_eq!(
            arguments(&p).unwrap(),
            [
                OsString::from("-F"),
                path.into_os_string(),
                "--".into(),
                "actual-alias".into()
            ]
        );
    }

    #[test]
    fn rejects_option_injection_in_destination() {
        let p = Profile::new(
            "bad".into(),
            Connection::Manual {
                host: "-oProxyCommand=bad".into(),
                user: "root".into(),
                port: 22,
                identity_file: None,
            },
        );
        assert!(arguments(&p).is_err());
    }
}

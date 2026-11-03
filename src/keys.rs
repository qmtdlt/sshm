use crate::{
    model::{Profile, clean_text},
    permissions, ssh,
    store::Store,
};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "密钥目录不能是符号链接"
    );
    permissions::owner_only(path, true)
}

fn key_pair(directory: &Path, profile: &Profile) -> Result<(PathBuf, String)> {
    let private = directory.join("id_ed25519");
    let public = directory.join("id_ed25519.pub");
    ensure!(
        private.exists() == public.exists(),
        "密钥文件不完整，请检查 {}；已有文件未覆盖",
        directory.display()
    );
    if !private.exists() {
        println!("生成此连接专用的本机 Ed25519 密钥…");
        let status = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C"])
            .arg(format!("sshm:{}", profile.id))
            .arg("-f")
            .arg(&private)
            .stdin(Stdio::null())
            .status()
            .context("无法启动 ssh-keygen，请安装 OpenSSH 客户端")?;
        ensure!(status.success(), "生成密钥失败，已有文件不会覆盖");
    } else {
        println!("复用此连接已有的本机密钥…");
    }
    for path in [&private, &public] {
        ensure!(
            !fs::symlink_metadata(path)?.file_type().is_symlink(),
            "密钥文件不能是符号链接"
        );
        permissions::owner_only(path, false)?;
    }
    let text = fs::read_to_string(&public)?;
    let words: Vec<_> = text.split_whitespace().collect();
    ensure!(
        words.len() >= 2
            && words[0] == "ssh-ed25519"
            && !words[1].is_empty()
            && words[1]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)),
        "公钥格式无效"
    );
    let public = format!("ssh-ed25519 {} sshm:{}", words[1], profile.id);
    Ok((dunce::canonicalize(private)?, public))
}

pub fn install_script(public_key: &str) -> Result<String> {
    ensure!(
        !public_key.contains(['\'', '\n', '\r', '\0']),
        "公钥包含无效字符"
    );
    Ok(format!(
        r#"set -eu
umask 077
test -n "$HOME"
mkdir -p "$HOME/.ssh"
chmod 700 "$HOME/.ssh"
file="$HOME/.ssh/authorized_keys"
touch "$file"
chmod 600 "$file"
if ! awk -v key='{blob}' '{{for(i=1;i<=NF;i++) if($i==key) found=1}} END {{exit !found}}' "$file"; then
    printf '\n%s\n' '{public_key}' >> "$file"
fi
printf 'Public key installed.\n'
"#,
        blob = public_key
            .split_whitespace()
            .nth(1)
            .context("公钥格式无效")?
    ))
}

fn command_args(profile: &Profile, verify: bool) -> Result<Vec<OsString>> {
    let mut args = vec!["-T".into()];
    for option in [
        "ConnectTimeout=10",
        "ClearAllForwardings=yes",
        "PermitLocalCommand=no",
        "RemoteCommand=none",
        "ControlPath=none",
        "ControlMaster=no",
    ] {
        args.extend(["-o".into(), option.into()]);
    }
    if verify {
        for option in [
            "BatchMode=yes",
            "PreferredAuthentications=publickey",
            "PasswordAuthentication=no",
            "KbdInteractiveAuthentication=no",
            "IdentitiesOnly=yes",
        ] {
            args.extend(["-o".into(), option.into()]);
        }
    }
    args.extend(ssh::arguments(profile)?);
    args.push(if verify { "true" } else { "sh -s" }.into());
    Ok(args)
}

/// Called only after the user confirms the target and the remote change.
pub fn setup(profile: &Profile, store: &Store) -> Result<Profile> {
    profile.validate()?;
    ensure!(!profile.needs_binding(), "请先绑定本机 SSH 配置");
    if let Some(path) = profile.identity_file() {
        ensure!(
            path.is_file(),
            "指定的密钥文件不存在，请先修正路径或重新选择已有密钥"
        );
    }
    let current = store
        .load()?
        .into_iter()
        .find(|p| p.id == profile.id)
        .context("连接已被删除")?;
    ensure!(
        current.revision == profile.revision,
        "连接已更改，请重新选择后设置密钥"
    );
    let _signals = ssh::ParentSignals::new()?;
    println!("先检查已有密钥或 SSH agent 是否已能登录…");
    let mut preflight = vec![
        OsString::from("-o"),
        OsString::from("StrictHostKeyChecking=yes"),
    ];
    preflight.extend(command_args(profile, true)?);
    if profile.identity_file().is_none() {
        // Keep the agent available when no explicit file was selected.
        if let Some(index) = preflight.iter().position(|v| v == "IdentitiesOnly=yes") {
            preflight[index] = "IdentitiesOnly=no".into();
        }
    }
    if Command::new("ssh")
        .args(preflight)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("无法启动系统 ssh")?
        .success()
    {
        let mut verified = profile.clone();
        verified.key_login_configured = true;
        let saved = if verified != current {
            store.save(verified, Some(profile.revision))?
        } else {
            current
        };
        println!("已有密钥登录可用，已复用当前配置。");
        return Ok(saved);
    }
    let mut _lock = None;
    let (private, public) = if let Some(path) = profile.identity_file() {
        println!("复用已有密钥：{}", clean_text(&path.display().to_string()));
        (path.clone(), existing_public_key(path, profile)?)
    } else {
        let root = store.directory.join("keys");
        private_directory(&root)?;
        let directory = root.join(profile.id.to_string());
        private_directory(&directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("setup.lock"))?;
        lock.try_lock_exclusive()
            .context("这个连接正在另一个窗口设置密钥")?;
        _lock = Some(lock);
        key_pair(&directory, profile)?
    };
    let script = install_script(&public)?;
    println!(
        "连接 {}，请按 SSH 提示输入首次登录密码。",
        clean_text(&profile.address())
    );
    let mut args: Vec<OsString> = vec!["-i".into(), private.as_os_str().to_owned()];
    args.extend(command_args(profile, false)?);
    let mut child = Command::new("ssh")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("无法启动系统 ssh")?;
    let write_result = child
        .stdin
        .take()
        .context("无法传递公钥安装脚本")?
        .write_all(script.as_bytes());
    let status = child.wait()?;
    ensure!(
        status.success(),
        "公钥配置未完成（退出码 {:?}），连接设置未改变",
        status.code()
    );
    write_result.context("无法完整传递公钥安装脚本")?;
    println!("验证是否可以通过密钥登录…");
    let mut updated = profile.clone();
    updated.set_identity_file(private);
    updated.key_login_configured = true;
    let status = Command::new("ssh")
        .args(command_args(&updated, true)?)
        .stdin(Stdio::null())
        .status()
        .context("无法启动密钥验证")?;
    ensure!(
        status.success(),
        "公钥配置完成，但免密码验证未通过；连接设置未改变。已有密钥带口令时，请先用 ssh-add 加载到 SSH agent；服务器也可能要求二次认证或使用其他 authorized_keys 路径"
    );
    let updated = store
        .save(updated, Some(profile.revision))
        .context("服务器密钥配置成功，但本地保存失败；可以重新设置，已有密钥会复用")?;
    println!("密钥登录已配置，本机密钥路径已保存。以后选择此连接即可登录。");
    Ok(updated)
}

fn existing_public_key(private: &Path, profile: &Profile) -> Result<String> {
    ensure!(private.is_file(), "已有密钥文件不存在");
    let mut public_path = private.as_os_str().to_owned();
    public_path.push(".pub");
    let public_path = PathBuf::from(public_path);
    let text = if public_path.is_file() {
        ensure!(fs::metadata(&public_path)?.len() <= 16384, "公钥文件过大");
        fs::read_to_string(public_path)?
    } else {
        let output = Command::new("ssh-keygen")
            .args(["-y", "-f"])
            .arg(private)
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .context("无法从已有密钥读取公钥")?;
        ensure!(
            output.status.success(),
            "无法读取已有密钥的公钥；已有文件未修改"
        );
        String::from_utf8(output.stdout)?
    };
    let words: Vec<_> = text.split_whitespace().collect();
    ensure!(
        words.len() >= 2
            && [
                "ssh-ed25519",
                "ssh-rsa",
                "ecdsa-sha2-nistp256",
                "ecdsa-sha2-nistp384",
                "ecdsa-sha2-nistp521",
                "sk-ssh-ed25519@openssh.com",
                "sk-ecdsa-sha2-nistp256@openssh.com"
            ]
            .contains(&words[0])
            && words[1]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)),
        "已有公钥格式不支持"
    );
    Ok(format!("{} {} sshm:{}", words[0], words[1], profile.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_explicit_key_is_rejected_before_any_login_attempt() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store {
            directory: temp.path().join("config"),
        };
        let mut profile = Profile::new(
            "test".into(),
            crate::model::Connection::Manual {
                host: "localhost".into(),
                user: "tester".into(),
                port: 22,
                identity_file: None,
            },
        );
        profile.set_identity_file(temp.path().join("missing"));
        assert!(
            setup(&profile, &store)
                .unwrap_err()
                .to_string()
                .contains("指定的密钥文件不存在")
        );
        assert!(!store.directory.exists());
    }
    #[test]
    fn public_key_cannot_escape_the_remote_shell_quote() {
        assert!(install_script("ssh-ed25519 test'bad").is_err());
        assert!(install_script("ssh-ed25519 test\nother").is_err());
    }

    #[test]
    fn native_key_generation_is_readable_by_openssh_and_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("key with spaces");
        private_directory(&directory).unwrap();
        let profile = Profile::new(
            "test".into(),
            crate::model::Connection::Manual {
                host: "localhost".into(),
                user: "tester".into(),
                port: 22,
                identity_file: None,
            },
        );
        let (private, public) = key_pair(&directory, &profile).unwrap();
        let original = fs::read(&private).unwrap();
        let output = Command::new("ssh-keygen")
            .args(["-y", "-f"])
            .arg(&private)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = public
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(String::from_utf8_lossy(&output.stdout).starts_with(&expected));
        key_pair(&directory, &profile).unwrap();
        assert!(original == fs::read(private).unwrap());
        fs::write(directory.join("id_ed25519.pub"), "broken").unwrap();
        assert!(key_pair(&directory, &profile).is_err());
    }

    #[test]
    fn installation_script_preserves_existing_keys_and_is_idempotent() {
        let shell = std::env::var_os("SSHM_TEST_SH").unwrap_or_else(|| "sh".into());
        if Command::new(&shell)
            .arg("-c")
            .arg("exit 0")
            .status()
            .is_err()
        {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let ssh_dir = temp.path().join(".ssh");
        fs::create_dir(&ssh_dir).unwrap();
        let file = ssh_dir.join("authorized_keys");
        fs::write(&file, "existing-key-without-final-newline").unwrap();
        let script = install_script("ssh-ed25519 AAAAtest sshm:test").unwrap();
        for _ in 0..2 {
            let status = Command::new(&shell)
                .arg("-c")
                .arg(&script)
                .env("HOME", temp.path())
                .status()
                .unwrap();
            assert!(status.success());
        }
        let content = fs::read_to_string(file).unwrap();
        assert!(content.starts_with("existing-key-without-final-newline\n"));
        assert_eq!(content.matches("ssh-ed25519 AAAAtest sshm:test").count(), 1);
        let file = ssh_dir.join("authorized_keys");
        let restricted = "command=\"echo restricted\" ssh-ed25519 AAAAtest old-comment\n";
        fs::write(&file, restricted).unwrap();
        assert!(
            Command::new(&shell)
                .arg("-c")
                .arg(&script)
                .env("HOME", temp.path())
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), restricted);
    }
}

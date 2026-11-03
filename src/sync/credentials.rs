//! Explicitly consented headless Linux credential storage. Never a silent fallback.
//! The worker enables this backend only on Linux; Unix builds exercise its tests.
use super::google::Credential;
use crate::store::Store;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const MAX_BYTES: u64 = 16 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    credential: Credential,
}

fn directory(store: &Store) -> PathBuf {
    store.directory.join("credentials")
}
pub fn path(store: &Store) -> PathBuf {
    directory(store).join("google-drive-credentials.json")
}

fn check(path: &Path, directory: bool, private: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("无法检查本地凭据文件权限")?;
    ensure!(!metadata.file_type().is_symlink(), "凭据路径不能是符号链接");
    ensure!(
        if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        },
        "凭据路径类型无效"
    );
    // geteuid has no preconditions and performs no mutation.
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "凭据路径不属于当前用户"
    );
    let mask = if private { 0o077 } else { 0o022 };
    ensure!(
        metadata.permissions().mode() & mask == 0,
        "凭据路径权限过宽；目录需要 0700，文件需要 0600"
    );
    Ok(())
}

pub fn save(store: &Store, credential: &Credential) -> Result<()> {
    check(&store.directory, true, false)?;
    let dir = directory(store);
    match DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => anyhow::bail!("无法创建本地凭据目录"),
    }
    check(&dir, true, true)?;
    let target = path(store);
    if fs::symlink_metadata(&target).is_ok() {
        check(&target, false, true)?;
    }
    let bytes = serde_json::to_vec(&Envelope {
        version: 1,
        credential: credential.clone(),
    })?;
    ensure!(bytes.len() as u64 <= MAX_BYTES, "凭据内容超过大小限制");
    let mut temporary = tempfile::NamedTempFile::new_in(&dir).context("无法创建凭据临时文件")?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temporary.write_all(&bytes).context("无法写入本地凭据")?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(&target)
        .map_err(|_| anyhow::anyhow!("无法保存本地凭据"))?;
    File::open(&dir)?.sync_all()?;
    Ok(())
}

pub fn load(store: &Store) -> Result<Option<Credential>> {
    let dir = directory(store);
    if !dir.try_exists()? && fs::symlink_metadata(&dir).is_err() {
        return Ok(None);
    }
    check(&store.directory, true, false)?;
    check(&dir, true, true)?;
    let target = path(store);
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&target)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => anyhow::bail!("无法打开本地凭据文件"),
    };
    check(&target, false, true)?;
    ensure!(
        file.metadata()?.len() <= MAX_BYTES,
        "本地凭据文件超过大小限制"
    );
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_BYTES, "本地凭据文件超过大小限制");
    let envelope: Envelope = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("本地凭据文件损坏，请退出登录后重新授权"))?;
    ensure!(envelope.version == 1, "不支持的本地凭据版本");
    let credential = envelope.credential;
    ensure!(
        !credential.account.is_empty()
            && !credential.refresh_token.is_empty()
            && !credential.client_id.is_empty(),
        "本地凭据不完整"
    );
    Ok(Some(credential))
}

pub fn remove(store: &Store) -> Result<()> {
    let dir = directory(store);
    if !dir.try_exists()? && fs::symlink_metadata(&dir).is_err() {
        return Ok(());
    }
    check(&store.directory, true, false)?;
    check(&dir, true, true)?;
    // unlink removes the entry, not the target, even if the credential entry was replaced by a symlink.
    match fs::remove_file(path(store)) {
        Ok(()) => {
            File::open(&dir)?.sync_all()?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            anyhow::bail!("无法删除本地凭据；请检查 credentials/google-drive-credentials.json")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, Store, Credential) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store {
            directory: tmp.path().to_path_buf(),
        };
        let credential = Credential {
            account: "test".into(),
            email: "user@example.test".into(),
            refresh_token: "PRIVATE_REFRESH".into(),
            client_id: "test.apps.googleusercontent.com".into(),
        };
        (tmp, store, credential)
    }
    #[test]
    fn remembers_refreshes_and_deletes_without_touching_connections() {
        let (_tmp, store, mut credential) = setup();
        fs::write(store.path(), "unchanged connections").unwrap();
        assert!(load(&store).unwrap().is_none());
        save(&store, &credential).unwrap();
        assert_eq!(
            load(&store).unwrap().unwrap().refresh_token,
            "PRIVATE_REFRESH"
        );
        assert_eq!(fs::metadata(path(&store)).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(directory(&store)).unwrap().mode() & 0o777,
            0o700
        );
        credential.refresh_token = "ROTATED_REFRESH".into();
        save(&store, &credential).unwrap();
        assert_eq!(
            load(&store).unwrap().unwrap().refresh_token,
            "ROTATED_REFRESH"
        );
        remove(&store).unwrap();
        assert!(load(&store).unwrap().is_none());
        assert_eq!(
            fs::read_to_string(store.path()).unwrap(),
            "unchanged connections"
        );
    }
    #[test]
    fn refuses_unsafe_paths_permissions_and_corruption_without_leaking_contents() {
        use std::os::unix::fs::symlink;
        let (tmp, store, credential) = setup();
        save(&store, &credential).unwrap();
        fs::set_permissions(path(&store), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(&store).is_err());
        assert!(save(&store, &credential).is_err());
        remove(&store).unwrap();
        symlink(store.path(), path(&store)).unwrap();
        assert!(save(&store, &credential).is_err());
        remove(&store).unwrap();
        save(&store, &credential).unwrap();
        fs::write(path(&store), "PRIVATE_INVALID_TOKEN").unwrap();
        let error = load(&store).err().unwrap();
        assert!(!format!("{error:#}").contains("PRIVATE"));
        remove(&store).unwrap();
        fs::remove_dir(directory(&store)).unwrap();
        symlink(tmp.path(), directory(&store)).unwrap();
        assert!(save(&store, &credential).is_err());
        assert!(load(&store).is_err());
    }
}

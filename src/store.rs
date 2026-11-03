use crate::model::{Connection, Profile, normalized_name, sort_profiles};
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const VERSION: u32 = 2;
const MAX_SIZE: u64 = 64 * 1024 * 1024;
pub static CHANGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Data {
    version: u32,
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub sync: crate::sync::model::State,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub sync_archives: std::collections::BTreeMap<String, crate::sync::model::State>,
}

#[derive(Clone)]
pub struct Store {
    pub directory: PathBuf,
}

#[derive(Default, Debug)]
pub struct ImportResult {
    pub added: usize,
    pub skipped: Vec<String>,
}

impl Store {
    pub fn default_location() -> Result<Self> {
        let directory = if let Some(path) = std::env::var_os("SSHM_CONFIG_DIR") {
            ensure!(!path.is_empty(), "SSHM_CONFIG_DIR 不能为空");
            PathBuf::from(path)
        } else {
            directories::BaseDirs::new()
                .context("无法确定配置目录")?
                .config_dir()
                .join("sshm")
        };
        Ok(Self { directory })
    }

    pub fn path(&self) -> PathBuf {
        self.directory.join("connections.json")
    }

    pub fn load(&self) -> Result<Vec<Profile>> {
        let mut profiles = self.read_data()?.profiles;
        sort_profiles(&mut profiles);
        Ok(profiles)
    }

    pub(crate) fn read_data(&self) -> Result<Data> {
        let path = self.path();
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Data {
                    version: VERSION,
                    profiles: vec![],
                    sync: Default::default(),
                    sync_archives: Default::default(),
                });
            }
            Err(error) => {
                return Err(error).with_context(|| format!("无法读取 {}", path.display()));
            }
        };
        let mut bytes = vec![];
        file.take(MAX_SIZE + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_SIZE,
            "配置文件超过 64 MiB，已停止读取"
        );
        let data: Data = serde_json::from_slice(&bytes).with_context(|| {
            format!("配置文件损坏或格式不支持，原文件未修改：{}", path.display())
        })?;
        ensure!(
            data.version == 1 || data.version == VERSION,
            "不支持配置版本 {}，原文件未修改",
            data.version
        );
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for profile in &data.profiles {
            profile.validate().context("配置记录无效，原文件未修改")?;
            ensure!(
                ids.insert(profile.id) && names.insert(normalized_name(&profile.name)),
                "配置包含重复 ID 或名称，原文件未修改"
            );
        }
        Ok(data)
    }

    pub fn find(&self, name: &str) -> Result<Profile> {
        self.load()?
            .into_iter()
            .find(|p| normalized_name(&p.name) == normalized_name(name))
            .with_context(|| format!("没有名为「{name}」的连接；使用 sshm list 查看"))
    }

    fn transaction<T>(&self, edit: impl FnOnce(&mut Vec<Profile>) -> Result<T>) -> Result<T> {
        self.update_data(|data| {
            let result = edit(&mut data.profiles)?;
            data.sync.capture(&mut data.profiles);
            Ok(result)
        })
    }

    pub(crate) fn update_data<T>(&self, edit: impl FnOnce(&mut Data) -> Result<T>) -> Result<T> {
        if !self.directory.exists() {
            fs::create_dir_all(&self.directory).context("无法创建配置目录")?;
            private_directory(&self.directory)?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.directory.join("connections.lock"))?;
        lock.try_lock_exclusive()
            .context("另一个 sshm 正在保存，请稍后重试")?;
        let mut data = self.read_data()?;
        let original = data.clone();
        if data.sync.account.is_some() {
            data.sync.assign_sources(&mut data.profiles);
            for profile in &mut data.profiles {
                if data
                    .sync
                    .baseline
                    .get(&profile.id)
                    .is_some_and(|old| old != &crate::sync::model::Portable::from_profile(profile))
                {
                    profile.revision = profile.revision.saturating_add(1);
                }
            }
            data.sync.capture(&mut data.profiles);
        }
        let result = edit(&mut data)?;
        if data == original && self.path().exists() {
            return Ok(result);
        }
        if original.version == 1 {
            let bytes = fs::read(self.path())?;
            let mut backup = self.directory.join("connections.v1.backup.json");
            if backup.exists() && fs::read(&backup)? != bytes {
                backup = self
                    .directory
                    .join(format!("connections.v1.backup.{}.json", Uuid::new_v4()));
            }
            if !backup.exists() {
                let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
                crate::permissions::owner_only(file.path(), false)?;
                file.write_all(&bytes)?;
                file.as_file().sync_all()?;
                file.persist_noclobber(backup).map_err(|e| e.error)?;
            }
        }
        data.version = VERSION;
        let mut temp = tempfile::NamedTempFile::new_in(&self.directory)?;
        crate::permissions::owner_only(temp.path(), false)?;
        let bytes = serde_json::to_vec_pretty(&data)?;
        ensure!(
            (bytes.len() as u64) < MAX_SIZE,
            "同步历史超过本地大小限制，原文件未修改"
        );
        temp.write_all(&bytes)?;
        temp.write_all(b"\n")?;
        temp.as_file().sync_all()?;
        temp.persist(self.path())
            .map_err(|e| e.error)
            .context("无法保存配置，原文件未修改")?;
        CHANGES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(result)
    }

    pub fn bind_source(&self, profile: &Profile, path: &Path) -> Result<()> {
        let preview = crate::import::preview(path)?;
        let path = dunce::canonicalize(path)?;
        self.update_data(|data| {
            data.sync.assign_sources(&mut data.profiles);
            let current = data.profiles.iter().find(|p| p.id == profile.id).context("连接已删除")?;
            ensure!(current.revision == profile.revision, "连接已更改，请重新打开绑定");
            let source = current.import_source.context("此连接没有同步来源")?;
            for p in data.profiles.iter().filter(|p| p.import_source == Some(source)) {
                if let Connection::SshConfig { alias, .. } = &p.connection {
                    ensure!(preview.profiles.iter().any(|candidate| matches!(&candidate.connection, Connection::SshConfig { alias: a, .. } if a == alias)), "所选文件缺少别名：{alias}");
                }
            }
            data.sync.bindings.insert(source, path.clone());
            for p in data.profiles.iter_mut().filter(|p| p.import_source == Some(source)) {
                if let Connection::SshConfig { path: target, .. } = &mut p.connection {
                    *target = path.clone(); p.key_login_configured = false; p.revision += 1;
                }
            }
            Ok(())
        })
    }

    pub fn sync_capture(&self) -> Result<crate::sync::model::State> {
        ensure!(self.path().is_file(), "配置文件暂时缺失，已暂停同步");
        self.update_data(|data| {
            data.sync.capture(&mut data.profiles);
            Ok(data.sync.clone())
        })
    }

    pub fn sync_merge(
        &self,
        account: &str,
        batches: &[crate::sync::model::Batch],
        files: &[String],
        sent: &[Uuid],
    ) -> Result<()> {
        ensure!(self.path().is_file(), "配置文件暂时缺失，已暂停同步");
        self.update_data(|data| {
            ensure!(
                data.sync.enabled && data.sync.account.as_deref() == Some(account),
                "同步账号已改变"
            );
            data.sync.capture(&mut data.profiles);
            data.sync.ingest(batches)?;
            data.sync.materialize(&mut data.profiles)?;
            data.sync.seen_files.extend(files.iter().cloned());
            for id in sent {
                data.sync.pending.remove(id);
            }
            Ok(())
        })
    }

    pub fn sync_account(&self, account: &str, email: &str, confirmed: bool) -> Result<()> {
        self.update_data(|data| {
            let switching = data
                .sync
                .account
                .as_deref()
                .is_some_and(|old| old != account);
            ensure!(!switching || confirmed, "更换账号需要确认合并本机连接");
            if switching {
                let bindings = data.sync.bindings.clone();
                let mut previous = data.sync_archives.remove(account).unwrap_or_default();
                previous.bindings.extend(bindings);
                // Switching merges current records; absence in the other
                // account is not a new deletion. Already queued tombstones
                // remain intact and resume when returning to this account.
                previous
                    .baseline
                    .retain(|id, _| data.profiles.iter().any(|p| &p.id == id));
                let old = std::mem::replace(&mut data.sync, previous);
                if let Some(account) = &old.account {
                    data.sync_archives.insert(account.clone(), old);
                }
                for p in &mut data.profiles {
                    p.sync_conflict = false;
                }
            }
            data.sync.account = Some(account.into());
            data.sync.email = email.into();
            data.sync.enabled = true;
            data.sync.capture(&mut data.profiles);
            Ok(())
        })
    }

    pub fn save(&self, mut profile: Profile, expected_revision: Option<u64>) -> Result<Profile> {
        profile.validate()?;
        self.transaction(|profiles| {
            ensure!(
                !profiles.iter().any(|p| p.id != profile.id
                    && normalized_name(&p.name) == normalized_name(&profile.name)),
                "名称已存在，请换一个名称"
            );
            if let Some(expected) = expected_revision {
                let old = profiles
                    .iter_mut()
                    .find(|p| p.id == profile.id)
                    .context("连接已被另一个窗口删除，请重新打开列表")?;
                ensure!(
                    old.revision == expected,
                    "连接已被另一个窗口修改，请取消并重新打开编辑"
                );
                profile.last_used = old.last_used;
                profile.revision = old.revision + 1;
                *old = profile.clone();
            } else {
                ensure!(!profiles.iter().any(|p| p.id == profile.id), "连接已存在");
                profile.revision = 1;
                profiles.push(profile.clone());
            }
            Ok(profile)
        })
    }

    pub fn remove(&self, profile: &Profile) -> Result<()> {
        self.transaction(|profiles| {
            let index = profiles
                .iter()
                .position(|p| p.id == profile.id)
                .context("连接已被删除")?;
            ensure!(
                profiles[index].revision == profile.revision,
                "连接已被其他窗口修改，请重新确认删除"
            );
            profiles.remove(index);
            Ok(())
        })
    }

    pub fn mark_used(&self, id: Uuid, timestamp: u64) -> Result<()> {
        self.transaction(|profiles| {
            let profile = profiles
                .iter_mut()
                .find(|p| p.id == id)
                .context("连接已被删除")?;
            profile.last_used = Some(profile.last_used.unwrap_or(0).max(timestamp));
            Ok(())
        })
    }

    pub fn import(&self, candidates: Vec<Profile>) -> Result<ImportResult> {
        if candidates.is_empty() {
            return Ok(ImportResult::default());
        }
        self.transaction(|profiles| {
            let mut result = ImportResult::default();
            for mut candidate in candidates {
                candidate.validate()?;
                if !matches!(candidate.connection, Connection::SshConfig { .. }) {
                    bail!("只能导入 SSH 配置引用");
                }
                if profiles
                    .iter()
                    .any(|p| p.connection == candidate.connection)
                {
                    result.skipped.push(format!("{}：已导入", candidate.name));
                } else if profiles
                    .iter()
                    .any(|p| normalized_name(&p.name) == normalized_name(&candidate.name))
                {
                    result.skipped.push(format!("{}：名称冲突", candidate.name));
                } else {
                    candidate.revision = 1;
                    profiles.push(candidate);
                    result.added += 1;
                }
            }
            Ok(result)
        })
    }
}

#[cfg(unix)]
fn private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn private_directory(_: &Path) -> Result<()> {
    Ok(())
}

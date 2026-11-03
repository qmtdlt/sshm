//! Stable-release updates. No OAuth credentials or connection data enter this module.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use fs2::FileExt;
use reqwest::blocking::Client;
use semver::Version;
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "macos", windows, test))]
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const REPOSITORY: &str = "qmtdlt/sshm";
const METADATA_LIMIT: u64 = 256 * 1024;
const PACKAGE_LIMIT: u64 = 128 * 1024 * 1024;
const PUBLIC_KEY: Option<&str> = option_env!("SSHM_UPDATE_PUBLIC_KEY");

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
pub mod windows;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    payload: String,
    signature: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub version: Version,
    pub artifacts: Vec<Artifact>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub os: String,
    pub arch: String,
    pub min_macos: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}
struct Update {
    version: Version,
    artifact: Artifact,
    // Preserve the exact signed bytes for independent verification after handoff.
    envelope: Vec<u8>,
}

fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("sshm/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(120))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let host = attempt.url().host_str().unwrap_or_default();
            if attempt.previous().len() >= 5 {
                attempt.error("重定向次数过多")
            } else if attempt.url().scheme() == "https"
                && matches!(
                    host,
                    "github.com"
                        | "api.github.com"
                        | "release-assets.githubusercontent.com"
                        | "objects.githubusercontent.com"
                )
            {
                attempt.follow()
            } else {
                attempt.error("更新下载重定向至非受信地址")
            }
        }))
        .build()?)
}

fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "更新响应超过大小限制");
    Ok(bytes)
}

fn decode_manifest(bytes: &[u8], key: &str, tag: &str) -> Result<Manifest> {
    let envelope: Envelope = serde_json::from_slice(bytes).context("更新清单格式无效")?;
    let key: [u8; 32] = STANDARD
        .decode(key)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("更新公钥长度无效"))?;
    let payload = STANDARD.decode(envelope.payload)?;
    let signature = Signature::from_slice(&STANDARD.decode(envelope.signature)?)?;
    VerifyingKey::from_bytes(&key)?
        .verify_strict(&payload, &signature)
        .context("更新清单签名校验失败")?;
    let manifest: Manifest = serde_json::from_slice(&payload)?;
    ensure!(manifest.schema == 1, "不支持的更新清单版本");
    ensure!(
        manifest.version.pre.is_empty() && manifest.version.build.is_empty(),
        "仅支持稳定版更新"
    );
    ensure!(
        tag == format!("v{}", manifest.version),
        "发布标签与签名版本不一致"
    );
    for artifact in &manifest.artifacts {
        ensure!(
            artifact.size > 0 && artifact.size <= PACKAGE_LIMIT,
            "安装包大小无效"
        );
        ensure!(
            artifact.sha256.len() == 64 && artifact.sha256.bytes().all(|c| c.is_ascii_hexdigit()),
            "安装包摘要无效"
        );
        let prefix = format!("https://github.com/{REPOSITORY}/releases/download/{tag}/");
        let name = artifact
            .url
            .strip_prefix(&prefix)
            .context("安装包不属于指定发布源")?;
        let extension = match (artifact.os.as_str(), artifact.arch.as_str()) {
            ("macos", "aarch64" | "x86_64") => {
                macos_version(&artifact.min_macos)?;
                ".pkg"
            }
            ("windows", "x86_64") => {
                // Retain this schema-1 field for existing macOS clients, which validate
                // min_macos on every artifact. Windows does not use this value.
                ensure!(artifact.min_macos == "0", "Windows 清单兼容字段无效");
                ".exe"
            }
            _ => anyhow::bail!("不支持的更新产物平台或架构"),
        };
        ensure!(
            name.ends_with(extension)
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".-_".contains(&c)),
            "安装包地址无效"
        );
    }
    let mut platforms = std::collections::HashSet::new();
    for artifact in &manifest.artifacts {
        ensure!(
            platforms.insert((&artifact.os, &artifact.arch)),
            "更新清单存在重复的平台产物"
        );
    }
    Ok(manifest)
}

fn macos_version(value: &str) -> Result<(u32, u32, u32)> {
    let parts: Vec<_> = value.split('.').collect();
    ensure!((1..=3).contains(&parts.len()), "macOS 版本无效");
    let major = parts[0].parse()?;
    let minor = parts.get(1).unwrap_or(&"0").parse()?;
    let patch = parts.get(2).unwrap_or(&"0").parse()?;
    Ok((major, minor, patch))
}

fn select(manifest: Manifest, current: &Version, os: &str, arch: &str) -> Result<Option<Update>> {
    if manifest.version <= *current {
        return Ok(None);
    }
    let mut artifacts = manifest
        .artifacts
        .into_iter()
        .filter(|a| a.os == os && a.arch == arch);
    let artifact = artifacts
        .next()
        .context("新版尚无适合本系统和架构的安装包")?;
    ensure!(artifacts.next().is_none(), "更新清单存在重复的平台产物");
    Ok(Some(Update {
        version: manifest.version,
        artifact,
        envelope: Vec::new(),
    }))
}

fn check() -> Result<Option<Update>> {
    let key = PUBLIC_KEY
        .filter(|v| !v.is_empty())
        .context("此构建尚未配置更新公钥；请从项目发布页获取安装包")?;
    let http = client()?;
    let response = http
        .get(format!(
            "https://api.github.com/repos/{REPOSITORY}/releases/latest"
        ))
        .timeout(Duration::from_secs(15))
        .send()?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let release: Release =
        serde_json::from_slice(&read_limited(response.error_for_status()?, METADATA_LIMIT)?)?;
    ensure!(
        !release.draft && !release.prerelease,
        "发布源没有提供稳定版"
    );
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .context("发布标签必须以 v 开头")?,
    )?;
    ensure!(
        version.pre.is_empty() && version.build.is_empty(),
        "发布标签不是稳定版"
    );
    if version <= Version::parse(env!("CARGO_PKG_VERSION"))? {
        return Ok(None);
    }
    // Existing macOS 0.4.0 clients reject .exe entries anywhere in update.json.
    // Keep their feed intact; both feeds use the same schema and signing key.
    let feed = if cfg!(windows) {
        "update-windows.json"
    } else {
        "update.json"
    };
    let response = http
        .get(format!(
            "https://github.com/{REPOSITORY}/releases/download/{}/{feed}",
            release.tag_name
        ))
        .timeout(Duration::from_secs(15))
        .send()?
        .error_for_status()?;
    let envelope = read_limited(response, METADATA_LIMIT)?;
    let manifest = decode_manifest(&envelope, key, &release.tag_name)?;
    let mut update = select(
        manifest,
        &Version::parse(env!("CARGO_PKG_VERSION"))?,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )?;
    if let Some(update) = &mut update {
        update.envelope = envelope;
    }
    Ok(update)
}

fn cache_dir() -> Result<PathBuf> {
    Ok(directories::BaseDirs::new()
        .context("无法确定更新缓存目录")?
        .cache_dir()
        .join("sshm")
        .join("updates"))
}
fn lock(path: &Path) -> Result<fs::File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock_exclusive()
        .context("另一个 sshm 正在检查或安装更新")?;
    Ok(file)
}

/// Only the main list starts this worker. It never writes to the terminal.
pub fn background() -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    if !cfg!(any(target_os = "macos", windows))
        || PUBLIC_KEY.is_none_or(str::is_empty)
        || std::env::var("SSHM_UPDATE_CHECK").is_ok_and(|v| v == "0")
    {
        return rx;
    }
    std::thread::spawn(move || {
        let work = || -> Result<()> {
            let cache = cache_dir()?;
            fs::create_dir_all(&cache)?;
            let _lock = lock(&cache.join("check.lock"))?;
            let stamp = cache.join("last-check");
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            if let Ok(previous) = fs::read_to_string(&stamp)
                && let Ok(previous) = previous.trim().parse::<u64>()
                && previous <= now
                && now - previous < 86400
            {
                return Ok(());
            }
            // Record attempts too, so offline launches do not repeatedly hit the network.
            fs::write(stamp, now.to_string())?;
            if let Some(update) = check()? {
                let _ = tx.send(format!(
                    "新版 {} 可用；退出列表后运行 sshm update",
                    update.version
                ));
            }
            Ok(())
        };
        let _ = work();
    });
    rx
}

#[cfg(any(target_os = "macos", windows, test))]
fn verify_download(bytes: &[u8], artifact: &Artifact) -> Result<()> {
    ensure!(
        bytes.len() as u64 == artifact.size,
        "安装包长度不符，未安装"
    );
    ensure!(
        format!("{:x}", Sha256::digest(bytes)).eq_ignore_ascii_case(&artifact.sha256),
        "安装包校验失败，未安装"
    );
    Ok(())
}

pub fn run(check_only: bool) -> Result<i32> {
    ensure!(
        cfg!(any(target_os = "macos", windows)),
        "目前支持 macOS 和 Windows 安装更新；Linux 后续提供"
    );
    let Some(update) = check()? else {
        println!("当前没有可用的较新稳定版（或尚未发布稳定版）。");
        return Ok(0);
    };
    println!(
        "当前 {} → 新版 {}（{} / {}）",
        env!("CARGO_PKG_VERSION"),
        update.version,
        update.artifact.os,
        update.artifact.arch
    );
    if check_only {
        return Ok(0);
    }
    install(update)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn install(_: Update) -> Result<i32> {
    anyhow::bail!("目前支持 macOS 和 Windows 安装更新")
}

#[cfg(target_os = "macos")]
use macos::install;
#[cfg(windows)]
use windows::install;

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    fn artifact() -> Artifact {
        Artifact {
            os: "macos".into(),
            arch: "aarch64".into(),
            min_macos: "11.0".into(),
            url: "https://github.com/qmtdlt/sshm/releases/download/v0.5.0/sshm.pkg".into(),
            size: 3,
            sha256: format!("{:x}", Sha256::digest(b"pkg")),
        }
    }
    fn signed(mutator: impl FnOnce(&mut Manifest)) -> (Vec<u8>, String) {
        let key = SigningKey::from_bytes(&[7; 32]);
        let mut manifest = Manifest {
            schema: 1,
            version: Version::parse("0.5.0").unwrap(),
            artifacts: vec![artifact()],
        };
        mutator(&mut manifest);
        let payload = serde_json::to_vec(&manifest).unwrap();
        (serde_json::to_vec(&serde_json::json!({"payload": STANDARD.encode(&payload), "signature": STANDARD.encode(key.sign(&payload).to_bytes())})).unwrap(), STANDARD.encode(key.verifying_key().as_bytes()))
    }
    #[test]
    fn signature_and_release_identity_are_required() {
        let (bytes, key) = signed(|_| {});
        assert!(decode_manifest(&bytes, &key, "v0.5.0").is_ok());
        assert!(decode_manifest(&bytes, &key, "v0.6.0").is_err());
        assert!(decode_manifest(&bytes, &STANDARD.encode([9; 32]), "v0.5.0").is_err());
        let mut envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        envelope["payload"] = STANDARD.encode(b"tampered").into();
        assert!(decode_manifest(&serde_json::to_vec(&envelope).unwrap(), &key, "v0.5.0").is_err());
    }
    #[test]
    fn rejects_unsafe_or_incompatible_metadata() {
        for url in [
            "http://example.com/a.pkg",
            "https://github.com/qmtdlt/sshm/releases/download/v0.5.0/../a.pkg",
            "https://github.com/other/repo/releases/download/v0.5.0/a.pkg",
        ] {
            let (bytes, key) = signed(|m| m.artifacts[0].url = url.into());
            assert!(decode_manifest(&bytes, &key, "v0.5.0").is_err());
        }
        let (bytes, key) = signed(|m| m.schema = 2);
        assert!(decode_manifest(&bytes, &key, "v0.5.0").is_err());
        let (bytes, key) = signed(|m| m.artifacts[0].size = PACKAGE_LIMIT + 1);
        assert!(decode_manifest(&bytes, &key, "v0.5.0").is_err());
    }
    #[test]
    fn does_not_downgrade_or_guess_architecture() {
        let (bytes, key) = signed(|_| {});
        let manifest = || decode_manifest(&bytes, &key, "v0.5.0").unwrap();
        assert!(
            select(
                manifest(),
                &Version::parse("0.6.0").unwrap(),
                "macos",
                "aarch64"
            )
            .unwrap()
            .is_none()
        );
        assert!(
            select(
                manifest(),
                &Version::parse("0.5.0").unwrap(),
                "macos",
                "aarch64"
            )
            .unwrap()
            .is_none()
        );
        assert!(
            select(
                manifest(),
                &Version::parse("0.4.0").unwrap(),
                "macos",
                "x86_64"
            )
            .is_err()
        );
        assert!(
            select(
                manifest(),
                &Version::parse("0.4.0").unwrap(),
                "macos",
                "aarch64"
            )
            .unwrap()
            .is_some()
        );
    }
    #[test]
    fn corrupted_and_truncated_downloads_fail() {
        assert!(verify_download(b"pkg", &artifact()).is_ok());
        assert!(verify_download(b"bad", &artifact()).is_err());
        assert!(verify_download(b"pk", &artifact()).is_err());
        assert!(read_limited(&b"1234"[..], 3).is_err());
    }
    #[test]
    fn windows_and_macos_artifacts_keep_platform_validation() {
        let windows = |m: &mut Manifest| {
            let a = &mut m.artifacts[0];
            a.os = "windows".into();
            a.arch = "x86_64".into();
            a.min_macos = "0".into();
            a.url = "https://github.com/qmtdlt/sshm/releases/download/v0.5.0/sshm-0.5.0-windows-x86_64.exe".into();
        };
        let (bytes, key) = signed(windows);
        let m = decode_manifest(&bytes, &key, "v0.5.0").unwrap();
        assert!(
            select(m, &Version::parse("0.4.0").unwrap(), "windows", "x86_64")
                .unwrap()
                .is_some()
        );
        for mutate in [
            |m: &mut Manifest| m.artifacts[0].arch = "aarch64".into(),
            |m: &mut Manifest| m.artifacts[0].url.push_str("?redirect=evil"),
            |m: &mut Manifest| m.artifacts[0].url = m.artifacts[0].url.replace(".exe", ".pkg"),
            |m: &mut Manifest| m.artifacts.push(m.artifacts[0].clone()),
        ] {
            let (bytes, key) = signed(|m| {
                windows(m);
                mutate(m);
            });
            assert!(decode_manifest(&bytes, &key, "v0.5.0").is_err());
        }
    }
    #[test]
    fn locking_and_macos_comparison() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lock");
        let first = lock(&path).unwrap();
        assert!(lock(&path).is_err());
        drop(first);
        assert!(lock(&path).is_ok());
        assert!(macos_version("11.10").unwrap() > macos_version("11.9.9").unwrap());
        assert!(macos_version("invalid").is_err());
    }
}

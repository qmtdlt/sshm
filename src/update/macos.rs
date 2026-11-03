use super::*;
const TEAM_ID: Option<&str> = option_env!("SSHM_APPLE_TEAM_ID");

pub(super) fn install(update: Update) -> Result<i32> {
    use std::{
        io::{self, IsTerminal, Write},
        process::Command,
    };
    let team = TEAM_ID
        .filter(|id| {
            id.len() == 10
                && id
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        })
        .context("此构建尚未配置 Apple 签名团队，不能在线安装")?;
    let installed = Path::new("/usr/local/bin/sshm");
    let executable = std::env::current_exe()?.canonicalize()?;
    let root = Path::new("/usr/local/libexec/sshm/versions");
    ensure!(
        executable.starts_with(root)
            && fs::symlink_metadata(installed)?.file_type().is_symlink()
            && installed.canonicalize()? == executable,
        "当前程序不是受管 pkg 安装；请使用原安装方式升级，避免覆盖其他版本"
    );
    let system = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()?;
    ensure!(system.status.success(), "无法读取 macOS 版本");
    ensure!(
        macos_version(String::from_utf8_lossy(&system.stdout).trim())?
            >= macos_version(&update.artifact.min_macos)?,
        "新版需要更高版本的 macOS"
    );
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "更新需要交互式终端确认"
    );
    print!("下载并安装？安装时需要管理员授权 [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        println!("已取消。");
        return Ok(0);
    }
    let cache = cache_dir()?;
    fs::create_dir_all(&cache)?;
    let _lock = lock(&cache.join("install.lock"))?;
    let staging = tempfile::tempdir_in(&cache)?;
    let response = client()?
        .get(&update.artifact.url)
        .send()?
        .error_for_status()?;
    let bytes = read_limited(response, update.artifact.size)?;
    verify_download(&bytes, &update.artifact)?;
    let package = staging.path().join("update.pkg");
    fs::write(&package, bytes)?;
    // The privileged script copies into a root-owned directory and rechecks the hash
    // and Apple trust there, before installer ever opens the package as root.
    let status = Command::new("/usr/bin/sudo")
        .args([
            "/bin/sh",
            "-c",
            include_str!("../../scripts/macos/install-update.sh"),
            "sshm-update",
        ])
        .arg(&package)
        .arg(update.artifact.sha256.to_ascii_lowercase())
        .arg(team)
        .status()?;
    ensure!(
        status.success(),
        "安装未完成；旧版保留在 /usr/local/libexec/sshm/previous，可按文档恢复"
    );
    let output = Command::new(installed).arg("--version").output()?;
    ensure!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == format!("sshm {}", update.version),
        "安装器已退出，但入口版本不符；请按文档检查或恢复旧版"
    );
    println!(
        "已升级到 {}。请退出其他旧窗口后重新启动 sshm。",
        update.version
    );
    Ok(0)
}

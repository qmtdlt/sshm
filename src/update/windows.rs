//! Per-user Inno Setup updates. The old executable is copied out of the install
//! directory so it can supervise replacement after the original process exits.
use super::*;
use std::{
    io::{self, IsTerminal, Write},
    os::windows::{fs::OpenOptionsExt, process::CommandExt},
    process::{Command, Stdio},
    time::Instant,
};

const CERTIFICATE: Option<&str> = option_env!("SSHM_WINDOWS_CERT_SHA256");
const NO_WINDOW: u32 = 0x08000000;
const MARKER: &str = "sshm-install.ini";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    parent: u32,
    directory: PathBuf,
    version: Version,
}

fn certificate() -> Result<&'static str> {
    CERTIFICATE
        .filter(|v| v.len() == 64 && v.bytes().all(|c| c.is_ascii_hexdigit()))
        .context("此构建尚未配置 Windows 签名证书指纹，不能在线安装；请使用本地安装包")
}

fn managed(directory: &Path) -> Result<()> {
    let marker = fs::read_to_string(directory.join(MARKER))
        .context("当前程序不是 Inno Setup 受管安装；请使用原安装方式升级")?;
    ensure!(
        marker.lines().any(|l| l.trim() == "Owner=sshm-inno-v1")
            && directory.join("unins000.exe").is_file(),
        "安装来源无法确认，拒绝覆盖"
    );
    Ok(())
}

fn authenticode(package: &Path, certificate: &str) -> Result<()> {
    // Constant PowerShell program; paths and fingerprints are data in environment
    // variables, never interpolated as shell code. No execution-policy bypass.
    let script = r#"$ErrorActionPreference='Stop'; $s=Get-AuthenticodeSignature -LiteralPath $env:SSHM_VERIFY_FILE; if($s.Status -ne 'Valid'){throw 'Invalid Authenticode signature'}; $h=[Security.Cryptography.SHA256]::Create(); try{$v=([BitConverter]::ToString($h.ComputeHash($s.SignerCertificate.RawData))).Replace('-',''); if($v -ne $env:SSHM_VERIFY_CERT){throw 'Unexpected signing certificate'}}finally{$h.Dispose()}"#;
    let shell = std::env::var_os("SystemRoot").context("无法定位 Windows 系统目录")?;
    let output =
        Command::new(PathBuf::from(shell).join("System32/WindowsPowerShell/v1.0/powershell.exe"))
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .env("SSHM_VERIFY_FILE", package)
            .env("SSHM_VERIFY_CERT", certificate)
            .creation_flags(NO_WINDOW)
            .output()?;
    ensure!(
        output.status.success(),
        "安装包 Windows 信任或发布者校验失败，未安装"
    );
    Ok(())
}

fn verify_version(path: &Path, version: &Version) -> Result<()> {
    let output = Command::new(path)
        .arg("--version")
        .creation_flags(NO_WINDOW)
        .output()?;
    ensure!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == format!("sshm {version}"),
        "安装后的程序版本验证失败"
    );
    Ok(())
}

pub(super) fn install(update: Update) -> Result<i32> {
    certificate()?;
    let executable = dunce::canonicalize(std::env::current_exe()?)?;
    let directory = executable.parent().context("无法确定安装目录")?;
    managed(directory)?;
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "更新需要交互式终端确认"
    );
    print!("下载并安装？请先退出其他 sshm 窗口；此窗口将退出并启动安装器 [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        println!("已取消。");
        return Ok(0);
    }
    let cache = cache_dir()?;
    fs::create_dir_all(&cache)?;
    let _install = lock(&cache.join("install.lock"))?;
    let handoff = lock(&cache.join("handoff.lock"))?;
    let staging = tempfile::Builder::new()
        .prefix("windows-")
        .tempdir_in(&cache)?;
    crate::permissions::owner_only(staging.path(), true)?;
    let response = client()?
        .get(&update.artifact.url)
        .send()?
        .error_for_status()?;
    let bytes = read_limited(response, update.artifact.size)?;
    verify_download(&bytes, &update.artifact)?;
    let package = staging.path().join("update.exe");
    fs::write(&package, bytes)?;
    authenticode(&package, certificate()?)?;
    fs::write(staging.path().join("manifest.json"), &update.envelope)?;
    fs::write(
        staging.path().join("job.json"),
        serde_json::to_vec(&Job {
            parent: std::process::id(),
            directory: directory.to_path_buf(),
            version: update.version,
        })?,
    )?;
    fs::copy(&executable, staging.path().join("worker.exe"))?;
    let log = fs::File::create(staging.path().join("worker.log"))?;
    drop(handoff); // install.lock prevents another parent until the worker owns handoff.lock.
    let mut child = Command::new(staging.path().join("worker.exe"))
        .arg("__finish-update")
        .arg(staging.path())
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .creation_flags(NO_WINDOW)
        .spawn()
        .context("无法启动更新交接进程；旧版未更改")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !staging.path().join("ready").exists() {
        ensure!(child.try_wait()?.is_none(), "更新交接失败；旧版未更改");
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("更新交接超时；旧版未更改");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let path = staging.keep();
    println!(
        "校验通过，安装器即将打开。完成后重新运行 sshm --version。\n更新结果与日志：{}",
        path.display()
    );
    Ok(0)
}

/// Internal handoff entry. It cannot bypass the embedded signing key or publisher.
pub fn finish_from_args() -> Result<()> {
    let path = std::env::args_os().nth(2).context("缺少更新任务目录")?;
    let path = PathBuf::from(path);
    let result = finish(&path);
    let message = match &result {
        Ok(()) => "安装成功，版本已验证。".to_owned(),
        Err(e) => format!("更新未完成：{e:#}"),
    };
    let _ = fs::write(path.join("result.txt"), message);
    // worker.exe cannot delete itself on Windows. Retain it and bounded logs for
    // diagnosis; remove the much larger installer after the child has exited.
    let _ = fs::remove_file(path.join("update.exe"));
    result
}

fn wait_for_parent(parent_id: u32, ready_path: &Path) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    // Open the process handle before acknowledging readiness: a PID cannot be
    // reused underneath this wait, and the parent is still holding install.lock.
    let parent = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parent_id) };
    ensure!(!parent.is_null(), "无法等待原进程退出；请重新运行更新");
    let ready = fs::write(ready_path, b"ready");
    let wait = if ready.is_ok() {
        unsafe { WaitForSingleObject(parent, 30_000) }
    } else {
        u32::MAX
    };
    unsafe {
        CloseHandle(parent);
    }
    ready?;
    ensure!(wait == WAIT_OBJECT_0, "旧进程未退出，未安装");
    Ok(())
}

fn finish(path: &Path) -> Result<()> {
    let job: Job = serde_json::from_slice(&read_limited(
        fs::File::open(path.join("job.json"))?,
        METADATA_LIMIT,
    )?)?;
    managed(&job.directory)?;
    let cache = cache_dir()?;
    let _handoff = lock(&cache.join("handoff.lock"))?;
    wait_for_parent(job.parent, &path.join("ready"))?;
    let _install = lock(&cache.join("install.lock"))?;
    let manifest = decode_manifest(
        &read_limited(fs::File::open(path.join("manifest.json"))?, METADATA_LIMIT)?,
        PUBLIC_KEY.context("缺少更新公钥")?,
        &format!("v{}", job.version),
    )?;
    let update = select(
        manifest,
        &Version::parse(env!("CARGO_PKG_VERSION"))?,
        "windows",
        "x86_64",
    )?
    .context("拒绝同版本或降级更新")?;
    let package = path.join("update.exe");
    // Hold a read-only sharing handle through installer execution. No other process
    // can rewrite/delete the checked file between hash/signature verification and use.
    let file = OpenOptions::new().read(true).share_mode(1).open(&package)?;
    verify_download(
        &read_limited(&file, update.artifact.size)?,
        &update.artifact,
    )?;
    authenticode(&package, certificate()?)?;
    install_verified(&package, path, &job.directory, &update.version)
}

fn install_verified(
    package: &Path,
    staging: &Path,
    directory: &Path,
    version: &Version,
) -> Result<()> {
    let installed = directory.join("sshm.exe");
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    verify_version(&installed, &current).context("已安装版本在交接期间发生变化，未安装")?;
    // An extra supervisor backup covers installer failure and post-install
    // verification failure, including failures after Inno has finished copying.
    let backup = staging.join("before-update.exe");
    fs::copy(&installed, &backup).context("无法备份旧版，未安装")?;
    let old_marker = fs::read(directory.join(MARKER))?;
    let status = Command::new(package)
        .arg(format!("/DIR={}", directory.display()))
        .arg(format!("/LOG={}", staging.join("installer.log").display()))
        .args([
            "/NORESTART",
            "/NOCLOSEAPPLICATIONS",
            "/NORESTARTAPPLICATIONS",
        ])
        .status();
    let result = match status {
        Ok(status) if status.success() => verify_version(&installed, version),
        Ok(status) if matches!(status.code(), Some(2 | 5)) => Err(anyhow::anyhow!("安装已取消")),
        Ok(status) => Err(anyhow::anyhow!(
            "安装器失败（退出码 {:?}），请查看 installer.log",
            status.code()
        )),
        Err(error) => Err(anyhow::Error::new(error).context("无法启动安装器")),
    };
    if let Err(error) = result {
        // Do not overwrite a file still in use. Keep the independent backup and
        // report manual recovery instead of scheduling a replacement at reboot.
        let recovery = (|| -> Result<()> {
            if fs::read(&installed).ok() != fs::read(&backup).ok() {
                fs::copy(&backup, &installed)?;
            }
            fs::write(directory.join(MARKER), old_marker)?;
            verify_version(&installed, &current)
        })();
        if let Err(recovery) = recovery {
            anyhow::bail!(
                "{error:#}；恢复未完成：{recovery:#}。关闭所有 sshm 后从 {} 恢复旧程序",
                backup.display()
            );
        }
        anyhow::bail!("{error:#}；旧版已恢复并验证");
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsigned_and_foreign_packages_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(managed(dir.path()).is_err());
        assert!(authenticode(&std::env::current_exe().unwrap(), &"0".repeat(64)).is_err());
    }

    #[test]
    fn handoff_waits_for_the_original_process_to_exit() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let mut parent = Command::new("cmd.exe")
            .args(["/d", "/c", "set /p sshm_test_input="])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(NO_WINDOW)
            .spawn()
            .unwrap();
        let id = parent.id();
        let ready_worker = ready.clone();
        let worker = std::thread::spawn(move || wait_for_parent(id, &ready_worker));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ready.exists());
        assert!(!worker.is_finished());
        parent.stdin.take().unwrap().write_all(b"exit\r\n").unwrap();
        parent.wait().unwrap();
        worker.join().unwrap().unwrap();
    }

    #[test]
    fn supervisor_restores_after_cancellation_failure_or_wrong_version() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture.rs");
        let program = dir.path().join("fixture.exe");
        let source = r#"
use std::{fs, path::PathBuf};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--version") {
        println!("sshm @VERSION@"); return;
    }
    let dest = PathBuf::from(args.iter().find_map(|s| s.strip_prefix("/DIR=")).unwrap());
    // Simulate a misbehaving installer that changed bytes even when cancelled.
    fs::write(dest.join("sshm.exe"), b"broken executable").unwrap();
    let name = std::env::current_exe().unwrap();
    let code = match name.file_stem().unwrap().to_str().unwrap() {
        "cancel" => 2, "fail" => 4, _ => 0,
    };
    std::process::exit(code);
}
"#;
        fs::write(
            &fixture,
            source.replace("@VERSION@", env!("CARGO_PKG_VERSION")),
        )
        .unwrap();
        let compiled = Command::new("rustc")
            .arg("--edition=2024")
            .arg(&fixture)
            .arg("-o")
            .arg(&program)
            .creation_flags(NO_WINDOW)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        for case in ["cancel", "fail", "wrong-version"] {
            let staging = dir.path().join(case);
            fs::create_dir(&staging).unwrap();
            let directory = staging.join("中文 installed directory");
            fs::create_dir(&directory).unwrap();
            fs::copy(&program, directory.join("sshm.exe")).unwrap();
            fs::write(directory.join(MARKER), b"original marker").unwrap();
            let installer = staging.join(format!("{case}.exe"));
            fs::copy(&program, &installer).unwrap();
            let result = install_verified(
                &installer,
                &staging,
                &directory,
                &Version::parse("99.0.0").unwrap(),
            );
            assert!(result.unwrap_err().to_string().contains("旧版已恢复并验证"));
            assert_eq!(
                fs::read(directory.join("sshm.exe")).unwrap(),
                fs::read(&program).unwrap()
            );
            assert_eq!(
                fs::read(directory.join(MARKER)).unwrap(),
                b"original marker"
            );
        }
    }
}

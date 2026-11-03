# Windows 安装、更新与分发

当前源码版本：0.5.0；优先 Windows x64，最低安装器条件 Windows 10。当前安装包仅供本地试用，未发布 Release。Windows 安装与更新改动未改变 Google 同步源码、配置目录、Credential Manager 服务名及来源绑定规则。

## 安装与卸载

- Inno Setup 6.4+ `.exe`，`PrivilegesRequired=lowest`；默认 `%LOCALAPPDATA%\Programs\sshm`，只写当前用户安装信息和 PATH。系统 OpenSSH 仍为运行前提。
- 支持首次安装、同版本重装、旧版覆盖升级；拒绝降级、外来 `sshm.exe`、安装目录迁移、目录链接及含分号的路径。空格、中文路径可以使用。
- PATH 按大小写不敏感、引号、末尾斜杠和环境变量展开后的路径去重；保留其他条目和原顺序。已有等价条目不会被认领，卸载不删除它。新增条目的所有权记在 `sshm-install.ini`。
- 新开 PowerShell/CMD 后运行 `sshm --version` 或 `sshm`。已有终端的环境不会自动刷新；从旧终端打开的子进程也可能继承旧 PATH，应从开始菜单/Explorer 新开终端。`where.exe sshm` 可检查其他版本是否抢先匹配。
- 系统“已安装的应用”或安装目录的 `unins000.exe` 卸载。只清理本安装器跟踪的文件、`sshm.previous.exe`、安装标记和自己添加的 PATH；目录中用户另放的文件保留。
- 默认保留 `%APPDATA%\sshm` 的连接、同步状态、来源绑定和生成密钥、`%USERPROFILE%\.ssh`、Windows 凭据管理器内的 `sshm.google-drive` 登录状态。没有清空数据开关，也不会请求重新注册 Google 应用。

## 在线更新

```powershell
sshm update --check
sshm update
$env:SSHM_UPDATE_CHECK = '0' # 关闭本次环境的每日后台检查，手动检查不受影响
```

公共逻辑在 `src/update.rs`，平台安装逻辑分别在 `src/update/windows.rs`、`src/update/macos.rs`。只接受 GitHub `qmtdlt/sshm` 的较新稳定版标签；请求超时、响应大小限制、Ed25519 清单签名、标签/版本/平台/架构、产物长度和 SHA-256 均必须通过。正式 Windows 更新还要求 Authenticode 系统信任有效且签名证书 DER SHA-256 与构建时固定指纹一致。没有跳过校验的参数或运行时信任覆盖。

Windows 下载 `update-windows.json`；macOS 继续下载 `update.json`。这两个文件采用相同 schema=1 信封和**同一正式 Ed25519 密钥**，位于同一个 Release。分开文件是为了兼容已有 macOS 0.4.0 对全部 URL 的 `.pkg` 检查；不是重新建立一套 Windows 更新信任。

更新步骤：默认取消的终端确认 → 验证受管安装来源 → 下载并检查签名/摘要 → 将当前程序复制到仅当前用户可访问的随机缓存目录 → 后台接手进程取得交接锁并打开原进程句柄 → 原程序退出 → 接手进程再次验证完整签名清单和安装包 → 打开正常交互安装器 → 验证实际安装的 `sshm --version`。

接手前后的两个文件锁衔接，避免并发更新；安装器另有互斥锁。安装包校验后保持不允许写入/删除的共享读取句柄，直到安装器退出。其他正在运行的 sshm 或文件占用会阻止替换，提示退出后重试；不杀进程、不终止 SSH 会话、不安排重启后覆盖。

安装器在复制前运行候选程序 `--version`，备份旧程序后安装，复制后再次核对。失败恢复旧程序；`ssPostInstall` 异常会返回自定义退出码 10，不能被误认为成功。在线接手进程额外保留独立备份，安装器取消、报错或最终版本不符时恢复并核对旧版。权限、文件占用或磁盘异常使恢复失败时会明确报告备份位置。

更新 CLI 在成功交接后返回 0，表示“安装器已接手”，不是“安装已成功”。最终结果写在 `%LOCALAPPDATA%\sshm\updates\windows-*\result.txt`，同目录有 `worker.log`、`installer.log`。实际目录由 CLI 打印。安装器退出后清理下载的 exe；接手程序自身和诊断日志保留，确认不再有更新进程时可删除整个已完成任务目录。此缓存不是连接数据目录。

断电、强制结束安装器或磁盘损坏不能保证自动回滚。正常升级保留安装目录内的 `sshm.previous.exe`；同版重装不覆盖上一个不同版本。人工恢复前先退出全部 sshm，然后：

```powershell
$dir = "$env:LOCALAPPDATA\Programs\sshm" # 自定义安装目录需替换
& "$dir\sshm.previous.exe" --version
Copy-Item -LiteralPath "$dir\sshm.previous.exe" -Destination "$dir\sshm.exe" -Force
& "$dir\sshm.exe" --version
```

若目录备份不可用，使用更新结果中明确给出的 `before-update.exe`。恢复只涉及程序，不回填配置或云端数据。本次不引入数据格式迁移；将来改变格式须另行处理备份和降级兼容。证书或更新公钥轮换必须设计过渡版本，不能直接换密钥导致既有客户端失去信任。

## 构建与产物

需要 Rust MSVC 工具链、PowerShell 7、Inno Setup 6.4+。复用已有 Google Desktop app JSON；不重新注册。普通源码 `cargo build` 不带配置是预期行为。

```powershell
./scripts/package-windows.ps1 -OAuthConfig '仓库外的现有桌面客户端配置.json' `
  -UpdatePublicKeyFile '仓库外的原更新公钥.pub' -Iscc 'ISCC.exe'
```

可继续使用 `SSHM_GOOGLE_CLIENT_ID` / `SSHM_GOOGLE_CLIENT_SECRET` 构建变量，或 `SSHM_GOOGLE_OAUTH_CONFIG` 文件路径。脚本核验客户端 ID 与公钥已编入程序，只报告布尔状态，不输出值。客户端应用信息按现有设计编入二进制；OAuth JSON 原件、用户令牌、连接文件、SSH 私钥和更新私钥不进入安装包。payload 仅 `sshm.exe`；Inno 自行生成卸载器和安装标记。

未签名输出：`dist/windows-local-trial/sshm-0.5.0-windows-x86_64-local-trial.exe`，文件名、产品描述和界面均标记 LOCAL TRIAL。`build-info.json` 记录版本、目标、源码提交及是否有未提交变更、摘要、配置存在性；它不是可信更新清单。没有原更新公钥时允许完成本地试用构建，`sshm update --check` 会明确报缺少公钥，不能当作在线验收成功。

正式构建另需：

| 配置 | 用途 |
| --- | --- |
| `SSHM_UPDATE_PUBLIC_KEY` 或 `-UpdatePublicKeyFile` | Mac 原有 Ed25519 公钥，必须一致 |
| `SSHM_WINDOWS_SIGN_SHA1` | CurrentUser\My 证书库中可签名的证书选择指纹 |
| `SSHM_WINDOWS_CERT_SHA256` | 该证书 DER 的 SHA-256，编入程序约束发布者 |
| `SSHM_TIMESTAMP_URL` | 签名服务商的 HTTPS RFC 3161 时间戳服务 |
| `SSHM_UPDATE_SIGNING_KEY` | 原 Ed25519 私钥 seed，仅离线签发清单或受控 CI 使用；不进入应用构建 |

```powershell
./scripts/package-windows.ps1 -OAuthConfig '仓库外的现有配置.json' -Sign -SignTool 'signtool.exe'
# 将最终签名安装器放入 dist 根目录供签名示例读取
Copy-Item dist/windows-signed/*.exe dist/
cargo run --locked --example sign_update -- windows-x86_64
# 也可在聚合所有平台最终产物后一次生成两份清单：
cargo run --locked --example sign_update -- aarch64 x86_64 windows-x86_64
```

正式构建签署程序、安装器和卸载器，核对系统信任和固定证书。更新清单必须在最终签名后生成，且签名器要求私钥与指定公钥相符。正式文件名为 `sshm-<Cargo版本>-windows-x86_64.exe`，标签必须 `v<Cargo版本>`；Cargo.lock、安装器文件版本及清单来自同一版本。签名不能保证立即消除 SmartScreen 的信誉提示，需在发行环境实测。

`.github/workflows/package-windows.yml` 与现有跨平台 CI、macOS 打包工作流并行，标签或手动触发，只上传 Actions 产物。默认试用；手动 signed 选项才使用正式证书和原更新密钥。CI 新增 Secrets 为 `SSHM_WINDOWS_PFX`（base64）、`SSHM_WINDOWS_PFX_PASSWORD`，复用原 OAuth Secrets 与 `SSHM_UPDATE_SIGNING_KEY`；公钥、证书 SHA-256、时间戳地址放 Repository Variables。证书临时导入当前用户库，任务结束移除；硬件/云签名证书需按供应商方式配置 SignTool，不能假定一定可导出 PFX。

将 Windows 安装器与 `update-windows.json`、Mac pkg 与 `update.json` 附到同一稳定 Release 是另一次发布操作，本次未执行。

## 2026-11-03 验证记录

本机已有 Inno Setup；找到的现有 Google Desktop app 配置与原 Windows 0.4.0 二进制中的客户端 ID 一致，已复用于 0.5.0 构建。未找到原 Ed25519 公钥/私钥安全配置或 Windows 代码签名证书，未生成新的正式密钥，也没有创建自签证书冒充正式签名。

验证脚本 `scripts/test-windows-installer.ps1` 使用实际 Inno 安装器，安装到独立中文/空格目录及默认用户目录；测试前拒绝覆盖已有受管安装，测试后卸载，保留产物和日志。测试会短暂修改用户 PATH 并恢复；有真实已安装版本时应换隔离 Windows 账号运行。故障和取消只注入临时编译的测试安装器，不进入交付包，也不绕过正式更新校验。

```powershell
./scripts/test-windows-installer.ps1 -Iscc 'ISCC.exe' -OldBinary '原0.4.0的sshm.exe'
```

未指定 `-OldBinary` 时，CI 使用明确标识的旧版测试替身；本机验收使用原有真实 0.4.0。后台交接的 Win32 进程等待、签名篡改、平台选择、大小/摘要和并发锁由 Rust 自动化测试覆盖。Google 测试只使用模拟服务，真实授权、浏览器回调和双机同步未实测。

最终执行结果记录于本文后续验收表；详细安装日志与 JSON 结果位于 `dist/windows-tests/`。真实默认目录中的文件只比对可读取的配置、私钥和绑定文件摘要；无读取权限的系统 SSH 文件不会改 ACL 或提升权限，不能声称已逐字节核验。

| 检查 | 结果与边界 |
| --- | --- |
| 格式和 Clippy `-D warnings` | Windows 通过 |
| Rust 完整测试 | 116 项通过，包含监督进程故障恢复与 19 项伪终端测试 |
| 发行二进制启动与交互 | 指向实际打包 release 二进制，另跑 19 项隔离伪终端测试全部通过；OAuth/SSH/配置目录均为测试隔离设置，没有访问真实账号或服务器 |
| 真实 Inno 安装/升级/卸载 | 31 项自动化断言通过；包含真实 0.4.0 → 0.5.0、同版重装、降级拒绝、默认目录及中文空格目录 |
| 目标目录防覆盖 | 最终包额外 3 项实测通过：已有非受管主程序、备份程序或卸载器时，返回 7 且原文件摘要不变 |
| PATH | CMD/PowerShell 使用持久化用户/系统 PATH 的新子进程可直接执行 sshm；去重、保留外来条目及卸载清理通过；没有使用桌面自动化打开终端 |
| 失败与占用 | 真实文件共享锁和运行中映像阻止替换；安装后失败注入恢复可运行旧版；临时测试安装器取消保留旧版 |
| 更新接手进程 | 原生进程句柄等待测试通过；模拟安装器取消/失败/版本错误后恢复旧版通过；正式签名线上下载升级尚未执行 |
| 数据 | 可读 `%APPDATA%\sshm` 和 `.ssh` 文件摘要未变，Credential Manager 列表未变；一份系统保护的 `administrators_authorized_keys` 无权读取，不算摘要验收 |
| 包内容与配置 | Inno 6.4.2，展开确认 payload 只有 `sshm.exe`；同一 Google 应用注入成功；试用包未附带 JSON、令牌、私钥或连接数据 |
| 更新门禁 | 试用包 `update --check` 返回缺少原公钥的明确错误；无签名包被 Authenticode 检查拒绝；签名/摘要篡改单元测试通过 |
| macOS 交叉编译 | 尝试 `cargo check --lib --target aarch64-apple-darwin --locked`，因本机缺少 `cc`/Apple C 交叉工具链在 ring 构建阶段失败；未声称 Mac 编译或运行通过 |
| 真实 Google / 发布 | 未执行真实授权、双机同步、Git 提交/推送、Release 或公证发布 |

最终试用包 SHA-256：`eb970d3c40ab402172185958b97d45bde1cfc8ff8aabde7dca4989058c7a85d6`。完整安装验收日志位于 `dist/windows-tests/run-ec0957d3129b4dca9b1ab6cf4a6a2c5d/`，汇总为 `results.json`；防覆盖检查汇总为 `guard-results.json`。验收安装均已卸载，用户 PATH 恢复，未留下默认目录下的受管安装。试用包保留，用户可自行双击安装；正式在线更新仍受下列缺失配置阻塞。

仍需安全提供或配置：Mac 原更新公钥（本地开启检查只需公钥）、原签名私钥的受控签发环境，以及 Windows 正式代码签名证书/签名服务和时间戳配置。私钥不要贴入聊天、源码或安装包。补齐后再验收真实签名包下载更新、安装器手动取消、SmartScreen、Windows 10 最低系统、干净用户环境及 Google 双机同步。

参考：[Inno 按用户安装](https://jrsoftware.org/ishelp/topic_setup_privilegesrequired.htm)、[环境变量刷新](https://jrsoftware.org/ishelp/topic_setup_changesenvironment.htm)、[安装退出码](https://jrsoftware.org/ishelp/topic_setupexitcodes.htm)、[安装事件和自定义退出码](https://jrsoftware.org/ishelp/topic_scriptevents.htm)。

# macOS 安装、更新与分发

本阶段实现安装与更新链路，不代表已经正式发布。本文末尾的 Mac 实测对应 0.4.0；当前源码为 0.5.0（增加 Windows 支持），需要在 Mac 重新构建验证。ARM64 优先，Intel 已提供构建入口和 CI 任务，需分别验收。

## 使用方式

安装 `.pkg` 后，新开标准 macOS Terminal，执行 `sshm --version` 或 `sshm`。安装目标是当前启动卷，安装器要求管理员授权；不改用户的 shell 配置。自定义 PATH 若遗漏 `/usr/local/bin`，应自行恢复该路径；可用 `type -a sshm` 检查其他安装版本是否抢先匹配。

```sh
sshm update --check   # 检查稳定版，不下载安装包
sshm update           # 默认取消；输入 y 后下载，sudo 调用系统 installer
SSHM_UPDATE_CHECK=0 sshm  # 关闭此次运行的后台检查
```

仅主列表启动后台检查，每个用户最多一天尝试一次（离线失败也计入）；消息通过 UI 通道在列表空闲提示处显示，SSH 会话内不输出。环境变量设为 0 可永久配置在个人 shell 环境中；手动 `--check` 不受该开关和每日限制影响。帮助、版本、list、直接 connect 不启动后台更新检查。

网络检查连接超时 5 秒，元数据请求总超时 15 秒，包下载总超时 120 秒；元数据最大 256 KiB，包最大 128 MiB。没有可见的较新稳定版时返回 0；网络、签名、兼容或安装失败返回 1；用户取消返回 0，参数错误返回 2。没有配置公钥的开发构建会明确报错，不信任无签名元数据；GitHub 404 表示无可见发布，无法区分私有仓库与无发布。当前只支持公开仓库下载，不携带 GitHub 或 Google 用户令牌。

## 文件布局、恢复和数据

- 入口：`/usr/local/bin/sshm`，链接到 `/usr/local/libexec/sshm/versions/<版本>/sshm`。
- 每个版本只包含程序和 `manage` 管理脚本；上一版链接为 `/usr/local/libexec/sshm/previous`，更早版本也保留到卸载，不自动清理。
- 安装前拒绝同版本覆盖、降级安装、外来命令、可被普通用户写入的安装目录及目录符号链接；Intel Homebrew 若拥有 `/usr/local`，将明确拒绝安装，应沿用原安装方式，不自动改目录所有权。
- 新版安装后先检查 `--version` 的退出码与文本，再原子切换入口；切换后检查失败尝试恢复。下载失败不会进入安装阶段；安装中断可能留下未启用的版本目录，不改已有数据。系统断电无法依赖退出钩子，需核查入口及保留版本，不能承诺所有中断自动回滚。
- 用户配置目录、v2 数据格式、Keychain 服务名和 OAuth client ID 不变；不以 root 启动 TUI，不读写用户连接或凭据。更换程序签名身份可能触发 Keychain 访问授权提示，需真实登录验证。
- 缓存与单用户更新锁位于 `~/Library/Caches/sshm/updates`，下载临时文件正常退出时清理；管理员阶段另复制到 root 私有临时目录，再校验摘要和签名，避免检查后包被替换。

发生升级后异常，可使用已安装版本的管理脚本恢复上一版（将版本号换成实际值）：

```sh
sudo /usr/local/libexec/sshm/versions/0.4.0/manage rollback
```

首次安装没有上一版时会停止。正常卸载：

```sh
sudo /usr/local/libexec/sshm/versions/0.4.0/manage uninstall
```

卸载仅删除受管入口、程序版本和安装收据；连接、`~/.ssh`、本机生成密钥、Keychain 凭据、更新缓存均保留。升级失败留下同版本目录时，不直接重跑覆盖；先确认入口仍指向可用旧版，再由维护者检查失败目录后处理。本阶段不引入数据迁移；未来改变格式必须另做迁移备份与降级兼容方案。

## 本地构建

需要 macOS、Xcode Command Line Tools、Rust、Python 3.11+。默认最低系统 11.0，仅为构建目标，需在最低系统实测后才能承诺兼容。

首次建立独立更新签名密钥，只执行一次；私钥留在仓库外，备份并妥善保管，不能每次构建重新生成：

```sh
python3 scripts/init-update-key.py "$HOME/.config/sshm-release/update-signing.key"
```

此工具生成 0600 私钥文件与 `.pub` 公钥文件。后续所有版本复用同一公钥；私钥丢失会影响既有客户端更新，轮换必须另行设计旧密钥签发新信任的过渡版本。Apple 签名团队同样应保持稳定。

```sh
python3 scripts/package-macos.py \
  --oauth-config /path/outside/repository/google-desktop.json \
  --update-key "$HOME/.config/sshm-release/update-signing.key"
python3 scripts/verify-macos-package.py
python3 scripts/test-macos-installer.py
```

脚本使用原有 OAuth 构建变量，不需要重新注册 Google 应用；JSON 原件与更新私钥不进入安装包。输出为 `dist/sshm-<版本>-macos-aarch64.pkg`；未传 `--sign` 时仅供本地试用，不是正式公证包，不通过在线安装的 Apple 信任校验。

Intel 使用 `--arch x86_64`（先安装对应 Rust target），原生 Intel CI 使用同一脚本。正式构建需要设置：

| 名称 | 用途 |
| --- | --- |
| `SSHM_UPDATE_PUBLIC_KEY` | base64 Ed25519 公钥；使用 `--update-key` 时自动推导 |
| `SSHM_APPLE_TEAM_ID` | 固定签名团队标识，编入程序，用于更新包校验 |
| `SSHM_CODESIGN_IDENTITY` | Developer ID Application 身份 |
| `SSHM_INSTALLER_IDENTITY` | Developer ID Installer 身份 |
| `SSHM_GOOGLE_CLIENT_ID` / `SSHM_GOOGLE_CLIENT_SECRET` | 现有 OAuth 配置；也可通过 `--oauth-config` 安全注入 |

同一开发团队的 Developer ID 证书可以用于不同应用；应用使用稳定标识 `com.qmtdlt.sshm`。程序用 Application 证书启用 hardened runtime 和时间戳；pkg 用 Installer 证书签名。`--sign` 只签名；显式追加 `--notary-profile <已保存的配置名>` 才上传 Apple 公证，成功后附加并验证票据。其他 app 的公证配置可在团队及认证有效时复用，新产物仍需重新提交公证。

## 发布产物与可信更新

发布源固定为 `qmtdlt/sshm` 的 GitHub Releases，标签 `v<版本>`。Cargo、标签、pkg 和签名清单版本必须一致；不自动降级或安装预发布版本。当前只选择正在运行程序的架构，ARM Mac 上的 Intel 旧程序不会自动跨架构切换。

`update.json` 是 `{ "payload": "base64(JSON)", "signature": "base64(Ed25519签名)" }`，对 payload 原始字节签名。payload 包含 schema=1、稳定版本号及 artifacts；每项包含 os、arch、min_macos、url、size、sha256。URL 必须属于同一仓库及版本标签，版本及架构必须匹配；同一平台的重复条目、无效摘要及过大产物被拒绝。

完成签名、公证和 stapling 后，才对最终 pkg 生成摘要和清单。使用安全环境注入 `SSHM_UPDATE_SIGNING_KEY`（32 字节 seed 的 base64）和 `SSHM_UPDATE_PUBLIC_KEY`，然后：

```sh
cargo run --locked --example sign_update -- aarch64 x86_64
```

只发布 ARM64 时仅传 `aarch64`。此命令只写 `dist/update.json`，不上传任何文件。在线安装除校验独立清单签名、大小和 SHA-256 外，还要求 `pkgutil` 验证指定团队的 Installer 签名、`spctl` 安装评估通过；没有任何跳过校验选项。

Windows 0.5.0 增加 `windows-x86_64` 签名参数，输出同一 Release 中的 `update-windows.json`，复用相同 Ed25519 密钥。**不要把 Windows exe 条目放进 macOS 的 `update.json`**：已安装的 macOS 0.4.0 会检查全部条目的 `.pkg` 后缀。两个签名清单可以同时生成，分别保留各平台入口；无需重建正式更新密钥。macOS 的安装、Apple 团队校验和管理员脚本移至 `src/update/macos.rs`，行为保持原有设计。

`.github/workflows/package-macos.yml` 在版本标签或手动触发时准备两架构产物，不创建或发布 GitHub Release。默认生成本地试用包；手动选择 signed 才导入临时签名 Keychain、提交 Apple 公证并生成清单。签名流水线另外需要 Secrets：`SSHM_APPLE_CERTIFICATES_P12`（含两类身份和私钥的 p12，base64）、`SSHM_APPLE_CERTIFICATES_PASSWORD`、`SSHM_APPLE_ID`、`SSHM_APPLE_APP_PASSWORD`、`SSHM_UPDATE_SIGNING_KEY` 和两类签名身份；公钥与 Team ID 用 Repository Variables。OAuth 配置继续用 Secrets。CI 结束删除临时 Keychain，不触及现有 Windows 工作流。

公开发布必须另外授权：将两架构包及对应清单附到同一 Release 草稿，核对后发布稳定版。建议启用不可变 Release，避免已发布产物被替换。发布前验收至少两版安装升级、拒绝授权、真实失败恢复、最低系统、Intel 原生运行，以及 Mac/Windows 真实登录与同步；当前模拟测试不替代这些验收。

参考：[Apple 公证工作流](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)、[GitHub Release 管理](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository)、[GitHub macOS runner 架构](https://docs.github.com/en/actions/how-tos/write-workflows/choose-where-workflows-run/choose-the-runner-for-a-job)。

## 2026-11-03 本轮验证记录

环境：macOS 27.0.1，Apple Silicon。原有 README 入口及待办文档变更已保留并更新状态；没有提交、推送、创建 Release 或上传 Apple 公证。

| 验证 | 结果 |
| --- | --- |
| `cargo fmt --check`、Clippy `-D warnings` | 通过 |
| `cargo test --all-targets --locked` | 113 项通过，包含 Google 模拟服务、存储、SSH 和 19 项伪终端测试 |
| 安装脚本隔离测试 | 9 项通过：安装、升级、前后检查失败恢复、手动安装冲突、同版/降级拒绝、目录保护、摘要/团队/系统信任门禁 |
| ARM64、Intel pkg 构建与展开检查 | 均通过：payload 仅两文件，架构、版本、脚本语法正确；Intel 未原生运行 |
| ARM64 内置配置 | 同一 OAuth 配置、公钥与团队标识存在；更新私钥未包含在程序中，JSON 原件未入包 |
| 本机首次安装 | 用户在系统安装器完成授权；收据为 com.qmtdlt.sshm / 0.4.0，程序为 root:wheel / 0755 |
| 新终端入口 | 新 zsh 解析到 `/usr/local/bin/sshm`，版本 0.4.0；实际程序与构建文件摘要相同 |
| 已安装程序运行 | 隔离配置目录中打开列表并正常退出；未使用真实连接数据 |
| 在线检查 | 成功返回无较新可见稳定版；尚无实际下载并升级到已发布新版的验收 |

伪终端测试原先未回应 macOS 的光标位置查询，也直接在带定位转义码的输出中匹配中文；本轮补齐终端应答和文本匹配，并在测试中关闭后台更新。夹具保存只对短暂锁冲突限时重试，不改生产存储/同步行为。

本地更新私钥位于 `$HOME/.config/sshm-release/update-signing.key`（0600），公钥同目录 `.pub` 文件；请在仓库之外安全备份。现有 Developer ID Application 身份可用；本机未找到可用的 Developer ID Installer 身份，需补齐后再做正式包和公证。当前 dist 包与 update.json 仅供本地试用，未正式签名或发布。

未验收：真正干净的新 Mac、最低 macOS 11.0、Intel 真机、正式包 Gatekeeper/Keychain 授权、真实版本更新和断电恢复、实际卸载、Mac/Windows 真实 Google 登录与双机同步。Windows Google 接入源码未修改，但本轮未在 Windows 运行测试。

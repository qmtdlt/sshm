# Linux 一条命令安装

目标仅为方便安装。首版 Linux x64，不制作 deb/rpm，不实现 Linux 自动更新；Mac 使用 Docker 构建，无需单独准备 Linux 开发电脑。

## 用户安装

公网安装已于 2026-11-03 发布并通过匿名 Linux 容器验证；下载仓库与源码仓库分离，源码继续保持私有。

```sh
curl -fsSL https://raw.githubusercontent.com/qmtdlt/sshm-releases/main/install.sh | sh
```

默认安装到 `/usr/local/bin/sshm`。非 root 用户由 sudo 提示授权，不要求把整条下载命令作为 root 执行；脚本校验签名、压缩包摘要和程序启动后才安装。入口是普通可执行文件，不改 shell 配置；标准 PATH 中已有 `/usr/local/bin` 时可立即输入 `sshm`。若目录不在 PATH 或已有其他版本遮蔽，脚本会说明，而不宣称命令已可直接使用。

依赖：Linux x86_64、curl、OpenSSL 3+、tar/gzip、常见 POSIX/coreutils 工具和 OpenSSH 客户端。缺少依赖时会明确停止，不擅自修改系统软件源。旧 CentOS 和旧 OpenSSL 不在首版承诺范围内；架构不符直接拒绝，不下载错误的程序。若不能使用 sudo，可以安装到个人目录：

```sh
curl -fsSL https://raw.githubusercontent.com/qmtdlt/sshm-releases/main/install.sh | SSHM_INSTALL_DIR="$HOME/.local/bin" sh
```

个人目录需要已加入 PATH。也可在 `sh` 前设置 `SSHM_VERSION=0.4.0` 安装指定稳定版；指定版本必须与签名清单一致。默认使用公开下载仓库 `qmtdlt/sshm-releases` 的最新稳定 Release，所有下载要求 HTTPS，GitHub 无法访问时停止并保留旧程序。

重复运行命令：相同内容不重复写入；来自本脚本且未被修改的旧程序可替换。程序包、签名不符、下载中断或新版不能启动时保留原程序；不覆盖包管理器、手动安装或其他工具建立的入口。原子替换前保存旧文件，正常错误/信号退出时恢复；强制杀进程或断电可能留下 `.sshm-install.lock` 目录，需要维护者检查其中 old/old-receipt 后恢复，不承诺断电自动恢复。

脚本仅管理安装目录中的 `sshm` 和 `.sshm-install.sha256`，不会修改 `~/.config/sshm`、`~/.ssh` 或系统凭据。卸载本脚本安装的系统入口时，确认路径仍属于本脚本后执行：

```sh
sudo rm /usr/local/bin/sshm /usr/local/bin/.sshm-install.sha256
```

个人安装则删除个人目录内的对应两个文件，数据仍保留。Linux 的 `sshm update` 暂未实现，不与此次安装脚本混为一谈。

## 在 Mac 上构建

需要 Docker Desktop 已运行、Python 3.11+ 和 Rust（本机 Rust 只负责清单签名）。Docker 在隔离的 Linux amd64 环境编译，Apple Silicon Mac 会使用模拟执行，首次构建及镜像下载较慢。

```sh
python3 scripts/package-linux.py --oauth-config /path/outside/repository/google-desktop.json
```

也可复用 `SSHM_GOOGLE_CLIENT_ID` 和 `SSHM_GOOGLE_CLIENT_SECRET` 构建环境。使用原有 Google 应用，不重新注册。构建配置经 BuildKit secret 注入，JSON 原件不复制到镜像或产物；只允许必要源码进入 Docker 构建上下文。应用客户端配置仍按原设计编入程序，不包含任何用户令牌或 SSH 私钥。

构建目标 `x86_64-unknown-linux-musl`，采用静态链接减少 glibc 版本差异；构建拒绝存在动态解释器的 ELF，验证 x86_64 ELF 格式和版本启动。静态链接不等于所有发行版和内核均兼容，按实际容器测试范围说明支持情况。

输出：

- `dist/linux/sshm`：原始程序，供检查。
- `dist/linux/sshm-linux-x86_64.tar.gz`：只有一个 sshm 文件的安装包。
- `dist/linux/linux-SHA256SUMS`：版本号与安装包 SHA-256。
- `dist/linux/linux-SHA256SUMS.sig`：之后用现有私钥签出的原始 Ed25519 签名。

构建会删除旧签名，避免把旧签名当成新包签名。先编译签名工具，再仅向它的运行环境安全注入现有 `SSHM_UPDATE_SIGNING_KEY`：

```sh
env -u SSHM_UPDATE_SIGNING_KEY cargo build --locked --example sign_linux
# 通过安全环境向下列进程提供 SSHM_UPDATE_SIGNING_KEY，不在命令行中粘贴私钥
target/debug/examples/sign_linux
```

签名工具会核对私钥与 install.sh 内置公钥是否匹配，不能换用另一套密钥。Mac 上已有私钥保存在仓库外 `$HOME/.config/sshm-release/update-signing.key`；不要打印、提交或作为发布附件上传。发布清单单独签名，不仅依赖与包同处下载的哈希；安装脚本使用 OpenSSL 验证签名。首次获取安装脚本本身仍以 GitHub HTTPS 为信任入口。

## 自动化与发布边界

`.github/workflows/package-linux.yml` 在版本标签或手动触发时构建、签名、验证并上传 Actions 产物，不创建或发布 Release。需要现有 OAuth 配置 Secrets 和同一 `SSHM_UPDATE_SIGNING_KEY`；不修改 macOS/Windows 的构建和更新清单结构。

源码仓库 `qmtdlt/sshm` 保持私有。公开测试使用独立下载仓库 `qmtdlt/sshm-releases`，仅上传安装脚本、简要说明和 Linux 产物；不公开源码、构建配置或凭据。把 tar.gz、linux-SHA256SUMS、linux-SHA256SUMS.sig 一起附到下载仓库对应稳定 Release，并确保 install.sh 已推送到该仓库 main。程序版本、清单版本和 Release 标签一致；下载仓库的最新稳定 Release 必须包含完整 Linux 产物。macOS/Windows 的原有发布与更新地址本轮不更改。自动化不替用户决定公开发布。

Google 登录、浏览器和 Secret Service 的原有要求不变。无桌面服务器不保证具备浏览器和可用凭据服务；本轮不增加无头授权、不改变 Google 同步行为，安装测试不等于真实 Google 授权验收。

参考：[Docker 多平台构建](https://docs.docker.com/build/building/multi-platform/)、[Rust 静态链接](https://doc.rust-lang.org/reference/linkage.html)、[OpenSSL Ed25519 验证](https://docs.openssl.org/3.0/man1/openssl-pkeyutl/)。

## 2026-11-03 验证记录

在 Apple Silicon Mac 的 Docker Desktop 中构建并运行 Linux amd64 容器，编译器 Rust 1.99.1，构建镜像 Alpine 3.24.3（镜像摘要固定在 Dockerfile）。产物为约 2.6 MiB 的压缩包，原始程序约 5.8 MiB；file/readelf 确认为 x86_64 static-pie ELF，无动态解释器。

| 检查 | 结果 |
| --- | --- |
| Ubuntu 22.04、Debian 12、Rocky Linux 9 | 同一个静态程序通过版本、帮助和空列表检查 |
| Ubuntu 22.04 完整安装 | 本地 HTTPS 服务提供实际签名产物，真实 curl 管道安装至 /usr/local/bin；新 shell 能找到命令，交互列表进入/退出通过 |
| Ubuntu 22.04 权限与恢复 | 非 root 个人安装通过；无管理员授权时系统安装被拒绝；重复安装与损坏包拒绝通过，原程序及数据保留 |
| Alpine 3.24 完整安装 | 实际签名验证、系统 PATH、交互列表、重复安装、损坏包拒绝与数据保留通过 |
| 安装脚本隔离测试 | 9 项通过，含签名/下载失败、版本/架构不符、异常压缩包条目、外来程序、并发锁、收据提交失败恢复 |
| Rust 检查 | fmt、Clippy -D warnings 通过；113 项回归测试串行运行通过 |
| 内容检查 | 内置原有 OAuth 应用配置，源码中无真实配置，程序不含更新签名私钥，压缩包仅包含 sshm |

首次并行回归中，已有同步锁测试出现一次释放后立即重新取得锁失败；本轮未修改同步代码，串行完整回归全部通过。该记录不能当作已修复并行测试稳定性。

完整安装测试只把下载地址换为容器内临时 HTTPS 服务，保留正式安装脚本的发布公钥和签名校验，没有提供跳过验证选项，也没有访问或发布 GitHub Release。Docker 容器共用宿主虚拟机的 Linux 内核，不能据此承诺旧内核、所有发行版、真实桌面 Secret Service 或 Google 登录兼容。

初次本地实施未执行公开 GitHub 安装；随后按用户“我要公网测试”的要求完成下述发布。仍未验收旧 CentOS、Linux ARM64、真实 Google 登录及跨机同步。macOS/Windows 程序源码与更新协议未修改。

## 公网发布与匿名安装验证

用户明确要求公网测试后，建立公开下载仓库 [qmtdlt/sshm-releases](https://github.com/qmtdlt/sshm-releases)，仅包含 README、install.sh 和 Release 附件；原源码仓库 qmtdlt/sshm 仍为 PRIVATE。

已发布 [v0.4.0 Linux x64 公网试用版](https://github.com/qmtdlt/sshm-releases/releases/tag/v0.4.0)。压缩包 SHA-256：`0d1c4a4e8cb5f23dd506669893bb038e174091c0cb175ab7426c5859f7fb1765`。发布前逐一核对 GitHub 附件大小与摘要和本地产物一致。

在全新 Ubuntu 22.04 amd64 容器中，不提供 GitHub 账号、令牌或本地下载替身，执行本文顶部的实际公网命令：下载、独立签名校验和系统安装成功；`sshm --version` 返回 0.4.0，空列表命令成功，已安装程序与本地构建逐字节一致。

以后发布 Linux 新版时，需同步私有仓库中的 install.sh 到公开下载仓库，并将该版本的完整三件产物附到下载仓库 Release；源码仓库的 Actions 仍只准备产物，不自动公开发布。公网服务器需能访问 GitHub、raw.githubusercontent.com 及其附件下载域名。

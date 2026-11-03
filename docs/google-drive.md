# Google Drive 接入与验证

## 一次性注册（开发者负责）

本仓库不包含真实 OAuth 客户端、用户令牌或任何预注册服务。以下步骤由应用维护者完成一次，最终用户不需要创建 Google Cloud 项目。

1. 在 Google Cloud Console 创建或选择专用于 sshm 的项目，启用 Google Drive API。
2. 在 Google Auth Platform 配置应用名称、支持邮箱、受众及联系信息；面向其他 Google 账号发布时选择外部受众。
3. 添加 `https://www.googleapis.com/auth/drive.appdata`、`openid`、`email` 权限；不申请整个 Drive 的访问权。
4. 创建 **Desktop app（桌面应用）** OAuth 客户端并下载 JSON。不是 Web application，不需要服务器回调域名；CLI 临时监听 `http://127.0.0.1:<随机端口>/oauth/callback`。
5. 测试阶段添加测试账号。对外发行前完成 Google 要求的发布、品牌验证等步骤；测试状态的离线授权可能有较短有效期，不能据此承诺永久登录。

参考：[桌面 OAuth](https://developers.google.com/identity/protocols/oauth2/native-app)、[应用数据目录](https://developers.google.com/workspace/drive/api/guides/appdata)、[授权有效期](https://developers.google.com/identity/protocols/oauth2#expiration)。

## 开发运行

将下载文件放在源码目录之外，不提交到版本库。其结构应包含 `installed.client_id`，以及 Google 提供的 `installed.client_secret`（若有）。程序忽略文件里的端点地址，只访问固定的 Google 官方端点。

PowerShell：

```powershell
$env:SSHM_GOOGLE_OAUTH_CONFIG = 'D:\private\sshm-google-desktop.json'
cargo run --locked
```

macOS / Linux：

```sh
SSHM_GOOGLE_OAUTH_CONFIG="$HOME/.config/sshm-google-desktop.json" cargo run --locked
```

主列表按 `g` 授权。需要在运行 CLI 的同一台电脑打开浏览器；浏览器启动失败时可复制屏幕中的链接到该电脑的浏览器。回调随机 state 与 PKCE 验证授权，最长等待三分钟，Esc 取消。

## 正式构建

构建时设置 `SSHM_GOOGLE_CLIENT_ID` 和 Google 提供的 `SSHM_GOOGLE_CLIENT_SECRET`（若有），然后执行 `cargo build --release --locked`。它们通过 `option_env!` 编入二进制。最终用户不必设置环境变量。

桌面应用无法保守静态 client secret，它不是服务端机密，也不是用户的刷新令牌；安全性依赖 PKCE 和用户授权。不能在二进制或 CI 中嵌入任何用户 access/refresh token。运行时 `SSHM_GOOGLE_OAUTH_CONFIG` 优先于内置应用信息，变更客户端 ID 后需要重新登录；不同 Google 应用的隐藏目录不会自动共享。

刷新令牌的系统存储：Windows Credential Manager、macOS Keychain、Linux Secret Service；服务名 `sshm.google-drive`，按本地配置目录隔离。Linux 需要可用且已解锁的 Secret Service，会话没有该服务时显示「仅本次登录」。程序不回退为磁盘明文令牌。

## 同步协议与边界

- 云端是标记 `appProperties.sshm_sync=1` 的不可变 JSON 变更批次，放在 `appDataFolder`。目录在普通 Drive 文件列表中不可见。
- 每条变更有 UUID、逻辑连接 ID、父版本 ID，以及可同步字段或删除标记。读取时先收集全部新批次，再验证父版本和环路，最后原子合并；不依赖上传时间或客户端时钟。
- 发送失败或结果不确定时保留待发送记录；再次上传可能有重复文件，但变更 ID 去重保证不会产生重复连接。部分批次上传成功后分别确认，未成功部分下次继续。
- 本地 `connections.json` v2 同时保存连接、同步基线、来源绑定、待发送记录和历史；本机字段不进入批次。`connections.lock` 保护原子写入，`sync.lock` 保证同一配置目录只有一个网络同步执行者。
- 切换账号时本地保留旧账号的同步历史与待发送记录；回到旧账号可继续发送离线修改，其他账号中没有的记录不会被误当作删除。
- 首版保留历史，不自动清理远端文件或删除标记；单响应/批次上限 4 MiB、本地文件上限 64 MiB、变更与远端文件各最多 100,000 条。达到限制时提示并保留原文件，不自行删历史。
- 远端文件缺失不代表连接删除，只有明确的删除事件才传播。用户主动清空应用数据后的恢复/重置不属于首版功能，不应手工删除部分历史文件。
- 不定时查询云端，启动和登录也不自动拉取。本地配置变化后自动推送不可变批次，主列表或同步管理按 `p` 才拉取、合并并推送待发送修改；新电脑登录后同样需要按 `p`。
- 只有待推送修改会自动重试：指数退避通常 10 秒起、最高 5 分钟，并遵守服务端 Retry-After（最多一天）。手动拉取失败必须再按 `p`。没有待上传修改时，后台不调用 Drive API，也不定时刷新令牌；本地每秒的文件检查不产生网络请求。没有后台常驻，短命令结束前未上传的变更下次运行继续。
- `sshm list`、帮助和版本命令保持纯本地只读。连接和交互命令运行期间启用已登录的同步，但连接始终使用当前本地快照，不等待网络。

## 自动化与真实验收

自动化测试只用临时目录、本地 HTTP 模拟服务、模拟 SSH 和 Ratatui 测试缓冲区；不登录 Google，不上传真实数据，不调用 Computer Use。

注册完成后使用两台测试电脑及无敏感内容的测试连接验收：

1. 两台电脑登录同一客户端的同一账号；双方原有连接均保留，按 `p` 后合并结果一致。
2. A 新增/修改/删除连接后自动推送，B 空闲或重新启动时不获取；B 按 `p` 后获取其他电脑的修改。
3. 双方离线编辑同一连接后联网，保留稳定的冲突副本；删除多余版本后各端收敛，不反复复制。
4. 新电脑的导入项必须绑定本机 SSH 配置；本机密钥、路径、最近使用和登录验证状态不能从另一端传来。
5. 撤销 Google 授权后出现重新登录提示；取消登录、换账号取消、系统凭据不可用、程序中断均不能丢失本地配置。
6. 验证 Windows、macOS、Linux 各自的系统凭据持久化、浏览器回调和退出登录删除凭据。

未完成上述真实验证前，只能报告“代码与模拟测试通过”，不能报告“真实 Google 双机同步已验证”。

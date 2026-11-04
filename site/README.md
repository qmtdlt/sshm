# sshm 网站

根据已选的第 1 版设计实现，支持桌面和手机浏览。

```sh
npm ci
npm run dev -- --host 0.0.0.0
npm run build
```

GitHub Pages 部署时使用静态目录 `dist/client`。资源使用相对路径，可放在 `/sshm/` 下；无服务端依赖。

当前 macOS、Windows 下载指向 GitHub Actions 试用产物，需要登录 GitHub。发布公开 Release 后，应替换为长期下载地址。Linux 使用主仓库安装脚本。

视觉检查记录见 `design-qa.md`。`design/reference.png` 为设计参考，其他本地检查截图不提交。

# sshm 网站

根据已选的第 1 版设计实现，支持桌面和手机浏览。

```sh
npm ci
npm run dev -- --host 0.0.0.0
npm run build
```

GitHub Pages 部署时使用静态目录 `dist/client`。资源使用相对路径，可放在 `/sshm/` 下；无服务端依赖。

macOS、Windows 下载指向 GitHub Release；Windows 安装包仍为未签名试用版。Linux 使用主仓库安装脚本。

视觉检查记录见 `design-qa.md`。`design/reference.png` 为设计参考，其他本地检查截图不提交。

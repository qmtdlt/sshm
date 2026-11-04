# Prototype Instructions

Run the local server yourself and open the preview in the browser available to this environment. Do not give the user server-start instructions when you can run it.

Before making substantial visual changes, use the Product Design plugin's `get-context` skill when the visual source is unclear or no longer matches the current goal. When the user gives durable prototype-specific design feedback, preferences, or decisions, record them in `AGENTS.md`.

When implementing from a selected generated mock, treat that image as the source of truth for layout, component anatomy, density, spacing, color, typography, visible content, and hierarchy.

Build app UI in `src/`. Keep `.openai/hosting.json`, `worker/index.js`, `scripts/prepare-sites-build.mjs`, and `tests/sites-worker.test.mjs` intact so the same local prototype can be handed to Sites. Before a Sites handoff, run `npm run build` and `npm run test:sites`; the build must leave `dist/client/index.html`, `dist/server/index.js`, and `dist/.openai/hosting.json`.

## sshm website direction

- The user selected the first generated design: `design/reference.png`.
- Preserve its warm ivory background, pine green accents, restrained typography and three-section layout.
- Website hosting target is GitHub Pages under `/sshm/`. Keep assets compatible with relative base paths.
- Downloads must reflect real published availability. Do not present Actions trial artifacts as signed public releases.
- Support responsive phone browsing. The user retracted the earlier instruction to omit mobile layouts.
- The second section must emphasize entering `sshm` once and following in-app key prompts, with an add/select/connect demonstration and persistent a/e/arrows/Enter hints, instead of teaching subcommands.
- Keep download dialogs concise: omit platform/version introductions and explanatory notes per the user-marked screenshots; use download icons on installer cards. Preserve Linux command copying and documentation links.
- Second section title: 输入 sshm (sshm retains code semantics but uses the same sans-serif font as the hero title). Supporting text: 按照提示添加、编辑、连接服务器.
- Display the Linux install command on one unbroken line with horizontal scrolling when needed; copying must retain the complete command.
- Hero title is now sshm, with the smaller subtitle 轻量的 SSH 管理工具. Omit the descriptive paragraph below; keep only the product name, subtitle and actions.
- Hero and second-section titles share the sans-serif family and weight 800. Their supporting copy uses weight 600; the second sshm stays green.
- Third section title: 配置信息云同步; supporting text: 使用你自己的 Google Drive 同步服务器信息. Neither ends with a Chinese full stop.
- Latest hero: remove the large sshm name. Use 轻量的 SSH 管理工具 as the sole enlarged heading; fit its font size to the column and keep it on one line at desktop and phone widths.
- Keep the repository link in the navigation; omit the duplicate 查看源码 link beside the hero download button.
- In the desktop hero, vertically center the entire copy/actions group against the adjacent terminal image.

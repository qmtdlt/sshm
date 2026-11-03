use super::{
    app::{App, Confirmation, KeyPurpose, Screen},
    form::Form,
    input::Input,
};
use crate::model::{Connection, clean_text, last_used_label, now};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap},
};

fn accent() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}
fn muted() -> Style {
    Style::default().fg(Color::DarkGray)
}
fn selected() -> Style {
    Style::default().fg(Color::Black).bg(Color::Cyan)
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.setup_confirmation_visible = area.width >= 60 && area.height >= 20;
    if area.width < 38 || area.height < 12 {
        frame.render_widget(
            Paragraph::new(
                "窗口过小，请至少调整到 38 列 × 12 行。\n仍可按 q 退出；表单中按 Esc 取消。",
            )
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(area);
    let title = match &app.screen {
        Screen::Sync { .. } => "Google Drive 同步".into(),
        Screen::BindConfig { .. } => "绑定本机 SSH 配置".into(),
        Screen::List => format!(
            "SSH Manager   {} / {} 个连接",
            app.filtered().len(),
            app.profiles.len()
        ),
        Screen::Form(form) if form.original.is_some() => "编辑连接".into(),
        Screen::Form(_) => "新增连接".into(),
        Screen::ImportPath(_) | Screen::Import { .. } => "导入 SSH 配置".into(),
        Screen::Confirm { .. } => "确认操作".into(),
        Screen::Help => "快捷键帮助".into(),
        Screen::KeyPicker { .. } => "选择本机已有密钥".into(),
        Screen::FirstConnect { .. } => "首次连接：设置登录方式".into(),
        Screen::Report { title, .. } => title.clone(),
    };
    frame.render_widget(
        Paragraph::new(clean_text(&title)).style(accent()),
        chunks[0],
    );
    app.page_size = usize::from(chunks[1].height.saturating_sub(3)).max(1);
    match &app.screen {
        Screen::Sync { offset } => {
            let status = &app.sync_status;
            let mut text = format!(
                "{}\n{}\n\n",
                clean_text(&status.email),
                clean_text(&status.message)
            );
            if status.session_only {
                text.push_str("仅本次运行登录：系统凭据存储不可用，下次需重新登录。\n\n");
            }
            if status.confirm_account {
                text.push_str("y 确认合并到新账号    n / Esc 取消\n");
            } else if status.authorizing {
                text.push_str("请在本机浏览器完成授权；未自动打开时复制下面的链接。\n");
                if let Some(url) = &status.url {
                    text.push_str(url);
                }
            } else {
                text.push_str("l 登录 / 更换账号\np 拉取并合并云端\nc 查看冲突\no 退出登录\n\n本机配置修改自动推送；不定时查询云端。\n登录或启动后，按 p 获取其他电脑的修改。\n不上传 SSH 私钥或本机路径，令牌只存本机。");
            }
            frame.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: false })
                    .scroll((*offset, 0)),
                chunks[1],
            );
        }
        Screen::BindConfig { profile, input } => {
            let layout =
                Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(chunks[1]);
            draw_input(frame, input, "本机 SSH 配置文件路径", true, layout[0]);
            frame.render_widget(Paragraph::new(format!("连接：{}\n核对所需 Host 别名后绑定同来源的连接。\n不会复制、修改或执行 SSH 配置。", clean_text(&profile.name))).wrap(Wrap { trim: false }), layout[1]);
        }
        Screen::List => draw_list(frame, app, chunks[1]),
        Screen::FirstConnect { profile, choice } => {
            if !app.setup_confirmation_visible {
                frame.render_widget(
                    Paragraph::new(
                        "请放大终端到至少 60 列 × 20 行，查看首次登录说明。\nEsc 取消。",
                    )
                    .wrap(Wrap { trim: false }),
                    chunks[1],
                );
            } else {
                let layout =
                    Layout::vertical([Constraint::Min(6), Constraint::Length(3)]).split(chunks[1]);
                let text = format!(
                    "连接：{}  {}\n\n推荐先配置密钥，成功后本机后续连接免输服务器密码。\n已有密钥会复用；需要时追加服务器公钥。\n新生成的私钥仅保存在本机，无额外口令。\n适用 Linux/macOS；临时登录不会记住密码。",
                    clean_text(&profile.name),
                    clean_text(&profile.address())
                );
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), layout[0]);
                let table = Table::new(
                    [
                        Row::new(["配置密钥并连接（推荐）"]),
                        Row::new(["仅本次密码 / 原方式登录"]),
                        Row::new(["取消"]),
                    ],
                    [Constraint::Min(1)],
                )
                .row_highlight_style(selected())
                .highlight_symbol("> ");
                let mut state = TableState::default().with_selected(Some(*choice));
                frame.render_stateful_widget(table, layout[1], &mut state);
            }
        }
        Screen::KeyPicker {
            candidates,
            cursor,
            purpose,
        } => {
            let mut rows: Vec<_> = candidates
                .iter()
                .map(|c| {
                    Row::new(vec![
                        clean_text(&c.path.display().to_string()),
                        c.source.clone(),
                    ])
                })
                .collect();
            rows.push(Row::new(vec![
                match purpose {
                    KeyPurpose::Form(_) => "不指定路径，自动检测 / 使用 SSH 默认行为",
                    KeyPurpose::Setup(_) | KeyPurpose::SetupAndConnect(_) => {
                        "检查默认登录，必要时生成专用新密钥"
                    }
                }
                .to_owned(),
                String::new(),
            ]));
            let table = Table::new(rows, [Constraint::Min(20), Constraint::Length(26)])
                .header(Row::new(["密钥路径", "来源"]).style(muted()))
                .row_highlight_style(selected())
                .highlight_symbol("> ");
            let mut state = TableState::default().with_selected(Some(*cursor));
            frame.render_stateful_widget(table, chunks[1], &mut state);
        }
        Screen::Form(form) => draw_form(frame, form, chunks[1]),
        Screen::Confirm {
            action: Confirmation::SetupKey(_),
            ..
        } if !app.setup_confirmation_visible => {
            frame.render_widget(
                Paragraph::new("请放大终端到至少 60 列 × 20 行，以查看密钥设置说明。\nEsc 取消。 ")
                    .wrap(Wrap { trim: false }),
                chunks[1],
            );
        }
        Screen::Confirm { action, yes } => draw_confirmation(frame, action, *yes, chunks[1]),
        Screen::ImportPath(input) => {
            let layout =
                Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(chunks[1]);
            draw_input(frame, input, "配置文件路径", true, layout[0]);
            frame.render_widget(Paragraph::new("Enter 预览可导入的别名；支持 ~ 和带空格的路径。\n只保存配置引用，原文件不会修改。").style(muted()).wrap(Wrap { trim: false }), layout[1]);
        }
        Screen::Import {
            preview,
            checked,
            cursor,
        } => {
            if preview.profiles.is_empty() {
                let mut text = "没有可导入的明确 Host 别名；通配规则不会生成连接。\n".to_owned();
                text.push_str(
                    &preview
                        .warnings
                        .iter()
                        .map(|s| clean_text(s))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), chunks[1]);
            } else {
                let rows = preview
                    .profiles
                    .iter()
                    .zip(checked)
                    .map(|(profile, checked)| {
                        Row::new(vec![
                            if *checked {
                                "[x]".to_owned()
                            } else {
                                "[ ]".to_owned()
                            },
                            clean_text(&profile.name),
                        ])
                    });
                let table = Table::new(rows, [Constraint::Length(4), Constraint::Min(10)])
                    .header(Row::new(["", "SSH 别名"]).style(muted()))
                    .row_highlight_style(selected())
                    .highlight_symbol("> ")
                    .block(
                        Block::default()
                            .borders(Borders::BOTTOM)
                            .title_bottom(format!(
                                "已选 {} / {}   扫描提示 {} 条",
                                checked.iter().filter(|c| **c).count(),
                                checked.len(),
                                preview.warnings.len()
                            )),
                    );
                let mut state = TableState::default().with_selected(Some(*cursor));
                frame.render_stateful_widget(table, chunks[1], &mut state);
            }
        }
        Screen::Report { lines, offset, .. } => {
            let text = if lines.is_empty() {
                "操作完成。按 Enter 返回。".into()
            } else {
                lines
                    .iter()
                    .skip(*offset)
                    .map(|s| clean_text(s))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), chunks[1]);
        }
        Screen::Help => {
            let text = "↑/↓ 或 j/k       选择连接\nPageUp/PageDown   翻页；Home/End 跳到首尾\nEnter            连接\n/                搜索名称、地址、用户、标签\n搜索中 Enter     保留筛选并返回列表\nEsc              清空筛选 / 取消当前操作\na / e / d        新增 / 编辑 / 删除\ns                设置本机密钥登录\ni                导入 SSH 配置\ng                Google Drive 登录 / 同步管理\np                手动拉取并合并云端\nq / Ctrl+C       退出（只在主列表生效）\n\n表单：Tab / Shift+Tab 切换；Enter 下一项或确认\n输入框：←/→、Home/End；Ctrl+U 清除光标前内容\nSSH 会话：全部按键交给 ssh，输入 exit 结束会话";
            frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), chunks[1]);
        }
    }
    frame.render_widget(
        Paragraph::new(clean_text(&app.message))
            .style(Style::default().fg(if app.error { Color::Red } else { Color::Green }))
            .wrap(Wrap { trim: false }),
        chunks[2],
    );
    let footer = match &app.screen {
        Screen::Sync { .. } => "↑/↓ 滚动  Esc 返回 / 取消授权",
        Screen::BindConfig { .. } => "Enter 核对并绑定  Esc 取消",
        Screen::List if app.searching => "输入搜索词  Enter 保留筛选  Esc 清空",
        Screen::FirstConnect { .. } => "↑/↓ 或 Tab 选择  Enter 继续  Esc 取消",
        Screen::List => {
            "Enter 连接  / 搜索  a 新增  e 编辑  d 删除\ns 设置密钥  i 导入  p 拉取云端  ? 帮助  q 退出"
        }
        Screen::Form(form) if form.imported() => "Tab 切换  F2 绑定配置  Enter 确认  Esc 取消",
        Screen::Form(_) => "Tab 切换  F2 选择已有密钥  Enter 确认  Esc 取消",
        Screen::KeyPicker { .. } => "↑/↓ 选择  Enter 使用  Esc 返回；不会修改已有密钥文件",
        Screen::Import { .. } => "Space 勾选  a 全选/取消全选  Enter 导入  Esc 取消",
        Screen::Confirm { .. } => "Tab / ← / → 切换  Enter 确认  Esc 返回",
        Screen::Report { .. } => "↑/↓ 翻看详情  Enter / Esc 返回",
        Screen::Help => "Esc / q 关闭帮助",
        Screen::ImportPath(_) => "Enter 预览  Esc 取消",
    };
    frame.render_widget(
        Paragraph::new(footer)
            .style(muted())
            .wrap(Wrap { trim: false }),
        chunks[3],
    );
    let label = if app.sync_status.logged_in {
        format!("[g] Google Drive：{}", app.sync_status.message)
    } else if app.sync_status.authorizing {
        "[g] Google Drive：等待授权".into()
    } else {
        "[g] 登录 Google Drive 以同步".into()
    };
    frame.render_widget(Paragraph::new(clean_text(&label)).style(muted()), chunks[4]);
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(area);
    let (text, cursor) = app
        .query
        .visible(chunks[0].width.saturating_sub(6) as usize);
    frame.render_widget(
        Paragraph::new(format!("搜索: {text}")).style(if app.searching {
            accent()
        } else {
            muted()
        }),
        chunks[0],
    );
    if app.searching {
        frame.set_cursor_position((chunks[0].x + 6 + cursor, chunks[0].y));
    }
    let profiles = app.filtered();
    if profiles.is_empty() {
        let message = if app.profiles.is_empty() {
            "还没有连接。按 a 新增，或按 i 导入已有 SSH 配置。"
        } else {
            "没有匹配的连接。按 / 修改搜索，或按 Esc 清空。"
        };
        frame.render_widget(
            Paragraph::new(message).wrap(Wrap { trim: false }),
            chunks[1],
        );
        return;
    }
    let mut headers = vec!["名称", "地址"];
    let mut widths = vec![Constraint::Percentage(25), Constraint::Min(12)];
    let user = area.width >= 60;
    let tags = area.width >= 80;
    let recent = area.width >= 100;
    if user {
        headers.push("用户");
        widths.push(Constraint::Length(12));
    }
    if tags {
        headers.push("标签");
        widths.push(Constraint::Length(12));
    }
    if recent {
        headers.push("最近使用");
        widths.push(Constraint::Length(12));
    }
    let rows = profiles.iter().map(|p| {
        let mut cells = vec![
            Cell::from(clean_text(&p.name)),
            Cell::from(clean_text(&p.address())),
        ];
        if user {
            cells.push(Cell::from(clean_text(p.user())));
        }
        if tags {
            cells.push(Cell::from(clean_text(&p.tags.join(", "))));
        }
        if recent {
            cells.push(Cell::from(last_used_label(p.last_used, now())));
        }
        Row::new(cells)
    });
    let table = Table::new(rows, widths)
        .header(Row::new(headers).style(muted()))
        .column_spacing(2)
        .row_highlight_style(selected())
        .highlight_symbol("> ");
    let mut state = TableState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(table, chunks[1], &mut state);
}

fn draw_form(frame: &mut Frame, form: &Form, area: Rect) {
    let note = if form.imported() {
        let Connection::SshConfig { path, alias } = &form.original.as_ref().unwrap().connection
        else {
            unreachable!()
        };
        format!(
            "引用 {} → {}\n连接参数在原文件维护，本工具只编辑名称和标签。",
            clean_text(&path.display().to_string()),
            clean_text(alias)
        )
    } else {
        if form.discovery_note.is_empty() {
            "私钥路径留空时自动检测；F2 选择本机已有密钥。".into()
        } else {
            format!(
                "{}\nF2 选择已有密钥；可手动修改自动填入的路径。",
                clean_text(&form.discovery_note)
            )
        }
    };
    let chunks = Layout::vertical([
        Constraint::Length(if form.imported() || !form.discovery_note.is_empty() {
            2
        } else {
            1
        }),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(area);
    frame.render_widget(Paragraph::new(note).style(muted()), chunks[0]);
    let count = (chunks[1].height / 3).max(1) as usize;
    let focus_field = form.focus.min(form.fields.len().saturating_sub(1));
    let start = focus_field.saturating_sub(count.saturating_sub(1));
    for (index, field) in form.fields.iter().enumerate().skip(start).take(count) {
        let y = chunks[1].y + ((index - start) * 3) as u16;
        let rect = Rect::new(
            chunks[1].x,
            y,
            chunks[1].width,
            3.min(chunks[1].bottom().saturating_sub(y)),
        );
        draw_input(frame, &field.input, field.label, form.focus == index, rect);
    }
    let labels = if form.connect_default {
        ["保存并连接", "仅保存", "取消"]
    } else {
        ["保存", "保存并连接", "取消"]
    };
    let buttons = Line::from(
        labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                Span::styled(
                    format!(" {label} "),
                    if form.focus == form.fields.len() + index {
                        selected()
                    } else {
                        muted()
                    },
                )
            })
            .collect::<Vec<_>>(),
    );
    frame.render_widget(Paragraph::new(buttons), chunks[2]);
}

fn draw_input(frame: &mut Frame, input: &Input, label: &str, focus: bool, area: Rect) {
    let block = Block::bordered()
        .title(label)
        .border_style(if focus { accent() } else { muted() });
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (text, cursor) = input.visible(inner.width as usize);
    frame.render_widget(Paragraph::new(text), inner);
    if focus && inner.width > 0 && inner.height > 0 {
        frame.set_cursor_position((inner.x + cursor.min(inner.width - 1), inner.y));
    }
}

fn draw_confirmation(frame: &mut Frame, action: &Confirmation, yes: bool, area: Rect) {
    let text = match action {
        Confirmation::Delete(profile) => format!(
            "删除「{}」？\n{}\n\n仅删除本工具记录，保留 SSH 配置和私钥。",
            clean_text(&profile.name),
            clean_text(&profile.address())
        ),
        Confirmation::Discard(_) => "还有未保存的修改，是否放弃？".into(),
        Confirmation::SetupKey(profile) if profile.identity_file().is_some() => format!(
            "为「{}」复用已有密钥？\n目标：{}\n\n已有密钥：{}\n先验证已有授权；可以登录就直接保存。\n仅在需要时追加公钥到远端 authorized_keys。\n适用 Linux/macOS；已有密钥的权限和口令保持原样。",
            clean_text(&profile.name),
            clean_text(&profile.address()),
            clean_text(
                &profile
                    .identity_file()
                    .unwrap()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            )
        ),
        Confirmation::SetupKey(profile) => format!(
            "为「{}」设置密钥登录？\n目标：{}\n\n生成本机无口令私钥，权限仅限当前用户。\n公钥追加到远端 ~/.ssh/authorized_keys，保留已有公钥。\n适用 Linux/macOS；首次可能需要服务器密码。\n私钥仅留在本机，验证成功后保存连接设置。",
            clean_text(&profile.name),
            clean_text(&profile.address())
        ),
    };
    let chunks = Layout::vertical([Constraint::Min(2), Constraint::Length(2)]).split(area);
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), chunks[0]);
    let cancel = if matches!(action, Confirmation::Discard(_)) {
        " 继续编辑 "
    } else {
        " 取消 "
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(cancel, if !yes { selected() } else { muted() }),
            Span::raw("   "),
            Span::styled(" 确认 ", if yes { selected() } else { muted() }),
        ])),
        chunks[1],
    );
}

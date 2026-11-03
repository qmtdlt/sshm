pub mod app;
mod form;
mod input;
mod render;

#[cfg(test)]
mod tests;

use crate::{keys, model::clean_text, ssh, store::Store};
use anyhow::{Context, Result, ensure};
use app::{Action, App, Entry};
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::{self, IsTerminal, Write};

struct TerminalGuard {
    active: bool,
}
impl TerminalGuard {
    fn enter(&mut self) -> Result<()> {
        enable_raw_mode()?;
        self.active = true;
        execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
        Ok(())
    }
    fn leave(&mut self) -> Result<()> {
        if self.active {
            let restore = execute!(
                io::stdout(),
                DisableBracketedPaste,
                LeaveAlternateScreen,
                crossterm::cursor::Show
            );
            let raw = disable_raw_mode();
            self.active = false;
            restore?;
            raw?;
        }
        Ok(())
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.leave();
    }
}

pub fn run(store: Store, entry: Entry) -> Result<i32> {
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "此命令需要交互式终端；在 PowerShell、Terminal 等终端中运行，查看列表可用 sshm list"
    );
    let updates = matches!(&entry, Entry::List).then(crate::update::background);
    let mut pending_update = None;
    let mut app = App::new(store, entry)?;
    let sync = crate::sync::worker::Worker::start(app.store.clone());
    let mut next_reload = std::time::Instant::now();
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        previous_hook(info);
    }));
    let mut guard = TerminalGuard { active: false };
    guard.enter().context("无法初始化终端")?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    loop {
        if let Some(updates) = &updates {
            while let Ok(message) = updates.try_recv() {
                pending_update = Some(message);
            }
        }
        if matches!(app.screen, app::Screen::List)
            && app.message.is_empty()
            && let Some(message) = pending_update.take()
        {
            app.notice(message, false);
        }
        while let Ok(status) = sync.events.try_recv() {
            app.sync_status = status;
        }
        if std::time::Instant::now() >= next_reload {
            let id = app.selected_profile().map(|p| p.id);
            if let Err(error) = app.reload(id) {
                app.notice(format!("刷新失败：{error:#}"), true);
            }
            next_reload = std::time::Instant::now() + std::time::Duration::from_secs(1);
        }
        terminal.draw(|frame| render::draw(frame, &mut app))?;
        if !event::poll(std::time::Duration::from_millis(100))? {
            continue;
        }
        let action = app.event(event::read()?);
        let setup_then_connect = matches!(&action, Action::SetupAndConnect(_));
        match action {
            Action::Sync(command) => {
                if matches!(command, crate::sync::worker::Command::Login) {
                    app.sync_status.authorizing = true;
                }
                sync.send(command);
            }
            Action::CancelSync => sync.cancel_login(),
            Action::SetupKey(profile) => {
                guard.leave()?;
                let result = keys::setup(&profile, &app.store);
                let (message, error) = match result {
                    Ok(_) => (
                        "密钥登录已配置，选中连接后按 Enter 即可登录".to_owned(),
                        false,
                    ),
                    Err(error) => (format!("{error:#}"), true),
                };
                println!("\n{}", clean_text(&message));
                if app.standalone {
                    return Ok(i32::from(error));
                }
                print!("按 Enter 返回连接列表…");
                io::stdout().flush()?;
                let mut line = String::new();
                io::stdin().read_line(&mut line)?;
                app.notice(message, error);
                if let Err(error) = app.reload(Some(profile.id)) {
                    app.notice(format!("刷新失败：{error:#}"), true);
                }
                guard.enter()?;
                terminal.clear()?;
            }
            Action::None => {}
            Action::Quit(code) => {
                guard.leave()?;
                if app.standalone && !app.message.is_empty() {
                    println!("{}", clean_text(&app.message));
                }
                return Ok(code);
            }
            Action::Connect(profile) | Action::SetupAndConnect(profile) => {
                guard.leave()?;
                let result = if setup_then_connect {
                    keys::setup(&profile, &app.store).and_then(|ready| ssh::run(&ready, &app.store))
                } else {
                    ssh::run(&profile, &app.store)
                };
                if app.standalone {
                    return result;
                }
                match result {
                    Ok(0) => app.notice(format!("会话已结束：{}", profile.name), false),
                    other => {
                        let message = match other {
                            Ok(code) => format!("SSH 已结束，退出码 {code}"),
                            Err(error) => format!("{error:#}"),
                        };
                        eprintln!("\n{}", clean_text(&message));
                        print!("按 Enter 返回连接列表…");
                        io::stdout().flush()?;
                        let mut line = String::new();
                        io::stdin().read_line(&mut line)?;
                        app.notice(message, true);
                    }
                }
                if let Err(error) = app.reload(Some(profile.id)) {
                    app.notice(format!("刷新失败：{error:#}"), true);
                }
                guard.enter()?;
                terminal.clear()?;
            }
        }
    }
}

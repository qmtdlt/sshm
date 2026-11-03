use super::{form::Form, input::Input};
use crate::discovery::{Candidate, Discovery};
use crate::{import, model::Profile, store::Store};
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::path::PathBuf;
use uuid::Uuid;

pub enum Entry {
    List,
    Add(bool),
    Edit(Profile),
    Remove(Profile),
    SetupKey(Profile),
    Connect(Profile),
    Import(PathBuf),
}

pub enum Screen {
    List,
    Sync {
        offset: u16,
    },
    BindConfig {
        profile: Profile,
        input: Input,
    },
    Form(Form),
    Confirm {
        action: Confirmation,
        yes: bool,
    },
    ImportPath(Input),
    Import {
        preview: import::Preview,
        checked: Vec<bool>,
        cursor: usize,
    },
    Report {
        title: String,
        lines: Vec<String>,
        offset: usize,
    },
    Help,
    FirstConnect {
        profile: Profile,
        choice: usize,
    },
    KeyPicker {
        candidates: Vec<Candidate>,
        cursor: usize,
        purpose: KeyPurpose,
    },
}

pub enum KeyPurpose {
    Form(Form),
    Setup(Profile),
    SetupAndConnect(Profile),
}

pub enum Confirmation {
    Delete(Profile),
    Discard(Form),
    SetupKey(Profile),
}
pub enum Action {
    None,
    Sync(crate::sync::worker::Command),
    CancelSync,
    Quit(i32),
    Connect(Profile),
    SetupKey(Profile),
    SetupAndConnect(Profile),
}

pub struct App {
    pub store: Store,
    pub profiles: Vec<Profile>,
    pub screen: Screen,
    pub query: Input,
    pub searching: bool,
    pub selected: usize,
    pub standalone: bool,
    pub message: String,
    pub error: bool,
    pub page_size: usize,
    pub setup_confirmation_visible: bool,
    pub discovery: Discovery,
    pub sync_status: crate::sync::worker::Status,
}

impl App {
    pub fn new(store: Store, entry: Entry) -> Result<Self> {
        let profiles = store.load()?;
        let discovery = Discovery::standard()?;
        let standalone = !matches!(entry, Entry::List);
        let screen = match entry {
            Entry::List => Screen::List,
            Entry::Add(connect) => Screen::Form(Form::new(None, connect)),
            Entry::Edit(profile) if profile.needs_binding() => Screen::BindConfig {
                profile,
                input: Input::default(),
            },
            Entry::Edit(profile) => {
                let mut form = Form::new(Some(profile), false);
                form.detect_identity(&discovery);
                Screen::Form(form)
            }
            Entry::Remove(profile) => Screen::Confirm {
                action: Confirmation::Delete(profile),
                yes: false,
            },
            Entry::SetupKey(profile) | Entry::Connect(profile) if profile.needs_binding() => {
                Screen::BindConfig {
                    profile,
                    input: Input::default(),
                }
            }
            Entry::SetupKey(profile) => setup_screen(&discovery, profile),
            Entry::Connect(profile) => Screen::FirstConnect { profile, choice: 0 },
            Entry::Import(path) => {
                let preview = import::preview(&path)?;
                let checked = vec![false; preview.profiles.len()];
                Screen::Import {
                    preview,
                    checked,
                    cursor: 0,
                }
            }
        };
        Ok(Self {
            store,
            profiles,
            screen,
            query: Input::default(),
            searching: false,
            selected: 0,
            standalone,
            message: String::new(),
            error: false,
            page_size: 10,
            setup_confirmation_visible: false,
            discovery,
            sync_status: Default::default(),
        })
    }

    pub fn filtered(&self) -> Vec<&Profile> {
        self.profiles
            .iter()
            .filter(|p| p.matches(&self.query.value))
            .collect()
    }
    pub fn selected_profile(&self) -> Option<Profile> {
        self.filtered().get(self.selected).map(|p| (*p).clone())
    }
    pub fn notice(&mut self, message: impl Into<String>, error: bool) {
        self.message = message.into();
        self.error = error;
    }

    pub fn reload(&mut self, id: Option<Uuid>) -> Result<()> {
        self.profiles = self.store.load()?;
        let filtered = self.filtered();
        let selected = id
            .and_then(|id| filtered.iter().position(|p| p.id == id))
            .unwrap_or(self.selected)
            .min(filtered.len().saturating_sub(1));
        self.selected = selected;
        Ok(())
    }

    fn finish(&mut self, message: impl Into<String>) -> Action {
        self.notice(message, false);
        self.screen = Screen::List;
        if self.standalone {
            Action::Quit(0)
        } else {
            Action::None
        }
    }

    fn cancel_form(&mut self, form: Form) -> Action {
        if form.dirty() {
            self.screen = Screen::Confirm {
                action: Confirmation::Discard(form),
                yes: false,
            };
            Action::None
        } else {
            self.finish("已取消")
        }
    }

    fn connect(&mut self, profile: Profile) -> Action {
        if profile.needs_binding() {
            self.screen = Screen::BindConfig {
                profile,
                input: Input::default(),
            };
            return Action::None;
        }
        if profile.key_login_configured {
            Action::Connect(profile)
        } else {
            self.screen = Screen::FirstConnect { profile, choice: 0 };
            self.message.clear();
            Action::None
        }
    }

    pub fn event(&mut self, event: Event) -> Action {
        if let Event::Paste(text) = event {
            match &mut self.screen {
                Screen::List if self.searching => {
                    self.query.insert(&text);
                    self.selected = 0;
                }
                Screen::Form(form) if form.focus < form.fields.len() => {
                    form.fields[form.focus].input.insert(&text)
                }
                Screen::ImportPath(input) | Screen::BindConfig { input, .. } => input.insert(&text),
                _ => {}
            }
            return Action::None;
        }
        let Event::Key(key) = event else {
            return Action::None;
        };
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        let cancel = key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL));
        let screen = std::mem::replace(&mut self.screen, Screen::List);
        match screen {
            Screen::Sync { mut offset } => {
                use crate::sync::worker::Command;
                if cancel {
                    if self.sync_status.authorizing {
                        return Action::CancelSync;
                    }
                    if self.sync_status.confirm_account {
                        return Action::Sync(Command::ConfirmAccount(false));
                    }
                    return Action::None;
                }
                match key.code {
                    KeyCode::Down => offset = offset.saturating_add(1),
                    KeyCode::Up => offset = offset.saturating_sub(1),
                    _ => {}
                }
                self.screen = Screen::Sync { offset };
                if self.sync_status.authorizing {
                    return Action::None;
                }
                if self.sync_status.confirm_account {
                    return match key.code {
                        KeyCode::Char('y') => Action::Sync(Command::ConfirmAccount(true)),
                        KeyCode::Char('n') => Action::Sync(Command::ConfirmAccount(false)),
                        _ => Action::None,
                    };
                }
                match key.code {
                    KeyCode::Char('l') => return Action::Sync(Command::Login),
                    KeyCode::Char('p') if self.sync_status.logged_in => {
                        return Action::Sync(Command::Now);
                    }
                    KeyCode::Char('o') => return Action::Sync(Command::Logout),
                    KeyCode::Char('c') => {
                        let _ = self.reload(None);
                        self.screen = Screen::Report {
                            title: "同步冲突：可返回列表编辑或删除多余版本".into(),
                            lines: self
                                .profiles
                                .iter()
                                .filter(|p| p.sync_conflict)
                                .map(|p| format!("{}  {}", p.name, p.address()))
                                .collect(),
                            offset: 0,
                        };
                        return Action::None;
                    }
                    _ => {}
                }
                self.screen = Screen::Sync { offset };
                Action::None
            }
            Screen::BindConfig { profile, mut input } => {
                if cancel {
                    return self.finish("已取消绑定");
                }
                if key.code == KeyCode::Enter {
                    match crate::model::expand_path(&input.value)
                        .and_then(|path| self.store.bind_source(&profile, &path))
                    {
                        Ok(()) => {
                            let _ = self.reload(Some(profile.id));
                            return self.finish("已绑定本机 SSH 配置；现在可以连接或编辑");
                        }
                        Err(e) => self.notice(format!("绑定失败：{e:#}"), true),
                    }
                } else {
                    input.key(key);
                }
                self.screen = Screen::BindConfig { profile, input };
                Action::None
            }
            Screen::FirstConnect {
                profile,
                mut choice,
            } => {
                if cancel || (key.code == KeyCode::Enter && choice == 2) {
                    return self.finish("已取消连接");
                }
                if key.code == KeyCode::Enter {
                    if choice == 1 {
                        return Action::Connect(profile);
                    }
                    if !self.setup_confirmation_visible {
                        self.notice("请将终端放大到至少 60 列 × 20 行，查看登录设置说明", true);
                        self.screen = Screen::FirstConnect { profile, choice };
                        return Action::None;
                    }
                    let found = self.discovery.find(&profile);
                    if let Some(candidate) = found.automatic {
                        let mut profile = profile;
                        profile.set_identity_file(candidate.path);
                        return Action::SetupAndConnect(profile);
                    }
                    if found.target_matched {
                        self.screen = Screen::KeyPicker {
                            candidates: found.candidates,
                            cursor: 0,
                            purpose: KeyPurpose::SetupAndConnect(profile),
                        };
                        return Action::None;
                    }
                    // Unrelated local keys are not evidence of authorization
                    // for this target. Check native defaults/agent before
                    // generating a dedicated key, without asking for paths.
                    return Action::SetupAndConnect(profile);
                }
                match key.code {
                    KeyCode::Tab | KeyCode::Down | KeyCode::Right => choice = (choice + 1) % 3,
                    KeyCode::BackTab | KeyCode::Up | KeyCode::Left => choice = (choice + 2) % 3,
                    _ => {}
                }
                self.screen = Screen::FirstConnect { profile, choice };
                Action::None
            }
            Screen::List => self.list_key(key),
            Screen::Help => {
                if !cancel && key.code != KeyCode::Char('q') {
                    self.screen = Screen::Help;
                }
                Action::None
            }
            Screen::Form(mut form) => {
                if cancel {
                    return self.cancel_form(form);
                }
                if key.code == KeyCode::F(2) && form.imported() {
                    let profile = form.original.as_ref().unwrap().clone();
                    let path = match &profile.connection {
                        crate::model::Connection::SshConfig { path, .. } => {
                            path.display().to_string()
                        }
                        _ => String::new(),
                    };
                    if form.dirty() {
                        self.notice("请先保存或取消表单修改，再重新绑定本机配置", true);
                        self.screen = Screen::Form(form);
                    } else {
                        self.screen = Screen::BindConfig {
                            profile,
                            input: Input::new(path),
                        };
                    }
                    return Action::None;
                }
                if key.code == KeyCode::F(2) && !form.imported() {
                    let found = form.key_candidates(&self.discovery);
                    self.screen = Screen::KeyPicker {
                        candidates: found.candidates,
                        cursor: 0,
                        purpose: KeyPurpose::Form(form),
                    };
                    self.notice(found.note, false);
                    return Action::None;
                }
                let previous_focus = form.focus;
                if key.code == KeyCode::Tab || key.code == KeyCode::BackTab {
                    form.focus_next(
                        key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT),
                    );
                } else if key.code == KeyCode::Enter {
                    if form.focus < form.fields.len() {
                        form.focus_next(false);
                    } else {
                        let button = form.focus - form.fields.len();
                        if button == 2 {
                            return self.cancel_form(form);
                        }
                        match form.profile(&self.profiles) {
                            Err((field, message)) => {
                                form.focus = field;
                                self.notice(message, true);
                            }
                            Ok(profile) => match self
                                .store
                                .save(profile, form.original.as_ref().map(|p| p.revision))
                            {
                                Ok(saved) => {
                                    let connect = if form.connect_default {
                                        button == 0
                                    } else {
                                        button == 1
                                    };
                                    if let Err(e) = self.reload(Some(saved.id)) {
                                        self.notice(format!("已保存，但刷新失败：{e:#}"), true);
                                    }
                                    if connect {
                                        return self.connect(saved);
                                    }
                                    return self.finish(format!("已保存：{}", saved.name));
                                }
                                Err(e) => self.notice(format!("{e:#}"), true),
                            },
                        }
                    }
                } else if form.focus < form.fields.len() {
                    form.fields[form.focus].input.key(key);
                } else if matches!(key.code, KeyCode::Left | KeyCode::Right) {
                    let count = form.fields.len();
                    let button = form.focus - count;
                    form.focus =
                        count + (button + if key.code == KeyCode::Left { 2 } else { 1 }) % 3;
                }
                if form.focus != previous_focus && (1..=3).contains(&previous_focus) {
                    form.detect_identity(&self.discovery);
                }
                self.screen = Screen::Form(form);
                Action::None
            }
            Screen::Confirm { action, mut yes } => {
                if cancel || (key.code == KeyCode::Enter && !yes) {
                    match action {
                        Confirmation::Discard(form) => self.screen = Screen::Form(form),
                        Confirmation::Delete(_) => return self.finish("已取消删除"),
                        Confirmation::SetupKey(_) => return self.finish("已取消密钥设置"),
                    }
                } else if key.code == KeyCode::Enter && yes {
                    match action {
                        Confirmation::Discard(_) => return self.finish("已放弃修改"),
                        Confirmation::SetupKey(profile) => {
                            if self.setup_confirmation_visible {
                                return Action::SetupKey(profile);
                            }
                            self.notice("请将终端放大到至少 60 列 × 20 行，查看完整设置说明", true);
                            self.screen = Screen::Confirm {
                                action: Confirmation::SetupKey(profile),
                                yes: false,
                            };
                        }
                        Confirmation::Delete(profile) => match self.store.remove(&profile) {
                            Ok(()) => {
                                if let Err(e) = self.reload(None) {
                                    self.notice(format!("{e:#}"), true);
                                }
                                return self.finish(format!("已删除：{}", profile.name));
                            }
                            Err(e) => {
                                self.notice(format!("{e:#}"), true);
                                self.screen = Screen::Confirm {
                                    action: Confirmation::Delete(profile),
                                    yes: false,
                                };
                            }
                        },
                    }
                } else {
                    if matches!(
                        key.code,
                        KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right
                    ) {
                        yes = !yes;
                    }
                    self.screen = Screen::Confirm { action, yes };
                }
                Action::None
            }
            Screen::ImportPath(mut input) => {
                if cancel {
                    return self.finish("已取消导入");
                }
                if key.code == KeyCode::Enter {
                    match crate::model::expand_path(&input.value).and_then(|p| import::preview(&p))
                    {
                        Ok(preview) => {
                            let checked = vec![false; preview.profiles.len()];
                            self.screen = Screen::Import {
                                preview,
                                checked,
                                cursor: 0,
                            };
                            self.notice(
                                "Space 勾选，a 全选，Enter 导入；原 SSH 配置不会修改",
                                false,
                            );
                            return Action::None;
                        }
                        Err(e) => self.notice(format!("{e:#}"), true),
                    }
                } else {
                    input.key(key);
                }
                self.screen = Screen::ImportPath(input);
                Action::None
            }
            Screen::Import {
                preview,
                mut checked,
                mut cursor,
            } => {
                if cancel {
                    return self.finish("已取消导入");
                }
                match key.code {
                    KeyCode::Char(' ') if !checked.is_empty() => checked[cursor] = !checked[cursor],
                    KeyCode::Char('a') => {
                        let select = !checked.iter().all(|c| *c);
                        checked.fill(select);
                    }
                    KeyCode::Enter => {
                        let candidates = preview
                            .profiles
                            .iter()
                            .zip(&checked)
                            .filter(|(_, checked)| **checked)
                            .map(|(p, _)| p.clone())
                            .collect();
                        match self.store.import(candidates) {
                            Ok(result) => {
                                let title = format!(
                                    "已导入 {} 个，跳过 {} 个",
                                    result.added,
                                    result.skipped.len()
                                );
                                let mut lines = result.skipped;
                                lines.extend(preview.warnings);
                                if let Err(e) = self.reload(None) {
                                    lines.push(format!("刷新失败：{e:#}"));
                                }
                                self.notice(title.clone(), false);
                                self.screen = Screen::Report {
                                    title,
                                    lines,
                                    offset: 0,
                                };
                                return Action::None;
                            }
                            Err(e) => self.notice(format!("{e:#}"), true),
                        }
                    }
                    _ => cursor = move_cursor(cursor, checked.len(), self.page_size, key.code),
                }
                self.screen = Screen::Import {
                    preview,
                    checked,
                    cursor,
                };
                Action::None
            }
            Screen::Report {
                title,
                lines,
                mut offset,
            } => {
                if cancel || matches!(key.code, KeyCode::Enter | KeyCode::Char('q')) {
                    return self.finish(title);
                }
                offset = move_cursor(offset, lines.len(), self.page_size, key.code);
                self.screen = Screen::Report {
                    title,
                    lines,
                    offset,
                };
                Action::None
            }
            Screen::KeyPicker {
                candidates,
                mut cursor,
                purpose,
            } => {
                if cancel {
                    match purpose {
                        KeyPurpose::Form(form) => self.screen = Screen::Form(form),
                        KeyPurpose::Setup(_) => return self.finish("已取消密钥设置"),
                        KeyPurpose::SetupAndConnect(profile) => {
                            self.screen = Screen::FirstConnect { profile, choice: 0 }
                        }
                    }
                } else if key.code == KeyCode::Enter {
                    let path = candidates.get(cursor).map(|c| c.path.clone());
                    match purpose {
                        KeyPurpose::Form(mut form) => {
                            form.choose_identity(path);
                            self.screen = Screen::Form(form);
                        }
                        KeyPurpose::Setup(mut profile) => {
                            if let Some(path) = path {
                                profile.set_identity_file(path);
                            }
                            self.screen = Screen::Confirm {
                                action: Confirmation::SetupKey(profile),
                                yes: false,
                            };
                        }
                        KeyPurpose::SetupAndConnect(mut profile) => {
                            if let Some(path) = path {
                                profile.set_identity_file(path);
                            }
                            return Action::SetupAndConnect(profile);
                        }
                    }
                    self.message.clear();
                } else {
                    cursor = move_cursor(cursor, candidates.len() + 1, self.page_size, key.code);
                    self.screen = Screen::KeyPicker {
                        candidates,
                        cursor,
                        purpose,
                    };
                }
                Action::None
            }
        }
    }

    fn list_key(&mut self, key: KeyEvent) -> Action {
        if self.searching {
            match key.code {
                KeyCode::Esc => {
                    self.query = Input::default();
                    self.searching = false;
                    self.selected = 0;
                }
                KeyCode::Enter => self.searching = false,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.searching = false;
                }
                _ => {
                    self.query.key(key);
                    self.selected = 0;
                }
            }
            return Action::None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::Quit(0);
        }
        if matches!(
            key.code,
            KeyCode::Enter | KeyCode::Char('a' | 'e' | 'd' | 's')
        ) {
            let id = self.selected_profile().map(|p| p.id);
            if let Err(error) = self.reload(id) {
                self.notice(format!("刷新失败：{error:#}"), true);
                return Action::None;
            }
        }
        match key.code {
            KeyCode::Char('g') => {
                self.screen = Screen::Sync { offset: 0 };
                if !self.sync_status.logged_in
                    && !self.sync_status.authorizing
                    && !self.sync_status.confirm_account
                {
                    return Action::Sync(crate::sync::worker::Command::Login);
                }
            }
            KeyCode::Char('p') => {
                if self.sync_status.logged_in {
                    return Action::Sync(crate::sync::worker::Command::Now);
                }
                self.notice("请先按 g 登录 Google Drive，再按 p 拉取云端", true);
            }
            KeyCode::Char('q') => return Action::Quit(0),
            KeyCode::Char('s') => {
                if let Some(profile) = self.selected_profile() {
                    self.screen = if profile.needs_binding() {
                        Screen::BindConfig {
                            profile,
                            input: Input::default(),
                        }
                    } else {
                        setup_screen(&self.discovery, profile)
                    };
                    self.message.clear();
                }
            }
            KeyCode::Char('/') => self.searching = true,
            KeyCode::Esc => {
                self.query = Input::default();
                self.selected = 0;
            }
            KeyCode::Char('?') => self.screen = Screen::Help,
            KeyCode::Char('a') => {
                self.message.clear();
                self.screen = Screen::Form(Form::new(None, false));
            }
            KeyCode::Char('e') => {
                if let Some(profile) = self.selected_profile() {
                    self.message.clear();
                    if profile.needs_binding() {
                        self.screen = Screen::BindConfig {
                            profile,
                            input: Input::default(),
                        };
                        return Action::None;
                    }
                    let mut form = Form::new(Some(profile), false);
                    form.detect_identity(&self.discovery);
                    self.screen = Screen::Form(form);
                }
            }
            KeyCode::Char('d') => {
                if let Some(profile) = self.selected_profile() {
                    self.screen = Screen::Confirm {
                        action: Confirmation::Delete(profile),
                        yes: false,
                    };
                }
            }
            KeyCode::Enter => {
                if let Some(profile) = self.selected_profile() {
                    return self.connect(profile);
                }
            }
            KeyCode::Char('i') => match import::default_path() {
                Ok(path) => {
                    self.message.clear();
                    self.screen = Screen::ImportPath(Input::new(path.display().to_string()));
                }
                Err(e) => self.notice(format!("{e:#}"), true),
            },
            _ => {
                self.selected = move_cursor(
                    self.selected,
                    self.filtered().len(),
                    self.page_size,
                    key.code,
                )
            }
        }
        Action::None
    }
}

fn setup_screen(discovery: &Discovery, mut profile: Profile) -> Screen {
    let found = discovery.find(&profile);
    if let Some(candidate) = found.automatic {
        profile.set_identity_file(candidate.path);
    } else if profile.identity_file().is_none() && !found.candidates.is_empty() {
        return Screen::KeyPicker {
            candidates: found.candidates,
            cursor: 0,
            purpose: KeyPurpose::Setup(profile),
        };
    }
    Screen::Confirm {
        action: Confirmation::SetupKey(profile),
        yes: false,
    }
}

fn move_cursor(current: usize, count: usize, page: usize, key: KeyCode) -> usize {
    let last = count.saturating_sub(1);
    match key {
        KeyCode::Up | KeyCode::Char('k') => current.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => (current + 1).min(last),
        KeyCode::PageUp => current.saturating_sub(page),
        KeyCode::PageDown => (current + page).min(last),
        KeyCode::Home => 0,
        KeyCode::End => last,
        _ => current.min(last),
    }
}

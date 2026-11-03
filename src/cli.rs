use crate::{
    import,
    model::{clean_text, expand_path, last_used_label, now},
    ssh,
    store::Store,
    ui::{self, app::Entry},
};
use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "sshm",
    version,
    about = "轻量的本地 SSH 连接管理器",
    long_about = "轻量的本地 SSH 连接管理器。不带子命令时打开交互列表；实际连接使用系统 OpenSSH。",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// 打印已保存的连接并退出
    List,
    /// 按完整名称连接；结束后返回命令行
    Connect { name: String },
    /// 打开新增表单
    Add {
        /// 保存后立即连接
        #[arg(long)]
        connect: bool,
    },
    /// 编辑指定连接；导入项只能修改名称与标签
    Edit { name: String },
    /// 确认后删除记录，保留原 SSH 配置和私钥
    Remove { name: String },
    /// 确认后为 Linux/macOS 服务器配置本机专用密钥登录
    SetupKey { name: String },
    /// 预览并选择 SSH 别名，默认读取 ~/.ssh/config
    Import { path: Option<String> },
}

pub fn run(cli: Cli, store: Store) -> Result<i32> {
    match cli.command {
        None => ui::run(store, Entry::List),
        Some(Commands::List) => {
            let profiles = store.load()?;
            if profiles.is_empty() {
                println!("还没有连接。运行 sshm add 新增，或 sshm import 导入。");
            } else {
                println!("名称\t地址\t用户\t标签\t最近使用");
                for p in profiles {
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        clean_text(&p.name),
                        clean_text(&p.address()),
                        clean_text(p.user()),
                        clean_text(&p.tags.join(", ")),
                        last_used_label(p.last_used, now())
                    );
                }
            }
            Ok(0)
        }
        Some(Commands::Connect { name }) => {
            let profile = store.find(&name)?;
            if profile.key_login_configured {
                let _sync = crate::sync::worker::Worker::start(store.clone());
                ssh::run(&profile, &store)
            } else {
                ui::run(store, Entry::Connect(profile))
            }
        }
        Some(Commands::Add { connect }) => ui::run(store, Entry::Add(connect)),
        Some(Commands::Edit { name }) => {
            let profile = store.find(&name)?;
            ui::run(store, Entry::Edit(profile))
        }
        Some(Commands::Remove { name }) => {
            let profile = store.find(&name)?;
            ui::run(store, Entry::Remove(profile))
        }
        Some(Commands::Import { path }) => {
            let path = path
                .map(|p| expand_path(&p))
                .unwrap_or_else(import::default_path)?;
            ui::run(store, Entry::Import(path))
        }
        Some(Commands::SetupKey { name }) => {
            let profile = store.find(&name)?;
            ui::run(store, Entry::SetupKey(profile))
        }
    }
}

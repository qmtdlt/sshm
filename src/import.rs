use crate::model::{Connection, Profile, expand_path, valid_alias};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct Preview {
    pub profiles: Vec<Profile>,
    pub warnings: Vec<String>,
}

pub fn default_path() -> Result<PathBuf> {
    expand_path("~/.ssh/config")
}

/// Enumerates literal aliases only. Never evaluates Match exec, ProxyCommand,
/// tokens, or environment variables from the imported configuration.
pub fn preview(path: &Path) -> Result<Preview> {
    let root = dunce::canonicalize(path)
        .with_context(|| format!("无法读取 SSH 配置：{}", path.display()))?;
    let include_base = expand_path("~/.ssh")?;
    let mut result = Preview::default();
    let mut visited = HashSet::new();
    scan(&root, &root, &include_base, &mut visited, &mut result, 0)?;
    result.profiles.sort_by_key(|p| p.name.to_lowercase());
    Ok(result)
}

fn scan(
    path: &Path,
    root: &Path,
    include_base: &Path,
    visited: &mut HashSet<PathBuf>,
    result: &mut Preview,
    depth: usize,
) -> Result<()> {
    ensure!(
        depth < 32 && visited.len() < 1024,
        "Include 嵌套或文件数量过多，已停止扫描"
    );
    let path = dunce::canonicalize(path)?;
    if !visited.insert(path.clone()) {
        return Ok(());
    }
    ensure!(
        fs::metadata(&path)?.len() <= 4 * 1024 * 1024,
        "SSH 配置文件超过 4 MiB"
    );
    let content = fs::read_to_string(&path)
        .with_context(|| format!("无法读取 UTF-8 配置 {}", path.display()))?;
    for (line_no, line) in content.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let end = line
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(line.len());
        let key = &line[..end];
        if !key.eq_ignore_ascii_case("host") && !key.eq_ignore_ascii_case("include") {
            continue;
        }
        let rest = line[end..]
            .trim_start()
            .strip_prefix('=')
            .unwrap_or(line[end..].trim_start())
            .trim_start();
        let words =
            split_words(rest).with_context(|| format!("{}:{}", path.display(), line_no + 1))?;
        if key.eq_ignore_ascii_case("host") {
            for alias in words {
                if valid_alias(&alias)
                    && !result
                        .profiles
                        .iter()
                        .any(|p| p.name.eq_ignore_ascii_case(&alias))
                {
                    result.profiles.push(Profile::new(
                        alias.clone(),
                        Connection::SshConfig {
                            path: root.to_path_buf(),
                            alias,
                        },
                    ));
                }
            }
        } else {
            for pattern in words {
                if pattern.contains('%') || pattern.contains('$') {
                    result
                        .warnings
                        .push(format!("未展开动态 Include：{pattern}"));
                    continue;
                }
                let include = if pattern.starts_with('~') {
                    expand_path(&pattern)?
                } else {
                    let p = PathBuf::from(&pattern);
                    if p.is_absolute() {
                        p
                    } else {
                        include_base.join(p)
                    }
                };
                let include = include.to_string_lossy().replace('\\', "/");
                match glob::glob(&include) {
                    Ok(paths) => {
                        for child in paths {
                            match child {
                                Ok(child) => {
                                    if let Err(error) =
                                        scan(&child, root, include_base, visited, result, depth + 1)
                                    {
                                        result
                                            .warnings
                                            .push(format!("{}：{error:#}", child.display()));
                                    }
                                }
                                Err(error) => result.warnings.push(error.to_string()),
                            }
                        }
                    }
                    Err(error) => result.warnings.push(format!("Include {pattern}：{error}")),
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn split_words(value: &str) -> Result<Vec<String>> {
    let mut result = vec![];
    let mut word = String::new();
    let mut quote = None;
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if chars
                .peek()
                .is_some_and(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '#' | '\\')) =>
            {
                word.push(chars.next().unwrap())
            }
            '"' | '\'' if quote == Some(ch) => quote = None,
            '"' | '\'' if quote.is_none() => quote = Some(ch),
            '#' if quote.is_none() => break,
            c if c.is_whitespace() && quote.is_none() => {
                if !word.is_empty() {
                    result.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    ensure!(quote.is_none(), "Host/Include 中的引号未闭合");
    if !word.is_empty() {
        result.push(word);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_literals_quotes_equals_and_includes_without_execution() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("extra config");
        fs::write(&child, "Host included\n HostName 127.0.0.1\n").unwrap();
        let root = dir.path().join("config");
        fs::write(&root, format!("\u{feff}Host=dev \"prod\" *.example.com !excluded\nHost dev\nInclude \"{}\"\nMatch exec \"do-not-run\"\n HostName ignored\nHost -bad\n", child.display().to_string().replace('\\', "/"))).unwrap();
        let result = preview(&root).unwrap();
        assert_eq!(
            result
                .profiles
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["dev", "included", "prod"]
        );
        assert!(result.profiles.iter().all(|p| matches!(&p.connection, Connection::SshConfig { path, .. } if path == &dunce::canonicalize(&root).unwrap())));
    }

    #[test]
    fn cycles_are_bounded_and_dynamic_includes_warn() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config");
        fs::write(
            &root,
            format!(
                "Host local\nInclude \"{}\"\nInclude %h/config\n",
                root.display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();
        let result = preview(&root).unwrap();
        assert_eq!(result.profiles.len(), 1);
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn malformed_host_does_not_silently_import() {
        assert!(split_words("\"broken").is_err());
        assert_eq!(split_words("dev # comment").unwrap(), ["dev"]);
    }
}

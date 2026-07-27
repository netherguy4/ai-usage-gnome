use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn expand_home(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return dirs::home_dir().unwrap_or_else(|| path.to_path_buf());
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    path.to_path_buf()
}

pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("ai-usage");
    }
    let user = env::var("USER").unwrap_or_else(|_| "unknown".to_owned());
    env::temp_dir().join(format!("ai-usage-{user}"))
}

pub fn state_file() -> PathBuf {
    runtime_dir().join("state.json")
}

pub fn data_dir() -> Result<PathBuf> {
    let base = dirs::data_local_dir().context("Не удалось определить каталог данных")?;
    Ok(base.join("ai-usage"))
}

pub fn claude_cache_file(account_id: &str) -> Result<PathBuf> {
    Ok(data_dir()?.join("claude").join(format!("{account_id}.json")))
}

pub fn secrets_file() -> Result<PathBuf> {
    let base = dirs::config_dir().context("Не удалось определить каталог конфигурации")?;
    Ok(base.join("ai-usage").join("secrets.env"))
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        let mut file = fs::File::create(&tmp)
            .with_context(|| format!("Не удалось создать {}", tmp.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("Не удалось заменить {}", path.display()))?;
    Ok(())
}

pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() {
        bail!("ID не может быть пустым");
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        bail!("ID может содержать только латинские буквы, цифры, '-' и '_'");
    }
    Ok(())
}

pub fn env_name_for_id(id: &str) -> String {
    let normalized: String = id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("AI_USAGE_DEEPSEEK_{normalized}")
}

pub fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn load_secrets_env() -> Result<()> {
    let path = secrets_file()?;
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(&path)?;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if env::var_os(key.trim()).is_none() {
                env::set_var(key.trim(), value.trim());
            }
        }
    }
    Ok(())
}

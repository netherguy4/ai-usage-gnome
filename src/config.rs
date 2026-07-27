use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_refresh_seconds")]
    pub refresh_seconds: u64,
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_seconds: default_refresh_seconds(),
            accounts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "lowercase")]
pub enum AccountConfig {
    Claude {
        id: String,
        name: String,
        config_dir: PathBuf,
        plan: Option<String>,
    },
    Codex {
        id: String,
        name: String,
        codex_home: PathBuf,
        #[serde(default = "default_codex_command")]
        command: String,
        #[serde(default = "default_codex_limit_id")]
        limit_id: String,
    },
    Deepseek {
        id: String,
        name: String,
        api_key_env: String,
        #[serde(default = "default_deepseek_base_url")]
        base_url: String,
    },
}

impl AccountConfig {
    pub fn id(&self) -> &str {
        match self {
            Self::Claude { id, .. } | Self::Codex { id, .. } | Self::Deepseek { id, .. } => id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Claude { name, .. } | Self::Codex { name, .. } | Self::Deepseek { name, .. } => name,
        }
    }

    pub fn provider(&self) -> &'static str {
        match self {
            Self::Claude { .. } => "claude",
            Self::Codex { .. } => "codex",
            Self::Deepseek { .. } => "deepseek",
        }
    }
}

fn default_refresh_seconds() -> u64 {
    120
}

fn default_codex_command() -> String {
    "codex".to_owned()
}

fn default_codex_limit_id() -> String {
    "codex".to_owned()
}

fn default_deepseek_base_url() -> String {
    "https://api.deepseek.com".to_owned()
}

pub fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("Не удалось определить каталог конфигурации")?;
    Ok(base.join("ai-usage").join("config.toml"))
}

pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("Не удалось прочитать {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("Некорректный TOML в {}", path.display()))
}

pub fn save(config: &Config) -> Result<PathBuf> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = toml::to_string_pretty(config)?;
    crate::util::atomic_write(&path, raw.as_bytes())?;
    Ok(path)
}

pub fn upsert_account(config: &mut Config, account: AccountConfig) {
    if let Some(index) = config.accounts.iter().position(|item| item.id() == account.id()) {
        config.accounts[index] = account;
    } else {
        config.accounts.push(account);
    }
}

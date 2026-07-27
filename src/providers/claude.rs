use std::fs;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{AccountState, QuotaWindow};

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaudeCache {
    pub updated_at: i64,
    pub model: Option<String>,
    pub five_hour: Option<QuotaWindow>,
    pub weekly: Option<QuotaWindow>,
}

pub async fn fetch(id: &str, name: &str, plan: Option<String>) -> Result<AccountState> {
    let path = crate::util::claude_cache_file(id)?;
    if !path.exists() {
        return Ok(AccountState {
            id: id.to_owned(),
            name: name.to_owned(),
            provider: "claude".to_owned(),
            status: "waiting".to_owned(),
            plan,
            email: None,
            model: None,
            five_hour: None,
            weekly: None,
            balances: Vec::new(),
            error: Some("Запусти Claude Code и отправь хотя бы один запрос".to_owned()),
            updated_at: crate::util::unix_now(),
        });
    }

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("Не удалось прочитать кеш Claude {}", path.display()))?;
    let cache: ClaudeCache = serde_json::from_str(&raw)
        .with_context(|| format!("Повреждён кеш Claude {}", path.display()))?;
    let age = crate::util::unix_now().saturating_sub(cache.updated_at);
    let status = if age > 86_400 { "stale" } else { "ok" };

    Ok(AccountState {
        id: id.to_owned(),
        name: name.to_owned(),
        provider: "claude".to_owned(),
        status: status.to_owned(),
        plan,
        email: None,
        model: cache.model,
        five_hour: cache.five_hour,
        weekly: cache.weekly,
        balances: Vec::new(),
        error: if status == "stale" {
            Some("Данные Claude старше 24 часов".to_owned())
        } else {
            None
        },
        updated_at: cache.updated_at,
    })
}

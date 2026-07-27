use std::env;

use anyhow::{bail, Context, Result};
use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde::Deserialize;

use crate::model::{AccountState, BalanceInfo};

#[derive(Debug, Deserialize)]
struct BalanceResponse {
    is_available: bool,
    #[serde(default)]
    balance_infos: Vec<RawBalance>,
}

#[derive(Debug, Deserialize)]
struct RawBalance {
    currency: String,
    total_balance: String,
    granted_balance: Option<String>,
    topped_up_balance: Option<String>,
}

pub async fn fetch(id: &str, name: &str, api_key_env: &str, base_url: &str) -> Result<AccountState> {
    let api_key = env::var(api_key_env)
        .with_context(|| format!("Переменная {api_key_env} не задана"))?;
    if api_key.trim().is_empty() {
        bail!("Переменная {api_key_env} пуста");
    }

    let endpoint = format!("{}/user/balance", base_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .get(endpoint)
        .header(ACCEPT, "application/json")
        .header(AUTHORIZATION, format!("Bearer {api_key}"))
        .send()
        .await
        .context("Не удалось подключиться к DeepSeek")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("DeepSeek вернул HTTP {status}: {}", body.trim());
    }

    let payload: BalanceResponse = response
        .json()
        .await
        .context("DeepSeek вернул некорректный JSON")?;

    Ok(AccountState {
        id: id.to_owned(),
        name: name.to_owned(),
        provider: "deepseek".to_owned(),
        status: if payload.is_available { "ok" } else { "depleted" }.to_owned(),
        plan: Some("API".to_owned()),
        email: None,
        model: None,
        five_hour: None,
        weekly: None,
        balances: payload
            .balance_infos
            .into_iter()
            .map(|item| BalanceInfo {
                currency: item.currency,
                total: item.total_balance,
                granted: item.granted_balance,
                topped_up: item.topped_up_balance,
            })
            .collect(),
        error: if payload.is_available {
            None
        } else {
            Some("Баланс недостаточен для API-запросов".to_owned())
        },
        updated_at: crate::util::unix_now(),
    })
}

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use crate::model::{AccountState, QuotaWindow};

pub async fn fetch(
    id: &str,
    name: &str,
    codex_home: &Path,
    command: &str,
    limit_id: &str,
) -> Result<AccountState> {
    let codex_home = crate::util::expand_home(codex_home);
    let mut child = Command::new(command)
        .arg("app-server")
        .env("CODEX_HOME", &codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("Не удалось запустить '{command} app-server'"))?;

    let mut stdin = child.stdin.take().context("Codex app-server не открыл stdin")?;
    let stdout = child.stdout.take().context("Codex app-server не открыл stdout")?;

    for message in [
        json!({
            "method": "initialize",
            "id": 0,
            "params": {
                "clientInfo": {
                    "name": "ai_usage_gnome",
                    "title": "AI Usage GNOME",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
        json!({"method": "initialized", "params": {}}),
        json!({"method": "account/read", "id": 1, "params": {"refreshToken": false}}),
        json!({"method": "account/rateLimits/read", "id": 2}),
    ] {
        stdin.write_all(serde_json::to_string(&message)?.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
    }
    stdin.flush().await?;

    let read_result = timeout(Duration::from_secs(20), async {
        let mut lines = BufReader::new(stdout).lines();
        let mut account_response: Option<Value> = None;
        let mut limits_response: Option<Value> = None;

        while let Some(line) = lines.next_line().await? {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            match message.get("id").and_then(Value::as_i64) {
                Some(1) => account_response = Some(message),
                Some(2) => limits_response = Some(message),
                _ => {}
            }
            if account_response.is_some() && limits_response.is_some() {
                break;
            }
        }
        Ok::<_, anyhow::Error>((account_response, limits_response))
    })
    .await
    .context("Codex app-server не ответил за 20 секунд")??;

    let _ = child.kill().await;

    let account_message = read_result.0.context("Codex не вернул account/read")?;
    reject_rpc_error(&account_message)?;
    let account = account_message
        .pointer("/result/account")
        .cloned()
        .unwrap_or(Value::Null);

    if account.is_null() {
        return Ok(AccountState {
            id: id.to_owned(),
            name: name.to_owned(),
            provider: "codex".to_owned(),
            status: "unauthenticated".to_owned(),
            plan: None,
            email: None,
            model: None,
            five_hour: None,
            weekly: None,
            balances: Vec::new(),
            error: Some(format!(
                "Выполни: CODEX_HOME={} {} login",
                codex_home.display(),
                command
            )),
            updated_at: crate::util::unix_now(),
        });
    }

    let email = account.get("email").and_then(Value::as_str).map(str::to_owned);
    let account_plan = account
        .get("planType")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let limits_message = read_result.1.context("Codex не вернул account/rateLimits/read")?;
    reject_rpc_error(&limits_message)?;
    let result = limits_message.get("result").context("В ответе Codex нет result")?;
    let bucket = result
        .get("rateLimitsByLimitId")
        .and_then(|buckets| buckets.get(limit_id))
        .or_else(|| result.get("rateLimits"))
        .context("Codex не вернул лимиты для выбранного bucket")?;

    let mut windows = Vec::new();
    for key in ["primary", "secondary"] {
        if let Some(window) = bucket.get(key).filter(|value| !value.is_null()) {
            if let Some(parsed) = parse_window(window) {
                windows.push(parsed);
            }
        }
    }

    if windows.is_empty() {
        bail!("Codex вернул пустой набор quota windows");
    }

    let (five_hour, weekly) = select_windows(&windows);

    let bucket_plan = bucket
        .get("planType")
        .and_then(Value::as_str)
        .map(str::to_owned);

    Ok(AccountState {
        id: id.to_owned(),
        name: name.to_owned(),
        provider: "codex".to_owned(),
        status: "ok".to_owned(),
        plan: account_plan.or(bucket_plan),
        email,
        model: None,
        five_hour,
        weekly,
        balances: Vec::new(),
        error: None,
        updated_at: crate::util::unix_now(),
    })
}

fn reject_rpc_error(message: &Value) -> Result<()> {
    if let Some(error) = message.get("error") {
        let text = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("неизвестная JSON-RPC ошибка");
        bail!("Codex app-server: {text}");
    }
    Ok(())
}

fn parse_window(value: &Value) -> Option<QuotaWindow> {
    let used = value.get("usedPercent")?.as_f64()?;
    let duration = value.get("windowDurationMins").and_then(Value::as_u64);
    let resets_at = value.get("resetsAt").and_then(Value::as_i64);
    Some(QuotaWindow::new(used, duration, resets_at))
}

fn select_windows(windows: &[QuotaWindow]) -> (Option<QuotaWindow>, Option<QuotaWindow>) {
    if windows.is_empty() {
        return (None, None);
    }

    if windows.len() == 1 {
        let window = windows[0].clone();
        let duration = window.duration_mins.unwrap_or(300);
        if duration.abs_diff(300) <= duration.abs_diff(10_080) {
            return (Some(window), None);
        }
        return (None, Some(window));
    }

    let five_index = closest_index(windows, 300, None).unwrap_or(0);
    let weekly_index = closest_index(windows, 10_080, Some(five_index));
    (
        Some(windows[five_index].clone()),
        weekly_index.map(|index| windows[index].clone()),
    )
}

fn closest_index(
    windows: &[QuotaWindow],
    target_mins: u64,
    excluded_index: Option<usize>,
) -> Option<usize> {
    windows
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != excluded_index)
        .min_by_key(|(_, window)| {
            window
                .duration_mins
                .map(|value| value.abs_diff(target_mins))
                .unwrap_or(u64::MAX)
        })
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_five_hour_and_weekly_windows() {
        let windows = vec![
            QuotaWindow::new(25.0, Some(300), Some(1)),
            QuotaWindow::new(50.0, Some(10_080), Some(2)),
        ];
        let (five, weekly) = select_windows(&windows);
        assert_eq!(five.unwrap().duration_mins, Some(300));
        assert_eq!(weekly.unwrap().duration_mins, Some(10_080));
    }

    #[test]
    fn maps_single_weekly_window_as_weekly() {
        let windows = vec![QuotaWindow::new(50.0, Some(10_080), Some(2))];
        let (five, weekly) = select_windows(&windows);
        assert!(five.is_none());
        assert!(weekly.is_some());
    }
}

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, Command};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::model::{AccountState, QuotaWindow};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);
/// Сколько байт stderr сохраняем для диагностики. Держим хвост, а не начало:
/// причина падения обычно в последних строках.
const STDERR_CAPTURE_BYTES: usize = 4096;

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
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("Не удалось запустить '{command} app-server'"))?;

    let mut stdin = child
        .stdin
        .take()
        .context("Codex app-server не открыл stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("Codex app-server не открыл stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("Codex app-server не открыл stderr")?;
    let stderr_task = tokio::spawn(capture_stderr(stderr));

    // Все четыре сообщения отправляются одним батчем: живой app-server 0.145
    // принимает такую последовательность и отвечает на initialize первым.
    // Ответ на id 0 всё равно разбирается — чтобы отличить отказ handshake
    // от молчания сервера.
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
        if let Err(error) = write_message(&mut stdin, &message).await {
            let details = collect_failure(&mut child, stderr_task).await;
            return Err(anyhow!("Codex app-server закрыл stdin: {error}{details}"));
        }
    }
    stdin.flush().await.ok();

    let exchange = timeout(RESPONSE_TIMEOUT, read_responses(stdout)).await;

    let responses = match exchange {
        Ok(Ok(responses)) => responses,
        Ok(Err(error)) => {
            let details = collect_failure(&mut child, stderr_task).await;
            return Err(anyhow!("Ошибка чтения ответа Codex: {error}{details}"));
        }
        Err(_) => {
            let details = collect_failure(&mut child, stderr_task).await;
            return Err(anyhow!(
                "Codex app-server не ответил за {} секунд{details}",
                RESPONSE_TIMEOUT.as_secs()
            ));
        }
    };

    // Ответы могут прийти в любом порядке: на неавторизованном профиле ошибка
    // на id 2 приходит раньше ответа на id 1.
    let Responses {
        handshake,
        account: account_response,
        limits: limits_response,
    } = responses;

    let account_message = match account_response {
        Some(message) => message,
        None => {
            let details = collect_failure(&mut child, stderr_task).await;
            let handshake_error = handshake
                .as_ref()
                .and_then(rpc_error_text)
                .map(|text| format!(" (initialize: {text})"))
                .unwrap_or_default();
            return Err(anyhow!(
                "Codex не вернул account/read{handshake_error}{details}"
            ));
        }
    };

    let limits_response = match limits_response {
        Some(message) => message,
        None => {
            let details = collect_failure(&mut child, stderr_task).await;
            return Err(anyhow!("Codex не вернул account/rateLimits/read{details}"));
        }
    };

    let _ = child.kill().await;
    stderr_task.abort();

    parse_account_state(
        id,
        name,
        command,
        &codex_home,
        limit_id,
        &account_message,
        &limits_response,
    )
}

struct Responses {
    handshake: Option<Value>,
    account: Option<Value>,
    limits: Option<Value>,
}

async fn write_message(
    stdin: &mut tokio::process::ChildStdin,
    message: &Value,
) -> std::io::Result<()> {
    stdin
        .write_all(serde_json::to_string(message)?.as_bytes())
        .await?;
    stdin.write_all(b"\n").await
}

async fn read_responses(stdout: tokio::process::ChildStdout) -> Result<Responses> {
    let mut lines = BufReader::new(stdout).lines();
    let mut responses = Responses {
        handshake: None,
        account: None,
        limits: None,
    };

    while let Some(line) = lines.next_line().await? {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match message.get("id").and_then(Value::as_i64) {
            Some(0) => responses.handshake = Some(message),
            Some(1) => responses.account = Some(message),
            Some(2) => responses.limits = Some(message),
            _ => {}
        }
        if responses.account.is_some() && responses.limits.is_some() {
            break;
        }
    }

    Ok(responses)
}

/// Читает stderr дочернего процесса, удерживая в памяти только хвост.
async fn capture_stderr(stderr: ChildStderr) -> Vec<u8> {
    let mut reader = BufReader::new(stderr);
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > STDERR_CAPTURE_BYTES * 2 {
                    buffer.drain(..buffer.len() - STDERR_CAPTURE_BYTES);
                }
            }
        }
    }
    if buffer.len() > STDERR_CAPTURE_BYTES {
        buffer.drain(..buffer.len() - STDERR_CAPTURE_BYTES);
    }
    buffer
}

/// Завершает процесс и собирает безопасную для показа диагностику.
async fn collect_failure(child: &mut Child, stderr_task: JoinHandle<Vec<u8>>) -> String {
    let status = match timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) => Some(status.to_string()),
        _ => {
            let _ = child.kill().await;
            None
        }
    };

    let captured = timeout(Duration::from_secs(2), stderr_task)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let stderr = redact(String::from_utf8_lossy(&captured).trim());

    let mut parts = Vec::new();
    if let Some(status) = status {
        parts.push(status);
    }
    if !stderr.is_empty() {
        parts.push(format!("stderr: {stderr}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join("; "))
    }
}

/// Вырезает из текста то, что похоже на токены и ключи, чтобы диагностика
/// Codex не утекала в state.json, логи и UI.
fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    let mut previous = String::new();

    let flush = |token: &mut String, previous: &mut String, out: &mut String| {
        if token.is_empty() {
            return;
        }
        if is_secret(token, previous) {
            out.push_str("<redacted>");
        } else {
            out.push_str(token);
        }
        previous.clear();
        previous.push_str(token);
        token.clear();
    };

    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '+' | '/' | '=') {
            token.push(ch);
        } else {
            flush(&mut token, &mut previous, &mut out);
            out.push(ch);
        }
    }
    flush(&mut token, &mut previous, &mut out);
    out
}

fn is_secret(token: &str, previous: &str) -> bool {
    if previous.eq_ignore_ascii_case("bearer") || previous.eq_ignore_ascii_case("token") {
        return true;
    }
    if token.starts_with("sk-") || token.starts_with("sk_") {
        return true;
    }
    if token.starts_with("eyJ") && token.len() > 20 {
        return true;
    }
    // Длинные непрерывные base64-подобные строки. Пути и версии исключены
    // требованием отсутствия '/' и '.', чтобы диагностика оставалась читаемой.
    token.len() >= 40
        && !token.contains('/')
        && !token.contains('.')
        && token.chars().any(|ch| ch.is_ascii_digit())
        && token.chars().any(|ch| ch.is_ascii_alphabetic())
}

/// Чистая часть провайдера: превращает пару JSON-RPC ответов в `AccountState`.
/// Вынесена отдельно, чтобы тестироваться на фикстурах без subprocess.
pub fn parse_account_state(
    id: &str,
    name: &str,
    command: &str,
    codex_home: &Path,
    limit_id: &str,
    account_message: &Value,
    limits_message: &Value,
) -> Result<AccountState> {
    reject_rpc_error(account_message)?;
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

    let email = account
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let account_plan = account
        .get("planType")
        .and_then(Value::as_str)
        .map(str::to_owned);

    reject_rpc_error(limits_message)?;
    let result = limits_message
        .get("result")
        .context("В ответе Codex нет result")?;
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

fn rpc_error_text(message: &Value) -> Option<String> {
    message.get("error").map(|error| {
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("неизвестная JSON-RPC ошибка")
            .to_owned()
    })
}

fn reject_rpc_error(message: &Value) -> Result<()> {
    if let Some(text) = rpc_error_text(message) {
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

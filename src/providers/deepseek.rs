use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde::Deserialize;

use crate::model::{AccountState, BalanceInfo};

/// Тело ответа об ошибке может быть большим или содержать лишнее; в state.json
/// и UI попадает только начало.
const MAX_ERROR_BODY: usize = 200;

/// Общий HTTP-клиент. Без явных таймаутов зависший запрос задерживал бы весь
/// refresh cycle, а новый Client на каждый вызов терял бы connection pool.
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .user_agent(concat!("ai-usage/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("не удалось создать HTTP-клиент")
    })
}

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

pub async fn fetch(
    id: &str,
    name: &str,
    api_key_env: &str,
    base_url: &str,
) -> Result<AccountState> {
    let api_key =
        env::var(api_key_env).with_context(|| format!("Переменная {api_key_env} не задана"))?;
    if api_key.trim().is_empty() {
        bail!("Переменная {api_key_env} пуста");
    }
    fetch_with_key(id, name, &api_key, base_url).await
}

/// Отделено от чтения окружения, чтобы тесты не мутировали глобальный env.
pub async fn fetch_with_key(
    id: &str,
    name: &str,
    api_key: &str,
    base_url: &str,
) -> Result<AccountState> {
    let endpoint = format!("{}/user/balance", base_url.trim_end_matches('/'));
    let response = client()
        .get(endpoint)
        .header(ACCEPT, "application/json")
        .header(AUTHORIZATION, format!("Bearer {api_key}"))
        .send()
        .await
        .context("Не удалось подключиться к DeepSeek")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let body = body.trim();
        let body: String = body.chars().take(MAX_ERROR_BODY).collect();
        bail!("DeepSeek вернул HTTP {status}: {body}");
    }

    let payload: BalanceResponse = response
        .json()
        .await
        .context("DeepSeek вернул некорректный JSON")?;

    Ok(AccountState {
        id: id.to_owned(),
        name: name.to_owned(),
        provider: "deepseek".to_owned(),
        status: if payload.is_available {
            "ok"
        } else {
            "depleted"
        }
        .to_owned(),
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    /// Минимальный HTTP/1.1 сервер на один запрос. Достаточно для проверки
    /// разбора ответов и заголовков, и не тянет тестовых зависимостей.
    struct MockServer {
        base_url: String,
        request: Arc<Mutex<String>>,
        handle: tokio::task::JoinHandle<()>,
    }

    impl MockServer {
        async fn start(status_line: &'static str, body: &'static str) -> Self {
            Self::start_raw(move |_| {
                format!(
                    "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            })
            .await
        }

        async fn start_raw<F>(respond: F) -> Self
        where
            F: Fn(&str) -> String + Send + 'static,
        {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let request = Arc::new(Mutex::new(String::new()));
            let captured = Arc::clone(&request);

            let handle = tokio::spawn(async move {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = vec![0_u8; 4096];
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                let raw = String::from_utf8_lossy(&buffer[..read]).into_owned();
                *captured.lock().unwrap() = raw.clone();
                let _ = socket.write_all(respond(&raw).as_bytes()).await;
                let _ = socket.flush().await;
            });

            Self {
                base_url: format!("http://{address}"),
                request,
                handle,
            }
        }

        fn request(&self) -> String {
            self.request.lock().unwrap().clone()
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    const OK_BODY: &str = r#"{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"}]}"#;

    #[tokio::test]
    async fn parses_a_successful_balance() {
        let server = MockServer::start("200 OK", OK_BODY).await;
        let state = fetch_with_key("ds", "DeepSeek", "secret-key", &server.base_url)
            .await
            .unwrap();

        assert_eq!(state.status, "ok");
        assert!(state.error.is_none());
        assert_eq!(state.balances.len(), 1);
        assert_eq!(state.balances[0].currency, "CNY");
        assert_eq!(state.balances[0].total, "110.00");
        assert_eq!(state.balances[0].granted.as_deref(), Some("10.00"));
    }

    #[tokio::test]
    async fn sends_bearer_authorization_to_the_documented_path() {
        let server = MockServer::start("200 OK", OK_BODY).await;
        fetch_with_key("ds", "DeepSeek", "secret-key", &server.base_url)
            .await
            .unwrap();

        let request = server.request();
        assert!(request.contains("GET /user/balance"), "{request}");
        assert!(request.contains("Bearer secret-key"), "{request}");
    }

    #[tokio::test]
    async fn trailing_slash_in_base_url_does_not_double_up() {
        let server = MockServer::start("200 OK", OK_BODY).await;
        let base = format!("{}/", server.base_url);
        fetch_with_key("ds", "DeepSeek", "k", &base).await.unwrap();
        assert!(server.request().contains("GET /user/balance HTTP"));
    }

    #[tokio::test]
    async fn reports_multiple_currencies() {
        let body = r#"{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"1.00"},{"currency":"USD","total_balance":"2.00"}]}"#;
        let server = MockServer::start("200 OK", body).await;
        let state = fetch_with_key("ds", "DeepSeek", "k", &server.base_url)
            .await
            .unwrap();
        assert_eq!(state.balances.len(), 2);
        assert_eq!(state.balances[1].currency, "USD");
        assert!(state.balances[0].granted.is_none());
    }

    #[tokio::test]
    async fn unavailable_balance_becomes_depleted() {
        let body =
            r#"{"is_available":false,"balance_infos":[{"currency":"CNY","total_balance":"0.00"}]}"#;
        let server = MockServer::start("200 OK", body).await;
        let state = fetch_with_key("ds", "DeepSeek", "k", &server.base_url)
            .await
            .unwrap();

        assert_eq!(state.status, "depleted");
        assert!(state.error.is_some());
        // Баланс всё равно показываем: пользователю важно увидеть ноль.
        assert_eq!(state.balances.len(), 1);
    }

    #[tokio::test]
    async fn http_401_is_an_error() {
        let server = MockServer::start("401 Unauthorized", r#"{"error":"invalid key"}"#).await;
        let error = fetch_with_key("ds", "DeepSeek", "bad", &server.base_url)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("401"), "{error}");
    }

    #[tokio::test]
    async fn long_error_bodies_are_truncated() {
        const BODY: &str = concat!(
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        assert_eq!(BODY.len(), 250);
        let server = MockServer::start("500 Internal Server Error", BODY).await;
        let error = fetch_with_key("ds", "DeepSeek", "k", &server.base_url)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            error.matches('A').count(),
            MAX_ERROR_BODY,
            "тело должно быть усечено до {MAX_ERROR_BODY} символов: {error}"
        );
    }

    #[tokio::test]
    async fn malformed_json_is_an_error() {
        let server = MockServer::start("200 OK", "{not json").await;
        let error = fetch_with_key("ds", "DeepSeek", "k", &server.base_url)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("некорректный JSON"), "{error}");
    }

    #[tokio::test]
    async fn missing_required_field_is_an_error() {
        let server = MockServer::start("200 OK", r#"{"balance_infos":[]}"#).await;
        assert!(fetch_with_key("ds", "DeepSeek", "k", &server.base_url)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn connection_refused_is_an_error_not_a_hang() {
        // Порт занят listener'ом, который сразу закрывается — соединиться некуда.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);

        let error = fetch_with_key("ds", "DeepSeek", "k", &format!("http://{address}"))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("DeepSeek"), "{error}");
    }

    #[tokio::test]
    async fn empty_api_key_is_rejected_before_any_request() {
        let variable = "AI_USAGE_TEST_DEEPSEEK_EMPTY_KEY";
        // Переменная заведомо не задана в тестовом окружении.
        let error = fetch("ds", "DeepSeek", variable, "http://127.0.0.1:1")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(variable), "{error}");
    }
}

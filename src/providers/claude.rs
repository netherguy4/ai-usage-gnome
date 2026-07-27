use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::model::{label_for_duration, AccountState, QuotaWindow, FIVE_HOUR_MINS, WEEKLY_MINS};

/// Пока порог не задан в конфиге, данные Claude считаются устаревшими через сутки.
pub const DEFAULT_STALE_SECONDS: i64 = 86_400;

/// Официальный endpoint, который опрашивает сам Claude Code.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";

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

// ── Разбор ответа об использовании ──────────────────────────────────────────

/// Одна запись из `limits[]`.
///
/// Массив `limits` — надмножество верхнеуровневых полей: только в нём
/// приходят лимиты, привязанные к модели (`weekly_scoped` + `scope.model`).
#[derive(Debug, Deserialize)]
struct LimitEntry {
    kind: String,
    percent: Option<f64>,
    resets_at: Option<String>,
    #[serde(default)]
    scope: Option<LimitScope>,
}

#[derive(Debug, Deserialize)]
struct LimitScope {
    #[serde(default)]
    model: Option<ScopedModel>,
}

#[derive(Debug, Deserialize)]
struct ScopedModel {
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    limits: Vec<LimitEntry>,
}

/// Превращает ответ endpoint в список окон.
///
/// Набор лимитов задаётся сервером и меняется вместе с тарифной политикой:
/// недельный лимит на конкретную модель может появиться и исчезнуть. Поэтому
/// ничего не фильтруем по белому списку — показываем всё, что пришло.
pub fn windows_from_usage(raw: &str) -> Result<Vec<QuotaWindow>> {
    let response: UsageResponse =
        serde_json::from_str(raw).context("Claude вернул некорректный JSON")?;

    let mut windows: Vec<QuotaWindow> = response
        .limits
        .into_iter()
        .filter_map(|entry| {
            let percent = entry.percent?;
            let model = entry
                .scope
                .and_then(|scope| scope.model)
                .and_then(|model| model.display_name);
            let duration = duration_for_kind(&entry.kind);
            let key = match &model {
                Some(model) => format!("{}:{}", entry.kind, model.to_lowercase()),
                None => entry.kind.clone(),
            };
            Some(QuotaWindow::new(
                key,
                label_for_kind(&entry.kind, model.as_deref(), duration),
                percent,
                duration,
                entry.resets_at.as_deref().and_then(rfc3339_to_unix),
            ))
        })
        .collect();

    windows.sort_by(|a, b| {
        a.duration_mins
            .unwrap_or(u64::MAX)
            .cmp(&b.duration_mins.unwrap_or(u64::MAX))
            .then_with(|| a.key.cmp(&b.key))
    });
    Ok(windows)
}

fn duration_for_kind(kind: &str) -> Option<u64> {
    match kind {
        "session" => Some(FIVE_HOUR_MINS),
        kind if kind.starts_with("weekly") => Some(WEEKLY_MINS),
        _ => None,
    }
}

fn label_for_kind(kind: &str, model: Option<&str>, duration: Option<u64>) -> String {
    let base = match kind {
        "session" => "5 часов".to_owned(),
        "weekly_all" => "Неделя".to_owned(),
        _ if duration.is_some() => label_for_duration(duration),
        kind => humanize(kind),
    };
    match model {
        Some(model) => format!("{base} · {model}"),
        None => base,
    }
}

fn humanize(raw: &str) -> String {
    raw.split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Время ───────────────────────────────────────────────────────────────────

/// Разбирает `2026-07-30T15:59:59.352117+00:00` в unix-секунды.
///
/// Своя реализация вместо зависимости от chrono: нужен ровно один формат,
/// который отдаёт этот endpoint.
fn rfc3339_to_unix(raw: &str) -> Option<i64> {
    let bytes = raw.as_bytes();
    if bytes.len() < 19 || bytes[10] != b'T' {
        return None;
    }
    let number = |from: usize, to: usize| raw.get(from..to)?.parse::<i64>().ok();

    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut stamp = days_from_civil(year, month as u32, day as u32) * 86_400
        + hour * 3600
        + minute * 60
        + second;

    // Смещение зоны: 'Z' либо ±HH:MM в хвосте.
    let tail = &raw[19..];
    if let Some(sign_at) = tail.rfind(['+', '-']) {
        let offset = &tail[sign_at..];
        if offset.len() >= 6 {
            let hours: i64 = offset.get(1..3)?.parse().ok()?;
            let minutes: i64 = offset.get(4..6)?.parse().ok()?;
            let total = hours * 3600 + minutes * 60;
            stamp += if offset.starts_with('-') {
                total
            } else {
                -total
            };
        }
    }
    Some(stamp)
}

/// Дни от 1970-01-01 (алгоритм Хиннанта).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let day_of_year = (153 * month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Строка для status line внутри самого Claude Code.
///
/// Это необязательная косметика: виджет берёт лимиты напрямую у Anthropic и в
/// hook не нуждается. Поэтому здесь ничего не кешируется — только форматируется
/// то, что Claude Code уже прислал в stdin.
pub fn status_line_from_payload(raw: &str) -> Result<String> {
    let payload: Value =
        serde_json::from_str(raw).context("Claude hook получил некорректный JSON")?;

    let mut parts = Vec::new();
    if let Some(model) = payload
        .pointer("/model/display_name")
        .and_then(Value::as_str)
    {
        parts.push(format!("[{model}]"));
    }

    if let Some(limits) = payload.get("rate_limits").and_then(Value::as_object) {
        let mut entries: Vec<_> = limits
            .iter()
            .filter_map(|(key, value)| {
                let used = value.get("used_percentage")?.as_f64()?;
                Some((key.clone(), 100.0 - used.clamp(0.0, 100.0)))
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (key, remaining) in entries {
            parts.push(format!("{} {remaining:.0}% left", short_key(&key)));
        }
    }

    Ok(parts.join(" · "))
}

fn short_key(key: &str) -> String {
    let (base, suffix) = if let Some(rest) = key.strip_prefix("five_hour") {
        ("5h", rest)
    } else if let Some(rest) = key.strip_prefix("seven_day") {
        ("7d", rest)
    } else {
        return key.to_owned();
    };
    match suffix.strip_prefix('_') {
        Some(suffix) if !suffix.is_empty() => format!("{base}/{suffix}"),
        _ => base.to_owned(),
    }
}

// ── Учётные данные и профиль ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<OauthCredentials>,
}

#[derive(Debug, Deserialize)]
struct OauthCredentials {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
}

#[derive(Debug, Default)]
pub struct Identity {
    pub email: Option<String>,
    pub plan: Option<String>,
}

/// Читает почту и тариф из конфига Claude Code.
///
/// Ни то ни другое не приходит с endpoint использования. Токены отсюда не
/// берутся — только идентификация аккаунта.
pub fn read_identity(config_dir: &Path) -> Identity {
    for path in identity_paths(config_dir) {
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let account = value.get("oauthAccount");
        let email = account
            .and_then(|account| account.get("emailAddress"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let plan = account
            .and_then(|account| account.get("organizationType"))
            .and_then(Value::as_str)
            .map(plan_label);

        if email.is_some() || plan.is_some() {
            return Identity { email, plan };
        }
    }
    Identity::default()
}

fn identity_paths(config_dir: &Path) -> Vec<PathBuf> {
    let dir = crate::util::expand_home(config_dir);
    let mut paths = vec![dir.join(".claude.json")];
    // Профиль по умолчанию: сам каталог ~/.claude, а .claude.json — рядом с ним.
    if let Some(parent) = dir.parent() {
        paths.push(parent.join(".claude.json"));
    }
    paths
}

/// `claude_max` → `Max`. Неизвестные значения показываем как есть.
fn plan_label(raw: &str) -> String {
    humanize(raw.strip_prefix("claude_").unwrap_or(raw))
}

/// Кеш использования, который Claude Code сам сохраняет в `.claude.json`.
/// Служит запасным источником, когда access token протух.
fn cached_usage(config_dir: &Path) -> Option<(Vec<QuotaWindow>, i64)> {
    for path in identity_paths(config_dir) {
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let Some(cached) = value.get("cachedUsageUtilization") else {
            continue;
        };
        let fetched_at = cached.get("fetchedAtMs").and_then(Value::as_i64)? / 1000;
        let utilization = cached.get("utilization")?;
        let windows = windows_from_usage(&utilization.to_string()).ok()?;
        if !windows.is_empty() {
            return Some((windows, fetched_at));
        }
    }
    None
}

fn read_token(config_dir: &Path) -> Option<OauthCredentials> {
    let path = crate::util::expand_home(config_dir).join(".credentials.json");
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str::<CredentialsFile>(&raw).ok()?.oauth
}

/// Строка для `doctor`: есть ли рабочий доступ к лимитам.
pub fn credentials_status(config_dir: &Path) -> (bool, String) {
    let path = crate::util::expand_home(config_dir).join(".credentials.json");
    let Some(credentials) = read_token(config_dir) else {
        return (false, format!("нет входа, ожидался {}", path.display()));
    };
    match credentials.expires_at {
        Some(expires_at) if expires_at / 1000 <= crate::util::unix_now() => (
            false,
            "токен истёк — запусти Claude Code, он обновит его сам".to_owned(),
        ),
        Some(expires_at) => {
            let hours = (expires_at / 1000 - crate::util::unix_now()) / 3600;
            (true, format!("вход есть, токен ещё {hours} ч"))
        }
        None => (true, "вход есть".to_owned()),
    }
}

// ── Провайдер ───────────────────────────────────────────────────────────────

pub async fn fetch(
    id: &str,
    name: &str,
    plan: Option<String>,
    config_dir: &Path,
    stale_seconds: i64,
) -> Result<AccountState> {
    let _ = id;
    let identity = read_identity(config_dir);
    let plan = plan.or(identity.plan);
    let now = crate::util::unix_now();

    let state =
        |status: &str, windows: Vec<QuotaWindow>, error: Option<String>, at: i64| AccountState {
            id: id.to_owned(),
            name: name.to_owned(),
            provider: "claude".to_owned(),
            status: status.to_owned(),
            plan: plan.clone(),
            email: identity.email.clone(),
            model: None,
            windows,
            balances: Vec::new(),
            error,
            updated_at: at,
        };

    match live_usage(config_dir, now).await {
        Ok(windows) => Ok(state("ok", windows, None, now)),
        Err(reason) => {
            // Токен протух или сеть недоступна — показываем последнее, что
            // сохранил сам Claude Code, честно указав возраст.
            if let Some((windows, fetched_at)) = cached_usage(config_dir) {
                let age = now.saturating_sub(fetched_at);
                let status = if age > stale_seconds { "stale" } else { "ok" };
                let error =
                    (status == "stale").then(|| format!("{reason}; показан кеш Claude Code"));
                return Ok(state(status, windows, error, fetched_at));
            }
            Ok(state("waiting", Vec::new(), Some(reason.to_string()), now))
        }
    }
}

/// Запрашивает актуальные лимиты у Anthropic. Ошибка означает «нужен запасной
/// источник», а не отказ провайдера.
async fn live_usage(config_dir: &Path, now: i64) -> Result<Vec<QuotaWindow>> {
    let Some(credentials) = read_token(config_dir) else {
        bail!("Выполни вход: claude");
    };
    if let Some(expires_at) = credentials.expires_at {
        // expiresAt в миллисекундах; запас в минуту, чтобы не ловить гонку.
        if expires_at / 1000 <= now + 60 {
            bail!("Токен Claude истёк — запусти Claude Code, он обновит его сам");
        }
    }

    let response = client()
        .get(USAGE_URL)
        .bearer_auth(&credentials.access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header("anthropic-beta", OAUTH_BETA)
        .send()
        .await
        .context("Не удалось связаться с Anthropic")?;

    let status = response.status();
    if !status.is_success() {
        // Тело не показываем: оно может содержать детали аккаунта.
        bail!("Anthropic вернул HTTP {status}");
    }

    let body = response.text().await.context("Пустой ответ Anthropic")?;
    let windows = windows_from_usage(&body)?;
    if windows.is_empty() {
        bail!("Anthropic не вернул ни одного лимита");
    }
    Ok(windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USAGE: &str = include_str!("../../tests/fixtures/claude_usage.json");

    fn windows() -> Vec<QuotaWindow> {
        windows_from_usage(USAGE).unwrap()
    }

    fn keys(windows: &[QuotaWindow]) -> Vec<&str> {
        windows.iter().map(|w| w.key.as_str()).collect()
    }

    #[test]
    fn parses_the_live_usage_response() {
        let windows = windows();
        assert_eq!(
            keys(&windows),
            ["session", "weekly_all", "weekly_scoped:fable"]
        );

        assert_eq!(windows[0].label, "5 часов");
        assert_eq!(windows[0].remaining_percent, 71.0);
        assert_eq!(windows[0].duration_mins, Some(FIVE_HOUR_MINS));

        assert_eq!(windows[1].label, "Неделя");
        assert_eq!(windows[1].remaining_percent, 34.0);
    }

    #[test]
    fn keeps_the_per_model_limit() {
        // Ради него всё и затевалось: лимит модели есть только в limits[].
        let windows = windows();
        let fable = windows
            .iter()
            .find(|w| w.key == "weekly_scoped:fable")
            .expect("лимит Fable должен сохраниться");
        assert_eq!(fable.label, "Неделя · Fable");
        assert_eq!(fable.remaining_percent, 0.0);
        assert_eq!(fable.duration_mins, Some(WEEKLY_MINS));
    }

    #[test]
    fn a_new_scoped_limit_needs_no_code_change() {
        let raw = r#"{"limits":[{"kind":"weekly_scoped","percent":10,
            "scope":{"model":{"display_name":"Haiku"}}}]}"#;
        let windows = windows_from_usage(raw).unwrap();
        assert_eq!(windows[0].label, "Неделя · Haiku");
        assert_eq!(windows[0].key, "weekly_scoped:haiku");
    }

    #[test]
    fn an_unknown_kind_still_gets_a_label() {
        let raw = r#"{"limits":[{"kind":"monthly_credits","percent":30}]}"#;
        let windows = windows_from_usage(raw).unwrap();
        assert_eq!(windows[0].label, "Monthly Credits");
        assert!(windows[0].duration_mins.is_none());
    }

    #[test]
    fn a_withdrawn_limit_simply_disappears() {
        let raw = r#"{"limits":[{"kind":"session","percent":5}]}"#;
        let windows = windows_from_usage(raw).unwrap();
        assert_eq!(keys(&windows), ["session"]);
    }

    #[test]
    fn entries_without_a_percent_are_skipped() {
        let raw = r#"{"limits":[{"kind":"session"},{"kind":"weekly_all","percent":1}]}"#;
        assert_eq!(keys(&windows_from_usage(raw).unwrap()), ["weekly_all"]);
    }

    #[test]
    fn an_empty_limits_array_is_not_an_error() {
        assert!(windows_from_usage(r#"{"limits":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(windows_from_usage("{not json").is_err());
    }

    #[test]
    fn order_is_stable_and_shortest_first() {
        let first = keys(&windows()).join(",");
        for _ in 0..5 {
            assert_eq!(keys(&windows()).join(","), first);
        }
        let durations: Vec<_> = windows().iter().map(|w| w.duration_mins).collect();
        let mut sorted = durations.clone();
        sorted.sort();
        assert_eq!(durations, sorted);
    }

    #[test]
    fn parses_the_timestamp_format_the_endpoint_returns() {
        // 2026-07-30T15:59:59Z
        assert_eq!(
            rfc3339_to_unix("2026-07-30T15:59:59.352117+00:00"),
            Some(1_785_427_199)
        );
        assert_eq!(rfc3339_to_unix("2026-07-30T15:59:59Z"), Some(1_785_427_199));
        assert_eq!(rfc3339_to_unix("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn applies_the_timezone_offset() {
        let utc = rfc3339_to_unix("2026-07-30T15:00:00Z").unwrap();
        // +02:00 означает, что тот же момент наступил на два часа раньше по UTC.
        assert_eq!(rfc3339_to_unix("2026-07-30T17:00:00+02:00"), Some(utc));
        assert_eq!(rfc3339_to_unix("2026-07-30T13:00:00-02:00"), Some(utc));
    }

    #[test]
    fn rejects_junk_timestamps() {
        for raw in ["", "не дата", "2026-07-30", "2026-13-40T00:00:00Z"] {
            assert!(rfc3339_to_unix(raw).is_none(), "принято: {raw}");
        }
    }

    #[test]
    fn handles_leap_years_and_century_rules() {
        assert_eq!(rfc3339_to_unix("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(rfc3339_to_unix("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(rfc3339_to_unix("2026-01-01T00:00:00Z"), Some(1_767_225_600));
    }

    #[test]
    fn plan_labels_are_readable() {
        assert_eq!(plan_label("claude_max"), "Max");
        assert_eq!(plan_label("claude_pro"), "Pro");
        assert_eq!(plan_label("enterprise"), "Enterprise");
    }

    #[test]
    fn reads_email_and_plan_from_the_claude_config() {
        let dir = std::env::temp_dir().join(format!(
            "ai-usage-identity-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let profile = dir.join(".claude");
        fs::create_dir_all(&profile).unwrap();
        // Для профиля по умолчанию файл лежит рядом с каталогом, а не внутри.
        fs::write(
            dir.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"user@example.com","organizationType":"claude_max"}}"#,
        )
        .unwrap();

        let identity = read_identity(&profile);
        assert_eq!(identity.email.as_deref(), Some("user@example.com"));
        assert_eq!(identity.plan.as_deref(), Some("Max"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn falls_back_to_the_cache_claude_code_writes() {
        let dir = std::env::temp_dir().join(format!(
            "ai-usage-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let profile = dir.join(".claude");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            dir.join(".claude.json"),
            r#"{"cachedUsageUtilization":{"fetchedAtMs":1785144456226,
                "utilization":{"limits":[{"kind":"session","percent":3}]}}}"#,
        )
        .unwrap();

        let (windows, fetched_at) = cached_usage(&profile).expect("кеш должен читаться");
        assert_eq!(windows[0].key, "session");
        assert_eq!(fetched_at, 1_785_144_456);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_claude_config_is_not_an_error() {
        let identity = read_identity(Path::new("/nonexistent/profile"));
        assert!(identity.email.is_none());
        assert!(identity.plan.is_none());
        assert!(cached_usage(Path::new("/nonexistent/profile")).is_none());
        assert!(read_token(Path::new("/nonexistent/profile")).is_none());
    }
}

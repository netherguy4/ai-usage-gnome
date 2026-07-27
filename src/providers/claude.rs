use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::model::{label_for_duration, AccountState, QuotaWindow, FIVE_HOUR_MINS, WEEKLY_MINS};

/// Пока порог не задан в конфиге, данные Claude считаются устаревшими через
/// четверть часа. Это 5% пятичасового окна: более старым цифрам верить нельзя,
/// а раньше порог в сутки прятал отказ endpoint за многочасовым кешем.
pub const DEFAULT_STALE_SECONDS: i64 = 900;

/// Официальный endpoint, который опрашивает сам Claude Code.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";

/// Минимум между удачными запросами к endpoint лимитов.
///
/// Endpoint отвечает 429 задолго до общего интервала обновления: на живом
/// аккаунте опрос раз в 120 с держал его в отказе постоянно. Пять минут — 1.7%
/// пятичасового окна, для показа этого более чем достаточно.
const MIN_POLL_SECONDS: i64 = 300;

/// Пауза после 429: 5 → 10 → 20 → 40 → 60 минут. Бюджет endpoint делится с
/// самим Claude Code, поэтому фиксированного интервала мало — нужен отход.
/// Потолок в час: дальше отходить незачем, недельное окно столько не меняется.
const BACKOFF_MAX_SECONDS: i64 = 3_600;

/// Пауза после прочих ошибок: сеть или истёкший токен чинятся сами, долбить их
/// смысла нет, но и отходить надолго не за что.
const RETRY_SECONDS: i64 = 60;

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
            Some(
                QuotaWindow::new(
                    key,
                    label_for_kind(&entry.kind, model.as_deref(), duration),
                    percent,
                    duration,
                    entry.resets_at.as_deref().and_then(rfc3339_to_unix),
                )
                .with_scope(model),
            )
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
        // organizationType даёт только 'claude_max' — без множителя лимитов,
        // а именно он отличает Max 5x от Max 20x. Тариф пользователя важнее
        // тарифа организации: у участника может быть свой.
        let plan = [
            "userRateLimitTier",
            "organizationRateLimitTier",
            "organizationType",
        ]
        .iter()
        .filter_map(|field| account.and_then(|account| account.get(*field)))
        .find_map(Value::as_str)
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

fn plan_label(raw: &str) -> String {
    crate::util::humanize_plan(raw)
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

/// Наш собственный кеш ответа endpoint.
///
/// Живёт в runtime-каталоге рядом со `state.json` и переживает перезапуск
/// сервиса: без него каждый рестарт начинал бы с пустой паузы и сразу же ловил
/// 429. Здесь же хранится момент следующей попытки.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct UsageCache {
    #[serde(default)]
    fetched_at: i64,
    #[serde(default)]
    next_attempt_at: i64,
    #[serde(default)]
    failures: u32,
    #[serde(default)]
    windows: Vec<QuotaWindow>,
}

fn usage_cache_path(id: &str) -> PathBuf {
    // `validate_id` уже не пускает сюда ничего лишнего, но имя файла собирается
    // из внешней строки — фильтр стоит дешевле разбирательства с '../'.
    let safe: String = id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    crate::util::runtime_dir().join(format!("claude-usage-{safe}.json"))
}

fn read_usage_cache(path: &Path) -> UsageCache {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_usage_cache(path: &Path, cache: &UsageCache) {
    let Ok(bytes) = serde_json::to_vec(cache) else {
        return;
    };
    // Ошибка записи не повод терять уже полученные цифры: кеш — ускоритель,
    // а не источник истины. Но молчать о ней нельзя: без кеша теряется и момент
    // следующей попытки, то есть пауза после `429` перестаёт соблюдаться.
    if let Err(error) = crate::util::atomic_write_mode(path, &bytes, Some(0o600)) {
        eprintln!("Не удалось сохранить кеш лимитов Claude: {error}");
    }
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

    let cache_path = usage_cache_path(id);
    let mut cache = read_usage_cache(&cache_path);

    // Пока не вышла пауза — в сеть не идём вовсе. Лишний запрос не просто
    // бесполезен: он продлевает отказ endpoint и для нас, и для Claude Code.
    let mut failure = None;
    if now >= cache.next_attempt_at {
        match live_usage(config_dir, now).await {
            Ok(windows) => {
                cache = UsageCache {
                    fetched_at: now,
                    next_attempt_at: now + MIN_POLL_SECONDS,
                    failures: 0,
                    windows: windows.clone(),
                };
                write_usage_cache(&cache_path, &cache);
                return Ok(state("ok", windows, None, now));
            }
            Err(error) => {
                cache.failures = cache.failures.saturating_add(1);
                cache.next_attempt_at = now
                    + if error.rate_limited {
                        backoff_seconds(cache.failures)
                    } else {
                        RETRY_SECONDS
                    };
                write_usage_cache(&cache_path, &cache);
                failure = Some(error.message);
            }
        }
    }

    // Запроса не было или он не удался — показываем самое свежее из того, что
    // уже есть: наш собственный ответ endpoint либо кеш Claude Code.
    let own = (!cache.windows.is_empty()).then(|| (cache.windows.clone(), cache.fetched_at));
    let Some((windows, fetched_at)) = choose_fallback(own, cached_usage(config_dir), now) else {
        let reason = failure.unwrap_or_else(|| "Лимиты Claude ещё не получены".to_owned());
        return Ok(state("waiting", Vec::new(), Some(reason), now));
    };

    let age = now.saturating_sub(fetched_at);
    if windows.is_empty() {
        let reason =
            failure.unwrap_or_else(|| format!("данные Claude устарели на {}", humanize_age(age)));
        return Ok(state("stale", Vec::new(), Some(reason), fetched_at));
    }

    if age <= stale_seconds {
        return Ok(state("ok", windows, None, fetched_at));
    }

    let reason = match failure {
        Some(reason) => format!("{reason}; показаны данные {} назад", humanize_age(age)),
        None => format!("данные Claude обновлялись {} назад", humanize_age(age)),
    };
    Ok(state("stale", windows, Some(reason), fetched_at))
}

/// Выбирает, что показать вместо живого ответа.
///
/// Источников два — наш кеш и кеш Claude Code, — и выигрывает тот, что свежее:
/// Claude Code обновляет свой только при запуске, но после долгого простоя
/// сервиса именно он и окажется новее.
///
/// Окна, чей период уже закончился, отбрасываются: показывать использование
/// прошлого периода — врать, а считать его сброшенным — гадать. Пустой список
/// на выходе означает «данные есть, но все протухли», и это не то же самое, что
/// «данных нет вовсе».
fn choose_fallback(
    own: Option<(Vec<QuotaWindow>, i64)>,
    claude_code: Option<(Vec<QuotaWindow>, i64)>,
    now: i64,
) -> Option<(Vec<QuotaWindow>, i64)> {
    let (windows, fetched_at) = [own, claude_code]
        .into_iter()
        .flatten()
        .max_by_key(|(_, fetched_at)| *fetched_at)?;
    let current = windows
        .into_iter()
        .filter(|window| window.resets_at.is_none_or(|resets_at| resets_at > now))
        .collect();
    Some((current, fetched_at))
}

fn backoff_seconds(failures: u32) -> i64 {
    let shift = failures.saturating_sub(1).min(8);
    (MIN_POLL_SECONDS.saturating_mul(1 << shift)).min(BACKOFF_MAX_SECONDS)
}

fn humanize_age(seconds: i64) -> String {
    let minutes = seconds / 60;
    match minutes {
        ..=0 => "меньше минуты".to_owned(),
        1..=59 => format!("{minutes} мин"),
        _ => format!("{} ч {} мин", minutes / 60, minutes % 60),
    }
}

/// Ошибка живого запроса. `rate_limited` отделяет «нас попросили подождать» от
/// «что-то сломалось»: пауза у этих случаев разная.
struct LiveError {
    message: String,
    rate_limited: bool,
}

impl LiveError {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            rate_limited: false,
        }
    }
}

/// Запрашивает актуальные лимиты у Anthropic. Ошибка означает «нужен запасной
/// источник», а не отказ провайдера.
async fn live_usage(config_dir: &Path, now: i64) -> Result<Vec<QuotaWindow>, LiveError> {
    let Some(credentials) = read_token(config_dir) else {
        return Err(LiveError::plain("Выполни вход: claude"));
    };
    if let Some(expires_at) = credentials.expires_at {
        // expiresAt в миллисекундах; запас в минуту, чтобы не ловить гонку.
        if expires_at / 1000 <= now + 60 {
            return Err(LiveError::plain(
                "Токен Claude истёк — запусти Claude Code, он обновит его сам",
            ));
        }
    }

    let response = client()
        .get(USAGE_URL)
        .bearer_auth(&credentials.access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header("anthropic-beta", OAUTH_BETA)
        .send()
        .await
        .map_err(|_| LiveError::plain("Не удалось связаться с Anthropic"))?;

    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(LiveError {
            message: "Anthropic ограничил частоту запросов".to_owned(),
            rate_limited: true,
        });
    }
    if !status.is_success() {
        // Тело не показываем: оно может содержать детали аккаунта.
        return Err(LiveError::plain(format!("Anthropic вернул HTTP {status}")));
    }

    let body = response
        .text()
        .await
        .map_err(|_| LiveError::plain("Пустой ответ Anthropic"))?;
    let windows = windows_from_usage(&body).map_err(|error| LiveError::plain(error.to_string()))?;
    if windows.is_empty() {
        return Err(LiveError::plain("Anthropic не вернул ни одного лимита"));
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
    fn plan_labels_distinguish_rate_limit_tiers() {
        // Без множителя нельзя понять, какой Max используется.
        assert_eq!(plan_label("default_claude_max_5x"), "Max 5x");
        assert_eq!(plan_label("default_claude_max_20x"), "Max 20x");
        assert_eq!(plan_label("default_claude_zero"), "Zero");
        assert_eq!(plan_label("claude_max"), "Max");
        assert_eq!(plan_label("enterprise"), "Enterprise");
    }

    #[test]
    fn prefers_the_rate_limit_tier_over_the_organization_type() {
        let dir = std::env::temp_dir().join(format!(
            "ai-usage-tier-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let profile = dir.join(".claude");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            dir.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"u@example.com",
                "organizationType":"claude_max",
                "organizationRateLimitTier":"default_claude_max_20x"}}"#,
        )
        .unwrap();

        assert_eq!(read_identity(&profile).plan.as_deref(), Some("Max 20x"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_member_tier_wins_over_the_organization_tier() {
        let dir = std::env::temp_dir().join(format!(
            "ai-usage-member-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let profile = dir.join(".claude");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            dir.join(".claude.json"),
            r#"{"oauthAccount":{"userRateLimitTier":"default_claude_max_5x",
                "organizationRateLimitTier":"default_claude_max_20x"}}"#,
        )
        .unwrap();

        assert_eq!(read_identity(&profile).plan.as_deref(), Some("Max 5x"));
        fs::remove_dir_all(&dir).unwrap();
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

    fn window(key: &str, percent: f64, resets_at: Option<i64>) -> QuotaWindow {
        QuotaWindow::new(key, key, percent, Some(FIVE_HOUR_MINS), resets_at)
    }

    #[test]
    fn the_pause_after_a_rate_limit_grows_and_stops_growing() {
        assert_eq!(backoff_seconds(1), MIN_POLL_SECONDS);
        assert_eq!(backoff_seconds(2), 2 * MIN_POLL_SECONDS);
        assert_eq!(backoff_seconds(3), 4 * MIN_POLL_SECONDS);
        assert_eq!(backoff_seconds(4), 8 * MIN_POLL_SECONDS);
        assert_eq!(backoff_seconds(5), BACKOFF_MAX_SECONDS);
        // Счётчик отказов растёт, пока endpoint молчит; час — потолок, и сдвиг
        // на 30 позиций не должен обнулять его переполнением.
        assert_eq!(backoff_seconds(100), BACKOFF_MAX_SECONDS);
        assert_eq!(backoff_seconds(u32::MAX), BACKOFF_MAX_SECONDS);
    }

    #[test]
    fn the_freshest_source_wins() {
        let mine = vec![window("session", 10.0, Some(2_000))];
        let theirs = vec![window("session", 90.0, Some(2_000))];

        let (windows, at) = choose_fallback(
            Some((mine.clone(), 500)),
            Some((theirs.clone(), 900)),
            1_000,
        )
        .unwrap();
        assert_eq!(at, 900);
        assert_eq!(windows[0].used_percent, 90.0);

        let (windows, at) = choose_fallback(Some((mine, 900)), Some((theirs, 500)), 1_000).unwrap();
        assert_eq!(at, 900);
        assert_eq!(windows[0].used_percent, 10.0);
    }

    #[test]
    fn a_window_whose_period_ended_is_dropped_rather_than_guessed() {
        let windows = vec![
            window("session", 97.0, Some(900)),
            window("weekly_all", 62.0, Some(5_000)),
            window("no_reset", 5.0, None),
        ];

        let (current, _) = choose_fallback(Some((windows, 100)), None, 1_000).unwrap();
        assert_eq!(keys(&current), ["weekly_all", "no_reset"]);
    }

    #[test]
    fn every_window_expired_is_not_the_same_as_no_data() {
        let windows = vec![window("session", 97.0, Some(900))];
        let (current, at) = choose_fallback(Some((windows, 100)), None, 1_000).unwrap();
        assert!(current.is_empty(), "истёкшее окно показывать нечего");
        assert_eq!(at, 100, "возраст данных при этом известен");

        assert!(
            choose_fallback(None, None, 1_000).is_none(),
            "источников нет вовсе — это другое состояние"
        );
    }

    #[test]
    fn the_cache_survives_a_restart_and_a_damaged_file() {
        let dir = std::env::temp_dir().join(format!(
            "ai-usage-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude-usage-main.json");

        let cache = UsageCache {
            fetched_at: 1_785_000_000,
            next_attempt_at: 1_785_000_300,
            failures: 2,
            windows: vec![window("session", 42.0, Some(1_785_010_000))],
        };
        write_usage_cache(&path, &cache);

        let read = read_usage_cache(&path);
        assert_eq!(read.fetched_at, 1_785_000_000);
        assert_eq!(read.next_attempt_at, 1_785_000_300);
        assert_eq!(read.failures, 2);
        assert_eq!(read.windows[0].used_percent, 42.0);

        // Кеш лежит в runtime-каталоге; чужим глазам он не нужен.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "кеш не должен быть доступен всем");
        }

        fs::write(&path, "{ это не json").unwrap();
        let broken = read_usage_cache(&path);
        assert_eq!(broken.next_attempt_at, 0, "битый кеш не блокирует запрос");
        assert!(broken.windows.is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_age_of_the_data_reads_like_a_sentence() {
        assert_eq!(humanize_age(0), "меньше минуты");
        assert_eq!(humanize_age(-5), "меньше минуты");
        assert_eq!(humanize_age(59), "меньше минуты");
        assert_eq!(humanize_age(60), "1 мин");
        assert_eq!(humanize_age(3_540), "59 мин");
        assert_eq!(humanize_age(3_600), "1 ч 0 мин");
        assert_eq!(humanize_age(19_860), "5 ч 31 мин");
    }

    #[test]
    fn the_cache_file_name_cannot_escape_the_runtime_directory() {
        let path = usage_cache_path("../../etc/passwd");
        assert_eq!(path.parent(), Some(crate::util::runtime_dir().as_path()));
        assert_eq!(
            path.file_name().unwrap(),
            "claude-usage-______etc_passwd.json"
        );
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

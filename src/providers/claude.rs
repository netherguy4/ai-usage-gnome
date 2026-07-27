use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{label_for_duration, AccountState, QuotaWindow, FIVE_HOUR_MINS, WEEKLY_MINS};

/// Пока порог не задан в конфиге, данные Claude считаются устаревшими через сутки.
pub const DEFAULT_STALE_SECONDS: i64 = 86_400;

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaudeCache {
    pub updated_at: i64,
    pub model: Option<String>,
    #[serde(default)]
    pub windows: Vec<QuotaWindow>,
}

/// Payload, который Claude Code передаёт status-line команде в stdin.
///
/// `rate_limits` разбирается как открытая карта, а не как набор известных
/// полей: помимо документированных `five_hour` и `seven_day` Claude Code
/// оперирует `seven_day_opus`, `seven_day_sonnet`,
/// `seven_day_overage_included` и лимитами конкретных моделей, которые
/// появляются и исчезают вместе с тарифной политикой. Захардкоженный список
/// молча терял бы новые лимиты.
#[derive(Debug, Deserialize)]
pub struct ClaudeInput {
    model: Option<ClaudeModel>,
    rate_limits: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
struct ClaudeModel {
    display_name: Option<String>,
}

impl ClaudeInput {
    /// Превращает payload в кеш. Чистая функция — тестируется на фикстуре.
    pub fn into_cache(self, now: i64) -> ClaudeCache {
        ClaudeCache {
            updated_at: now,
            model: self.model.and_then(|model| model.display_name),
            windows: self.rate_limits.map(parse_windows).unwrap_or_default(),
        }
    }
}

/// Разбирает карту лимитов в упорядоченный список окон.
fn parse_windows(limits: serde_json::Map<String, Value>) -> Vec<QuotaWindow> {
    let mut windows: Vec<QuotaWindow> = limits
        .into_iter()
        .filter_map(|(key, value)| {
            let used = value.get("used_percentage")?.as_f64()?;
            let resets_at = value.get("resets_at").and_then(Value::as_i64);
            let duration = duration_for_key(&key);
            Some(QuotaWindow::new(
                &key,
                label_for_key(&key, duration),
                used,
                duration,
                resets_at,
            ))
        })
        .collect();

    // Короткие окна выше длинных, внутри одной длительности — по имени ключа,
    // иначе порядок скакал бы от обновления к обновлению (карта не упорядочена).
    // Общий лимит оказывается перед уточнёнными сам: 'seven_day' — префикс
    // 'seven_day_opus' и потому сортируется раньше.
    windows.sort_by(|a, b| {
        a.duration_mins
            .unwrap_or(u64::MAX)
            .cmp(&b.duration_mins.unwrap_or(u64::MAX))
            .then_with(|| a.key.cmp(&b.key))
    });
    windows
}

fn duration_for_key(key: &str) -> Option<u64> {
    if key.starts_with("five_hour") {
        Some(FIVE_HOUR_MINS)
    } else if key.starts_with("seven_day") {
        Some(WEEKLY_MINS)
    } else {
        None
    }
}

/// Подпись лимита. Известные ключи получают осмысленное имя, незнакомые —
/// собираются из базового окна и суффикса, чтобы новый лимит появился в
/// интерфейсе сам, без правки кода.
fn label_for_key(key: &str, duration: Option<u64>) -> String {
    match key {
        "five_hour" => return "5 часов".to_owned(),
        "seven_day" => return "Неделя".to_owned(),
        "seven_day_overage_included" => return "Неделя · с превышением".to_owned(),
        _ => {}
    }

    let base = label_for_duration(duration);
    let suffix = key
        .strip_prefix("seven_day_")
        .or_else(|| key.strip_prefix("five_hour_"));

    match suffix {
        Some(suffix) if !suffix.is_empty() => format!("{base} · {}", humanize(suffix)),
        _ if duration.is_some() => base,
        _ => humanize(key),
    }
}

fn humanize(raw: &str) -> String {
    raw.split('_')
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

impl ClaudeCache {
    /// Строка, которую hook печатает обратно в Claude Code.
    pub fn status_line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(model) = &self.model {
            parts.push(format!("[{model}]"));
        }
        for window in &self.windows {
            parts.push(format!(
                "{} {:.0}% left",
                short_label(window),
                window.remaining_percent
            ));
        }
        parts.join(" · ")
    }
}

/// Компактная подпись для однострочного status line внутри Claude Code.
fn short_label(window: &QuotaWindow) -> String {
    let base = match window.duration_mins {
        Some(FIVE_HOUR_MINS) => "5h".to_owned(),
        Some(WEEKLY_MINS) => "7d".to_owned(),
        _ => window.key.clone(),
    };
    match window
        .key
        .strip_prefix("seven_day_")
        .or_else(|| window.key.strip_prefix("five_hour_"))
    {
        Some(suffix) if !suffix.is_empty() => format!("{base}/{suffix}"),
        _ => base,
    }
}

pub async fn fetch(
    id: &str,
    name: &str,
    plan: Option<String>,
    config_dir: &Path,
    stale_seconds: i64,
) -> Result<AccountState> {
    let identity = read_identity(config_dir);
    let plan = plan.or(identity.plan);

    let path = crate::util::claude_cache_file(id)?;
    if !path.exists() {
        return Ok(AccountState {
            id: id.to_owned(),
            name: name.to_owned(),
            provider: "claude".to_owned(),
            status: "waiting".to_owned(),
            plan,
            email: identity.email,
            model: None,
            windows: Vec::new(),
            balances: Vec::new(),
            error: Some("Запусти Claude Code и отправь хотя бы один запрос".to_owned()),
            updated_at: crate::util::unix_now(),
        });
    }

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("Не удалось прочитать кеш Claude {}", path.display()))?;
    let cache: ClaudeCache = serde_json::from_str(&raw)
        .with_context(|| format!("Повреждён кеш Claude {}", path.display()))?;

    Ok(state_from_cache(
        id,
        name,
        plan,
        identity.email,
        cache,
        stale_seconds,
        crate::util::unix_now(),
    ))
}

/// Чистая часть: кеш плюс текущее время дают `AccountState`.
#[allow(clippy::too_many_arguments)]
pub fn state_from_cache(
    id: &str,
    name: &str,
    plan: Option<String>,
    email: Option<String>,
    cache: ClaudeCache,
    stale_seconds: i64,
    now: i64,
) -> AccountState {
    let age = now.saturating_sub(cache.updated_at);
    let is_stale = age > stale_seconds;

    AccountState {
        id: id.to_owned(),
        name: name.to_owned(),
        provider: "claude".to_owned(),
        status: if is_stale { "stale" } else { "ok" }.to_owned(),
        plan,
        email,
        model: cache.model,
        windows: cache.windows,
        balances: Vec::new(),
        error: is_stale.then(|| {
            let hours = age / 3600;
            format!("Данные Claude обновлялись {hours} ч назад")
        }),
        updated_at: cache.updated_at,
    }
}

#[derive(Debug, Default)]
pub struct Identity {
    pub email: Option<String>,
    pub plan: Option<String>,
}

/// Читает почту и тариф из конфига Claude Code.
///
/// Ни то ни другое не приходит в status-line payload, поэтому берём из
/// `.claude.json`, который пишет сам Claude Code. Файл лежит рядом с
/// профилем; для профиля по умолчанию (`~/.claude`) он находится в домашнем
/// каталоге. Токенов оттуда не читаем.
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

fn identity_paths(config_dir: &Path) -> Vec<std::path::PathBuf> {
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
    let trimmed = raw.strip_prefix("claude_").unwrap_or(raw);
    humanize(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUSLINE: &str = include_str!("../../tests/fixtures/claude_statusline.json");
    const STATUSLINE_MODELS: &str =
        include_str!("../../tests/fixtures/claude_statusline_model_limits.json");

    fn cache_of(raw: &str) -> ClaudeCache {
        serde_json::from_str::<ClaudeInput>(raw)
            .expect("фикстура должна разбираться")
            .into_cache(1_000)
    }

    fn keys(cache: &ClaudeCache) -> Vec<&str> {
        cache.windows.iter().map(|w| w.key.as_str()).collect()
    }

    #[test]
    fn parses_real_statusline_payload() {
        let cache = cache_of(STATUSLINE);
        assert_eq!(cache.model.as_deref(), Some("Opus 5"));
        assert_eq!(keys(&cache), ["five_hour", "seven_day"]);

        let five = &cache.windows[0];
        assert_eq!(five.used_percent, 42.5);
        assert_eq!(five.remaining_percent, 57.5);
        assert_eq!(five.duration_mins, Some(FIVE_HOUR_MINS));
        assert_eq!(five.resets_at, Some(1_800_000_000));
        assert_eq!(five.label, "5 часов");

        assert_eq!(cache.windows[1].remaining_percent, 90.0);
        assert_eq!(cache.windows[1].label, "Неделя");
    }

    #[test]
    fn keeps_per_model_limits_such_as_fable() {
        // Главное требование: лимит конкретной модели не должен теряться.
        let cache = cache_of(STATUSLINE_MODELS);
        assert_eq!(
            keys(&cache),
            [
                "five_hour",
                "seven_day",
                "seven_day_fable",
                "seven_day_opus",
                "seven_day_overage_included",
                "seven_day_sonnet",
            ]
        );
        let fable = cache
            .windows
            .iter()
            .find(|w| w.key == "seven_day_fable")
            .expect("лимит Fable должен сохраниться");
        assert_eq!(fable.label, "Неделя · Fable");
        assert_eq!(fable.remaining_percent, 12.0);
    }

    #[test]
    fn an_unknown_future_limit_still_gets_a_readable_label() {
        // Лимит временный: если Anthropic заменит его другим, код менять не надо.
        let input = r#"{"rate_limits":{"seven_day_haiku_turbo":{"used_percentage":5.0}}}"#;
        let cache = cache_of(input);
        assert_eq!(cache.windows[0].label, "Неделя · Haiku Turbo");
        assert_eq!(cache.windows[0].duration_mins, Some(WEEKLY_MINS));
    }

    #[test]
    fn a_limit_with_an_unrecognised_shape_is_labelled_not_dropped() {
        let input = r#"{"rate_limits":{"monthly_credits":{"used_percentage":30.0}}}"#;
        let cache = cache_of(input);
        assert_eq!(cache.windows[0].label, "Monthly Credits");
        assert!(cache.windows[0].duration_mins.is_none());
    }

    #[test]
    fn a_removed_limit_simply_disappears() {
        let with_fable = cache_of(STATUSLINE_MODELS);
        assert!(with_fable
            .windows
            .iter()
            .any(|w| w.key == "seven_day_fable"));
        let without = cache_of(STATUSLINE);
        assert!(!without.windows.iter().any(|w| w.key == "seven_day_fable"));
    }

    #[test]
    fn window_order_is_stable_across_parses() {
        // serde_json::Map не гарантирует порядок, а прыгающие строки в меню
        // выглядят как баг.
        let first = keys(&cache_of(STATUSLINE_MODELS)).join(",");
        for _ in 0..5 {
            assert_eq!(keys(&cache_of(STATUSLINE_MODELS)).join(","), first);
        }
    }

    #[test]
    fn shorter_windows_come_first() {
        let cache = cache_of(STATUSLINE_MODELS);
        let durations: Vec<_> = cache.windows.iter().map(|w| w.duration_mins).collect();
        let mut sorted = durations.clone();
        sorted.sort();
        assert_eq!(durations, sorted);
    }

    #[test]
    fn entries_without_a_percentage_are_skipped() {
        let input =
            r#"{"rate_limits":{"five_hour":{"resets_at":5},"seven_day":{"used_percentage":10.0}}}"#;
        assert_eq!(keys(&cache_of(input)), ["seven_day"]);
    }

    #[test]
    fn formats_status_line_with_remaining_not_used() {
        assert_eq!(
            cache_of(STATUSLINE).status_line(),
            "[Opus 5] · 5h 58% left · 7d 90% left"
        );
    }

    #[test]
    fn status_line_names_per_model_limits() {
        let line = cache_of(STATUSLINE_MODELS).status_line();
        assert!(line.contains("7d/fable 12% left"), "{line}");
        assert!(line.contains("5h "), "{line}");
    }

    #[test]
    fn payload_without_rate_limits_is_accepted() {
        // Так выглядит payload у не-подписчика или до первого ответа модели.
        let cache = cache_of(r#"{"model":{"display_name":"Opus 5"}}"#);
        assert!(cache.windows.is_empty());
        assert_eq!(cache.status_line(), "[Opus 5]");
    }

    #[test]
    fn plan_labels_are_readable() {
        assert_eq!(plan_label("claude_max"), "Max");
        assert_eq!(plan_label("claude_pro"), "Pro");
        assert_eq!(plan_label("enterprise"), "Enterprise");
    }

    #[test]
    fn fresh_cache_is_ok() {
        let state = state_from_cache(
            "id",
            "Claude",
            None,
            None,
            cache_of(STATUSLINE),
            DEFAULT_STALE_SECONDS,
            1_500,
        );
        assert_eq!(state.status, "ok");
        assert!(state.error.is_none());
        assert_eq!(state.updated_at, 1_000);
    }

    #[test]
    fn cache_older_than_threshold_is_stale_but_keeps_quota() {
        let now = 1_000 + DEFAULT_STALE_SECONDS + 1;
        let state = state_from_cache(
            "id",
            "Claude",
            None,
            None,
            cache_of(STATUSLINE),
            DEFAULT_STALE_SECONDS,
            now,
        );
        assert_eq!(state.status, "stale");
        assert!(state.error.is_some());
        assert_eq!(state.windows.len(), 2);
    }

    #[test]
    fn clock_going_backwards_does_not_mark_stale() {
        let state = state_from_cache(
            "id",
            "Claude",
            None,
            None,
            cache_of(STATUSLINE),
            DEFAULT_STALE_SECONDS,
            0,
        );
        assert_eq!(state.status, "ok");
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
    fn missing_claude_config_is_not_an_error() {
        let identity = read_identity(Path::new("/nonexistent/profile"));
        assert!(identity.email.is_none());
        assert!(identity.plan.is_none());
    }
}

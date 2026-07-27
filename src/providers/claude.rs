use std::fs;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{AccountState, QuotaWindow};

/// Пока порог не задан в конфиге, данные Claude считаются устаревшими через сутки.
pub const DEFAULT_STALE_SECONDS: i64 = 86_400;

const FIVE_HOUR_MINS: u64 = 300;
const WEEKLY_MINS: u64 = 10_080;

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaudeCache {
    pub updated_at: i64,
    pub model: Option<String>,
    pub five_hour: Option<QuotaWindow>,
    pub weekly: Option<QuotaWindow>,
}

/// Payload, который Claude Code передаёт status-line команде в stdin.
/// Схема совпадает с документацией `claude` 2.1.220: `rate_limits.five_hour`
/// и `rate_limits.seven_day` присутствуют только у подписчиков и только после
/// первого ответа модели.
#[derive(Debug, Deserialize)]
pub struct ClaudeInput {
    model: Option<ClaudeModel>,
    rate_limits: Option<ClaudeRateLimits>,
}

#[derive(Debug, Deserialize)]
struct ClaudeModel {
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeRateLimits {
    five_hour: Option<ClaudeWindow>,
    seven_day: Option<ClaudeWindow>,
}

#[derive(Debug, Deserialize)]
struct ClaudeWindow {
    used_percentage: f64,
    resets_at: Option<i64>,
}

impl ClaudeInput {
    /// Превращает payload в кеш. Чистая функция — тестируется на фикстуре.
    pub fn into_cache(self, now: i64) -> ClaudeCache {
        let window = |source: Option<ClaudeWindow>, duration| {
            source.map(|window| {
                QuotaWindow::new(window.used_percentage, Some(duration), window.resets_at)
            })
        };
        let (five_hour, weekly) = match self.rate_limits {
            Some(limits) => (
                window(limits.five_hour, FIVE_HOUR_MINS),
                window(limits.seven_day, WEEKLY_MINS),
            ),
            None => (None, None),
        };

        ClaudeCache {
            updated_at: now,
            model: self.model.and_then(|model| model.display_name),
            five_hour,
            weekly,
        }
    }
}

impl ClaudeCache {
    /// Строка, которую hook печатает обратно в Claude Code.
    pub fn status_line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(model) = &self.model {
            parts.push(format!("[{model}]"));
        }
        if let Some(window) = &self.five_hour {
            parts.push(format!("5h {:.0}% left", window.remaining_percent));
        }
        if let Some(window) = &self.weekly {
            parts.push(format!("7d {:.0}% left", window.remaining_percent));
        }
        parts.join(" · ")
    }
}

pub async fn fetch(
    id: &str,
    name: &str,
    plan: Option<String>,
    stale_seconds: i64,
) -> Result<AccountState> {
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

    Ok(state_from_cache(
        id,
        name,
        plan,
        cache,
        stale_seconds,
        crate::util::unix_now(),
    ))
}

/// Чистая часть: кеш плюс текущее время дают `AccountState`.
pub fn state_from_cache(
    id: &str,
    name: &str,
    plan: Option<String>,
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
        email: None,
        model: cache.model,
        five_hour: cache.five_hour,
        weekly: cache.weekly,
        balances: Vec::new(),
        error: is_stale.then(|| {
            let hours = age / 3600;
            format!("Данные Claude обновлялись {hours} ч назад")
        }),
        updated_at: cache.updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUSLINE: &str = include_str!("../../tests/fixtures/claude_statusline.json");

    fn cache_from_fixture() -> ClaudeCache {
        serde_json::from_str::<ClaudeInput>(STATUSLINE)
            .expect("фикстура должна разбираться")
            .into_cache(1_000)
    }

    #[test]
    fn parses_real_statusline_payload() {
        let cache = cache_from_fixture();
        assert_eq!(cache.model.as_deref(), Some("Opus 5"));

        let five = cache.five_hour.as_ref().expect("должно быть 5h окно");
        assert_eq!(five.used_percent, 42.5);
        assert_eq!(five.remaining_percent, 57.5);
        assert_eq!(five.duration_mins, Some(FIVE_HOUR_MINS));
        assert_eq!(five.resets_at, Some(1_800_000_000));

        let weekly = cache.weekly.as_ref().expect("должно быть недельное окно");
        assert_eq!(weekly.remaining_percent, 90.0);
        assert_eq!(weekly.duration_mins, Some(WEEKLY_MINS));
    }

    #[test]
    fn formats_status_line_with_remaining_not_used() {
        assert_eq!(
            cache_from_fixture().status_line(),
            "[Opus 5] · 5h 58% left · 7d 90% left"
        );
    }

    #[test]
    fn payload_without_rate_limits_is_accepted() {
        // Так выглядит payload у не-подписчика или до первого ответа модели.
        let input: ClaudeInput =
            serde_json::from_str(r#"{"model":{"display_name":"Opus 5"}}"#).unwrap();
        let cache = input.into_cache(1_000);
        assert!(cache.five_hour.is_none());
        assert!(cache.weekly.is_none());
        assert_eq!(cache.status_line(), "[Opus 5]");
    }

    #[test]
    fn payload_with_only_one_window_keeps_the_other_empty() {
        let input: ClaudeInput = serde_json::from_str(
            r#"{"rate_limits":{"five_hour":{"used_percentage":10.0,"resets_at":5}}}"#,
        )
        .unwrap();
        let cache = input.into_cache(1_000);
        assert!(cache.five_hour.is_some());
        assert!(cache.weekly.is_none());
    }

    #[test]
    fn fresh_cache_is_ok() {
        let cache = cache_from_fixture();
        let state = state_from_cache("id", "Claude", None, cache, DEFAULT_STALE_SECONDS, 1_500);
        assert_eq!(state.status, "ok");
        assert!(state.error.is_none());
        assert_eq!(state.updated_at, 1_000);
    }

    #[test]
    fn cache_older_than_threshold_is_stale_but_keeps_quota() {
        let cache = cache_from_fixture();
        let now = 1_000 + DEFAULT_STALE_SECONDS + 1;
        let state = state_from_cache("id", "Claude", None, cache, DEFAULT_STALE_SECONDS, now);
        assert_eq!(state.status, "stale");
        assert!(state.error.is_some());
        // Устаревание не должно стирать сами лимиты.
        assert!(state.five_hour.is_some());
        assert!(state.weekly.is_some());
    }

    #[test]
    fn threshold_is_configurable() {
        let cache = cache_from_fixture();
        let state = state_from_cache("id", "Claude", None, cache, 10, 1_020);
        assert_eq!(state.status, "stale");
    }

    #[test]
    fn clock_going_backwards_does_not_mark_stale() {
        let cache = cache_from_fixture();
        let state = state_from_cache("id", "Claude", None, cache, DEFAULT_STALE_SECONDS, 0);
        assert_eq!(state.status, "ok");
    }
}

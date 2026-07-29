use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{AccountState, QuotaWindow, FIVE_HOUR_MINS, WEEKLY_MINS};

#[derive(Debug, Deserialize)]
struct StatusPayload {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    plan_tier: Option<String>,
    #[serde(default)]
    model: Option<ModelInfo>,
    #[serde(default)]
    quota: BTreeMap<String, QuotaInfo>,
}

#[derive(Debug, Deserialize)]
struct ModelInfo {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuotaInfo {
    #[serde(default)]
    remaining_fraction: Option<f64>,
    #[allow(dead_code)]
    #[serde(default)]
    reset_time: Option<String>,
    #[serde(default)]
    reset_in_seconds: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Snapshot {
    email: Option<String>,
    plan: Option<String>,
    model: Option<String>,
    windows: Vec<QuotaWindow>,
    updated_at: i64,
}

pub fn snapshot_path(account_id: &str) -> PathBuf {
    crate::util::runtime_dir().join(format!("antigravity-usage-{account_id}.json"))
}

pub fn hook_script_path(account_id: &str) -> Result<PathBuf> {
    let base = dirs::config_dir().context("Не удалось определить каталог конфигурации")?;
    Ok(base
        .join("ai-usage")
        .join("hooks")
        .join(format!("agy-{account_id}.sh")))
}

pub fn install_hook_script(account_id: &str) -> Result<PathBuf> {
    crate::util::validate_id(account_id)?;
    let path = hook_script_path(account_id)?;
    let executable = std::env::current_exe()?;
    let body = format!(
        "#!/bin/sh\nexec {} agy-hook --account {}\n",
        crate::util::shell_single_quote(&executable.to_string_lossy()),
        crate::util::shell_single_quote(account_id),
    );
    crate::util::atomic_write_mode(&path, body.as_bytes(), Some(0o700))?;
    Ok(path)
}

pub fn remove_hook_script(account_id: &str) -> Result<bool> {
    let path = hook_script_path(account_id)?;
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).with_context(|| format!("Не удалось удалить {}", path.display()))?;
    Ok(true)
}

/// Official `agy` custom status-line hook.
///
/// Antigravity sends account and quota metadata as JSON on stdin. The hook
/// stores only the display fields needed by the widget; credentials and the
/// full provider response never leave `agy`.
pub fn status_line_from_payload(account_id: &str, raw: &str) -> Result<String> {
    crate::util::validate_id(account_id)?;
    let now = crate::util::unix_now();
    let Some(snapshot) = snapshot_from_payload(raw, now)? else {
        return Ok("AI Usage · ожидание квоты".to_owned());
    };

    let line = status_line_text(&snapshot);
    let path = snapshot_path(account_id);
    let bytes = serde_json::to_vec_pretty(&snapshot)?;
    crate::util::atomic_write_mode(&path, &bytes, Some(0o600))?;
    Ok(line)
}

pub async fn fetch(id: &str, name: &str, stale_seconds: i64) -> Result<AccountState> {
    let path = snapshot_path(id);
    if !path.exists() {
        let mut state = AccountState::error(
            id,
            name,
            "antigravity",
            "Открой agy и подключи status-line hook",
        );
        state.status = "waiting".to_owned();
        return Ok(state);
    }

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("Не удалось прочитать {}", path.display()))?;
    let snapshot: Snapshot = serde_json::from_str(&raw)
        .with_context(|| format!("Некорректный snapshot {}", path.display()))?;

    let now = crate::util::unix_now();
    let age = now.saturating_sub(snapshot.updated_at).max(0);
    let stale = age > stale_seconds;

    Ok(AccountState {
        id: id.to_owned(),
        name: name.to_owned(),
        provider: "antigravity".to_owned(),
        status: if stale { "stale" } else { "ok" }.to_owned(),
        plan: snapshot.plan.as_deref().map(crate::util::humanize_plan),
        email: snapshot.email,
        model: snapshot.model,
        windows: snapshot.windows,
        balances: Vec::new(),
        error: stale.then(|| {
            format!(
                "agy не обновлял квоту {}; открой agy или выполни /usage",
                format_age(age)
            )
        }),
        updated_at: snapshot.updated_at,
    })
}

fn snapshot_from_payload(raw: &str, now: i64) -> Result<Option<Snapshot>> {
    let payload: StatusPayload =
        serde_json::from_str(raw).context("Некорректный JSON от agy status line")?;
    let model = payload.model.and_then(|model| {
        model
            .display_name
            .filter(|value| !value.trim().is_empty())
            .or_else(|| model.id.filter(|value| !value.trim().is_empty()))
    });

    let mut windows: Vec<QuotaWindow> = payload
        .quota
        .into_iter()
        .filter_map(|(key, quota)| quota_window(&key, quota, now))
        .collect();

    if windows.is_empty() {
        return Ok(None);
    }

    sort_windows(&mut windows, model.as_deref());
    // The first window represents the active model pool. Treat it as the
    // account-wide headline; the remaining pools stay scoped so an exhausted
    // inactive pool does not force the panel summary to 0%.
    if let Some(active) = windows.first_mut() {
        active.scope = None;
    }

    Ok(Some(Snapshot {
        email: payload.email.filter(|value| !value.trim().is_empty()),
        plan: payload.plan_tier.filter(|value| !value.trim().is_empty()),
        model,
        windows,
        updated_at: now,
    }))
}

fn quota_window(key: &str, quota: QuotaInfo, now: i64) -> Option<QuotaWindow> {
    let remaining = quota.remaining_fraction?;
    if !remaining.is_finite() {
        return None;
    }
    let remaining_percent = (remaining * 100.0).clamp(0.0, 100.0);
    let resets_at = quota
        .reset_in_seconds
        .filter(|seconds| *seconds > 0)
        .map(|seconds| now.saturating_add(seconds));

    Some(
        QuotaWindow::new(
            key,
            label_for_key(key),
            100.0 - remaining_percent,
            duration_for_key(key),
            resets_at,
        )
        .with_scope(scope_for_key(key)),
    )
}

fn duration_for_key(key: &str) -> Option<u64> {
    let normalized = key.to_ascii_lowercase();
    if normalized.contains("weekly") || normalized.contains("week") {
        Some(WEEKLY_MINS)
    } else if normalized.contains("five_hour")
        || normalized.contains("five-hour")
        || normalized.contains("5h")
    {
        Some(FIVE_HOUR_MINS)
    } else {
        None
    }
}

fn scope_for_key(key: &str) -> Option<String> {
    let normalized = key.to_ascii_lowercase();
    if normalized.contains("gemini") {
        Some("Gemini".to_owned())
    } else if normalized.starts_with("3p") || normalized.contains("third-party") {
        Some("Сторонние модели".to_owned())
    } else {
        None
    }
}

fn label_for_key(key: &str) -> String {
    match key {
        "gemini-weekly" => "Gemini · Неделя".to_owned(),
        "3p-weekly" => "Сторонние модели · Неделя".to_owned(),
        _ => key
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn sort_windows(windows: &mut [QuotaWindow], model: Option<&str>) {
    let model = model.unwrap_or_default().to_ascii_lowercase();
    windows.sort_by(|left, right| {
        let left_priority = bucket_priority(&left.key, &model);
        let right_priority = bucket_priority(&right.key, &model);
        left_priority
            .cmp(&right_priority)
            .then_with(|| left.duration_mins.cmp(&right.duration_mins))
            .then_with(|| left.key.cmp(&right.key))
    });
}

fn bucket_priority(key: &str, model: &str) -> u8 {
    let key = key.to_ascii_lowercase();
    let preferred = if model.contains("gemini") {
        key.contains("gemini")
    } else if !model.is_empty() {
        key.starts_with("3p") || key.contains("third-party")
    } else {
        false
    };
    if preferred {
        0
    } else {
        1
    }
}

fn status_line_text(snapshot: &Snapshot) -> String {
    let percent = snapshot
        .windows
        .first()
        .map(|window| window.remaining_percent.round() as i64);
    match percent {
        Some(percent) => format!("AI Usage · {percent}%"),
        None => "AI Usage · ожидание квоты".to_owned(),
    }
}

fn format_age(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds} с")
    } else if seconds < 3_600 {
        format!("{} мин", seconds / 60)
    } else if seconds < 86_400 {
        format!("{} ч {} мин", seconds / 3_600, (seconds % 3_600) / 60)
    } else {
        format!("{} д {} ч", seconds / 86_400, (seconds % 86_400) / 3_600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "model": {"id": "gemini-3.5-flash", "display_name": "Gemini 3.5 Flash (High)"},
      "quota": {
        "3p-weekly": {
          "remaining_fraction": 0.42,
          "reset_time": "2026-08-01T00:00:00Z",
          "reset_in_seconds": 7200
        },
        "gemini-weekly": {
          "remaining_fraction": 0.9378,
          "reset_time": "2026-08-05T07:50:32Z",
          "reset_in_seconds": 560580
        },
        "future-bucket": {
          "remaining_fraction": 0.25,
          "reset_in_seconds": 60
        }
      },
      "plan_tier": "Pro",
      "email": "developer@example.com"
    }"#;

    #[test]
    fn maps_official_payload_into_open_quota_windows() {
        let snapshot = snapshot_from_payload(SAMPLE, 1_000).unwrap().unwrap();
        assert_eq!(snapshot.email.as_deref(), Some("developer@example.com"));
        assert_eq!(snapshot.plan.as_deref(), Some("Pro"));
        assert_eq!(snapshot.model.as_deref(), Some("Gemini 3.5 Flash (High)"));
        assert_eq!(snapshot.windows.len(), 3);
        assert_eq!(snapshot.windows[0].key, "gemini-weekly");
        assert_eq!(snapshot.windows[0].remaining_percent.round(), 94.0);
        assert_eq!(snapshot.windows[0].resets_at, Some(561_580));
        assert_eq!(snapshot.windows[0].scope, None);
        assert_eq!(
            snapshot
                .windows
                .iter()
                .find(|window| window.key == "3p-weekly")
                .and_then(|window| window.scope.as_deref()),
            Some("Сторонние модели")
        );
        assert!(snapshot
            .windows
            .iter()
            .any(|window| window.key == "future-bucket"));
    }

    #[test]
    fn active_third_party_model_prioritizes_3p_bucket() {
        let raw = SAMPLE.replace("Gemini 3.5 Flash (High)", "Claude Opus 4.1");
        let snapshot = snapshot_from_payload(&raw, 1_000).unwrap().unwrap();
        assert_eq!(snapshot.windows[0].key, "3p-weekly");
    }

    #[test]
    fn empty_quota_does_not_overwrite_last_good_snapshot() {
        let result = snapshot_from_payload(r#"{"quota": {}, "plan_tier": "Pro"}"#, 1_000).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn clamps_provider_percentages() {
        let raw = r#"{
          "quota": {
            "too-high": {"remaining_fraction": 2.0, "reset_in_seconds": 10},
            "negative": {"remaining_fraction": -1.0, "reset_in_seconds": 10}
          }
        }"#;
        let snapshot = snapshot_from_payload(raw, 1_000).unwrap().unwrap();
        let high = snapshot
            .windows
            .iter()
            .find(|window| window.key == "too-high")
            .unwrap();
        let low = snapshot
            .windows
            .iter()
            .find(|window| window.key == "negative")
            .unwrap();
        assert_eq!(high.remaining_percent, 100.0);
        assert_eq!(low.remaining_percent, 0.0);
    }

    #[test]
    fn ignores_non_finite_quota_values() {
        let quota = QuotaInfo {
            remaining_fraction: Some(f64::NAN),
            reset_time: None,
            reset_in_seconds: Some(10),
        };
        assert!(quota_window("bad", quota, 1_000).is_none());
    }
}

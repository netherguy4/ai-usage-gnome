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
    /// Абсолютный момент сброса. Запасной источник: `agy` 1.1.8 присылает и
    /// его, и `reset_in_seconds`, но опираться на одно поле рискованно.
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

/// Снимок живёт вне tmpfs: `agy` — единственный источник этих цифр, и после
/// перезагрузки восстановить их некому, пока пользователь снова не откроет CLI.
pub fn snapshot_path(account_id: &str) -> PathBuf {
    crate::util::persistent_state_dir().join(format!("antigravity-usage-{account_id}.json"))
}

/// Путь, по которому снимок лежал до переезда на постоянный каталог.
fn legacy_snapshot_path(account_id: &str) -> PathBuf {
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
    adopt_legacy_snapshot(id, &path);
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
    let stale = age > stale_threshold(&snapshot.windows, stale_seconds);

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

/// Переносит снимок из tmpfs, если он остался там от прошлой версии.
///
/// Без этого обновление на живой сессии стоило бы пользователю ещё одного
/// «открой agy», хотя данные никуда не делись. Молча: снимок восстановим, и
/// срывать из-за него обновление аккаунта незачем.
fn adopt_legacy_snapshot(account_id: &str, path: &PathBuf) {
    if path.exists() {
        return;
    }
    let legacy = legacy_snapshot_path(account_id);
    if legacy == *path || !legacy.exists() {
        return;
    }
    let Ok(bytes) = fs::read(&legacy) else {
        return;
    };
    if crate::util::atomic_write_mode(path, &bytes, Some(0o600)).is_ok() {
        let _ = fs::remove_file(&legacy);
    }
}

/// Через сколько снимок `agy` перестаёт что-либо значить.
///
/// Глобальный `stale_seconds` рассчитан на провайдеров, которых демон
/// опрашивает сам: там пауза в опросе и правда означает «цифры могли уйти».
/// С `agy` иначе — закрытый CLI квоту не тратит, а открытый сам дёргает hook,
/// поэтому снимок остаётся точным всё окно, которое описывает. Порог в 15
/// минут висел бы предупреждением почти всегда, ничего при этом не сообщая.
///
/// Отсчитываем от самого короткого окна: когда оно прожито целиком, данные
/// заведомо описывают уже прошедший период. Глобальная настройка остаётся
/// нижней границей, чтобы её нельзя было случайно ослабить.
fn stale_threshold(windows: &[QuotaWindow], stale_seconds: i64) -> i64 {
    windows
        .iter()
        .filter_map(|window| window.duration_mins)
        .min()
        .and_then(|mins| i64::try_from(mins).ok())
        .map(|mins| mins.saturating_mul(60))
        .unwrap_or(stale_seconds)
        .max(stale_seconds)
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

    // Квота Antigravity — это семейства моделей, у каждого своё пятичасовое и
    // недельное окно. Активное семейство целиком считается общим лимитом
    // аккаунта: исчерпанное недельное окно блокирует работу так же, как
    // пятичасовое. Чужие семейства остаются scoped — там достаточно сменить
    // модель, и панель не обязана падать в 0%.
    let active = active_family(model.as_deref());
    let mut windows: Vec<QuotaWindow> = payload
        .quota
        .into_iter()
        .filter_map(|(key, quota)| quota_window(&key, quota, now, active))
        .collect();

    if windows.is_empty() {
        return Ok(None);
    }

    sort_windows(&mut windows, active);

    Ok(Some(Snapshot {
        email: payload.email.filter(|value| !value.trim().is_empty()),
        plan: payload.plan_tier.filter(|value| !value.trim().is_empty()),
        model,
        windows,
        updated_at: now,
    }))
}

fn quota_window(
    key: &str,
    quota: QuotaInfo,
    now: i64,
    active: Option<&str>,
) -> Option<QuotaWindow> {
    let remaining = quota.remaining_fraction?;
    if !remaining.is_finite() {
        return None;
    }
    let remaining_percent = (remaining * 100.0).clamp(0.0, 100.0);
    let resets_at = quota
        .reset_in_seconds
        .filter(|seconds| *seconds > 0)
        .map(|seconds| now.saturating_add(seconds))
        .or_else(|| quota.reset_time.as_deref().and_then(parse_rfc3339));

    let family = family_for_key(key);
    let scope = if Some(family.as_str()) == active {
        None
    } else {
        scope_for_family(&family)
    };

    Some(
        QuotaWindow::new(
            key,
            label_for_key(key),
            100.0 - remaining_percent,
            duration_for_key(key),
            resets_at,
        )
        .with_scope(scope),
    )
}

/// Окна, которые `agy` вешает на одно семейство: `gemini-5h`, `gemini-weekly`.
const WINDOW_SUFFIXES: [&str; 5] = ["5h", "five_hour", "five-hour", "weekly", "week"];

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

/// Семейство моделей, к которому привязан bucket.
///
/// `gemini-5h` и `gemini-weekly` — это одно семейство с двумя окнами, и панели
/// важно различать именно семейства: исчерпанное окно активного блокирует
/// работу, чужого — нет. Ключ с незнакомым окончанием остаётся отдельным
/// семейством целиком, поэтому будущие bucket-ы не склеиваются наугад.
fn family_for_key(key: &str) -> String {
    let normalized = key.to_ascii_lowercase();
    for suffix in WINDOW_SUFFIXES {
        for separator in ['-', '_'] {
            if let Some(head) = normalized.strip_suffix(&format!("{separator}{suffix}")) {
                return head.to_owned();
            }
        }
    }
    normalized
}

fn scope_for_family(family: &str) -> Option<String> {
    match family {
        "gemini" => Some("Gemini".to_owned()),
        "3p" | "third-party" | "third_party" => Some("Сторонние модели".to_owned()),
        // Незнакомое семейство считаем общим лимитом: пропустить блокирующее
        // окно хуже, чем лишний раз показать 0%.
        _ => None,
    }
}

/// Семейство, которым `agy` пользуется прямо сейчас.
///
/// Antigravity гоняет и Gemini, и сторонние модели через один аккаунт, но
/// квоту тратит только активная. Если модель неизвестна, ни одно семейство не
/// объявляется общим — панель тогда покажет худшее окно, а не случайное.
fn active_family(model: Option<&str>) -> Option<&'static str> {
    let model = model?.trim().to_ascii_lowercase();
    if model.is_empty() {
        None
    } else if model.contains("gemini") {
        Some("gemini")
    } else {
        Some("3p")
    }
}

fn label_for_key(key: &str) -> String {
    let family = family_for_key(key);
    let duration = duration_for_key(key);
    let window = crate::model::label_for_duration(duration);
    match (scope_for_family(&family), duration) {
        (Some(scope), _) => format!("{scope} · {window}"),
        (None, Some(_)) => format!("{} · {window}", title_case(&family)),
        // Ключ без узнаваемого окна показываем как есть: терять его нельзя.
        (None, None) => title_case(key),
    }
}

fn title_case(value: &str) -> String {
    value
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
        .join(" ")
}

fn sort_windows(windows: &mut [QuotaWindow], active: Option<&str>) {
    windows.sort_by(|left, right| {
        let left_priority = family_priority(&left.key, active);
        let right_priority = family_priority(&right.key, active);
        left_priority
            .cmp(&right_priority)
            .then_with(|| left.duration_mins.cmp(&right.duration_mins))
            .then_with(|| left.key.cmp(&right.key))
    });
}

fn family_priority(key: &str, active: Option<&str>) -> u8 {
    match active {
        Some(active) if family_for_key(key) == active => 0,
        _ => 1,
    }
}

fn status_line_text(snapshot: &Snapshot) -> String {
    // Худшее из общих окон, а не первое: запас в пятичасовом окне ничего не
    // значит, когда недельное того же семейства уже исчерпано.
    let general = snapshot
        .windows
        .iter()
        .filter(|window| window.scope.is_none())
        .map(|window| window.remaining_percent)
        .fold(f64::INFINITY, f64::min);
    let percent = if general.is_finite() {
        Some(general)
    } else {
        snapshot
            .windows
            .iter()
            .map(|window| window.remaining_percent)
            .reduce(f64::min)
    };
    match percent {
        Some(percent) => format!("AI Usage · {}%", percent.round() as i64),
        None => "AI Usage · ожидание квоты".to_owned(),
    }
}

/// Минимальный разбор RFC 3339 в unix-время.
///
/// Нужен только как запасной путь для `reset_time`, поэтому тянуть ради него
/// `chrono` в зависимости не стоит.
fn parse_rfc3339(value: &str) -> Option<i64> {
    let (date, rest) = value.trim().split_once(['T', 't'])?;
    let (time, offset) = split_offset(rest)?;

    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    if date.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut time = time.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next().unwrap_or("0").split('.').next()?.parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second - offset)
}

/// Делит хвост на время и смещение в секундах.
fn split_offset(rest: &str) -> Option<(&str, i64)> {
    if let Some(time) = rest.strip_suffix(['Z', 'z']) {
        return Some((time, 0));
    }
    let index = rest.rfind(['+', '-'])?;
    let (time, offset) = rest.split_at(index);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let mut parts = offset[1..].split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next().unwrap_or("0").parse().ok()?;
    Some((time, sign * (hours * 3_600 + minutes * 60)))
}

/// Число дней от 1970-01-01. Алгоритм `days_from_civil` Ховарда Хиннанта:
/// точен для любой даты григорианского календаря и не требует таблиц.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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

    /// Живой payload `agy` 1.1.8, снятый на Google AI Pro 29 июля 2026 года.
    /// Email заменён; остальное — как приходит из CLI, включая четыре bucket-а:
    /// два семейства моделей по два окна каждое.
    const SAMPLE: &str = r#"{
      "cwd": "/tmp",
      "model": {
        "id": "Gemini 3.6 Flash (High)",
        "display_name": "Gemini 3.6 Flash (High)",
        "effort": "high"
      },
      "version": "1.1.8",
      "product": "antigravity",
      "quota": {
        "3p-5h": {
          "remaining_fraction": 1,
          "reset_time": "2026-07-29T19:05:06Z",
          "reset_in_seconds": 17998
        },
        "3p-weekly": {
          "remaining_fraction": 1,
          "reset_time": "2026-08-05T14:05:06Z",
          "reset_in_seconds": 604798
        },
        "gemini-5h": {
          "remaining_fraction": 0.42,
          "reset_time": "2026-07-29T19:05:06Z",
          "reset_in_seconds": 17998
        },
        "gemini-weekly": {
          "remaining_fraction": 0.9378,
          "reset_time": "2026-08-05T14:05:06Z",
          "reset_in_seconds": 604798
        }
      },
      "agent_state": "initializing",
      "plan_tier": "Google AI Pro",
      "email": "developer@example.com"
    }"#;

    /// `agy` — единственный источник этих цифр. Снимок на tmpfs означал бы
    /// «открой agy» после каждой перезагрузки, хотя квота никуда не делась.
    #[test]
    fn the_snapshot_does_not_live_on_tmpfs() {
        let path = snapshot_path("agy-main");
        assert_ne!(path, legacy_snapshot_path("agy-main"));
        assert!(
            !path.starts_with(crate::util::runtime_dir()),
            "снимок оказался в runtime-каталоге: {}",
            path.display()
        );
        assert!(path.ends_with("antigravity-usage-agy-main.json"));
    }

    #[test]
    fn a_snapshot_left_in_the_runtime_dir_is_adopted() {
        let legacy = legacy_snapshot_path("adopt-test");
        let target = std::env::temp_dir().join(format!(
            "ai-usage-adopt-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_file(&target);
        crate::util::atomic_write_mode(&legacy, b"{\"windows\":[]}", Some(0o600)).unwrap();

        adopt_legacy_snapshot("adopt-test", &target);

        assert!(target.exists(), "снимок не переехал");
        assert!(!legacy.exists(), "старая копия осталась на tmpfs");
        let _ = fs::remove_file(&target);
    }

    fn window<'a>(snapshot: &'a Snapshot, key: &str) -> &'a QuotaWindow {
        snapshot
            .windows
            .iter()
            .find(|window| window.key == key)
            .unwrap()
    }

    #[test]
    fn maps_official_payload_into_open_quota_windows() {
        let snapshot = snapshot_from_payload(SAMPLE, 1_000).unwrap().unwrap();
        assert_eq!(snapshot.email.as_deref(), Some("developer@example.com"));
        assert_eq!(snapshot.plan.as_deref(), Some("Google AI Pro"));
        assert_eq!(snapshot.model.as_deref(), Some("Gemini 3.6 Flash (High)"));
        assert_eq!(snapshot.windows.len(), 4);

        let gemini = window(&snapshot, "gemini-5h");
        assert_eq!(gemini.remaining_percent.round(), 42.0);
        assert_eq!(gemini.resets_at, Some(18_998));
        assert_eq!(gemini.duration_mins, Some(FIVE_HOUR_MINS));
        assert_eq!(gemini.label, "Gemini · 5 часов");

        assert_eq!(window(&snapshot, "gemini-weekly").label, "Gemini · Неделя");
        assert_eq!(
            window(&snapshot, "3p-5h").label,
            "Сторонние модели · 5 часов"
        );
        assert_eq!(
            window(&snapshot, "3p-weekly").label,
            "Сторонние модели · Неделя"
        );
    }

    /// Главный смысл правки: у активного семейства общими становятся **оба**
    /// окна. Иначе исчерпанный недельный лимит не опустит число в панели —
    /// `panelWindow()` просто не увидит scoped-окно.
    #[test]
    fn every_window_of_the_active_family_counts_as_a_general_limit() {
        let snapshot = snapshot_from_payload(SAMPLE, 1_000).unwrap().unwrap();
        assert_eq!(window(&snapshot, "gemini-5h").scope, None);
        assert_eq!(window(&snapshot, "gemini-weekly").scope, None);
        assert_eq!(
            window(&snapshot, "3p-5h").scope.as_deref(),
            Some("Сторонние модели")
        );
        assert_eq!(
            window(&snapshot, "3p-weekly").scope.as_deref(),
            Some("Сторонние модели")
        );
    }

    #[test]
    fn active_third_party_model_prioritizes_3p_family() {
        let raw = SAMPLE.replace("Gemini 3.6 Flash (High)", "Claude Opus 4.1");
        let snapshot = snapshot_from_payload(&raw, 1_000).unwrap().unwrap();
        assert_eq!(snapshot.windows[0].key, "3p-5h");
        assert_eq!(window(&snapshot, "3p-weekly").scope, None);
        assert_eq!(
            window(&snapshot, "gemini-5h").scope.as_deref(),
            Some("Gemini")
        );
    }

    /// Без активной модели нельзя объявлять общим случайный bucket: панель
    /// должна показать худшее окно, а не первое попавшееся.
    #[test]
    fn an_unknown_active_model_leaves_every_family_scoped() {
        let raw = SAMPLE.replace(r#""display_name": "Gemini 3.6 Flash (High)","#, "");
        let raw = raw.replace(r#""id": "Gemini 3.6 Flash (High)","#, "");
        let snapshot = snapshot_from_payload(&raw, 1_000).unwrap().unwrap();
        assert_eq!(snapshot.model, None);
        assert!(snapshot.windows.iter().all(|window| window.scope.is_some()));
    }

    #[test]
    fn an_unknown_bucket_survives_with_a_readable_label() {
        let raw = SAMPLE.replace(
            r#""3p-5h": {"#,
            r#""future-pool": {"remaining_fraction": 0.25, "reset_in_seconds": 60}, "3p-5h": {"#,
        );
        let snapshot = snapshot_from_payload(&raw, 1_000).unwrap().unwrap();
        let future = window(&snapshot, "future-pool");
        assert_eq!(future.label, "Future Pool");
        assert_eq!(future.duration_mins, None);
        // Незнакомое семейство считаем общим: пропустить блокирующий лимит
        // хуже, чем лишний раз показать 0%.
        assert_eq!(future.scope, None);
    }

    #[test]
    fn falls_back_to_reset_time_when_seconds_are_missing() {
        let raw = r#"{
          "quota": {
            "gemini-weekly": {
              "remaining_fraction": 0.5,
              "reset_time": "2026-08-05T14:05:06Z"
            }
          }
        }"#;
        let snapshot = snapshot_from_payload(raw, 1_000).unwrap().unwrap();
        assert_eq!(
            window(&snapshot, "gemini-weekly").resets_at,
            Some(1_785_938_706)
        );
    }

    #[test]
    fn parses_rfc3339_with_and_without_offsets() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-07-29T19:05:06Z"), Some(1_785_351_906));
        // Високосный год и смещение часового пояса.
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(
            parse_rfc3339("2026-07-29T22:05:06+03:00"),
            Some(1_785_351_906)
        );
        assert_eq!(
            parse_rfc3339("2026-07-29T19:05:06.500Z"),
            Some(1_785_351_906)
        );
        assert_eq!(parse_rfc3339("не дата"), None);
        assert_eq!(parse_rfc3339("2026-13-01T00:00:00Z"), None);
    }

    /// Пятнадцать минут без `agy` — не повод для предупреждения: закрытый CLI
    /// квоту не тратит, и снимок всё ещё точен.
    #[test]
    fn a_short_pause_in_agy_is_not_staleness() {
        let snapshot = snapshot_from_payload(SAMPLE, 1_000).unwrap().unwrap();
        // Самое короткое окно — пятичасовое.
        assert_eq!(stale_threshold(&snapshot.windows, 900), 5 * 3_600);
    }

    #[test]
    fn a_snapshot_without_durations_falls_back_to_the_global_threshold() {
        let raw = r#"{"quota": {"future-pool": {"remaining_fraction": 0.5}}}"#;
        let snapshot = snapshot_from_payload(raw, 1_000).unwrap().unwrap();
        assert_eq!(stale_threshold(&snapshot.windows, 900), 900);
    }

    /// Глобальная настройка остаётся нижней границей: ослабить её окном нельзя.
    #[test]
    fn the_global_threshold_is_a_floor() {
        let snapshot = snapshot_from_payload(SAMPLE, 1_000).unwrap().unwrap();
        assert_eq!(stale_threshold(&snapshot.windows, 86_400), 86_400);
    }

    /// Строка внутри `agy` обязана показывать то же, что и панель.
    #[test]
    fn the_status_line_reports_the_worst_general_window() {
        let snapshot = snapshot_from_payload(SAMPLE, 1_000).unwrap().unwrap();
        assert_eq!(status_line_text(&snapshot), "AI Usage · 42%");
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
        assert!(quota_window("bad", quota, 1_000, None).is_none());
    }
}

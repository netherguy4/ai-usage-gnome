use serde::{Deserialize, Serialize};

/// Формат `state.json`. Несовместимое изменение требует увеличения версии и
/// либо обратной совместимости UI, либо явной миграции.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaWindow {
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub duration_mins: Option<u64>,
    pub resets_at: Option<i64>,
}

impl QuotaWindow {
    pub fn new(used_percent: f64, duration_mins: Option<u64>, resets_at: Option<i64>) -> Self {
        // NaN не сравнивается, поэтому clamp его не спасёт: считаем такой
        // процент отсутствующим и показываем полный лимит.
        let used_percent = if used_percent.is_nan() {
            0.0
        } else {
            used_percent.clamp(0.0, 100.0)
        };
        Self {
            used_percent,
            remaining_percent: 100.0 - used_percent,
            duration_mins,
            resets_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceInfo {
    pub currency: String,
    pub total: String,
    pub granted: Option<String>,
    pub topped_up: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountState {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub status: String,
    pub plan: Option<String>,
    pub email: Option<String>,
    pub model: Option<String>,
    pub five_hour: Option<QuotaWindow>,
    pub weekly: Option<QuotaWindow>,
    pub balances: Vec<BalanceInfo>,
    pub error: Option<String>,
    pub updated_at: i64,
}

impl AccountState {
    pub fn error(id: &str, name: &str, provider: &str, message: impl Into<String>) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            provider: provider.to_owned(),
            status: "error".to_owned(),
            plan: None,
            email: None,
            model: None,
            five_hour: None,
            weekly: None,
            balances: Vec::new(),
            error: Some(message.into()),
            updated_at: crate::util::unix_now(),
        }
    }

    fn has_data(&self) -> bool {
        self.five_hour.is_some() || self.weekly.is_some() || !self.balances.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    pub schema_version: u32,
    pub updated_at: i64,
    pub accounts: Vec<AccountState>,
}

/// Сохраняет последние удачные данные при разовом сбое провайдера.
///
/// Без этого одна сетевая ошибка стирала квоту из панели целиком, хотя
/// минутной давности цифры всё ещё полезны. Аккаунт помечается `stale`,
/// текст ошибки сохраняется, а `updated_at` остаётся от удачного ответа —
/// поэтому UI может показать реальный возраст данных.
pub fn merge_with_previous(
    previous: Option<AppState>,
    fresh: Vec<AccountState>,
) -> Vec<AccountState> {
    let Some(previous) = previous else {
        return fresh;
    };
    if previous.schema_version != SCHEMA_VERSION {
        return fresh;
    }

    fresh
        .into_iter()
        .map(|account| {
            if account.status != "error" || account.has_data() {
                return account;
            }
            let Some(old) = previous
                .accounts
                .iter()
                .find(|item| item.id == account.id && item.provider == account.provider)
            else {
                return account;
            };
            if !old.has_data() {
                return account;
            }

            AccountState {
                status: "stale".to_owned(),
                plan: old.plan.clone(),
                email: old.email.clone(),
                model: old.model.clone(),
                five_hour: old.five_hour.clone(),
                weekly: old.weekly.clone(),
                balances: old.balances.clone(),
                updated_at: old.updated_at,
                ..account
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(used: f64) -> QuotaWindow {
        QuotaWindow::new(used, Some(300), Some(42))
    }

    fn good(id: &str) -> AccountState {
        AccountState {
            id: id.to_owned(),
            name: "Codex".to_owned(),
            provider: "codex".to_owned(),
            status: "ok".to_owned(),
            plan: Some("plus".to_owned()),
            email: Some("user@example.com".to_owned()),
            model: None,
            five_hour: Some(window(25.0)),
            weekly: Some(window(54.0)),
            balances: Vec::new(),
            error: None,
            updated_at: 1_000,
        }
    }

    fn previous(accounts: Vec<AccountState>) -> Option<AppState> {
        Some(AppState {
            schema_version: SCHEMA_VERSION,
            updated_at: 1_000,
            accounts,
        })
    }

    #[test]
    fn clamps_percentage_into_range() {
        assert_eq!(QuotaWindow::new(-5.0, None, None).used_percent, 0.0);
        assert_eq!(QuotaWindow::new(-5.0, None, None).remaining_percent, 100.0);
        assert_eq!(QuotaWindow::new(150.0, None, None).used_percent, 100.0);
        assert_eq!(QuotaWindow::new(150.0, None, None).remaining_percent, 0.0);
    }

    #[test]
    fn treats_nan_as_zero_used() {
        let window = QuotaWindow::new(f64::NAN, None, None);
        assert_eq!(window.used_percent, 0.0);
        assert_eq!(window.remaining_percent, 100.0);
    }

    #[test]
    fn keeps_last_good_data_when_provider_fails() {
        let fresh = vec![AccountState::error("codex-main", "Codex", "codex", "сеть")];
        let merged = merge_with_previous(previous(vec![good("codex-main")]), fresh);

        assert_eq!(merged[0].status, "stale");
        assert_eq!(merged[0].error.as_deref(), Some("сеть"));
        assert_eq!(merged[0].weekly.as_ref().unwrap().used_percent, 54.0);
        assert_eq!(merged[0].plan.as_deref(), Some("plus"));
        // Возраст берётся от удачного ответа, иначе UI покажет свежесть, которой нет.
        assert_eq!(merged[0].updated_at, 1_000);
    }

    #[test]
    fn successful_refresh_overwrites_old_data() {
        let mut fresh = good("codex-main");
        fresh.weekly = Some(window(70.0));
        let merged = merge_with_previous(previous(vec![good("codex-main")]), vec![fresh]);
        assert_eq!(merged[0].status, "ok");
        assert_eq!(merged[0].weekly.as_ref().unwrap().used_percent, 70.0);
    }

    #[test]
    fn error_without_history_stays_an_error() {
        let fresh = vec![AccountState::error("codex-new", "Codex", "codex", "сеть")];
        let merged = merge_with_previous(previous(vec![good("codex-main")]), fresh);
        assert_eq!(merged[0].status, "error");
        assert!(merged[0].weekly.is_none());
    }

    #[test]
    fn does_not_borrow_data_from_a_different_provider_with_the_same_id() {
        let mut other = good("shared");
        other.provider = "claude".to_owned();
        let fresh = vec![AccountState::error("shared", "Codex", "codex", "сеть")];
        let merged = merge_with_previous(previous(vec![other]), fresh);
        assert_eq!(merged[0].status, "error");
    }

    #[test]
    fn ignores_state_written_by_a_different_schema_version() {
        let stale_schema = Some(AppState {
            schema_version: SCHEMA_VERSION + 1,
            updated_at: 1_000,
            accounts: vec![good("codex-main")],
        });
        let fresh = vec![AccountState::error("codex-main", "Codex", "codex", "сеть")];
        let merged = merge_with_previous(stale_schema, fresh);
        assert_eq!(merged[0].status, "error");
    }

    #[test]
    fn waiting_claude_account_is_not_backfilled() {
        // status 'waiting' — не ошибка, подменять его старыми данными нельзя.
        let mut fresh = AccountState::error("claude-main", "Claude", "claude", "ждём");
        fresh.status = "waiting".to_owned();
        let mut old = good("claude-main");
        old.provider = "claude".to_owned();

        let merged = merge_with_previous(previous(vec![old]), vec![fresh]);
        assert_eq!(merged[0].status, "waiting");
        assert!(merged[0].weekly.is_none());
    }
}

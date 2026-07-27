use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaWindow {
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub duration_mins: Option<u64>,
    pub resets_at: Option<i64>,
}

impl QuotaWindow {
    pub fn new(used_percent: f64, duration_mins: Option<u64>, resets_at: Option<i64>) -> Self {
        let used_percent = used_percent.clamp(0.0, 100.0);
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    pub schema_version: u32,
    pub updated_at: i64,
    pub accounts: Vec<AccountState>,
}

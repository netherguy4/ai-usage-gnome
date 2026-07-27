pub mod claude;
pub mod codex;
pub mod deepseek;

use crate::config::AccountConfig;
use crate::model::AccountState;

pub async fn fetch(account: AccountConfig, stale_seconds: i64) -> AccountState {
    let id = account.id().to_owned();
    let name = account.name().to_owned();
    let provider = account.provider();

    let result = match account {
        AccountConfig::Claude { id, name, plan, .. } => {
            claude::fetch(&id, &name, plan, stale_seconds).await
        }
        AccountConfig::Codex {
            id,
            name,
            codex_home,
            command,
            limit_id,
        } => codex::fetch(&id, &name, &codex_home, &command, &limit_id).await,
        AccountConfig::Deepseek {
            id,
            name,
            api_key_env,
            base_url,
        } => deepseek::fetch(&id, &name, &api_key_env, &base_url).await,
    };

    result.unwrap_or_else(|error| AccountState::error(&id, &name, provider, error.to_string()))
}

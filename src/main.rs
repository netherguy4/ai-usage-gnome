mod config;
mod model;
mod providers;
mod setup;
mod util;

use std::fs;
use std::io::Read;
use std::process::Command as StdCommand;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use config::AccountConfig;
use model::AppState;
use providers::claude::ClaudeInput;
use std::path::PathBuf;
use tokio::task::JoinSet;

#[derive(Debug, Parser)]
#[command(
    name = "ai-usage",
    version,
    about = "Claude, Codex and DeepSeek usage for GNOME"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run the background refresh loop.
    Daemon,
    /// Refresh once and print the resulting JSON.
    Once,
    /// Interactive account setup.
    Setup,
    /// Create an empty config if it does not exist.
    Init,
    /// Check configuration and external dependencies.
    Doctor,
    /// Manage accounts without editing config.toml by hand.
    #[command(subcommand)]
    Account(AccountCommand),
    /// Show or change global settings.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Claude Code status-line hook. Reads Claude JSON from stdin.
    ClaudeHook {
        #[arg(long)]
        account: String,
    },
    /// Restore Claude settings changed by the setup command.
    RestoreClaudeHooks,
}

#[derive(Debug, Subcommand)]
enum AccountCommand {
    /// List configured accounts.
    List,
    /// Add or update an account non-interactively.
    #[command(subcommand)]
    Add(AddCommand),
    /// Remove an account. Claude accounts also get their statusLine restored.
    Remove {
        id: String,
        /// Leave the Claude statusLine hook in place.
        #[arg(long)]
        keep_hook: bool,
    },
    /// Change the label shown in the panel.
    Rename { id: String, name: String },
}

#[derive(Debug, Subcommand)]
enum AddCommand {
    /// Claude Code profile backed by its own CLAUDE_CONFIG_DIR.
    Claude {
        #[command(flatten)]
        common: CommonArgs,
        #[arg(long, default_value = "~/.claude")]
        config_dir: PathBuf,
        #[arg(long)]
        plan: Option<String>,
        /// Do not touch settings.json; install the hook later with `setup`.
        #[arg(long)]
        no_hook: bool,
    },
    /// Codex profile backed by its own CODEX_HOME.
    Codex {
        #[command(flatten)]
        common: CommonArgs,
        #[arg(long, default_value = "~/.codex")]
        codex_home: PathBuf,
        #[arg(long, default_value = "codex")]
        command: String,
        #[arg(long, default_value = "codex")]
        limit_id: String,
    },
    /// DeepSeek API key. The key itself is read from stdin.
    Deepseek {
        #[command(flatten)]
        common: CommonArgs,
        #[arg(long, default_value = "https://api.deepseek.com")]
        base_url: String,
        /// Read the API key from stdin instead of prompting.
        #[arg(long)]
        api_key_stdin: bool,
    },
}

#[derive(Debug, Args)]
struct CommonArgs {
    #[arg(long)]
    id: String,
    #[arg(long)]
    name: Option<String>,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print the effective configuration.
    Show,
    /// Change refresh and staleness thresholds.
    Set {
        /// How often the daemon refreshes providers.
        #[arg(long)]
        refresh_seconds: Option<u64>,
        /// After how long Claude cache data counts as stale.
        #[arg(long)]
        stale_seconds: Option<i64>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    util::load_secrets_env()?;
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Daemon) {
        Commands::Daemon => daemon().await,
        Commands::Once => once().await,
        Commands::Setup => setup::run(),
        Commands::Init => init(),
        Commands::Doctor => doctor(),
        Commands::Account(command) => account(command),
        Commands::Config(command) => configure(command),
        Commands::ClaudeHook { account } => claude_hook(&account),
        Commands::RestoreClaudeHooks => setup::restore_claude_hooks(),
    }
}

fn account(command: AccountCommand) -> Result<()> {
    let mut config = config::load()?;
    match command {
        AccountCommand::List => {
            if config.accounts.is_empty() {
                println!("Аккаунтов нет. Добавь: ai-usage setup");
                return Ok(());
            }
            for account in &config.accounts {
                let detail = match account {
                    AccountConfig::Claude {
                        config_dir, plan, ..
                    } => format!(
                        "{}{}",
                        config_dir.display(),
                        plan.as_ref()
                            .map(|plan| format!(", тариф {plan}"))
                            .unwrap_or_default()
                    ),
                    AccountConfig::Codex {
                        codex_home,
                        limit_id,
                        ..
                    } => format!("{}, bucket {limit_id}", codex_home.display()),
                    AccountConfig::Deepseek { api_key_env, .. } => api_key_env.clone(),
                };
                println!(
                    "{:<16} {:<10} {:<20} {}",
                    account.id(),
                    account.provider(),
                    account.name(),
                    detail
                );
            }
            Ok(())
        }

        AccountCommand::Add(command) => {
            let Added {
                account,
                install_hook,
            } = build_account(command)?;
            let id = account.id().to_owned();

            // Валидация до записи: иначе можно оставить конфиг с двумя
            // аккаунтами на одном каталоге и сломать восстановление hook.
            let mut candidate = config.clone();
            config::upsert_account(&mut candidate, account);
            config::validate(&candidate)?;
            config = candidate;

            if install_hook {
                if let Some(AccountConfig::Claude { config_dir, .. }) =
                    config::find_account(&config, &id)
                {
                    let config_dir = config_dir.clone();
                    setup::install_claude_status_line(&id, &config_dir)?;
                    println!("Claude hook установлен в {}", config_dir.display());
                }
            }

            let path = config::save(&config)?;
            println!("Аккаунт '{id}' сохранён в {}", path.display());
            Ok(())
        }

        AccountCommand::Remove { id, keep_hook } => {
            let Some(removed) = config::remove_account(&mut config, &id) else {
                bail!("Аккаунт '{id}' не найден");
            };

            // Конфиг записывается до восстановления hook: иначе неудачное
            // восстановление оставило бы аккаунт и в конфиге, и без hook.
            config::save(&config)?;

            if let AccountConfig::Claude { config_dir, .. } = &removed {
                if keep_hook {
                    println!("statusLine оставлен без изменений.");
                } else {
                    let outcome = setup::restore_claude_account(&id, config_dir)?;
                    println!("Claude settings: {outcome:?}");
                }
            }
            println!("Аккаунт '{id}' удалён.");
            Ok(())
        }

        AccountCommand::Rename { id, name } => {
            let Some(account) = config.accounts.iter_mut().find(|item| item.id() == id) else {
                bail!("Аккаунт '{id}' не найден");
            };
            account.set_name(name.clone());
            config::save(&config)?;
            println!("Аккаунт '{id}' переименован в '{name}'.");
            Ok(())
        }
    }
}

struct Added {
    account: AccountConfig,
    install_hook: bool,
}

impl Added {
    fn plain(account: AccountConfig) -> Self {
        Self {
            account,
            install_hook: false,
        }
    }
}

fn build_account(command: AddCommand) -> Result<Added> {
    match command {
        AddCommand::Claude {
            common,
            config_dir,
            plan,
            no_hook,
        } => {
            util::validate_id(&common.id)?;
            Ok(Added {
                account: AccountConfig::Claude {
                    name: common.name.unwrap_or_else(|| common.id.clone()),
                    id: common.id,
                    config_dir,
                    plan,
                },
                install_hook: !no_hook,
            })
        }
        AddCommand::Codex {
            common,
            codex_home,
            command: executable,
            limit_id,
        } => {
            util::validate_id(&common.id)?;
            // Записываем абсолютный путь, как это делает интерактивный мастер:
            // демон стартует раньше, чем сессия наполнит PATH.
            let command = match util::resolve_command(&executable) {
                Some(resolved) => {
                    if resolved != executable {
                        println!("Команда '{executable}' найдена: {resolved}");
                    }
                    resolved
                }
                None => {
                    println!(
                        "Внимание: команда '{executable}' сейчас не найдена. \
                         Она будет искаться при каждом обновлении."
                    );
                    executable
                }
            };
            Ok(Added::plain(AccountConfig::Codex {
                name: common.name.unwrap_or_else(|| common.id.clone()),
                id: common.id,
                codex_home,
                command,
                limit_id,
            }))
        }
        AddCommand::Deepseek {
            common,
            base_url,
            api_key_stdin,
        } => {
            util::validate_id(&common.id)?;
            let key = if api_key_stdin {
                let mut key = String::new();
                std::io::stdin().read_to_string(&mut key)?;
                key.trim().to_owned()
            } else {
                rpassword::prompt_password("API key (ввод скрыт): ")?
            };
            if key.is_empty() {
                bail!("API key не может быть пустым");
            }

            let api_key_env = util::env_name_for_id(&common.id);
            setup::save_secret(&api_key_env, &key)?;
            Ok(Added::plain(AccountConfig::Deepseek {
                name: common.name.unwrap_or_else(|| common.id.clone()),
                id: common.id,
                api_key_env,
                base_url,
            }))
        }
    }
}

fn configure(command: ConfigCommand) -> Result<()> {
    let mut config = config::load()?;
    match command {
        ConfigCommand::Show => {
            println!("Файл:            {}", config::config_path()?.display());
            println!("refresh_seconds: {}", config.refresh_seconds);
            println!("  фактический:   {}", config.refresh_interval());
            println!("stale_seconds:   {}", config.stale_seconds);
            println!("Аккаунтов:       {}", config.accounts.len());
            Ok(())
        }
        ConfigCommand::Set {
            refresh_seconds,
            stale_seconds,
        } => {
            if refresh_seconds.is_none() && stale_seconds.is_none() {
                bail!("Укажи --refresh-seconds и/или --stale-seconds");
            }
            if let Some(value) = refresh_seconds {
                if value < config::MIN_REFRESH_SECONDS {
                    bail!(
                        "refresh_seconds не может быть меньше {}",
                        config::MIN_REFRESH_SECONDS
                    );
                }
                config.refresh_seconds = value;
            }
            if let Some(value) = stale_seconds {
                config.stale_seconds = value;
            }
            let path = config::save(&config)?;
            println!("Сохранено в {}", path.display());
            println!("Применить сейчас: systemctl --user restart ai-usage.service");
            Ok(())
        }
    }
}

fn init() -> Result<()> {
    let path = config::config_path()?;
    if path.exists() {
        println!("Конфигурация уже существует: {}", path.display());
        return Ok(());
    }
    config::save(&config::Config::default())?;
    println!("Создана конфигурация: {}", path.display());
    Ok(())
}

async fn daemon() -> Result<()> {
    loop {
        let config = config::load()?;
        let state = refresh(&config).await;
        write_state(&state)?;

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(config.refresh_interval())) => {},
            signal = tokio::signal::ctrl_c() => {
                signal?;
                break;
            }
        }
    }
    Ok(())
}

async fn once() -> Result<()> {
    let config = config::load()?;
    let state = refresh(&config).await;
    write_state(&state)?;
    println!("{}", serde_json::to_string_pretty(&state)?);
    Ok(())
}

/// Верхняя граница на один provider fetch. Провайдер, вышедший за неё,
/// становится обычным error state и не задерживает остальные аккаунты.
const PROVIDER_DEADLINE: Duration = Duration::from_secs(30);

async fn refresh(config: &config::Config) -> AppState {
    let mut tasks = JoinSet::new();
    for (index, account) in config.accounts.iter().cloned().enumerate() {
        let stale_seconds = config.stale_seconds;
        tasks.spawn(async move {
            let id = account.id().to_owned();
            let name = account.name().to_owned();
            let provider = account.provider();
            let fetch = providers::fetch(account, stale_seconds);
            let state = match tokio::time::timeout(PROVIDER_DEADLINE, fetch).await {
                Ok(state) => state,
                Err(_) => model::AccountState::error(
                    &id,
                    &name,
                    provider,
                    format!(
                        "Провайдер не ответил за {} секунд",
                        PROVIDER_DEADLINE.as_secs()
                    ),
                ),
            };
            (index, state)
        });
    }

    let mut ordered = vec![None; config.accounts.len()];
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((index, state)) => ordered[index] = Some(state),
            Err(error) => eprintln!("Ошибка задачи обновления: {error}"),
        }
    }

    let accounts = ordered.into_iter().flatten().collect();
    let accounts = model::merge_with_previous(read_state(), accounts);

    AppState {
        schema_version: model::SCHEMA_VERSION,
        updated_at: util::unix_now(),
        accounts,
    }
}

fn read_state() -> Option<AppState> {
    let raw = fs::read_to_string(util::state_file()).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_state(state: &AppState) -> Result<()> {
    let path = util::state_file();
    util::atomic_write(&path, serde_json::to_string_pretty(state)?.as_bytes())?;
    Ok(())
}

fn claude_hook(account: &str) -> Result<()> {
    util::validate_id(account)?;
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let input: ClaudeInput =
        serde_json::from_str(&raw).context("Claude hook получил некорректный JSON")?;

    let cache = input.into_cache(util::unix_now());
    let path = util::claude_cache_file(account)?;
    util::atomic_write(&path, serde_json::to_string_pretty(&cache)?.as_bytes())?;

    println!("{}", cache.status_line());
    Ok(())
}

fn doctor() -> Result<()> {
    let config_path = config::config_path()?;
    let config = config::load()?;
    println!("AI Usage doctor\n");
    println!(
        "{} config: {}",
        mark(config_path.exists()),
        config_path.display()
    );
    println!(
        "{} accounts: {}",
        mark(!config.accounts.is_empty()),
        config.accounts.len()
    );

    for account in &config.accounts {
        match account {
            config::AccountConfig::Claude {
                id,
                name,
                config_dir,
                ..
            } => {
                let settings = util::expand_home(config_dir).join("settings.json");
                println!(
                    "{} Claude '{}': hook {}",
                    mark(setup::is_hook_installed(id, config_dir)),
                    name,
                    settings.display()
                );
            }
            config::AccountConfig::Codex {
                name,
                command,
                codex_home,
                ..
            } => {
                let available = StdCommand::new(command).arg("--version").output().is_ok();
                println!(
                    "{} Codex '{}': command='{}', CODEX_HOME={}",
                    mark(available),
                    name,
                    command,
                    util::expand_home(codex_home).display()
                );
            }
            config::AccountConfig::Deepseek {
                name, api_key_env, ..
            } => {
                let present = std::env::var(api_key_env)
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false);
                println!("{} DeepSeek '{}': {}", mark(present), name, api_key_env);
            }
        }
    }

    println!("\nState file: {}", util::state_file().display());
    println!("Для сетевой проверки: ai-usage once");
    Ok(())
}

fn mark(ok: bool) -> &'static str {
    if ok {
        "[OK]"
    } else {
        "[WARN]"
    }
}

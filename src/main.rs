mod config;
mod model;
mod providers;
mod setup;
mod util;

use std::fs;
use std::io::Read;
use std::process::Command as StdCommand;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use model::{AppState, QuotaWindow};
use serde::Deserialize;
use tokio::task::JoinSet;

#[derive(Debug, Parser)]
#[command(name = "ai-usage", version, about = "Claude, Codex and DeepSeek usage for GNOME")]
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
    /// Claude Code status-line hook. Reads Claude JSON from stdin.
    ClaudeHook {
        #[arg(long)]
        account: String,
    },
    /// Restore Claude settings changed by the setup command.
    RestoreClaudeHooks,
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
        Commands::ClaudeHook { account } => claude_hook(&account),
        Commands::RestoreClaudeHooks => setup::restore_claude_hooks(),
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
            _ = tokio::time::sleep(Duration::from_secs(config.refresh_seconds.max(15))) => {},
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

async fn refresh(config: &config::Config) -> AppState {
    let mut tasks = JoinSet::new();
    for (index, account) in config.accounts.iter().cloned().enumerate() {
        tasks.spawn(async move { (index, providers::fetch(account).await) });
    }

    let mut ordered = vec![None; config.accounts.len()];
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((index, state)) => ordered[index] = Some(state),
            Err(error) => eprintln!("Ошибка задачи обновления: {error}"),
        }
    }

    AppState {
        schema_version: 1,
        updated_at: util::unix_now(),
        accounts: ordered.into_iter().flatten().collect(),
    }
}

fn write_state(state: &AppState) -> Result<()> {
    let path = util::state_file();
    util::atomic_write(&path, serde_json::to_string_pretty(state)?.as_bytes())?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ClaudeInput {
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

fn claude_hook(account: &str) -> Result<()> {
    util::validate_id(account)?;
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let input: ClaudeInput = serde_json::from_str(&raw).context("Claude hook получил некорректный JSON")?;
    let model = input.model.and_then(|item| item.display_name);
    let five_hour = input
        .rate_limits
        .as_ref()
        .and_then(|limits| limits.five_hour.as_ref())
        .map(|window| QuotaWindow::new(window.used_percentage, Some(300), window.resets_at));
    let weekly = input
        .rate_limits
        .as_ref()
        .and_then(|limits| limits.seven_day.as_ref())
        .map(|window| QuotaWindow::new(window.used_percentage, Some(10_080), window.resets_at));

    let cache = providers::claude::ClaudeCache {
        updated_at: util::unix_now(),
        model: model.clone(),
        five_hour: five_hour.clone(),
        weekly: weekly.clone(),
    };
    let path = util::claude_cache_file(account)?;
    util::atomic_write(&path, serde_json::to_string_pretty(&cache)?.as_bytes())?;

    let mut parts = Vec::new();
    if let Some(model) = model {
        parts.push(format!("[{model}]"));
    }
    if let Some(window) = five_hour {
        parts.push(format!("5h {:.0}% left", window.remaining_percent));
    }
    if let Some(window) = weekly {
        parts.push(format!("7d {:.0}% left", window.remaining_percent));
    }
    println!("{}", parts.join(" · "));
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
    println!("{} accounts: {}", mark(!config.accounts.is_empty()), config.accounts.len());

    for account in &config.accounts {
        match account {
            config::AccountConfig::Claude {
                id,
                name,
                config_dir,
                ..
            } => {
                let settings = util::expand_home(config_dir).join("settings.json");
                let hooked = fs::read_to_string(&settings)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                    .and_then(|value| {
                        value
                            .pointer("/statusLine/command")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .map(|command| command.contains("ai-usage") && command.contains(id))
                    .unwrap_or(false);
                println!("{} Claude '{}': hook {}", mark(hooked), name, settings.display());
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
                name,
                api_key_env,
                ..
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
    if ok { "[OK]" } else { "[WARN]" }
}

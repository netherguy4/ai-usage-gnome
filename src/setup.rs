use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::config::{self, AccountConfig, Config};

pub fn run() -> Result<()> {
    let mut config = config::load()?;

    println!("AI Usage — настройка аккаунтов\n");
    if config.accounts.is_empty() {
        println!("Аккаунтов пока нет.");
    } else {
        println!("Уже настроено:");
        for account in &config.accounts {
            println!("  • {} ({}, {})", account.name(), account.provider(), account.id());
        }
    }

    loop {
        println!("\nЧто добавить или обновить?");
        println!("  1) Claude Code");
        println!("  2) Codex");
        println!("  3) DeepSeek API");
        println!("  4) Завершить");
        match prompt("Выбор", Some("4"))?.as_str() {
            "1" => add_claude(&mut config)?,
            "2" => add_codex(&mut config)?,
            "3" => add_deepseek(&mut config)?,
            "4" | "" => break,
            _ => println!("Неизвестный пункт."),
        }
    }

    let path = config::save(&config)?;
    println!("\nКонфигурация сохранена: {}", path.display());
    restart_service();
    println!("Проверка: ai-usage doctor");
    Ok(())
}

fn add_claude(config: &mut Config) -> Result<()> {
    println!("\nClaude Code");
    let id = prompt("ID аккаунта", Some("claude-main"))?;
    crate::util::validate_id(&id)?;
    let name = prompt("Название в виджете", Some("Claude"))?;
    let plan = prompt("Тариф (можно оставить пустым)", Some("Pro"))?;
    let default_dir = if id == "claude-main" {
        "~/.claude".to_owned()
    } else {
        format!("~/.claude-{id}")
    };
    let config_dir = PathBuf::from(prompt("CLAUDE_CONFIG_DIR", Some(&default_dir))?);

    install_claude_status_line(&id, &config_dir)?;
    config::upsert_account(
        config,
        AccountConfig::Claude {
            id: id.clone(),
            name,
            config_dir: config_dir.clone(),
            plan: non_empty(plan),
        },
    );
    config::save(config)?;

    println!("Claude hook установлен.");
    if config_dir != PathBuf::from("~/.claude") {
        println!(
            "Запуск этого профиля: CLAUDE_CONFIG_DIR={} claude",
            config_dir.display()
        );
    }
    println!("Лимиты появятся после первого ответа Claude Code.");
    Ok(())
}

fn add_codex(config: &mut Config) -> Result<()> {
    println!("\nCodex");
    let id = prompt("ID аккаунта", Some("codex-main"))?;
    crate::util::validate_id(&id)?;
    let name = prompt("Название в виджете", Some("Codex"))?;
    let default_home = if id == "codex-main" {
        "~/.codex".to_owned()
    } else {
        format!("~/.codex-{id}")
    };
    let codex_home = PathBuf::from(prompt("CODEX_HOME", Some(&default_home))?);
    let entered_command = prompt("Команда Codex", Some("codex"))?;
    let command = resolve_command(&entered_command).unwrap_or(entered_command);
    println!("Будет использована команда: {command}");
    let limit_id = prompt("Rate-limit bucket", Some("codex"))?;

    config::upsert_account(
        config,
        AccountConfig::Codex {
            id,
            name,
            codex_home: codex_home.clone(),
            command: command.clone(),
            limit_id,
        },
    );
    config::save(config)?;

    let expanded_home = crate::util::expand_home(&codex_home);
    fs::create_dir_all(&expanded_home)?;
    if yes_no("Запустить вход в этот Codex-аккаунт сейчас?", true)? {
        let status = Command::new(&command)
            .arg("login")
            .env("CODEX_HOME", &expanded_home)
            .status()
            .with_context(|| format!("Не удалось запустить '{command} login'"))?;
        if !status.success() {
            println!("Вход завершился с кодом {status}. Можно повторить вручную.");
        }
    } else {
        println!(
            "Вход вручную: CODEX_HOME={} {} login",
            expanded_home.display(),
            command
        );
    }
    Ok(())
}

fn add_deepseek(config: &mut Config) -> Result<()> {
    println!("\nDeepSeek API");
    let id = prompt("ID аккаунта", Some("deepseek-main"))?;
    crate::util::validate_id(&id)?;
    let name = prompt("Название в виджете", Some("DeepSeek"))?;
    let key = rpassword::prompt_password("API key (ввод скрыт): ")?;
    if key.is_empty() {
        bail!("API key не может быть пустым");
    }
    if key.contains('\n') || key.contains('\r') {
        bail!("API key содержит перевод строки");
    }

    let env_name = crate::util::env_name_for_id(&id);
    save_secret(&env_name, &key)?;
    config::upsert_account(
        config,
        AccountConfig::Deepseek {
            id,
            name,
            api_key_env: env_name,
            base_url: "https://api.deepseek.com".to_owned(),
        },
    );
    config::save(config)?;
    println!("Ключ сохранён в secrets.env с правами 600.");
    Ok(())
}

fn install_claude_status_line(account_id: &str, config_dir: &Path) -> Result<()> {
    let dir = crate::util::expand_home(config_dir);
    fs::create_dir_all(&dir)?;
    let settings_path = dir.join("settings.json");
    let backup_path = dir.join("settings.json.ai-usage.bak");

    if settings_path.exists() && !backup_path.exists() {
        fs::copy(&settings_path, &backup_path).with_context(|| {
            format!(
                "Не удалось создать резервную копию {}",
                backup_path.display()
            )
        })?;
    }

    let mut settings = if settings_path.exists() {
        let raw = fs::read_to_string(&settings_path)?;
        serde_json::from_str::<Value>(&raw)
            .with_context(|| format!("Некорректный JSON в {}", settings_path.display()))?
    } else {
        json!({})
    };

    let object = settings
        .as_object_mut()
        .context("Корень Claude settings.json должен быть объектом")?;
    let executable = std::env::current_exe()?;
    let command = format!(
        "{} claude-hook --account {}",
        crate::util::shell_single_quote(&executable.to_string_lossy()),
        crate::util::shell_single_quote(account_id)
    );
    object.insert(
        "statusLine".to_owned(),
        json!({
            "type": "command",
            "command": command,
            "padding": 0
        }),
    );

    crate::util::atomic_write(&settings_path, serde_json::to_string_pretty(&settings)?.as_bytes())?;
    Ok(())
}

pub fn restore_claude_hooks() -> Result<()> {
    let config = config::load()?;
    for account in config.accounts {
        let AccountConfig::Claude {
            id, config_dir, ..
        } = account
        else {
            continue;
        };
        let dir = crate::util::expand_home(&config_dir);
        let settings_path = dir.join("settings.json");
        let backup_path = dir.join("settings.json.ai-usage.bak");

        if backup_path.exists() {
            if !settings_path.exists() {
                fs::copy(&backup_path, &settings_path)?;
                fs::remove_file(&backup_path)?;
                println!("Восстановлен {}", settings_path.display());
                continue;
            }

            let mut current: Value = serde_json::from_str(&fs::read_to_string(&settings_path)?)?;
            let backup: Value = serde_json::from_str(&fs::read_to_string(&backup_path)?)?;
            let current_is_ours = current
                .pointer("/statusLine/command")
                .and_then(Value::as_str)
                .map(|command| command.contains("ai-usage") && command.contains(&id))
                .unwrap_or(false);

            if current_is_ours {
                let previous_status_line = backup.get("statusLine").cloned();
                if let Some(object) = current.as_object_mut() {
                    match previous_status_line {
                        Some(value) => {
                            object.insert("statusLine".to_owned(), value);
                        }
                        None => {
                            object.remove("statusLine");
                        }
                    }
                }
                crate::util::atomic_write(
                    &settings_path,
                    serde_json::to_string_pretty(&current)?.as_bytes(),
                )?;
                println!("Восстановлен statusLine в {}", settings_path.display());
            } else {
                println!(
                    "statusLine в {} уже изменён пользователем; оставляю его как есть",
                    settings_path.display()
                );
            }
            fs::remove_file(&backup_path)?;
            continue;
        }

        if !settings_path.exists() {
            continue;
        }
        let raw = fs::read_to_string(&settings_path)?;
        let mut settings: Value = serde_json::from_str(&raw)?;
        let should_remove = settings
            .pointer("/statusLine/command")
            .and_then(Value::as_str)
            .map(|command| command.contains("ai-usage") && command.contains(&id))
            .unwrap_or(false);
        if should_remove {
            if let Some(object) = settings.as_object_mut() {
                object.remove("statusLine");
            }
            crate::util::atomic_write(
                &settings_path,
                serde_json::to_string_pretty(&settings)?.as_bytes(),
            )?;
            println!("Удалён AI Usage hook из {}", settings_path.display());
        }
    }
    Ok(())
}

fn save_secret(key: &str, value: &str) -> Result<()> {
    let path = crate::util::secrets_file()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut values = BTreeMap::new();
    if path.exists() {
        for line in fs::read_to_string(&path)?.lines() {
            if let Some((existing_key, existing_value)) = line.split_once('=') {
                values.insert(existing_key.trim().to_owned(), existing_value.trim().to_owned());
            }
        }
    }
    values.insert(key.to_owned(), value.to_owned());
    let contents = values
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    crate::util::atomic_write(&path, contents.as_bytes())?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    match default {
        Some(default) => print!("{label} [{default}]: "),
        None => print!("{label}: "),
    }
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let value = input.trim().to_owned();
    if value.is_empty() {
        Ok(default.unwrap_or_default().to_owned())
    } else {
        Ok(value)
    }
}

fn yes_no(label: &str, default: bool) -> Result<bool> {
    let hint = if default { "Y/n" } else { "y/N" };
    let answer = prompt(&format!("{label} ({hint})"), None)?;
    if answer.is_empty() {
        return Ok(default);
    }
    Ok(matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes" | "д" | "да"))
}

fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}


fn resolve_command(command: &str) -> Option<String> {
    let path = Path::new(command);
    if path.components().count() > 1 {
        let expanded = crate::util::expand_home(path);
        return expanded.exists().then(|| expanded.to_string_lossy().into_owned());
    }

    let search_path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&search_path) {
        let candidate = directory.join(command);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

fn restart_service() {
    let _ = Command::new("systemctl")
        .args(["--user", "restart", "ai-usage.service"])
        .status();
}

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
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
            println!(
                "  • {} ({}, {})",
                account.name(),
                account.provider(),
                account.id()
            );
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
    if config_dir.as_os_str() != "~/.claude" {
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

/// Установлен ли наш hook в `settings.json` этого профиля.
pub fn is_hook_installed(account_id: &str, config_dir: &Path) -> bool {
    let settings_path = crate::util::expand_home(config_dir).join("settings.json");
    fs::read_to_string(&settings_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .map(|settings| is_our_hook(&settings, account_id))
        .unwrap_or(false)
}

pub fn install_claude_status_line(account_id: &str, config_dir: &Path) -> Result<()> {
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
        "{} {}",
        crate::util::shell_single_quote(&executable.to_string_lossy()),
        hook_marker(account_id)
    );
    object.insert(
        "statusLine".to_owned(),
        json!({
            "type": "command",
            "command": command,
            "padding": 0
        }),
    );

    crate::util::atomic_write(
        &settings_path,
        serde_json::to_string_pretty(&settings)?.as_bytes(),
    )?;
    Ok(())
}

pub fn restore_claude_hooks() -> Result<()> {
    let config = config::load()?;
    for account in &config.accounts {
        if let AccountConfig::Claude { id, config_dir, .. } = account {
            let outcome = restore_claude_account(id, config_dir)?;
            println!(
                "{}",
                outcome.describe(&crate::util::expand_home(config_dir))
            );
        }
    }
    Ok(())
}

/// Что именно сделало восстановление. Отдельный тип нужен, чтобы тесты могли
/// проверять решение, а не разбирать печатаемый текст.
#[derive(Debug, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// Настроек не было и до установки.
    NothingToDo,
    /// Файл целиком вернули из backup.
    RestoredFromBackup,
    /// Вернули прежнее значение `statusLine`.
    RestoredStatusLine,
    /// `statusLine` не было до установки — просто убрали наш.
    RemovedHook,
    /// Пользователь поменял `statusLine` после установки: не трогаем.
    KeptUserValue,
}

impl RestoreOutcome {
    fn describe(&self, path: &Path) -> String {
        let path = path.join("settings.json");
        match self {
            Self::NothingToDo => format!("Нечего восстанавливать в {}", path.display()),
            Self::RestoredFromBackup => format!("Восстановлен {}", path.display()),
            Self::RestoredStatusLine => {
                format!("Восстановлен statusLine в {}", path.display())
            }
            Self::RemovedHook => format!("Удалён AI Usage hook из {}", path.display()),
            Self::KeptUserValue => format!(
                "statusLine в {} изменён пользователем; оставляю его как есть",
                path.display()
            ),
        }
    }
}

/// Восстанавливает Claude `settings.json` одного аккаунта.
///
/// Главный инвариант: если пользователь поменял `statusLine` уже после нашей
/// установки, его значение важнее backup и перезаписывать его нельзя.
pub fn restore_claude_account(account_id: &str, config_dir: &Path) -> Result<RestoreOutcome> {
    let dir = crate::util::expand_home(config_dir);
    let settings_path = dir.join("settings.json");
    let backup_path = dir.join("settings.json.ai-usage.bak");

    if backup_path.exists() {
        if !settings_path.exists() {
            fs::copy(&backup_path, &settings_path)?;
            fs::remove_file(&backup_path)?;
            return Ok(RestoreOutcome::RestoredFromBackup);
        }

        let mut current: Value = serde_json::from_str(&fs::read_to_string(&settings_path)?)?;
        let backup: Value = serde_json::from_str(&fs::read_to_string(&backup_path)?)?;

        let outcome = if is_our_hook(&current, account_id) {
            let previous = backup.get("statusLine").cloned();
            if let Some(object) = current.as_object_mut() {
                match previous.clone() {
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
            if previous.is_some() {
                RestoreOutcome::RestoredStatusLine
            } else {
                RestoreOutcome::RemovedHook
            }
        } else {
            RestoreOutcome::KeptUserValue
        };

        fs::remove_file(&backup_path)?;
        return Ok(outcome);
    }

    if !settings_path.exists() {
        return Ok(RestoreOutcome::NothingToDo);
    }

    let mut settings: Value = serde_json::from_str(&fs::read_to_string(&settings_path)?)?;
    if !is_our_hook(&settings, account_id) {
        return Ok(RestoreOutcome::KeptUserValue);
    }

    if let Some(object) = settings.as_object_mut() {
        object.remove("statusLine");
    }
    crate::util::atomic_write(
        &settings_path,
        serde_json::to_string_pretty(&settings)?.as_bytes(),
    )?;
    Ok(RestoreOutcome::RemovedHook)
}

/// Хвост команды, по которому мы узнаём собственный hook.
///
/// Опознавать по подстроке «ai-usage» в пути к бинарнику нельзя: путь зависит
/// от того, куда установлено приложение, и может как случайно совпасть, так и
/// не совпасть вовсе.
fn hook_marker(account_id: &str) -> String {
    format!(
        "claude-hook --account {}",
        crate::util::shell_single_quote(account_id)
    )
}

fn is_our_hook(settings: &Value, account_id: &str) -> bool {
    settings
        .pointer("/statusLine/command")
        .and_then(Value::as_str)
        .map(|command| command.contains(&hook_marker(account_id)))
        .unwrap_or(false)
}

pub fn save_secret(key: &str, value: &str) -> Result<()> {
    let path = crate::util::secrets_file()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut values = BTreeMap::new();
    if path.exists() {
        for line in fs::read_to_string(&path)?.lines() {
            if let Some((existing_key, existing_value)) = line.split_once('=') {
                values.insert(
                    existing_key.trim().to_owned(),
                    existing_value.trim().to_owned(),
                );
            }
        }
    }
    values.insert(key.to_owned(), value.to_owned());
    crate::util::atomic_write_mode(&path, render_secrets(&values).as_bytes(), Some(0o600))?;
    Ok(())
}

fn render_secrets(values: &BTreeMap<String, String>) -> String {
    values
        .iter()
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect()
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
    Ok(matches!(
        answer.to_ascii_lowercase().as_str(),
        "y" | "yes" | "д" | "да"
    ))
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
        return expanded
            .exists()
            .then(|| expanded.to_string_lossy().into_owned());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Сценарии C2 из `docs/handoff/TESTING.md`: установка hook поверх разных
    /// исходных `settings.json` и обратимость этой операции.
    struct Profile {
        dir: PathBuf,
    }

    impl Profile {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ai-usage-claude-{label}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        fn settings_path(&self) -> PathBuf {
            self.dir.join("settings.json")
        }

        fn backup_path(&self) -> PathBuf {
            self.dir.join("settings.json.ai-usage.bak")
        }

        fn write_settings(&self, value: Value) {
            fs::write(
                self.settings_path(),
                serde_json::to_string_pretty(&value).unwrap(),
            )
            .unwrap();
        }

        fn settings(&self) -> Value {
            serde_json::from_str(&fs::read_to_string(self.settings_path()).unwrap()).unwrap()
        }

        fn install(&self, account_id: &str) {
            install_claude_status_line(account_id, &self.dir).unwrap();
        }

        fn restore(&self, account_id: &str) -> RestoreOutcome {
            restore_claude_account(account_id, &self.dir).unwrap()
        }
    }

    impl Drop for Profile {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn user_status_line() -> Value {
        json!({"type": "command", "command": "my-own-statusline.sh", "padding": 1})
    }

    #[test]
    fn c2_1_no_settings_before_install() {
        let profile = Profile::new("c2-1");
        profile.install("claude-main");

        assert!(profile.settings_path().exists());
        // Backup не создаётся, если нечего сохранять.
        assert!(!profile.backup_path().exists());

        assert_eq!(profile.restore("claude-main"), RestoreOutcome::RemovedHook);
        assert!(profile.settings().get("statusLine").is_none());
    }

    #[test]
    fn c2_2_settings_without_status_line() {
        let profile = Profile::new("c2-2");
        profile.write_settings(json!({"theme": "dark", "permissions": {"defaultMode": "ask"}}));
        profile.install("claude-main");

        assert!(profile.backup_path().exists());
        assert_eq!(profile.restore("claude-main"), RestoreOutcome::RemovedHook);

        let settings = profile.settings();
        assert!(settings.get("statusLine").is_none());
        // Остальные пользовательские настройки обязаны уцелеть.
        assert_eq!(settings["theme"], "dark");
        assert_eq!(settings["permissions"]["defaultMode"], "ask");
        assert!(!profile.backup_path().exists());
    }

    #[test]
    fn c2_3_existing_user_status_line_is_restored() {
        let profile = Profile::new("c2-3");
        profile.write_settings(json!({"theme": "dark", "statusLine": user_status_line()}));
        profile.install("claude-main");

        // После установки в файле наша команда.
        assert!(profile.settings()["statusLine"]["command"]
            .as_str()
            .unwrap()
            .contains("claude-hook"));

        assert_eq!(
            profile.restore("claude-main"),
            RestoreOutcome::RestoredStatusLine
        );
        assert_eq!(profile.settings()["statusLine"], user_status_line());
        assert_eq!(profile.settings()["theme"], "dark");
    }

    #[test]
    fn c2_4_user_edit_after_install_is_not_overwritten() {
        let profile = Profile::new("c2-4");
        profile.write_settings(json!({"statusLine": user_status_line()}));
        profile.install("claude-main");

        // Пользователь снова поменял statusLine уже после установки.
        let newer = json!({"type": "command", "command": "even-newer.sh"});
        profile.write_settings(json!({"statusLine": newer.clone()}));

        assert_eq!(
            profile.restore("claude-main"),
            RestoreOutcome::KeptUserValue
        );
        assert_eq!(
            profile.settings()["statusLine"],
            newer,
            "более новое значение пользователя нельзя перезаписывать backup'ом"
        );
        // Backup всё равно убирается, чтобы не оставлять мусор.
        assert!(!profile.backup_path().exists());
    }

    #[test]
    fn c2_5_two_profiles_are_independent() {
        let work = Profile::new("c2-5-work");
        let personal = Profile::new("c2-5-personal");
        work.write_settings(json!({"theme": "work"}));
        personal.write_settings(json!({"theme": "personal"}));

        work.install("claude-work");
        personal.install("claude-personal");

        assert!(work.settings()["statusLine"]["command"]
            .as_str()
            .unwrap()
            .contains("claude-work"));
        assert!(personal.settings()["statusLine"]["command"]
            .as_str()
            .unwrap()
            .contains("claude-personal"));

        work.restore("claude-work");
        assert!(work.settings().get("statusLine").is_none());
        // Удаление одного профиля не должно трогать другой.
        assert!(personal.settings().get("statusLine").is_some());
        assert_eq!(personal.settings()["theme"], "personal");
    }

    #[test]
    fn restore_does_not_touch_a_hook_belonging_to_another_account() {
        let profile = Profile::new("foreign-hook");
        profile.install("claude-work");

        assert_eq!(
            profile.restore("claude-personal"),
            RestoreOutcome::KeptUserValue
        );
        assert!(profile.settings()["statusLine"]["command"]
            .as_str()
            .unwrap()
            .contains("claude-work"));
    }

    #[test]
    fn reinstall_does_not_overwrite_the_original_backup() {
        let profile = Profile::new("double-install");
        profile.write_settings(json!({"statusLine": user_status_line()}));

        profile.install("claude-main");
        profile.install("claude-main");

        let backup: Value =
            serde_json::from_str(&fs::read_to_string(profile.backup_path()).unwrap()).unwrap();
        assert_eq!(
            backup["statusLine"],
            user_status_line(),
            "второй install перезаписал backup нашей же командой"
        );

        profile.restore("claude-main");
        assert_eq!(profile.settings()["statusLine"], user_status_line());
    }

    #[test]
    fn restore_is_idempotent() {
        let profile = Profile::new("idempotent");
        profile.write_settings(json!({"theme": "dark"}));
        profile.install("claude-main");

        profile.restore("claude-main");
        let after_first = profile.settings();
        // Второй запуск не должен ни падать, ни менять файл.
        assert_eq!(
            profile.restore("claude-main"),
            RestoreOutcome::KeptUserValue
        );
        assert_eq!(profile.settings(), after_first);
    }

    #[test]
    fn restore_on_untouched_profile_is_a_no_op() {
        let profile = Profile::new("untouched");
        assert_eq!(profile.restore("claude-main"), RestoreOutcome::NothingToDo);
        assert!(!profile.settings_path().exists());
    }

    #[test]
    fn install_rejects_a_non_object_settings_root() {
        let profile = Profile::new("bad-root");
        fs::write(profile.settings_path(), "[1, 2, 3]").unwrap();
        assert!(install_claude_status_line("claude-main", &profile.dir).is_err());
    }

    #[test]
    fn install_refuses_to_clobber_invalid_json() {
        // Лучше упасть, чем потерять файл, который пользователь просто не дописал.
        let profile = Profile::new("bad-json");
        fs::write(profile.settings_path(), "{ broken").unwrap();
        assert!(install_claude_status_line("claude-main", &profile.dir).is_err());
        assert_eq!(
            fs::read_to_string(profile.settings_path()).unwrap(),
            "{ broken"
        );
    }

    #[test]
    fn hook_command_quotes_paths_with_spaces() {
        let profile = Profile::new("quoting");
        profile.install("claude-main");
        let command = profile.settings()["statusLine"]["command"]
            .as_str()
            .unwrap()
            .to_owned();

        assert!(command.starts_with('\''), "путь не в кавычках: {command}");
        assert!(command.contains("claude-hook --account 'claude-main'"));
    }

    #[test]
    fn hook_detection_does_not_depend_on_the_binary_path() {
        // Раньше признаком служила подстрока 'ai-usage' в пути; тогда hook,
        // установленный из каталога с другим именем, переставал опознаваться.
        let settings = json!({
            "statusLine": {
                "type": "command",
                "command": "'/opt/tools/bin/usage-widget' claude-hook --account 'claude-main'"
            }
        });
        assert!(is_our_hook(&settings, "claude-main"));
        assert!(!is_our_hook(&settings, "claude-work"));
    }

    #[test]
    fn similar_account_ids_are_not_confused() {
        let settings = json!({
            "statusLine": {"type": "command", "command": "x claude-hook --account 'claude'"}
        });
        assert!(is_our_hook(&settings, "claude"));
        // 'claude-main' начинается с 'claude', но это другой аккаунт.
        assert!(!is_our_hook(&settings, "claude-main"));
    }

    #[test]
    fn a_user_command_merely_mentioning_the_project_is_not_ours() {
        let settings = json!({
            "statusLine": {"type": "command", "command": "/home/u/ai-usage-scripts/mine.sh"}
        });
        assert!(!is_our_hook(&settings, "claude-main"));
    }

    #[test]
    fn saved_secret_keeps_other_keys() {
        let mut values = BTreeMap::new();
        values.insert("A".to_owned(), "1".to_owned());
        values.insert("B".to_owned(), "2".to_owned());
        let rendered = render_secrets(&values);
        assert_eq!(rendered, "A=1\nB=2\n");
    }
}

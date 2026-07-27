use std::env;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn expand_home(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return dirs::home_dir().unwrap_or_else(|| path.to_path_buf());
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    path.to_path_buf()
}

pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("ai-usage");
    }
    let user = env::var("USER").unwrap_or_else(|_| "unknown".to_owned());
    env::temp_dir().join(format!("ai-usage-{user}"))
}

pub fn state_file() -> PathBuf {
    runtime_dir().join("state.json")
}

pub fn secrets_file() -> Result<PathBuf> {
    let base = dirs::config_dir().context("Не удалось определить каталог конфигурации")?;
    Ok(base.join("ai-usage").join("secrets.env"))
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_mode(path, bytes, None)
}

/// Записывает файл атомарно. `mode` выставляется на временном файле до `rename`,
/// поэтому итоговый файл никогда не существует с более широкими правами.
pub fn atomic_write_mode(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        if let Some(mode) = mode {
            options.mode(mode);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("Не удалось создать {}", tmp.display()))?;
        if let Some(mode) = mode {
            // create-time mode проходит через umask, поэтому фиксируем права явно.
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("Не удалось заменить {}", path.display()))?;
    Ok(())
}

pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() {
        bail!("ID не может быть пустым");
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        bail!("ID может содержать только латинские буквы, цифры, '-' и '_'");
    }
    Ok(())
}

pub fn env_name_for_id(id: &str) -> String {
    let normalized: String = id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("AI_USAGE_DEEPSEEK_{normalized}")
}

pub fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Каталоги, которые пользовательские установщики (npm, standalone-инсталляторы
/// Claude/Codex) используют по умолчанию.
///
/// `ai-usage.service` поднимается из `default.target` раньше, чем GNOME-сессия
/// импортирует PATH логин-шелла: демон стартует с голым
/// `/usr/local/sbin:/usr/local/bin:/usr/bin`, где `~/.local/bin` нет. Поэтому
/// одного PATH для поиска бинарника провайдера недостаточно.
const EXTRA_BIN_DIRS: [&str; 3] = ["~/.local/bin", "~/bin", "/usr/local/bin"];

/// Разворачивает имя команды в абсолютный путь.
///
/// Возвращает `None`, если команду найти не удалось: вызывающий сам решает,
/// сообщить об этом или оставить исходное значение.
pub fn resolve_command(command: &str) -> Option<String> {
    let path = Path::new(command);
    if path.components().count() > 1 {
        let expanded = expand_home(path);
        return expanded
            .is_file()
            .then(|| expanded.to_string_lossy().into_owned());
    }

    let from_path = env::var_os("PATH")
        .map(|value| env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    let extra = EXTRA_BIN_DIRS.iter().map(|dir| expand_home(Path::new(dir)));

    for directory in from_path.into_iter().chain(extra) {
        let candidate = directory.join(command);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

pub fn load_secrets_env() -> Result<()> {
    let path = secrets_file()?;
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(&path)?;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if env::var_os(key.trim()).is_none() {
                env::set_var(key.trim(), value.trim());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "ai-usage-test-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn atomic_write_creates_parent_directories() {
        let dir = temp_dir("atomic-parents");
        let path = dir.join("nested").join("deeper").join("state.json");
        atomic_write(&path, b"{}").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn atomic_write_leaves_no_temporary_files() {
        let dir = temp_dir("atomic-temp");
        let path = dir.join("state.json");
        atomic_write(&path, b"first").unwrap();
        atomic_write(&path, b"second").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("tmp-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "остались временные файлы: {leftovers:?}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn secret_file_is_never_world_readable_even_for_an_instant() {
        // Права выставляются на временном файле до rename, поэтому итоговый
        // путь сразу появляется с 0600 — отдельного chmod после записи нет.
        let dir = temp_dir("secret-mode");
        let path = dir.join("secrets.env");
        atomic_write_mode(&path, b"KEY=value\n", Some(0o600)).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "получены права {mode:o}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rewriting_a_secret_keeps_restrictive_mode() {
        let dir = temp_dir("secret-rewrite");
        let path = dir.join("secrets.env");
        atomic_write_mode(&path, b"KEY=one\n", Some(0o600)).unwrap();
        atomic_write_mode(&path, b"KEY=two\n", Some(0o600)).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(fs::read_to_string(&path).unwrap(), "KEY=two\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn accepts_safe_ids() {
        for id in ["claude-main", "codex_work", "a1", "A-B_9"] {
            validate_id(id).unwrap_or_else(|error| panic!("{id} отклонён: {error}"));
        }
    }

    #[test]
    fn rejects_ids_that_could_escape_a_directory() {
        // ID подставляется в имя файла кеша, поэтому '/' и '..' недопустимы.
        for id in ["", "../etc", "a/b", "a b", "café", "a.b"] {
            assert!(validate_id(id).is_err(), "ID '{id}' должен быть отклонён");
        }
    }

    #[test]
    fn env_name_is_uppercase_and_prefixed() {
        assert_eq!(
            env_name_for_id("deepseek-main"),
            "AI_USAGE_DEEPSEEK_DEEPSEEK_MAIN"
        );
        assert_eq!(env_name_for_id("a_b"), "AI_USAGE_DEEPSEEK_A_B");
    }

    #[test]
    fn shell_quoting_survives_embedded_quotes() {
        assert_eq!(
            shell_single_quote("/usr/bin/ai-usage"),
            "'/usr/bin/ai-usage'"
        );
        assert_eq!(shell_single_quote("it's"), r"'it'\''s'");
        assert_eq!(
            shell_single_quote("/home/o'brien/bin/ai usage"),
            r"'/home/o'\''brien/bin/ai usage'"
        );
    }

    #[test]
    fn expands_only_leading_tilde() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home(Path::new("~")), home);
        assert_eq!(expand_home(Path::new("~/.codex")), home.join(".codex"));
        // '~' в середине пути — обычный символ.
        assert_eq!(
            expand_home(Path::new("/tmp/~/x")),
            PathBuf::from("/tmp/~/x")
        );
        assert_eq!(
            expand_home(Path::new("/absolute/path")),
            PathBuf::from("/absolute/path")
        );
    }

    #[test]
    fn resolves_a_command_found_only_in_a_user_bin_dir() {
        // Регрессия: ai-usage.service стартует из default.target раньше, чем
        // GNOME импортирует PATH сессии, и получает PATH без ~/.local/bin.
        // Поиск обязан покрывать пользовательские каталоги сам.
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let dir = home.join(".local/bin");
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let name = format!("ai-usage-test-probe-{}", std::process::id());
        let probe = dir.join(&name);
        if fs::write(&probe, b"#!/bin/sh\n").is_err() {
            return;
        }
        fs::set_permissions(&probe, fs::Permissions::from_mode(0o755)).unwrap();

        // PATH, с которым сервис реально стартует после reboot.
        let saved = env::var_os("PATH");
        env::set_var("PATH", "/usr/local/sbin:/usr/local/bin:/usr/bin");
        let resolved = resolve_command(&name);
        match saved {
            Some(value) => env::set_var("PATH", value),
            None => env::remove_var("PATH"),
        }
        let _ = fs::remove_file(&probe);

        assert_eq!(
            resolved.as_deref(),
            Some(probe.to_string_lossy().as_ref()),
            "команда из ~/.local/bin должна находиться без помощи PATH"
        );
    }

    #[test]
    fn resolves_an_absolute_path_only_if_it_exists() {
        assert_eq!(
            resolve_command("/usr/bin/env").as_deref(),
            Some("/usr/bin/env")
        );
        assert!(resolve_command("/nonexistent/path/to/codex").is_none());
    }

    #[test]
    fn unknown_command_resolves_to_nothing() {
        assert!(resolve_command("ai-usage-definitely-not-a-real-command").is_none());
    }

    #[test]
    fn does_not_resolve_a_directory_as_a_command() {
        assert!(resolve_command("/usr/bin").is_none());
    }
}

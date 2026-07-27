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

pub fn data_dir() -> Result<PathBuf> {
    let base = dirs::data_local_dir().context("Не удалось определить каталог данных")?;
    Ok(base.join("ai-usage"))
}

pub fn claude_cache_file(account_id: &str) -> Result<PathBuf> {
    Ok(data_dir()?
        .join("claude")
        .join(format!("{account_id}.json")))
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
    fn cache_path_is_scoped_per_account() {
        let one = claude_cache_file("claude-main").unwrap();
        let two = claude_cache_file("claude-work").unwrap();
        assert_ne!(one, two);
        assert!(one.ends_with("claude/claude-main.json"));
    }
}

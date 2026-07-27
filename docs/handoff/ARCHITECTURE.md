# Архитектура

> Родительский индекс: [`HANDOFF.md`](../../HANDOFF.md)

## Общая схема

```text
Claude Code statusLine ──┐
                         │ local cache
Codex app-server ────────┼──> Rust ai-usage daemon ──atomic write──> state.json
                         │                                      │
DeepSeek HTTP API ───────┘                                      └──> GNOME GJS extension
```

GNOME extension не обращается к провайдерам. Это сознательное разделение защищает GNOME Shell от сетевых задержек, ошибок subprocess и изменений API.

## Компоненты

### Rust CLI/daemon

Точка входа: `src/main.rs`.

Команды:

- `daemon` — бесконечный refresh loop;
- `once` — один сбор и вывод JSON;
- `setup` — интерактивная настройка;
- `doctor` — локальная диагностика;
- `init` — пустой config;
- `claude-hook` — внутренний status-line обработчик;
- `restore-claude-hooks` — восстановление Claude settings.

Каждый refresh клонирует account config и запускает provider fetch в `tokio::task::JoinSet`. Порядок аккаунтов после параллельного выполнения восстанавливается по исходному индексу.

### Модель состояния

Определена в `src/model.rs`.

`AppState`:

```text
schema_version
updated_at
accounts[]
```

`AccountState` содержит общие поля `id`, `name`, `provider`, `status`, `plan`, `email`, `model`, два quota window, balances, error и `updated_at`.

`QuotaWindow::new()` ограничивает `used_percent` диапазоном 0–100 и вычисляет `remaining_percent`.

Текущий `schema_version`: `1`.

### Конфигурация

`src/config.rs`, файл пользователя:

```text
$XDG_CONFIG_HOME/ai-usage/config.toml
обычно ~/.config/ai-usage/config.toml
```

Провайдеры хранятся как tagged TOML enum. Повторный ID в setup заменяет существующую запись. Удаления аккаунтов через CLI пока нет.

### Claude provider

Файлы: `src/providers/claude.rs`, hook в `src/main.rs`, установка в `src/setup.rs`.

Поток:

1. setup создаёт backup `settings.json.ai-usage.bak`, если settings уже существует и backup ещё не создан;
2. поле `statusLine` заменяется командой текущего `ai-usage` binary;
3. Claude Code передаёт JSON в stdin hook;
4. hook извлекает `model.display_name`, `rate_limits.five_hour`, `rate_limits.seven_day`;
5. cache пишется в `$XDG_DATA_HOME/ai-usage/claude/<account-id>.json`;
6. daemon читает cache, а не запускает Claude.

Ограничения архитектуры:

- тариф Claude не определяется автоматически и задаётся в config;
- данные обновляются только после ответа Claude Code;
- stale threshold жёстко задан как 24 часа;
- два account ID с одним `CLAUDE_CONFIG_DIR` конфликтуют за один `statusLine`.

### Codex provider

Файл: `src/providers/codex.rs`.

Для каждого refresh и каждого Codex аккаунта:

1. запускается `<command> app-server` с отдельным `CODEX_HOME`;
2. по stdin отправляются JSONL-сообщения `initialize`, `initialized`, `account/read`, `account/rateLimits/read`;
3. stdout читается до ответов ID `1` и `2`, максимум 20 секунд;
4. account даёт email и `planType`;
5. rate-limit bucket выбирается по `limit_id`, по умолчанию `codex`;
6. `primary` и `secondary` сопоставляются с окнами, ближайшими к 300 и 10 080 минутам;
7. subprocess завершается.

Важный риск: текущая реализация отправляет handshake и рабочие запросы подряд, не ожидая отдельного ответа `initialize`. Совместимость с актуальной версией Codex App Server нужно подтвердить живым тестом.

### DeepSeek provider

Файл: `src/providers/deepseek.rs`.

Выполняется `GET <base_url>/user/balance` с bearer token. Ключ читается из env, имя переменной хранится в config. Setup сохраняет значения в `secrets.env` с правами `0600`.

Текущий HTTP client не имеет явного request timeout. Зависший запрос потенциально задерживает весь refresh cycle.

### GNOME extension

Каталог: `extension/ai-usage@netherguy4/`.

Extension:

- создаёт `PanelMenu.Button`;
- читает `$XDG_RUNTIME_DIR/ai-usage/state.json`;
- использует `Gio.FileMonitor` и резервный timer раз в 60 секунд;
- показывает аккаунты, лимиты, reset countdown, balance и ошибки;
- panel label показывает минимальный remaining percentage;
- «Обновить сейчас» перезапускает `ai-usage.service`.

В metadata заявлены GNOME Shell 45–50. Пока это декларация, а не подтверждённая матрица совместимости.

## Файлы времени выполнения

```text
~/.local/bin/ai-usage
~/.local/bin/ai-usage-uninstall
~/.local/share/gnome-shell/extensions/ai-usage@netherguy4/
~/.local/share/ai-usage/claude/*.json
~/.config/ai-usage/config.toml
~/.config/ai-usage/secrets.env
~/.config/systemd/user/ai-usage.service
$XDG_RUNTIME_DIR/ai-usage/state.json
```

Все пути должны уважать XDG environment variables.

## Безопасность и приватность

- Claude/Codex auth-файлы остаются в официальных config/home каталогах провайдеров.
- Rust process не копирует OAuth tokens в собственный config.
- DeepSeek key находится в plaintext user-only env-файле, не в keyring.
- `state.json` не должен содержать токены или полные API responses.
- Codex stderr сейчас отбрасывается, что снижает риск случайной утечки, но сильно ухудшает диагностику. Безопасное логирование нужно проектировать с redaction.

## Точки расширения

Новый provider должен:

1. получить variant в `AccountConfig`;
2. вернуть общий `AccountState`;
3. быть подключён в `providers::fetch`;
4. получить setup flow и doctor check;
5. не передавать provider-specific secrets в state;
6. иметь mock/fixture tests до включения в release.

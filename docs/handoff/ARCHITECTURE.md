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

`AccountState` содержит общие поля `id`, `name`, `provider`, `status`, `plan`, `email`, `model`, открытый список `windows`, balances, error и `updated_at`.

Каждое окно — `{key, label, used_percent, remaining_percent, duration_mins, resets_at}`. Список открыт намеренно: провайдеры заводят и убирают лимиты (недельный лимит на конкретную модель у Claude), и фиксированная пара полей `five_hour`/`weekly` заставляла бы править код на каждое такое изменение, а незнакомые лимиты молча терялись бы.

`QuotaWindow::new()` ограничивает `used_percent` диапазоном 0–100 и вычисляет `remaining_percent`.

Текущий `schema_version`: `2`.

### Конфигурация

`src/config.rs`, файл пользователя:

```text
$XDG_CONFIG_HOME/ai-usage/config.toml
обычно ~/.config/ai-usage/config.toml
```

Провайдеры хранятся как tagged TOML enum. Повторный ID заменяет существующую запись. Управление аккаунтами — через `ai-usage account`.

### Claude provider

Файлы: `src/providers/claude.rs`, hook в `src/main.rs`, установка в `src/setup.rs`.

Поток:

1. setup создаёт backup `settings.json.ai-usage.bak`, если settings уже существует и backup ещё не создан;
2. поле `statusLine` заменяется командой текущего `ai-usage` binary;
3. Claude Code передаёт JSON в stdin hook;
4. hook извлекает `model.display_name` и **все** ключи из `rate_limits` — помимо документированных `five_hour`/`seven_day` там встречаются `seven_day_opus`, `seven_day_sonnet`, `seven_day_overage_included` и лимиты отдельных моделей;
5. cache пишется в `$XDG_DATA_HOME/ai-usage/claude/<account-id>.json`;
6. daemon читает cache, а не запускает Claude.

Наш hook опознаётся по маркеру `claude-hook --account '<id>'` в команде. Опознавание по подстроке «ai-usage» в пути к бинарнику было бы неверным: путь зависит от места установки и может как случайно совпасть, так и не совпасть.

Ограничения архитектуры:

- тариф и почта Claude читаются из `.claude.json`, который пишет сам Claude Code; заданный вручную `plan` в конфиге имеет приоритет;
- данные обновляются только после ответа Claude Code;
- stale threshold настраивается через `stale_seconds`, по умолчанию 24 часа;
- два account ID с одним `CLAUDE_CONFIG_DIR` отклоняются валидацией конфига.

### Codex provider

Файл: `src/providers/codex.rs`.

Для каждого refresh и каждого Codex аккаунта:

1. запускается `<command> app-server` с отдельным `CODEX_HOME`;
2. по stdin отправляются JSONL-сообщения `initialize`, `initialized`, `account/read`, `account/rateLimits/read`;
3. stdout читается до ответов ID `1` и `2`, максимум 20 секунд;
4. account даёт email и `planType`;
5. rate-limit bucket выбирается по `limit_id`, по умолчанию `codex`;
6. `primary` и `secondary` становятся окнами с подписью по длительности;
7. subprocess завершается.

Батч из четырёх сообщений отправляется без ожидания ответа на `initialize`. Это подтверждено живым тестом на app-server 0.145.0: сервер отвечает на `initialize` первым, а рабочие запросы принимает из того же батча. Ответ `id 0` всё равно разбирается — чтобы отличить отказ handshake от молчания сервера.

Порядок ответов не гарантирован: на неавторизованном профиле ошибка `id 2` приходит раньше ответа `id 1`. Чтение отбирает сообщения по `id`, а не по позиции.

stderr app-server читается в bounded-буфер (хвост 4 КБ) и проходит через redaction перед попаданием в текст ошибки. Без этого несуществующий `CODEX_HOME` давал сообщение «Codex не вернул account/read» без единого намёка на причину.

### DeepSeek provider

Файл: `src/providers/deepseek.rs`.

Выполняется `GET <base_url>/user/balance` с bearer token. Ключ читается из env, имя переменной хранится в config. Setup сохраняет значения в `secrets.env` с правами `0600`, выставленными на временном файле до `rename` — итоговый файл не существует с более широкими правами ни мгновения.

HTTP client общий, с таймаутами 15 с на запрос и 5 с на соединение. Сверх этого каждый provider fetch ограничен общим дедлайном в 30 секунд в `main::refresh`, поэтому зависший провайдер не задерживает остальные аккаунты.

### GNOME extension

Каталог: `extension/ai-usage@netherguy4/`.

Extension:

- создаёт `PanelMenu.Button`;
- читает `$XDG_RUNTIME_DIR/ai-usage/state.json`;
- использует `Gio.FileMonitor` и резервный timer раз в 60 секунд;
- показывает аккаунты, лимиты, reset countdown, balance, возраст данных и ошибки;
- отличает `stale` (данные есть, но устарели) от `error` (данных нет вовсе);
- panel label показывает значок каждого провайдера и его худший остаток: общий минимум не позволял понять, чей лимит заканчивается;
- «Обновить сейчас» перезапускает `ai-usage.service`.

В metadata заявлены GNOME Shell 48–50. Реально прогонялась только 50.3.

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
- Codex stderr попадает в ошибку только хвостом в 4 КБ и после redaction; сама redaction эвристическая и гарантией не является.
- Из `.claude.json` читаются только почта и тип организации; токены оттуда не берутся.

## Точки расширения

Новый provider должен:

1. получить variant в `AccountConfig`;
2. вернуть общий `AccountState`;
3. быть подключён в `providers::fetch`;
4. получить setup flow и doctor check;
5. не передавать provider-specific secrets в state;
6. иметь mock/fixture tests до включения в release.

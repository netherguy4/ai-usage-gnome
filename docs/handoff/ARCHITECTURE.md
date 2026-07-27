# Архитектура

> Родительский индекс: [`HANDOFF.md`](../../HANDOFF.md)

## Общая схема

```text
Anthropic usage API ─────┐
                         │
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
- `claude-hook` — необязательный status line внутри Claude Code;
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

Каждое окно — `{key, label, used_percent, remaining_percent, duration_mins, resets_at, scope}`. `scope` содержит имя модели, если лимит привязан к ней; UI отличает по нему блокирующие лимиты от необязательных, не полагаясь на формат ключа. Список открыт намеренно: провайдеры заводят и убирают лимиты (недельный лимит на конкретную модель у Claude), и фиксированная пара полей `five_hour`/`weekly` заставляла бы править код на каждое такое изменение, а незнакомые лимиты молча терялись бы.

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

Файлы: `src/providers/claude.rs`, необязательный hook в `src/main.rs` и `src/setup.rs`.

Поток:

1. из `<config_dir>/.credentials.json` читается OAuth access token — тот же файл, что ведёт сам Claude Code;
2. если с прошлой попытки не прошла пауза, запрос не делается вовсе;
3. иначе выполняется `GET https://api.anthropic.com/api/oauth/usage` с этим токеном;
4. массив `limits[]` превращается в окна: он надмножество верхнеуровневых полей и только в нём приходят лимиты, привязанные к модели (`weekly_scoped` + `scope.model`);
5. удачный ответ ложится в `$XDG_RUNTIME_DIR/ai-usage/claude-usage-<id>.json` (права `0600`) вместе с моментом следующей попытки;
6. если запроса не было или он не удался — показывается самое свежее из двух: наш кеш или `cachedUsageUtilization` из `.claude.json`, который Claude Code обновляет при запуске.

**Частота.** Endpoint отвечает `429` задолго до общего интервала обновления: опрос раз в 120 с на живом аккаунте держал его в отказе постоянно, и виджет молча показывал многочасовой кеш Claude Code. Между удачными запросами выдерживается минимум 5 минут (1.7% пятичасового окна), а `429` наращивает паузу 5 → 10 → 20 → 40 → 60 минут и сбрасывается первым успехом. Бюджет endpoint делится с самим Claude Code, поэтому фиксированного интервала мало — нужен отход.

**Возраст.** Окно, чей `resets_at` уже прошёл, о текущем периоде не говорит ничего: из запасных данных оно выбрасывается. Показывать использование закончившегося периода — врать, а считать его сброшенным — гадать. Наступивший `resets_at` в свежих данных — другое дело: это и означает, что окно началось заново, и UI показывает 100%.

Раньше данные приходили из status-line hook, то есть только когда пользователь работал в Claude Code. Активный опрос убрал это требование: виджету больше не нужно ни ставить hook, ни трогать `settings.json`.

Hook остался как необязательная косметика (`--hook`) — показать лимиты внутри самого Claude Code. Он опознаётся по маркеру `claude-hook --account '<id>'` в команде; опознавание по подстроке «ai-usage» в пути к бинарнику было бы неверным, так как путь зависит от места установки.

Ограничения архитектуры:

- тариф и почта Claude читаются из `.claude.json`. Тариф берётся из `userRateLimitTier` → `organizationRateLimitTier` → `organizationType`: только уровень лимитов различает `Max 5x` и `Max 20x`, тогда как `organizationType` для обоих даёт `claude_max`. Заданный вручную `plan` в конфиге имеет приоритет над определённым;
- access token живёт около 8 часов и обновляется только самим Claude Code — мы принципиально не пишем в `.credentials.json`, чтобы не сломать вход;
- stale threshold настраивается через `stale_seconds`, по умолчанию 15 минут — 5% пятичасового окна. Прежние сутки прятали отказ endpoint за многочасовым кешем: аккаунт числился `ok`, показывая цифры пятичасовой давности;
- два account ID с одним `CLAUDE_CONFIG_DIR` отклоняются валидацией конфига.

### Codex provider

Файл: `src/providers/codex.rs`.

Для каждого refresh и каждого Codex аккаунта:

1. запускается `<command> app-server` с отдельным `CODEX_HOME`;
2. по stdin отправляются JSONL-сообщения `initialize`, `initialized`, `account/read`, `account/rateLimits/read`;
3. stdout читается до ответов ID `1` и `2`, максимум 20 секунд;
4. account даёт email и `planType` (`plus`/`pro`/`business`/…), который приводится к тому же виду, что и тариф Claude;
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
- показывает аккаунты, лимиты, reset countdown, balance и ошибки;
- отличает `stale` (данные есть, но устарели) от `error` (данных нет вовсе);
- panel label показывает логотип провайдера и одно число. Порядок правил: исчерпанный общий лимит → `0%`; иначе пятичасовое окно; иначе недельное; при отсутствии лимитов — баланс. Окна с непустым `scope` (лимит конкретной модели) в правило блокировки не входят: их исчерпание не мешает работать;
- логотипы лежат в `icons/` как `-symbolic.svg`, поэтому GNOME красит их под тему панели; про товарные знаки — `icons/README.md`;
- значок и число каждого аккаунта живут в собственном контейнере, а класс `system-status-icon` намеренно не используется: тема задаёт ему `padding: 0 6px; margin: 0 4px` селектором `#panel .panel-button .system-status-icon`, который перебивает любой одиночный класс расширения, и между значком и числом появлялось 14 px пустоты;
- строки «обновлено» нет: возраст данных выводится только в тексте ошибки устаревшего аккаунта;
- кнопки ручного обновления нет: демон обновляет данные сам, а расширение подхватывает их через `Gio.FileMonitor`.

В metadata заявлены GNOME Shell 48–50. Реально прогонялась только 50.3.

## Файлы времени выполнения

```text
~/.local/bin/ai-usage
~/.local/bin/ai-usage-uninstall
~/.local/share/gnome-shell/extensions/ai-usage@netherguy4/
~/.config/ai-usage/config.toml
~/.config/ai-usage/secrets.env
~/.config/systemd/user/ai-usage.service
$XDG_RUNTIME_DIR/ai-usage/state.json
$XDG_RUNTIME_DIR/ai-usage/claude-usage-<account>.json
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

# План тестирования и приёмки

> Родительский индекс: [`HANDOFF.md`](../../HANDOFF.md)

## Текущее тестовое доказательство

```text
Дата:        27 июля 2026
Commit:      ветка main, после «Add provider fixtures, tests, account CLI and last-known-good state»
Среда:       Bluefin 44.20260721 (Silverblue), GNOME Shell 50.3, Wayland,
             Rust 1.97.1, codex-cli 0.145.0, claude 2.1.220
```

### Пройдено

**Сборка и статические гейты**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked      # 99 тестов
cargo build --release --locked
bash -n install.sh uninstall.sh install-online.sh scripts/package.sh
gjs -m extension/ai-usage@netherguy4/extension.js   # разбор модуля
```

**Codex — X1, живой аккаунт.** `ai-usage once` вернул `status=ok`, `plan=plus`, email и недельное окно (46% остатка, reset через 5 д). Подтверждено, что последовательность `initialize → initialized → account/read → account/rateLimits/read` принимается app-server 0.145 одним батчем, а ответы приходят в виде `result.rateLimitsByLimitId.codex.primary.{usedPercent,windowDurationMins,resetsAt}`.

**Codex — X2, два профиля.** `~/.codex` (авторизован) и `~/.codex-aiusage-x2` (пустой) в одном конфиге: email/plan/лимиты не смешиваются, второй профиль отдаёт `unauthenticated` с командой входа, ошибка одного не скрывает данные другого.

**Codex — X3, ошибки.** Отсутствующая команда, неавторизованный профиль и несуществующий `CODEX_HOME` дают разные внятные сообщения. Последний случай раньше давал только «Codex не вернул account/read»; теперь в ошибку попадает exit status и redacted-хвост stderr с настоящей причиной.

**Claude — C1.** `GET /api/oauth/usage` с токеном из `~/.claude/.credentials.json` вернул HTTP 200 и три лимита: `session`, `weekly_all` и `weekly_scoped` со `scope.model = Fable`. `ai-usage once` показал их без единого действия пользователя и без изменения `settings.json`. Проверено, что `restore-claude-hooks` возвращает `settings.json` к исходному виду после отказа от hook.

**Claude — C2, восстановление.** Шесть сценариев покрыты автотестами (`src/setup.rs`), плюс живая проверка: после `ai-usage-uninstall --keep-config` файл `~/.claude/settings.json` вернулся к исходному содержимому байт в байт, backup удалён.

**Установка и удаление.** `./install.sh --no-setup` на реальной системе: раскладка файлов, `secrets.env` с правами 600, user-service `enabled` + `active`. `ai-usage-uninstall --keep-config` убрал бинарь, расширение, unit и кеш, сохранив конфиг; повторный install подхватил сохранённый конфиг. Установка в путь `"/tmp/.../ai usage & tools/bin"` даёт корректный `ExecStart="..."`, `systemd-analyze verify` не жалуется, бинарь запускается.

**Online installer.** Логика сверки `SHA256SUMS` проверена на трёх случаях: целый архив принимается, изменённый на один байт отклоняется, отсутствие записи для архива отклоняется. Затем скрипт прогнан против настоящего GitHub Release `v0.1.0-rc.1` — и с управляющим терминалом, и без него; в обоих случаях архив скачан, checksum сверен, установка прошла, `ai-usage --version` вернул `0.1.0-rc.1`.

Этот прогон нашёл дефект: проверка `[[ -r /dev/tty && -w /dev/tty ]]` проходила и там, где перенаправление `</dev/tty` затем падало с ENXIO, из-за чего установка молча не выполнялась вовсе. Теперь наличие терминала проверяется попыткой его открыть.

**Release workflow.** `workflow_dispatch` собрал оба таргета; `file` подтвердил, что aarch64-артефакт — настоящий `ELF ARM aarch64`. `sha256sum -c SHA256SUMS` на скачанных архивах прошёл. Тег `v0.1.0-rc.1` опубликовал Release с тремя ассетами. Подстановка версии в `Cargo.toml` и `Cargo.lock` совместима со сборкой `--locked`: бинарь из архива сообщает `0.1.0-rc.1`.

**DeepSeek — D1, живой ключ.** Реальный ключ отдал `status=ok`, `plan=API` и запись баланса в USD с `granted`/`topped_up`. Ключ лёг в `secrets.env` с правами 600; в `config.toml` хранится только имя переменной окружения.

**Логика UI.** Чистые функции `extension.js` прогнаны под `gjs` на настоящем `state.json`: метка панели для ok/stale/error, форматирование возраста данных (включая часы, ушедшие вперёд, и нечисловой ввод), окна, деньги и статусы. Отдельным прогоном закрыт разбор пятичасового окна: живые данные, исчерпанный общий лимит, наступивший `resets_at` по свежим и по устаревшим данным, баланс без лимитов.

**Частота опроса Claude — 27 июля 2026.** Живой аккаунт показывал `5 часов: 100% осталось · сброс сейчас`, тогда как на деле оставалось 25%. Разбор: `GET /api/oauth/usage` отвечал `429` на каждом обращении, потому что демон опрашивал его раз в 120 с. Отказ был невидим — данные молча брались из `cachedUsageUtilization` и на момент проверки отстали на 5 ч 31 мин, а порог устаревания в сутки оставлял аккаунт в статусе `ok`. Пятичасовое окно к тому времени закончилось, и `resets_at` в прошлом превращался в «100% осталось».

Измерено вручную, с остановленным демоном: `429` содержит `Retry-After: 0` и никаких `anthropic-ratelimit-*`; через 60 с простоя приходит `200`; повторный запрос через ~90 с снова даёт `429`; при опросе раз в 5 минут отказов нет. Отсюда `MIN_POLL_SECONDS = 300` и отход 5 → 10 → 20 → 40 → 60 минут.

**last-known-good.** После удачного refresh провайдер намеренно сломан: аккаунт перешёл в `stale`, сохранив квоту, plan и прежний `updated_at`, и показал текст ошибки.

### Не пройдено — требует действия пользователя

- **Рендеринг расширения в GNOME Shell — частично пройдено.** Панель и меню с тремя настоящими аккаунтами отрисованы в живой сессии: логотипы, тарифы, почты, все лимиты, разделители и баланс на месте. Не проверены 0 и 1 аккаунт, длинные строки и многократные enable/disable. Отдельная особенность: при обновлении поверх установленного Shell продолжает исполнять ранее загруженную копию — `ReloadExtension` в D-Bus объявлен, но не реализован, поэтому нужен новый перезаход.
- **DeepSeek: ошибочные состояния.** Успешный ответ проверен на живом ключе; `depleted`, 401/403 и таймаут — только моками.
- **aarch64 release artifact** и online installer против настоящего GitHub Release.

## P0: компиляция и базовое качество

Выполнить из корня repository:

```bash
cargo generate-lockfile
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --release
./target/release/ai-usage --version
```

Ожидаемый результат: все команды завершаются с exit code 0, `Cargo.lock` добавлен в git.

Дополнительно:

```bash
./target/release/ai-usage init
./target/release/ai-usage doctor
./target/release/ai-usage once
```

Для первых запусков используй отдельные `HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_RUNTIME_DIR`, чтобы не менять реальные профили.

## P0: provider tests

### Claude — C1

Подготовить отдельный `CLAUDE_CONFIG_DIR` без `settings.json`.

Проверить:

- setup создаёт settings;
- statusLine command содержит абсолютный путь binary и правильный account ID;
- после одного ответа Claude создаётся cache;
- `ai-usage once` возвращает model, 5h и weekly;
- UI показывает remaining, а не used;
- после `stale_seconds` (по умолчанию 15 минут) данные получают stale state с возрастом в тексте ошибки.

### Claude — C2: безопасное восстановление

Сценарии:

1. settings до установки не было;
2. settings был без `statusLine`;
3. settings имел пользовательский `statusLine`;
4. после установки пользователь вручную изменил `statusLine` ещё раз;
5. два разных Claude profiles;
6. попытка добавить два account ID с одним config dir.

При uninstall первоначальный statusLine должен восстановиться в сценариях 2–3. В сценарии 4 новое пользовательское значение нельзя перезаписывать.

### Codex — X1: один аккаунт

```bash
CODEX_HOME="$HOME/.codex-ai-usage-test" codex login
ai-usage setup
ai-usage once
```

Проверить email, plan, обе quota windows и reset timestamps. Сохранить обезличенный JSON fixture ответа app-server в tests/fixtures.

Особенно проверить, принимает ли server текущую последовательность:

```text
initialize
initialized
account/read
account/rateLimits/read
```

Если нет — реализовать ожидание initialize response до остальных вызовов.

### Codex — X2: несколько аккаунтов

Настроить два независимых `CODEX_HOME`. Убедиться, что email/plan/quota не смешиваются и subprocess каждого профиля завершается.

### Codex — X3: ошибки

Проверить:

- command отсутствует;
- профиль не авторизован;
- app-server зависает;
- bucket ID отсутствует;
- есть только одно quota window;
- schema fields отличаются/отсутствуют;
- stderr большой или содержит потенциальный secret.

Ошибка одного профиля не должна скрывать другой.

### DeepSeek — D1

С настоящим key проверить:

- валидный баланс;
- несколько currency entries, если API их возвращает;
- `is_available=false`;
- 401/403;
- network timeout;
- malformed JSON.

Добавить request timeout и test server/mock до приёмки.

## P0: GNOME/Bluefin integration

Устанавливать сначала из source/release bundle, не через online installer:

```bash
./install.sh --no-setup
gnome-extensions info ai-usage@netherguy4
gnome-extensions enable ai-usage@netherguy4
systemctl --user status ai-usage.service
journalctl --user -u ai-usage.service -b
```

Проверить:

- icon появляется после enable или relogin;
- shell не показывает extension errors;
- menu корректно открывается при 0, 1 и нескольких аккаунтах;
- длинные names/email/errors не ломают layout;
- state file update отражается без restart shell;
- timer подхватывает state, даже если file monitor не создался;
- refresh button инициирует новый refresh;
- GNOME Shell остаётся отзывчивым при network/provider failure;
- enable/disable несколько раз не оставляет timer/monitor.

Снять фактическую версию GNOME:

```bash
gnome-shell --version
```

После теста оставить в `metadata.json` только подтверждённые major versions.

## P0: install/uninstall

На отдельном Linux user profile проверить:

- source install с Rust;
- release archive install без Rust;
- install поверх существующей версии;
- `--no-setup`;
- interactive setup;
- logout/login и автозапуск user-service;
- uninstall;
- uninstall `--keep-config`;
- повторная установка после обоих вариантов;
- отсутствие файлов/активных units после полного удаления.

Контроль:

```bash
systemctl --user is-enabled ai-usage.service
gnome-extensions info ai-usage@netherguy4
find ~/.local ~/.config -maxdepth 5 -iname '*ai-usage*' -print
```

Не удаляй пользовательские Codex/Claude auth directories: проект их не создаёт как собственные данные и не владеет ими.

## P0: release workflow

1. Запустить `workflow_dispatch` и скачать оба artifacts.
2. Проверить содержимое x86_64 bundle.
3. По возможности проверить aarch64 bundle на реальном arm64 Linux или эмуляции.
4. Создать prerelease tag, например `v0.1.0-rc.1`.
5. Проверить GitHub Release assets и `SHA256SUMS`.
6. Проверить `install-online.sh` на prerelease/временном repository после добавления checksum verification.

## Автоматические тесты, которых не хватает

Минимальный рекомендуемый набор:

- config serialize/deserialize/upsert;
- QuotaWindow clamp;
- Claude payload parsing и cache stale logic;
- Claude settings install/restore на fixtures;
- Codex JSON-RPC response fixtures: success, unauthenticated, error, missing bucket, one/two windows;
- DeepSeek mock HTTP success/errors/timeouts;
- state ordering при разной длительности provider tasks;
- secret file permissions и preservation других keys;
- shell integration smoke хотя бы через `gnome-extensions pack` и headless syntax validation.

## Формат записи результата

После каждого тестового этапа добавляй в этот файл короткую запись:

```text
Дата:
Commit/tag:
Среда:
Команды/сценарий:
Результат:
Артефакты/логи:
Оставшиеся проблемы:
```

Не добавляй токены, email реальных аккаунтов или содержимое auth-файлов.

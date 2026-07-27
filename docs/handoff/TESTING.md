# План тестирования и приёмки

> Родительский индекс: [`HANDOFF.md`](../../HANDOFF.md)

## Текущее тестовое доказательство

На момент handoff локально выполнены только проверки, не требующие Rust toolchain или настоящей GNOME session:

```bash
bash -n install.sh uninstall.sh install-online.sh scripts/package.sh
node --check extension/ai-usage@netherguy4/extension.js
```

Также подтверждены parsing конфигурационных файлов, packaging smoke и rootless install/uninstall smoke в изолированных XDG каталогах. Rust toolchain в исходной среде отсутствовал.

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
- после 24 часов cache получает stale state.

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

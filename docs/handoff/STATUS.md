# Инженерный статус

> Родительский индекс: [`HANDOFF.md`](../../HANDOFF.md)  
> Снимок состояния: **26 июля 2026 года**.

## Легенда

- **Реализовано** — код присутствует и логически завершён.
- **Проверено статически** — синтаксис/структура проверены без реального runtime.
- **Проверено smoke** — выполнен ограниченный сценарий в изолированной среде.
- **Требует интеграционного теста** — нужен настоящий provider/GNOME/systemd.
- **Не готово** — отсутствует или заведомо недостаточно для релиза.

## Что готово в коде

### Backend

- Rust CLI с командами `daemon`, `once`, `setup`, `init`, `doctor`.
- Параллельный сбор нескольких аккаунтов с сохранением порядка.
- Общая сериализуемая модель `AppState` schema v1.
- Атомарная запись `state.json`.
- Ошибка одного provider преобразуется в отдельный error state.
- Минимальный unit coverage выбора 5h/weekly Codex windows.

### Claude

- Multi-profile config через отдельные `CLAUDE_CONFIG_DIR`.
- Установка status-line hook.
- Backup исходного `settings.json`.
- Локальный cache лимитов и модели.
- Состояния waiting/ok/stale.
- Восстановление прежнего `statusLine` при удалении с защитой от перезаписи более нового пользовательского изменения.

### Codex

- Multi-profile config через отдельные `CODEX_HOME`.
- Запуск `codex app-server`.
- Account/rate-limit JSON-RPC parsing.
- Email/plan extraction.
- 20-секундный timeout чтения ответа.
- Состояние unauthenticated с командой входа.

### DeepSeek

- Bearer-auth запрос `/user/balance`.
- Несколько account configs с разными env names.
- Отображение всех возвращённых balance entries.
- Сохранение ключа в `secrets.env` с `0600`.

### UI

- Panel indicator и popup menu.
- Отображение нескольких аккаунтов.
- 5h/weekly remaining и reset countdown.
- DeepSeek money formatting.
- Error/waiting/stale/unauthenticated/depleted labels.
- File monitor плюс периодическая перечитка.
- Ручной refresh через restart user-service.

### Установка и доставка

- Rootless `install.sh`.
- `ai-usage-uninstall` и `--keep-config`.
- systemd user-service template.
- Online installer последнего GitHub Release.
- CI workflow.
- Release workflow для `x86_64` и `aarch64`.
- Packaging script и release checksums.

## Что фактически проверено

- `bash -n`: install, uninstall, online install, package scripts.
- `node --check`: `extension.js`.
- Парсинг metadata JSON, Cargo/config TOML и workflow YAML.
- Packaging smoke test.
- Rootless install/uninstall smoke в изолированных HOME/XDG каталогах с тестовым binary.

Ограничение доказательства: тестовый install smoke подтверждает раскладку файлов и обратимость скриптов, но не работу настоящего Rust binary, user systemd session или GNOME Shell.

## Что обязательно требует тестирования

- Любая компиляция Rust и корректность зависимостей на Rust 1.80/stable.
- `cargo test`, `clippy`, release build.
- Живой Codex App Server handshake и актуальная JSON schema.
- Правильность bucket `codex` для разных планов Codex.
- Два одновременно настроенных Codex аккаунта.
- Реальный Claude statusLine payload с `rate_limits`.
- Backup/restore Claude settings на существующем сложном JSON.
- Реальный DeepSeek key, currency entries, depleted/HTTP errors.
- GNOME runtime: enable/disable, file monitor, popup rendering, logout/login.
- Совместимость с фактической GNOME версией Bluefin.
- systemd user-service после reboot/login.
- Release workflow, aarch64 cross-build и установка release archive.
- Online installer на настоящем GitHub Release.

## Что требует доработки до уверенного `v0.1.0`

### P0 — релизные блокеры

- Добавить `Cargo.lock` для воспроизводимой сборки приложения.
- Изменить CI format step на `cargo fmt --all -- --check`.
- Запускать Clippy с `-- -D warnings`.
- Подтвердить или исправить последовательность Codex initialize handshake.
- Добавить явный HTTP timeout для DeepSeek и общий дедлайн provider refresh.
- Не отбрасывать всю диагностику Codex: добавить безопасный bounded stderr capture/redaction.
- Проверить checksum в `install-online.sh`, а не только публиковать `SHA256SUMS`.
- Зафиксировать поддержанные GNOME версии после runtime tests; убрать неподтверждённые из metadata.

### P1 — важная эксплуатационная доработка

- Добавить `ai-usage account list/remove` или эквивалентное управление без ручного TOML.
- Валидировать уникальность ID и уникальность Claude/Codex directories.
- Добавить provider fixtures/mocks и тесты parsing/error states.
- Сделать stale threshold и refresh interval управляемыми через setup.
- Сохранять last-good state при временном provider error вместо полного исчезновения quota data.
- Показать в UI возраст данных каждого аккаунта.
- Добавить upgrade flow и документировать совместимость config/state schema.
- Проверить quoting путей с пробелами в systemd template/install scripts.

## Что не готово

- Production-ready статус.
- Опубликованный и проверенный GitHub Release.
- Подтверждённая установка одной командой из release.
- Полная автоматическая детекция Claude plan.
- GUI Preferences для GNOME extension.
- Безопасное хранение DeepSeek key в Secret Service/keyring.
- Полноценные integration/e2e tests.
- Локализация UI.
- Документированная политика обновления/миграций.

## Известные риски и ограничения

- Claude quota cache зависит от фактического запуска Claude Code и ответа модели.
- Claude plan ручной и может быть неверным.
- Неправильный/изменившийся Codex JSON-RPC schema даст error state всем Codex profiles.
- Все Codex app-server процессы запускаются параллельно на каждом refresh; при большом числе аккаунтов это может быть тяжело.
- DeepSeek request без timeout может остановить завершение всего refresh cycle.
- Повреждённый `config.toml` завершает daemon; systemd будет пытаться его перезапускать.
- Два Claude account entries с одним config dir могут испортить ожидаемое восстановление hook.
- `secrets.env` — plaintext, хотя права ограничены пользователем.
- Online installer доверяет TLS/GitHub asset и пока не сверяет опубликованный checksum.
- Metadata GNOME 45–50 не подтверждена реальными тестами.

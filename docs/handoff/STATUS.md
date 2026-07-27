# Инженерный статус

> Родительский индекс: [`HANDOFF.md`](../../HANDOFF.md)  
> Снимок состояния: **27 июля 2026 года**.

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

### CLI управления

- `ai-usage account list/add/remove/rename` с неинтерактивными флагами.
- `ai-usage config show/set` для `refresh_seconds` и `stale_seconds`.
- Валидация уникальности ID и монопольности Claude/Codex каталогов.
- Удаление Claude-аккаунта восстанавливает его `statusLine`.

### Надёжность

- Таймауты HTTP DeepSeek (15 с запрос, 5 с соединение) и общий 30-секундный дедлайн на каждый provider fetch.
- Bounded redacted stderr Codex в тексте ошибки.
- last-known-good: при разовом сбое провайдера квота сохраняется, аккаунт помечается `stale` с настоящим возрастом данных.

## Что фактически проверено

Полные условия, команды и результаты — в [`TESTING.md`](TESTING.md). Кратко, на Bluefin 44.20260721 / GNOME Shell 50.3 / Rust 1.97.1 / codex-cli 0.145.0 / claude 2.1.220:

- `cargo fmt --check`, `clippy -D warnings`, `cargo test` (85 тестов), release build — все зелёные.
- Живой Codex handshake и схема ответа; план `plus` отдаёт одно недельное окно.
- Два независимых `CODEX_HOME` в одном конфиге.
- Ошибки Codex: нет команды, не авторизован, несуществующий `CODEX_HOME`.
- Реальный Claude status-line hook, кеш и оба окна; схема payload сверена с бинарём `claude` 2.1.220.
- Rootless install/uninstall/reinstall на настоящей системе, включая путь с пробелом и `&`.
- Восстановление `~/.claude/settings.json` байт в байт после удаления.
- Проверка `SHA256SUMS` в online installer на целом, изменённом и отсутствующем в списке архиве.
- Логика UI расширения под `gjs` на настоящем `state.json`.
- last-known-good при намеренно сломанном провайдере.

## Что обязательно требует тестирования

- **Рендеринг расширения в GNOME Shell.** На Wayland требуется перезаход в сессию, чтобы Shell увидел новое расширение; до этого `gnome-extensions enable` не работает. Не проверены: popup при разных наборах аккаунтов, file monitor, кнопка обновления, многократные enable/disable, длинные строки.
- systemd user-service после настоящего reboot/login.
- Реальный DeepSeek key: currency entries, depleted, 401/403.
- Release workflow, aarch64 cross-build и установка release archive.
- Online installer на настоящем GitHub Release.

## Что требует доработки до уверенного `v0.1.0`

### P0 — релизные блокеры

Все закрыты в коде; остались только проверки из раздела выше.

### P1 — важная эксплуатационная доработка

- Добавить upgrade flow и документировать совместимость config/state schema.
- Показать в UI возраст данных для аккаунтов без квоты (сейчас возраст выводится только когда данные есть).

## Что не готово

- Production-ready статус: код и локальная приёмка готовы, но расширение ни разу не отрисовывалось в GNOME Shell и релиз не публиковался.
- Опубликованный и проверенный GitHub Release.
- Подтверждённая установка одной командой из release.
- Полная автоматическая детекция Claude plan.
- GUI Preferences для GNOME extension.
- Безопасное хранение DeepSeek key в Secret Service/keyring.
- Локализация UI.
- Документированная политика обновления/миграций.

## Известные риски и ограничения

- Claude quota cache зависит от фактического запуска Claude Code и ответа модели.
- Claude plan ручной и может быть неверным.
- Неправильный/изменившийся Codex JSON-RPC schema даст error state всем Codex profiles.
- На плане Codex `plus` возвращается только недельное окно — строки «5 часов» в UI не будет. Это поведение провайдера, а не дефект.
- Все Codex app-server процессы запускаются параллельно на каждом refresh; при большом числе аккаунтов это может быть тяжело.
- Повреждённый `config.toml` завершает daemon; systemd будет пытаться его перезапускать.
- `secrets.env` — plaintext, хотя права ограничены пользователем (0600 с момента создания).
- Redaction диагностики Codex эвристическая: она рассчитана на известные формы токенов и не является гарантией.
- Metadata заявляет GNOME 48–50; реально проверена только 50.3.
- MSRV 1.86 продиктован графом зависимостей (`icu_*` через `url` → `reqwest`), а не самим кодом.

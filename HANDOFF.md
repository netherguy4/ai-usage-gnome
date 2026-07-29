# AI Usage for GNOME — handoff

> Актуальность: **29 июля 2026 года**  
> Стадия: **Backend и панель приняты на реальной системе; Antigravity реализован и прошёл CI, но требует живого теста с `agy`; осталась публикация стабильного релиза**.

Этот файл — короткая точка входа для разработчика или следующей модели. Не загружай всю документацию сразу: открой только файл, связанный с текущей задачей.

## Что это за проект

AI Usage for GNOME показывает в верхней панели GNOME состояние нескольких аккаунтов Claude Code, Codex, Google Antigravity и, опционально, баланс DeepSeek API. Проект решает проблему отсутствия единого быстрого представления подписочных лимитов при работе с несколькими AI-аккаунтами.

Архитектура гибридная: GNOME Shell extension на GJS отвечает только за UI, Rust-процесс собирает данные провайдеров и атомарно записывает локальный `state.json`.

## Состояние одним абзацем

Rust CLI/daemon, четыре provider-адаптера, multi-account config, Claude status-line hook, официальный `agy` status-line consumer, GNOME-индикатор, rootless install/uninstall, systemd user-service и GitHub Actions реализованы и собираются. На Bluefin 44 / GNOME Shell 50.3 проверены живьём: Codex handshake на реальном аккаунте и на втором независимом `CODEX_HOME`, активный опрос лимитов Claude и его поведение под `429`, живой ключ DeepSeek, rootless install/uninstall/reinstall, `secrets.env` с правами 600, last-known-good при сбое провайдера. Панель и меню отрисованы в настоящей сессии с Claude, Codex и DeepSeek. Antigravity использует официальный JSON status-line interface, не читает keyring/OAuth и прошёл CI на Rust 1.86; живой payload `agy` и рендер четвёртого провайдера ещё не проверены. 116 автотестов. **Стабильный релиз не публиковался.**

**Особенность разработки:** на Wayland GNOME Shell продолжает исполнять ранее загруженную копию расширения — `ReloadExtension` в D-Bus объявлен, но вне режима разработчика не реализован. Любая правка `extension.js` или `stylesheet.css` видна только после перезахода в сессию.

## Куда идти за деталями

| Текущая задача | Открыть |
|---|---|
| Понять назначение, пользователей и границы продукта | [`docs/handoff/PROJECT.md`](docs/handoff/PROJECT.md) |
| Понять компоненты, поток данных и файловую структуру | [`docs/handoff/ARCHITECTURE.md`](docs/handoff/ARCHITECTURE.md) |
| Продолжить или проверить интеграцию Antigravity | [`docs/handoff/ANTIGRAVITY.md`](docs/handoff/ANTIGRAVITY.md) |
| Узнать, что готово, не готово и требует доработки | [`docs/handoff/STATUS.md`](docs/handoff/STATUS.md) |
| Компилировать, проверять провайдеры и принимать MVP | [`docs/handoff/TESTING.md`](docs/handoff/TESTING.md) |
| Устанавливать, настраивать, удалять и выпускать релиз | [`docs/handoff/OPERATIONS.md`](docs/handoff/OPERATIONS.md) |
| Выбрать следующую задачу и не выйти за scope | [`docs/handoff/ROADMAP.md`](docs/handoff/ROADMAP.md) |

Пользовательская инструкция находится в [`README.md`](README.md). Она описывает предполагаемый happy path, а handoff-документация — фактическое инженерное состояние и риски.

## Рекомендуемый следующий шаг

1. Принять Antigravity на живом Google AI Pro аккаунте по чек-листу в [`ANTIGRAVITY.md`](docs/handoff/ANTIGRAVITY.md).
2. Доделать остаток чек-листа «P0: GNOME/Bluefin integration» в [`TESTING.md`](docs/handoff/TESTING.md): 0 и 1 аккаунт, длинные строки, многократные enable/disable.
3. Прогнать release workflow и проверить оба архива на текущем commit.
4. Выпустить следующий release candidate, проверить `SHA256SUMS` и `install-online.sh` на чистом профиле.
5. Только после зелёной матрицы — стабильный тег.

Подробная матрица — в [`TESTING.md`](docs/handoff/TESTING.md).

## Инварианты, которые нельзя случайно сломать

- Установка и удаление должны работать **без `sudo`** и не менять системные файлы.
- Несколько аккаунтов должны быть изолированы через отдельные `CLAUDE_CONFIG_DIR` / `CODEX_HOME`; для `agy` нельзя смешивать snapshot ID разных профилей.
- GNOME extension не должен выполнять сетевые запросы и не должен блокировать GNOME Shell.
- Сбой одного провайдера не должен скрывать данные остальных аккаунтов.
- API-ключи и auth-файлы нельзя выводить в `state.json`, логи или интерфейс.
- Antigravity provider не должен читать keyring/OAuth или сохранять полный status-line payload: только отображаемый quota snapshot.
- Удаление должно безопасно восстановить прежний Claude `statusLine`, не перезаписывая более новые пользовательские изменения.
- Изменения формата `state.json` требуют увеличения `schema_version` и обратной совместимости UI либо явной миграции.

## Как поддерживать handoff актуальным

После существенного изменения обнови минимум:

- дату и краткий статус в этом файле;
- соответствующий раздел в [`STATUS.md`](docs/handoff/STATUS.md);
- тестовое доказательство в [`TESTING.md`](docs/handoff/TESTING.md) или профильном handoff-файле;
- [`ROADMAP.md`](docs/handoff/ROADMAP.md), если изменился scope или приоритет.

Не отмечай интеграцию «готовой» только потому, что код существует. Для статуса «проверено» укажи среду, команду или сценарий, который реально прошёл.

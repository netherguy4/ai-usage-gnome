# Roadmap и границы scope

> Родительский индекс: [`HANDOFF.md`](../../HANDOFF.md)

Roadmap описывает направление, а не обещанные сроки. Приоритет меняется только после обновления этого файла и [`STATUS.md`](STATUS.md).

## P0 — довести существующий MVP до проверяемого релиза

- Скомпилировать и исправить Rust project.
- Добавить `Cargo.lock`.
- Усилить CI format/clippy gates.
- Проверить Codex handshake и response schema на живых аккаунтах.
- Добавить provider timeouts и безопасную диагностику.
- Протестировать Claude hook/restore.
- Протестировать extension на целевой Bluefin/GNOME.
- Добавить checksum verification online installer.
- Выпустить и проверить `v0.1.0-rc.1`.
- После успешной матрицы выпустить `v0.1.0`.

## P1 — сделать ежедневное использование удобным

- CLI управления аккаунтами: list, add, update, remove, rename.
- Non-interactive setup flags для автоматизации.
- Настройка refresh/stale thresholds через CLI.
- Last-known-good cache с явным отображением возраста.
- Более информативный panel summary без перегрузки панели.
- Цветовые/иконные warning states с учётом GNOME accessibility.
- Отдельный status для provider unavailable и stale-data-with-error.
- Safe diagnostics command с redacted provider details.
- Upgrade command или чёткий in-place upgrade flow.

## P1 — тестируемость и надёжность

- Fixture tests для Claude/Codex payloads.
- Mock HTTP tests DeepSeek.
- Integration test subprocess JSONL protocol.
- Tests config migration и uninstall restore.
- CI packaging/install smoke с настоящим binary.
- Проверка release asset на arm64.

## P2 — улучшения после стабильного MVP

- GNOME Preferences UI вместо обязательного CLI.
- Secret Service/libsecret для DeepSeek API keys.
- D-Bus API между daemon и extension вместо polling JSON, если это реально улучшит UX.
- Provider plugin abstraction после появления четвёртого провайдера, не раньше.
- Локализация минимум RU/EN/UK.
- Optional desktop notifications при достижении настраиваемого порога.
- Обновление/проверка новой версии из CLI без auto-update по умолчанию.

## В планах, но не для первого релиза

- Поддержка дополнительных официальных AI usage/balance APIs при наличии стабильного и разрешённого интерфейса.
- Больше типов Codex rate-limit buckets, если они встречаются в реальных responses.
- Точная human-readable дата reset и возраст данных.
- Более компактный режим панели для большого числа аккаунтов.

## Не планируется

Следующее остаётся вне scope, пока владелец проекта явно не изменит требования:

- web scraping страниц Claude.ai, ChatGPT или кабинетов провайдеров;
- чтение browser cookies/session tokens;
- обход, объединение или автоматическое переключение лимитов для сокрытия/нарушения provider policies;
- автоматическая покупка подписок, пополнение баланса или финансовые операции;
- хранение паролей Claude/OpenAI;
- собственная облачная служба, telemetry или синхронизация аккаунтов;
- полноценная аналитика токенов, стоимости запросов и истории prompts;
- управление моделями/чатами из виджета;
- замена официальных Claude/Codex clients;
- Windows/macOS версии этого GNOME extension;
- Conky/desktop wallpaper widget в рамках текущего repository;
- поддержка KDE Plasma в том же UI-коде;
- публикация в GNOME Extensions directory до подтверждения качества, совместимости и требований review.

## Принципы принятия новых задач

Новая функция подходит проекту, если она:

- помогает быстро понять доступность AI-аккаунтов;
- использует официальный или локально разрешённый интерфейс;
- не требует выдавать extension сетевые/секретные полномочия;
- сохраняет rootless и multi-account модель;
- может быть протестирована без реальных секретов через fixtures/mocks.

Функция должна быть отклонена или вынесена в отдельный проект, если превращает индикатор в полноценный AI-client, нарушает приватность или делает GNOME Shell зависимым от сети.

# Google Antigravity (`agy`)

> Родительский индекс: [`HANDOFF.md`](../../HANDOFF.md)  
> Добавлено в PR #2, 29 июля 2026 года.

## Назначение

Провайдер показывает подписочную квоту Google AI Pro / Antigravity рядом с Claude, Codex и DeepSeek. Источник данных — официальный custom status-line payload `agy`; AI Usage не читает system keyring, OAuth-токены и не вызывает Google API самостоятельно.

## Поток данных

```text
agy
  └─ JSON stdin → ai-usage agy-hook --account <id>
                    └─ atomic 0600 snapshot
                         $XDG_RUNTIME_DIR/ai-usage/antigravity-usage-<id>.json
                              └─ daemon provider fetch
                                   └─ state.json → GNOME extension
```

Hook сохраняет только `email`, `plan_tier`, активную модель, quota windows, reset timestamps и время обновления. Полный payload и credentials не сохраняются.

## Настройка

```bash
ai-usage account add antigravity --id agy-main --name "Google AI Pro" --hook
```

Команда создаёт user-only wrapper в `~/.config/ai-usage/hooks/` и печатает команду, которую нужно выполнить внутри `agy`:

```text
/statusline ~/.config/ai-usage/hooks/agy-agy-main.sh
```

Для совместного отображения со встроенной строкой `agy` используется `stack_with_default: true` в `~/.gemini/antigravity-cli/settings.json`.

## Семантика квоты

- Список bucket-ов открыт: незнакомые будущие ключи не теряются.
- Для активной модели соответствующий pool сортируется первым и становится headline без `scope`.
- Остальные model pools остаются scoped, поэтому исчерпанный неактивный pool не заставляет панель показывать общий `0%`.
- `remaining_fraction` переводится в `remaining_percent`; `reset_in_seconds` — в абсолютный `resets_at`.
- Пустой payload квоты не перезаписывает последний удачный snapshot.

## Свежесть и ошибки

`agy` вызывает hook только во время своей работы. Демон не делает фоновых запросов к Google. Если snapshot старше глобального `stale_seconds`, аккаунт сохраняет last-known-good данные, получает статус `stale` и подсказку открыть `agy` или выполнить `/usage`.

## Тестовое доказательство

GitHub Actions CI run `30457571367` для commit `ec698d15c53db20275d4c475f4f65609e6bc5ee1`:

- сборка на заявленном MSRV Rust 1.86 — успешно;
- `cargo fmt --all -- --check` — успешно;
- Clippy с `-D warnings` — успешно;
- 116 Rust-тестов — успешно;
- release build — успешно;
- синтаксис GNOME extension — успешно;
- ShellCheck установочных скриптов — успешно.

Новые unit-тесты проверяют официальный payload, выбор Gemini/third-party pool по активной модели, сохранение неизвестного bucket-а, пустую квоту, ограничение процентов и отбрасывание non-finite значений.

## Что ещё проверить живьём

- реальный payload текущей версии `agy` и фактические bucket keys на Google AI Pro;
- вызов hook после `/usage` и `/quota`;
- отображение плана, email, модели и reset countdown в GNOME Shell;
- stale-переход после закрытия `agy`;
- поведение `stack_with_default` в пользовательской конфигурации;
- два изолированных `agy`-профиля, если CLI поддерживает раздельные settings/session каталоги.

# Provider fixtures

Ответы провайдеров, снятые для тестов parsing без сети и без реальных секретов.

| Файл | Происхождение | Что проверяет |
|---|---|---|
| `codex_account_authenticated.json` | Живой `codex app-server` 0.145.0, 27 июля 2026. Email заменён на `user@example.com` | Успешный `account/read` |
| `codex_account_unauthenticated.json` | Живой app-server, пустой `CODEX_HOME` | `account: null` → status `unauthenticated` |
| `codex_rate_limits_plus.json` | Живой app-server, план `plus` | Только `primary` (10080 мин), `secondary: null` |
| `codex_rate_limits_unauthenticated_error.json` | Живой app-server, пустой `CODEX_HOME` | JSON-RPC `error -32600` |
| `codex_rate_limits_two_windows.json` | **Синтетический** — сконструирован по схеме живого ответа | Пара окон 5h + weekly |
| `claude_statusline.json` | Схема из Claude Code 2.1.220, значения синтетические | Payload status-line hook |

Порядок ответов важен: на неавторизованном профиле реальный app-server присылает
ошибку `id: 2` **раньше** ответа `id: 1`. Тест `handles_responses_in_reverse_order`
фиксирует это.

При обновлении фикстур не добавляй настоящие email, токены и содержимое `auth.json`.

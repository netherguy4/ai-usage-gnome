# AI Usage for GNOME

Нативный индикатор в верхней панели GNOME для нескольких аккаунтов:

- **Claude Code** — тариф, почта и все лимиты аккаунта: 5-часовой, недельный и отдельные лимиты моделей;
- **Codex** — тариф/email, остаток короткого и недельного лимитов;
- **Google Antigravity (`agy`)** — тариф, email, активная модель и все пулы квоты из официального status-line payload;
- **DeepSeek API** — доступный баланс;
- любое количество профилей через отдельные `CLAUDE_CONFIG_DIR`, `CODEX_HOME` и отдельные ID hook-снимков `agy`.

Проект состоит из маленького GNOME Shell extension на GJS и фонового Rust-сервиса. Расширение не делает сетевых запросов и читает только локальный `state.json`.

Инженерная передача проекта и актуальный статус: [`HANDOFF.md`](HANDOFF.md).

## Быстрая установка

После публикации первого GitHub Release доступна установка одной командой:

```bash
curl -fsSL https://raw.githubusercontent.com/netherguy4/ai-usage-gnome/main/install-online.sh | bash
```

Либо скачай архив для своей архитектуры из GitHub Releases, распакуй и выполни:

```bash
./install.sh
```

Установка полностью пользовательская, `sudo` не нужен. Скрипт:

1. ставит бинарник в `~/.local/bin/ai-usage`;
2. ставит расширение в `~/.local/share/gnome-shell/extensions/ai-usage@netherguy4`;
3. создаёт и запускает `systemd --user` service;
4. запускает интерактивный мастер аккаунтов.

После первой установки может понадобиться выйти из GNOME и войти снова, затем включить расширение:

```bash
gnome-extensions enable ai-usage@netherguy4
```

На Bluefin также можно включить его через **Extension Manager → Installed → AI Usage**.

### Установка из исходников

Нужен Rust 1.86 или новее:

```bash
git clone https://github.com/netherguy4/ai-usage-gnome.git
cd ai-usage-gnome
./install.sh
```

`install.sh` сам выполнит `cargo build --release`, если готового бинарника нет.

## Настройка

Интерактивный мастер:

```bash
ai-usage setup
```

Он умеет добавлять или обновлять Claude, Codex, DeepSeek и Antigravity. Повторный ID заменяет существующую запись.

Те же действия без диалога — например, для скриптов и dotfiles:

```bash
ai-usage account list
ai-usage account add codex  --id codex-work --name "Codex Work" --codex-home ~/.codex-work
ai-usage account add claude --id claude-main --config-dir ~/.claude
ai-usage account add antigravity --id agy-main --hook
printf '%s' "$DEEPSEEK_KEY" | ai-usage account add deepseek --id ds --api-key-stdin
ai-usage account rename codex-work "Codex рабочий"
ai-usage account remove codex-work
```

Удаление Claude-аккаунта возвращает прежний `statusLine`. Для Antigravity удаляется только созданный wrapper-скрипт: настройки `agy` не переписываются автоматически. Два аккаунта одного провайдера с общим каталогом отклоняются: они дрались бы за один профиль.

Частота обновления и порог устаревания:

```bash
ai-usage config show
ai-usage config set --refresh-seconds 60 --stale-seconds 43200
```

Проверка установки:

```bash
ai-usage doctor
ai-usage once
journalctl --user -u ai-usage.service -f
```

### Google Antigravity (`agy`)

Интеграция использует официальный custom status line интерфейс Antigravity CLI. `agy` сам передаёт скрипту JSON с `quota`, `plan_tier`, `email` и активной моделью. AI Usage не читает системный keyring, OAuth-токены и не вызывает внутренние Google API.

Добавь аккаунт и создай wrapper:

```bash
ai-usage account add antigravity --id agy-main --hook
```

Команда напечатает путь вроде:

```text
~/.config/ai-usage/hooks/agy-agy-main.sh
```

Внутри `agy` подключи его:

```text
/statusline ~/.config/ai-usage/hooks/agy-agy-main.sh
```

Чтобы сохранить встроенную строку `agy` и добавить строку AI Usage второй строкой, в `~/.gemini/antigravity-cli/settings.json` выставь:

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.config/ai-usage/hooks/agy-agy-main.sh",
    "stack_with_default": true
  }
}
```

`/usage` и `/quota` принудительно обновляют квоту с backend. При обычной работе hook вызывается при изменениях состояния агента. Snapshot сохраняется атомарно с правами `600` в:

```text
~/.local/state/ai-usage/antigravity-usage-agy-main.json
```

Не в `$XDG_RUNTIME_DIR`, как кеш остальных провайдеров: те данные демон восполняет сам, а квоту Antigravity присылает только `agy`. На tmpfs она пропадала бы при каждой перезагрузке, и панель просила бы заново подключить hook, хотя цифры никуда не делись.

Если `agy` закрыт, сеть не опрашивается и snapshot не обновляется — но и квота не тратится, поэтому цифры остаются верными. Поэтому устаревшими они считаются не через глобальные `stale_seconds`, а прожив самое короткое окно квоты (на Google AI Pro — 5 часов); глобальная настройка служит нижней границей. Только тогда появляется `~` рядом с процентом и подсказка открыть `agy` или выполнить `/usage`. Неизвестные будущие пулы квоты сохраняются и появляются в меню без обновления приложения.

**Какие лимиты приходят.** `agy` присылает четыре пула: два семейства моделей — Gemini и сторонние модели — и у каждого своё пятичасовое и недельное окно.

```text
Gemini · 5 часов              Сторонние модели · 5 часов
Gemini · Неделя               Сторонние модели · Неделя
```

В панель выносится семейство активной модели, причём **оба** его окна считаются общими: исчерпанное недельное блокирует работу так же, как пятичасовое. Окна второго семейства видны в меню, но панель в `0%` не роняют — там достаточно сменить модель.

### Несколько Claude-аккаунтов

Для каждого профиля укажи отдельный каталог, например:

```text
~/.claude
~/.claude-work
```

Запуск второго профиля:

```bash
CLAUDE_CONFIG_DIR="$HOME/.claude-work" claude
```

Лимиты запрашиваются напрямую у Anthropic — по тому же официальному endpoint, который опрашивает сам Claude Code, с OAuth-токеном из вашего профиля. Ничего настраивать и запускать не нужно: цифры обновляются в фоне вместе с остальными аккаунтами.

Этот endpoint ограничивает частоту: чаще чем раз в пять минут его не спрашивают, а ответ `429` увеличивает паузу до часа и сбрасывает её первым успехом. Бюджет общий с самим Claude Code, поэтому отказ — нормальная ситуация, а не поломка. Пока пауза не вышла, показывается последний удачный ответ с его настоящим возрастом.

`settings.json` при этом не меняется. Если хочется видеть лимиты **ещё и внутри** Claude Code, добавьте `--hook` — тогда в `settings.json` пропишется `statusLine`, а перед изменением появится резервная копия `settings.json.ai-usage.bak`, которую вернёт удаление аккаунта.

Список лимитов не зашит в код: показывается всё, что вернул сервер, включая лимиты отдельных моделей. Такие лимиты бывают временными — если Anthropic его уберёт, строка просто исчезнет, а если добавит новый, он появится сам, без обновления приложения.

Тариф и почта читаются из `.claude.json`, который пишет сам Claude Code. Тариф берётся из уровня лимитов, а не из типа подписки, поэтому видно **Max 5x** или **Max 20x**, а не просто «Max». Тариф участника организации имеет приоритет над тарифом самой организации. `--plan` нужен, только чтобы переопределить определённое значение.

**Ограничение Claude:** OAuth-токен живёт около восьми часов, и обновляет его только сам Claude Code. Если вы не запускали его дольше, виджет покажет последние данные из кеша Claude Code и пометит их возраст.

Устаревшими данные считаются через 15 минут — это 5% пятичасового окна (`ai-usage config set --stale-seconds`). Такой аккаунт получает `~` рядом с числом и строку с причиной и возрастом. Лимит, чей период уже закончился, не показывается вовсе: его использование относится к прошлому периоду и о текущем не говорит ничего.

### Несколько Codex-аккаунтов

Для каждого аккаунта укажи отдельный `CODEX_HOME`, например:

```text
~/.codex
~/.codex-work
```

Вход вручную:

```bash
CODEX_HOME="$HOME/.codex-work" codex login
```

Сервис запускает официальный `codex app-server` по stdio, делает handshake, затем вызывает:

```text
account/read
account/rateLimits/read
```

Из ответа выбирается bucket `codex` (настраивается полем `limit_id`), а его окна получают подпись по длительности. Тариф (`Plus`, `Pro`, `Business`, …) приходит из `planType`.

**Что показывается на разных планах.** Codex возвращает столько окон, сколько есть у тарифа. На `plus`, например, приходит только недельное окно — тогда строки «5 часов» в меню не будет. Это ответ провайдера, а не потеря данных.

### DeepSeek

API-ключ сохраняется в:

```text
~/.config/ai-usage/secrets.env
```

Файл имеет права `600` и подключается к user-service через `EnvironmentFile`. Баланс запрашивается официальным `GET /user/balance`.

## Ручной конфиг

Основной файл:

```text
~/.config/ai-usage/config.toml
```

Пример находится в [`config/config.example.toml`](config/config.example.toml). После ручного изменения перезапусти сервис:

```bash
systemctl --user restart ai-usage.service
```

## Удаление

После установки доступна одна команда:

```bash
ai-usage-uninstall
```

Из распакованного архива или репозитория также работает `./uninstall.sh`.

Удаляются бинарник, расширение, сервис, кеш и конфигурация. Claude `settings.json` восстанавливается автоматически.

Оставить конфигурацию и DeepSeek secrets для будущей переустановки:

```bash
ai-usage-uninstall --keep-config
```

## Команды

```text
ai-usage daemon                фоновый цикл обновления
ai-usage once                  одно обновление + JSON в stdout
ai-usage setup                 интерактивный мастер
ai-usage account list|add|remove|rename    управление аккаунтами
ai-usage config show|set       частота обновления и порог устаревания
ai-usage doctor                диагностика
ai-usage init                  создать пустой config.toml
ai-usage claude-hook ...       необязательный status line внутри Claude Code
ai-usage agy-hook ...          официальный status line hook Antigravity CLI
ai-usage restore-claude-hooks  восстановить Claude settings.json
```

## Что происходит при сбое провайдера

Разовая ошибка не стирает цифры: аккаунт помечается как устаревший, сохраняет последние удачные данные и показывает их возраст вместе с текстом ошибки. Провайдер, у которого данных не было вовсе, показывается отдельно — как недоступный. Сбой одного аккаунта не влияет на остальные: у каждого свой 30-секундный дедлайн.

## Что показано в самой панели

Логотип провайдера и одно число на аккаунт:

1. если исчерпан любой **общий** лимит — `0%`, потому что запас в коротком окне уже ничего не значит;
2. иначе пятичасовой лимит;
3. если у тарифа его нет — недельный или активный пул квоты;
4. у провайдеров без лимитов, как DeepSeek, — баланс.

Лимиты, привязанные к модели, из правила 1 исключены: исчерпанный лимит одной модели не блокирует работу, достаточно переключить модель. У Antigravity общими считаются **все** окна того семейства моделей, которым `agy` пользуется сейчас, — и пятичасовое, и недельное; окна остальных семейств остаются scoped. Именно поэтому панель не берёт минимум по всем окнам подряд.

Остальные лимиты видны в меню. Строки «обновлено» там нет: возраст появляется только когда данные устарели, вместе с причиной.

Кнопки «обновить» нет — данные обновляет фоновый сервис или официальный hook провайдера.

## GitHub Actions

- `.github/workflows/ci.yml` — `rustfmt`, тесты, Clippy, проверка JS и ShellCheck;
- `.github/workflows/release.yml` — сборка `x86_64` и `aarch64`, упаковка архивов;
- push тега `v0.1.0` автоматически создаёт GitHub Release;
- ручной запуск workflow собирает те же архивы как Actions artifacts без публикации Release.

Релиз:

```bash
git tag v0.1.0
git push origin v0.1.0
```

## Используемые официальные интерфейсы

- [Antigravity CLI status line](https://antigravity.google/docs/cli/statusline)
- [Antigravity Model Quotas (`/usage`)](https://antigravity.google/docs/cli/commands/usage)
- [Codex App Server](https://developers.openai.com/codex/app-server)
- [Claude Code status line](https://code.claude.com/docs/en/statusline) — для необязательного `--hook`
- [DeepSeek Get User Balance](https://api-docs.deepseek.com/api/get-user-balance)
- [GNOME Shell extension guide](https://gjs.guide/extensions/)

## Лицензия

GPL-3.0-or-later.

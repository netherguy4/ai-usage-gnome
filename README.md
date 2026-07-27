# AI Usage for GNOME

Нативный индикатор в верхней панели GNOME для нескольких аккаунтов:

- **Claude Code** — тариф, почта и все лимиты аккаунта: 5-часовой, недельный и отдельные лимиты моделей;
- **Codex** — тариф/email, остаток короткого и недельного лимитов;
- **DeepSeek API** — доступный баланс;
- любое количество профилей через отдельные `CLAUDE_CONFIG_DIR` и `CODEX_HOME`.

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

Он умеет добавлять или обновлять Claude, Codex и DeepSeek. Повторный ID заменяет существующую запись.

Те же действия без диалога — например, для скриптов и dotfiles:

```bash
ai-usage account list
ai-usage account add codex  --id codex-work --name "Codex Work" --codex-home ~/.codex-work
ai-usage account add claude --id claude-main --config-dir ~/.claude
printf '%s' "$DEEPSEEK_KEY" | ai-usage account add deepseek --id ds --api-key-stdin
ai-usage account rename codex-work "Codex рабочий"
ai-usage account remove codex-work
```

Удаление Claude-аккаунта возвращает прежний `statusLine`. Два аккаунта одного провайдера с общим каталогом отклоняются: они дрались бы за один профиль.

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

`settings.json` при этом не меняется. Если хочется видеть лимиты **ещё и внутри** Claude Code, добавьте `--hook` — тогда в `settings.json` пропишется `statusLine`, а перед изменением появится резервная копия `settings.json.ai-usage.bak`, которую вернёт удаление аккаунта.

Список лимитов не зашит в код: показывается всё, что вернул сервер, включая лимиты отдельных моделей. Такие лимиты бывают временными — если Anthropic его уберёт, строка просто исчезнет, а если добавит новый, он появится сам, без обновления приложения.

Тариф и почта читаются из `.claude.json`, который пишет сам Claude Code. `--plan` нужен, только чтобы переопределить определённое значение.

**Ограничение Claude:** OAuth-токен живёт около восьми часов, и обновляет его только сам Claude Code. Если вы не запускали его дольше, виджет покажет последние данные из кеша Claude Code и пометит их возраст.

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

Из ответа выбирается bucket `codex` (настраивается полем `limit_id`), а его окна получают подпись по длительности.

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
ai-usage restore-claude-hooks  восстановить Claude settings.json
```

## Что происходит при сбое провайдера

Разовая ошибка не стирает цифры: аккаунт помечается как устаревший, сохраняет последние удачные данные и показывает их возраст вместе с текстом ошибки. Провайдер, у которого данных не было вовсе, показывается отдельно — как недоступный. Сбой одного аккаунта не влияет на остальные: у каждого свой 30-секундный дедлайн.

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

- [Codex App Server](https://developers.openai.com/codex/app-server)
- [Claude Code status line](https://code.claude.com/docs/en/statusline) — для необязательного `--hook`
- [DeepSeek Get User Balance](https://api-docs.deepseek.com/api/get-user-balance)
- [GNOME Shell extension guide](https://gjs.guide/extensions/)

## Лицензия

GPL-3.0-or-later.

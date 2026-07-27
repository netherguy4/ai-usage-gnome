# AI Usage for GNOME

Нативный индикатор в верхней панели GNOME для нескольких аккаунтов:

- **Claude Code** — тариф, остаток 5-часового и недельного лимитов;
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

Нужен Rust toolchain:

```bash
git clone https://github.com/netherguy4/ai-usage-gnome.git
cd ai-usage-gnome
./install.sh
```

`install.sh` сам выполнит `cargo build --release`, если готового бинарника нет.

## Настройка

В любой момент:

```bash
ai-usage setup
```

Мастер умеет добавлять или обновлять Claude, Codex и DeepSeek. Повторный ID заменяет существующую запись, поэтому вручную TOML обычно редактировать не нужно.

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

Мастер добавляет в `settings.json` официальный Claude Code `statusLine` command. Он получает `rate_limits.five_hour` и `rate_limits.seven_day`, записывает их в локальный кеш и одновременно показывает краткую строку в Claude Code.

Перед изменением существующего `settings.json` создаётся резервная копия `settings.json.ai-usage.bak`. Удаление проекта восстанавливает её.

**Ограничение Claude:** данные обновляются только после ответа Claude Code. Если аккаунтом давно не пользовались, виджет помечает данные как устаревшие.

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

Из ответа выбирается bucket `codex` (настраивается полем `limit_id`) и окна, ближайшие к 5 часам и 7 дням.

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
ai-usage doctor                диагностика
ai-usage init                  создать пустой config.toml
ai-usage claude-hook ...       внутренний Claude status-line hook
ai-usage restore-claude-hooks  восстановить Claude settings.json
```

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
- [Claude Code status line](https://code.claude.com/docs/en/statusline)
- [DeepSeek Get User Balance](https://api-docs.deepseek.com/api/get-user-balance)
- [GNOME Shell extension guide](https://gjs.guide/extensions/)

## Лицензия

GPL-3.0-or-later.

# Эксплуатация, установка и релизы

> Родительский индекс: [`HANDOFF.md`](../../HANDOFF.md)

## Требования среды

Runtime:

- Linux с GNOME Shell;
- systemd user session — рекомендуемый путь;
- `gnome-extensions` или Extension Manager;
- Claude Code/Codex CLI только для соответствующих provider integrations;
- сеть для Codex account/rate-limit calls и DeepSeek.

Сборка из source:

- Rust stable, заявленный минимум в Cargo manifest — 1.86. Это диктуется графом зависимостей (`icu_*` 2.2 через `url` → `reqwest`), а не самим кодом; CI проверяет сборку на этой версии отдельным job;
- standard C linker;
- `python3` нужен `install.sh` для подстановки путей в systemd unit и `install-online.sh` для разбора GitHub API;
- Node используется только в CI для syntax check;
- `zip`, `tar` нужны packaging workflow.

## Установка

### Из repository

```bash
./install.sh
```

Если готового binary нет, script пытается выполнить `cargo build --release`.

### Из release bundle

```bash
tar -xzf ai-usage-gnome-<version>-<arch>.tar.gz
cd ai-usage-gnome-<version>-<arch>
./install.sh
```

### Online

```bash
curl -fsSL https://raw.githubusercontent.com/netherguy4/ai-usage-gnome/main/install-online.sh | bash
```

Installer скачивает `SHA256SUMS` из того же релиза и сверяет контрольную сумму архива до распаковки. Отсутствие записи для архива или расхождение суммы прерывают установку. Это проверено на целом, изменённом и отсутствующем в списке архиве, но ещё не на настоящем GitHub Release.

## Что делает install

- копирует binary в XDG user bin;
- копирует uninstall helper;
- устанавливает extension в XDG data;
- создаёт config/secrets;
- генерирует systemd user unit с абсолютными путями;
- запускает service;
- пытается enable extension;
- запускает interactive setup при TTY, если не передан `--no-setup`.

`sudo` не нужен и не должен добавляться.

## Настройка

Интерактивно:

```bash
ai-usage setup
```

Неинтерактивно:

```bash
ai-usage account list
ai-usage account add codex --id codex-work --name "Codex Work" --codex-home ~/.codex-work
ai-usage account add claude --id claude-main --config-dir ~/.claude --plan Max
printf '%s' "$KEY" | ai-usage account add deepseek --id ds --api-key-stdin
ai-usage account rename codex-work "Codex рабочий"
ai-usage account remove codex-work
ai-usage config show
ai-usage config set --refresh-seconds 60 --stale-seconds 43200
```

Удаление Claude-аккаунта восстанавливает его `statusLine`; `--keep-hook` оставляет hook на месте. Конфиг отклоняет дублирующиеся ID и два аккаунта одного провайдера с общим каталогом.

Проверки:

```bash
ai-usage doctor
ai-usage once
systemctl --user status ai-usage.service
journalctl --user -u ai-usage.service -f
```

## Обновление

Явного upgrade command пока нет. Предполагаемый MVP flow:

1. скачать новый release bundle;
2. запустить его `install.sh --no-setup` поверх существующей версии;
3. проверить `doctor` и service logs;
4. при изменении config/state schema выполнить документированную миграцию.

До реализации migration policy нельзя делать несовместимые изменения schema без bump и release notes.

## Удаление

```bash
ai-usage-uninstall
```

С сохранением config/secrets:

```bash
ai-usage-uninstall --keep-config
```

Перед удалением binary вызывается `restore-claude-hooks`. Если binary отсутствует или повреждён, automatic restore не произойдёт — это нужно учитывать в recovery инструкции.

### Ручное восстановление Claude при потерянном binary

Для каждого затронутого `CLAUDE_CONFIG_DIR`:

- если существует `settings.json.ai-usage.bak`, сначала сравнить его с текущим settings;
- восстановить только поле `statusLine`, а не безусловно весь JSON;
- если пользователь менял `statusLine` после установки, сохранить его изменение;
- удалить backup только после проверки.

## Runtime диагностика

### Нет значка

```bash
gnome-extensions info ai-usage@netherguy4
gnome-extensions enable ai-usage@netherguy4
journalctl --user -b | grep -i 'ai-usage\|gnome-shell'
```

После первой установки может понадобиться logout/login.

### `AI …`

Проверить state:

```bash
ls -l "$XDG_RUNTIME_DIR/ai-usage/state.json"
ai-usage once
```

### Codex error

```bash
CODEX_HOME="$HOME/.codex-profile" codex login
CODEX_HOME="$HOME/.codex-profile" codex app-server
```

Текст ошибки уже содержит exit status app-server и redacted-хвост его stderr, поэтому типичные причины (нет каталога, нет входа, не та команда) видны сразу из `ai-usage once`.

### Claude waiting/stale

Сначала `ai-usage doctor`: он скажет, есть ли вход и сколько живёт токен. Дальше по тексту ошибки в меню:

- **«Anthropic ограничил частоту запросов»** — endpoint отвечает `429`. Ждать: пауза растёт до часа и снимается первым успехом. Бюджет общий с самим Claude Code, так что при активной работе в нём это ожидаемо.
- **«Токен Claude истёк»** — обновить его может только сам Claude Code; достаточно его запустить.
- **«Выполни вход: claude»** — в `<config_dir>/.credentials.json` нет OAuth-записи.

Кеш последнего удачного ответа лежит в `$XDG_RUNTIME_DIR/ai-usage/claude-usage-<id>.json`. Там же хранится момент следующей попытки: удалить файл — значит разрешить запрос немедленно.

Проверить, что происходит на самом деле, можно так (токен в вывод не попадает):

```bash
ai-usage once | jq '.accounts[] | select(.provider=="claude") | {status, error, updated_at}'
```

### DeepSeek key не найден

Проверить имя env в config и строку в `~/.config/ai-usage/secrets.env`. Не печатать значение key в issue/log.

## CI

`ci.yml` проверяет:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
node --check extension/ai-usage@netherguy4/extension.js
shellcheck install.sh uninstall.sh install-online.sh scripts/package.sh
```

Отдельный job `msrv` читает `rust-version` из `Cargo.toml` и выполняет `cargo check --locked` на этой версии, чтобы заявленный минимум не расходился с реальным графом зависимостей.

Раньше format step выполнял `cargo fmt --all` без `--check` — то есть переписывал исходники вместо проверки и не мог упасть; clippy запускался без `-D warnings`. Оба гейта были декоративными.

## Release

Версия задаётся тегом:

```bash
git tag v0.1.0
git push origin v0.1.0
```

Workflow меняет package version в рабочей копии runner, строит два targets, упаковывает и публикует Release. Он не коммитит обновлённую версию обратно в repository.

До stable release:

- проверить, что source `Cargo.toml` version и tag policy понятны;
- решить, нужен ли release automation для changelog;
- проверить aarch64 artifact;
- проверить installer checksum;
- создать `v0.1.0-rc.1`, а не сразу final tag.

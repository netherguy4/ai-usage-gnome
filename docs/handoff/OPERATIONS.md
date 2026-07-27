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

- Rust stable, заявленный минимум в Cargo manifest — 1.80;
- standard C linker;
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

Online path не должен рекламироваться как безопасный production path, пока installer не сверяет `SHA256SUMS`.

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

```bash
ai-usage setup
```

Setup поддерживает add/update по ID. Remove пока выполняется ручным редактированием config, после чего нужен restart service.

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

Текущая версия скрывает stderr app-server; для глубокой диагностики может потребоваться временный debug build с redaction.

### Claude waiting/stale

Запустить Claude Code с правильным `CLAUDE_CONFIG_DIR`, выполнить один запрос и проверить cache в `~/.local/share/ai-usage/claude/`.

### DeepSeek key не найден

Проверить имя env в config и строку в `~/.config/ai-usage/secrets.env`. Не печатать значение key в issue/log.

## CI

`ci.yml` должен проверять:

```bash
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
node --check extension/ai-usage@netherguy4/extension.js
shellcheck ...
```

На момент handoff format и clippy steps слабее этого целевого режима; см. [`STATUS.md`](STATUS.md).

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

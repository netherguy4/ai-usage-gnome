#!/usr/bin/env bash
set -euo pipefail

UUID="ai-usage@netherguy4"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if [[ -f "$SCRIPT_DIR/Cargo.toml" || -d "$SCRIPT_DIR/bin" ]]; then
  ROOT="$SCRIPT_DIR"
else
  ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
fi

RUN_SETUP=1
for arg in "$@"; do
  case "$arg" in
    --no-setup) RUN_SETUP=0 ;;
    -h|--help)
      echo "Usage: ./install.sh [--no-setup]"
      exit 0
      ;;
    *) echo "Unknown argument: $arg" >&2; exit 2 ;;
  esac
done

command -v python3 >/dev/null 2>&1 || {
  echo "Required command not found: python3" >&2
  exit 1
}

BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
EXT_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$UUID"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/ai-usage"
SYSTEMD_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
BINARY="$BIN_DIR/ai-usage"
SECRETS="$CONFIG_DIR/secrets.env"
SERVICE="$SYSTEMD_DIR/ai-usage.service"

if [[ -x "$ROOT/bin/ai-usage" ]]; then
  SOURCE_BINARY="$ROOT/bin/ai-usage"
elif [[ -x "$ROOT/target/release/ai-usage" ]]; then
  SOURCE_BINARY="$ROOT/target/release/ai-usage"
elif [[ -f "$ROOT/Cargo.toml" ]] && command -v cargo >/dev/null 2>&1; then
  echo "Building Rust backend..."
  (cd "$ROOT" && cargo build --release)
  SOURCE_BINARY="$ROOT/target/release/ai-usage"
else
  echo "ai-usage binary not found." >&2
  echo "Use a GitHub Release archive or install Rust and run this script from the repository." >&2
  exit 1
fi

if [[ ! -d "$ROOT/extension/$UUID" ]]; then
  echo "GNOME extension files not found: $ROOT/extension/$UUID" >&2
  exit 1
fi
if [[ ! -f "$ROOT/systemd/ai-usage.service.in" ]]; then
  echo "systemd template not found." >&2
  exit 1
fi

mkdir -p "$BIN_DIR" "$EXT_DIR" "$CONFIG_DIR" "$SYSTEMD_DIR"
install -m 0755 "$SOURCE_BINARY" "$BINARY"
install -m 0755 "$ROOT/uninstall.sh" "$BIN_DIR/ai-usage-uninstall"
EXT_EXISTED=0
[[ -d "$EXT_DIR" ]] && EXT_EXISTED=1
rm -rf "$EXT_DIR"
mkdir -p "$EXT_DIR"
cp -a "$ROOT/extension/$UUID/." "$EXT_DIR/"

touch "$SECRETS"
chmod 600 "$SECRETS"
# Подстановка через python3, а не sed: пути могут содержать |, & и обратные
# слеши, которые sed интерпретирует как спецсимволы замены.
BINARY="$BINARY" SECRETS="$SECRETS" BIN_DIR="$BIN_DIR" python3 - \
  "$ROOT/systemd/ai-usage.service.in" "$SERVICE" <<'PY'
import os
import sys

source, target = sys.argv[1:3]
with open(source, encoding="utf-8") as handle:
    text = handle.read()
for name in ("BINARY", "SECRETS", "BIN_DIR"):
    text = text.replace(f"@{name}@", os.environ[name])
with open(target, "w", encoding="utf-8") as handle:
    handle.write(text)
PY

"$BINARY" init

if command -v systemctl >/dev/null 2>&1; then
  systemctl --user daemon-reload
  systemctl --user enable --now ai-usage.service
else
  echo "Warning: systemctl not found; run '$BINARY daemon' manually." >&2
fi

if command -v gnome-extensions >/dev/null 2>&1; then
  if ! gnome-extensions enable "$UUID" 2>/dev/null; then
    echo "GNOME Shell has not loaded the new extension yet. Log out and log back in, then run:"
    echo "  gnome-extensions enable $UUID"
  elif [[ "$EXT_EXISTED" -eq 1 ]]; then
    # На Wayland Shell нельзя перезапустить, а перезагрузить JS уже загруженного
    # расширения он не умеет: ReloadExtension объявлен, но не реализован.
    echo "Extension files were replaced. GNOME Shell keeps running the previously"
    echo "loaded copy until you log out and log back in."
  fi
else
  echo "Warning: gnome-extensions not found. Enable '$UUID' in Extension Manager." >&2
fi

if [[ "$RUN_SETUP" -eq 1 && -t 0 ]]; then
  "$BINARY" setup
fi

echo
echo "Installed AI Usage."
echo "Configure: $BINARY setup"
echo "Check:     $BINARY doctor"
echo "Remove:    ai-usage-uninstall"

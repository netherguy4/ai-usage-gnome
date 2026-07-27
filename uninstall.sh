#!/usr/bin/env bash
set -euo pipefail

UUID="ai-usage@netherguy4"
KEEP_CONFIG=0
for arg in "$@"; do
  case "$arg" in
    --keep-config) KEEP_CONFIG=1 ;;
    -h|--help)
      echo "Usage: ./uninstall.sh [--keep-config]"
      exit 0
      ;;
    *) echo "Unknown argument: $arg" >&2; exit 2 ;;
  esac
done

BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
BINARY="$BIN_DIR/ai-usage"
EXT_DIR="$DATA_HOME/gnome-shell/extensions/$UUID"
SERVICE="$CONFIG_HOME/systemd/user/ai-usage.service"
CONFIG_DIR="$CONFIG_HOME/ai-usage"
CACHE_DIR="$DATA_HOME/ai-usage"

if [[ -x "$BINARY" ]]; then
  "$BINARY" restore-claude-hooks || echo "Warning: Claude hooks could not be restored." >&2
fi

if command -v gnome-extensions >/dev/null 2>&1; then
  gnome-extensions disable "$UUID" 2>/dev/null || true
fi
if command -v systemctl >/dev/null 2>&1; then
  systemctl --user disable --now ai-usage.service 2>/dev/null || true
fi

rm -rf "$EXT_DIR" "$CACHE_DIR"
rm -f "$SERVICE" "$BINARY" "$BIN_DIR/ai-usage-uninstall"
if [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
  rm -rf "$XDG_RUNTIME_DIR/ai-usage"
fi
if [[ "$KEEP_CONFIG" -eq 0 ]]; then
  rm -rf "$CONFIG_DIR"
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl --user daemon-reload
fi

echo "AI Usage removed."
if [[ "$KEEP_CONFIG" -eq 1 ]]; then
  echo "Configuration preserved in $CONFIG_DIR"
fi
echo "Log out and back in if the panel icon is still visible."

#!/usr/bin/env bash
set -euo pipefail

REPO="netherguy4/ai-usage-gnome"
ARCH_RAW="$(uname -m)"
case "$ARCH_RAW" in
  x86_64|amd64) ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
  *) echo "Unsupported architecture: $ARCH_RAW" >&2; exit 1 ;;
esac

for command in curl tar python3; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "Required command not found: $command" >&2
    exit 1
  }
done

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

API="https://api.github.com/repos/$REPO/releases/latest"
JSON="$TMP/release.json"
curl -fsSL -H 'Accept: application/vnd.github+json' "$API" -o "$JSON"
URL="$(python3 - "$JSON" "$ARCH" <<'PY'
import json, sys
path, arch = sys.argv[1:]
data = json.load(open(path, encoding='utf-8'))
needle = f'-{arch}.tar.gz'
for asset in data.get('assets', []):
    name = asset.get('name', '')
    if name.endswith(needle):
        print(asset['browser_download_url'])
        break
else:
    raise SystemExit(f'No release asset for {arch}')
PY
)"

ARCHIVE="$TMP/package.tar.gz"
curl -fL "$URL" -o "$ARCHIVE"
tar -xzf "$ARCHIVE" -C "$TMP"
PACKAGE_DIR="$(find "$TMP" -mindepth 1 -maxdepth 1 -type d -name 'ai-usage-gnome-*' | head -n1)"
[[ -n "$PACKAGE_DIR" ]] || { echo "Invalid release archive" >&2; exit 1; }
if [[ -r /dev/tty && -w /dev/tty ]]; then
  exec "$PACKAGE_DIR/install.sh" "$@" </dev/tty
fi
exec "$PACKAGE_DIR/install.sh" "$@"

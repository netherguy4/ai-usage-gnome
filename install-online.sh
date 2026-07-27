#!/usr/bin/env bash
set -euo pipefail

REPO="netherguy4/ai-usage-gnome"
ARCH_RAW="$(uname -m)"
case "$ARCH_RAW" in
  x86_64|amd64) ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
  *) echo "Unsupported architecture: $ARCH_RAW" >&2; exit 1 ;;
esac

for command in curl tar python3 sha256sum; do
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
read -r URL ARCHIVE_NAME SUMS_URL <<EOF
$(python3 - "$JSON" "$ARCH" <<'PY'
import json, sys
path, arch = sys.argv[1:]
data = json.load(open(path, encoding='utf-8'))
assets = data.get('assets', [])
needle = f'-{arch}.tar.gz'
archive = next((a for a in assets if a.get('name', '').endswith(needle)), None)
if archive is None:
    raise SystemExit(f'No release asset for {arch}')
sums = next((a for a in assets if a.get('name') == 'SHA256SUMS'), None)
if sums is None:
    raise SystemExit('Release has no SHA256SUMS asset; refusing to install')
print(archive['browser_download_url'], archive['name'], sums['browser_download_url'])
PY
)
EOF

[[ -n "$URL" && -n "$ARCHIVE_NAME" && -n "$SUMS_URL" ]] || {
  echo "Could not resolve release assets" >&2
  exit 1
}

ARCHIVE="$TMP/$ARCHIVE_NAME"
curl -fL "$URL" -o "$ARCHIVE"
curl -fsSL "$SUMS_URL" -o "$TMP/SHA256SUMS"

# Сверяем ровно ту строку, которая относится к скачанному архиву, и требуем,
# чтобы она существовала: пустой grep не должен молча пройти проверку.
EXPECTED="$(awk -v name="$ARCHIVE_NAME" '
  { file = $2; sub(/^\.\//, "", file); sub(/^\*/, "", file) }
  file == name { print $1; found = 1 }
  END { if (!found) exit 1 }
' "$TMP/SHA256SUMS")" || {
  echo "SHA256SUMS has no entry for $ARCHIVE_NAME" >&2
  exit 1
}

ACTUAL="$(sha256sum "$ARCHIVE" | awk '{print $1}')"
if [[ "$EXPECTED" != "$ACTUAL" ]]; then
  echo "Checksum mismatch for $ARCHIVE_NAME" >&2
  echo "  expected: $EXPECTED" >&2
  echo "  actual:   $ACTUAL" >&2
  exit 1
fi
echo "Checksum verified: $ARCHIVE_NAME"

tar -xzf "$ARCHIVE" -C "$TMP"
PACKAGE_DIR="$(find "$TMP" -mindepth 1 -maxdepth 1 -type d -name 'ai-usage-gnome-*' | head -n1)"
[[ -n "$PACKAGE_DIR" ]] || { echo "Invalid release archive" >&2; exit 1; }
# Скрипт обычно запускают как `curl ... | bash`, поэтому stdin занят пайпом и
# интерактивный setup читать неоткуда. Подставляем /dev/tty, но только если он
# действительно открывается: файл может существовать, а управляющего терминала
# у процесса не быть, и тогда перенаправление свалит запуск целиком.
if (exec 3</dev/tty) 2>/dev/null; then
  exec "$PACKAGE_DIR/install.sh" "$@" </dev/tty
fi
exec "$PACKAGE_DIR/install.sh" "$@"

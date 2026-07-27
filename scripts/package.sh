#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "Usage: scripts/package.sh <binary> <version> <arch>" >&2
  exit 2
fi

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="$(realpath "$1")"
VERSION="$2"
ARCH="$3"
NAME="ai-usage-gnome-${VERSION}-${ARCH}"
STAGE="$ROOT/dist/$NAME"

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/extension" "$STAGE/systemd" "$ROOT/dist"
install -m 0755 "$BINARY" "$STAGE/bin/ai-usage"
cp -a "$ROOT/extension/ai-usage@netherguy4" "$STAGE/extension/"
cp "$ROOT/systemd/ai-usage.service.in" "$STAGE/systemd/"
cp "$ROOT/install.sh" "$ROOT/uninstall.sh" "$ROOT/install-online.sh" "$ROOT/README.md" "$ROOT/LICENSE" "$STAGE/"
cp "$ROOT/HANDOFF.md" "$STAGE/"
mkdir -p "$STAGE/docs"
cp -a "$ROOT/docs/handoff" "$STAGE/docs/"

(
  cd "$STAGE/extension/ai-usage@netherguy4"
  zip -q -r "$STAGE/ai-usage@netherguy4.zip" .
)

tar -C "$ROOT/dist" -czf "$ROOT/dist/$NAME.tar.gz" "$NAME"
rm -rf "$STAGE"
echo "$ROOT/dist/$NAME.tar.gz"

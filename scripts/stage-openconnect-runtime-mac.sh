#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -lt 2 ]; then
  echo "usage: $0 <source-dir> <arch>" >&2
  exit 1
fi

SOURCE_DIR="$1"
ARCH="$2"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RUNTIME_DIR="$REPO_ROOT/runtime/darwin-$ARCH"

if [ ! -d "$SOURCE_DIR" ]; then
  echo "source directory does not exist: $SOURCE_DIR" >&2
  exit 1
fi

mkdir -p "$RUNTIME_DIR"

for name in openconnect; do
  if [ ! -f "$SOURCE_DIR/$name" ]; then
    echo "missing required runtime file: $SOURCE_DIR/$name" >&2
    exit 1
  fi
  cp "$SOURCE_DIR/$name" "$RUNTIME_DIR/$name"
done

find "$SOURCE_DIR" -maxdepth 1 \( -name '*.dylib' -o -name 'LICENSE*' -o -name 'COPYING*' \) -type f -exec cp {} "$RUNTIME_DIR" \;
chmod 755 "$RUNTIME_DIR/openconnect"

echo "Staged OpenConnect runtime to $RUNTIME_DIR"

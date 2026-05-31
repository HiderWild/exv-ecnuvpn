#!/usr/bin/env bash
# stage-openconnect-runtime-mac.sh
# Collect openconnect and all its dynamic library dependencies into
# runtime/legacy-openconnect/darwin-$ARCH/ for explicit legacy diagnostic runs.
# This script is not part of production packaging.
#
# Usage:
#   ./scripts/stage-openconnect-runtime-mac.sh [openconnect-path] [arch]
#
#   openconnect-path  Path to the openconnect binary (default: auto-detect via `which`)
#   arch              x64 or arm64 (default: auto-detect from running system)
#
# The legacy diagnostic layout is flat:
# runtime/legacy-openconnect/darwin-$ARCH/openconnect + *.dylib.
set -euo pipefail

if [[ "${ECNUVPN_LEGACY_OPENCONNECT_RUNTIME:-}" != "1" ]]; then
  echo "Error: OpenConnect runtime staging is legacy diagnostic-only. Set ECNUVPN_LEGACY_OPENCONNECT_RUNTIME=1 to run this script." >&2
  exit 1
fi

OPENCONNECT="${1:-$(which openconnect 2>/dev/null || true)}"
ARCH="${2:-}"

if [[ -z "$OPENCONNECT" ]]; then
  echo "Error: openconnect not found in PATH. Install via: brew install openconnect" >&2
  exit 1
fi

if [[ ! -x "$OPENCONNECT" ]]; then
  echo "Error: $OPENCONNECT is not executable" >&2
  exit 1
fi

# Auto-detect arch from binary
if [[ -z "$ARCH" ]]; then
  FILE_ARCH=$(file "$OPENCONNECT" | grep -o 'arm64\|x86_64' | head -1)
  case "$FILE_ARCH" in
    arm64)  ARCH="arm64" ;;
    x86_64) ARCH="x64" ;;
    *)      ARCH="arm64" ;;
  esac
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="$PROJECT_ROOT/runtime/legacy-openconnect/darwin-$ARCH"
COPIED_LIST=$(mktemp)

trap 'rm -f "$COPIED_LIST"' EXIT

echo "Staging legacy diagnostic openconnect runtime for darwin-$ARCH..."
echo "  Source: $OPENCONNECT"
echo "  Output: $OUT_DIR"

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

# Copy the main binary
cp "$OPENCONNECT" "$OUT_DIR/openconnect"
chmod +x "$OUT_DIR/openconnect"

# Collect dylib dependencies recursively using otool -L
# We look for Homebrew paths (/opt/homebrew, /usr/local) and copy them
# into the same flat directory, rewriting install names to @executable_path/

collect_deps() {
  local binary="$1"
  otool -L "$binary" 2>/dev/null | while read -r line; do
    dep=$(echo "$line" | sed -n 's/^[[:space:]]*\([^ ]*\).*/\1/p')
    [[ -z "$dep" ]] && continue

    case "$dep" in
      /opt/homebrew/*|/usr/local/*)
        bname=$(basename "$dep")
        grep -qxF "$bname" "$COPIED_LIST" 2>/dev/null && continue
        echo "$bname" >> "$COPIED_LIST"

        if [[ -f "$dep" ]]; then
          echo "  Copying: $dep"
          cp "$dep" "$OUT_DIR/"
          chmod +w "$OUT_DIR/$bname"
          collect_deps "$dep"
        fi
        ;;
    esac
  done
}

collect_deps "$OPENCONNECT"

# Rewrite install names in all binaries and dylibs
echo "Rewriting install names..."

install_name_tool -id @executable_path/openconnect "$OUT_DIR/openconnect" 2>/dev/null || true

# Rewrite Homebrew dylib refs in the main binary
otool -L "$OPENCONNECT" 2>/dev/null | while read -r line; do
  dep=$(echo "$line" | sed -n 's/^[[:space:]]*\([^ ]*\).*/\1/p')
  [[ -z "$dep" ]] && continue
  case "$dep" in
    /opt/homebrew/*|/usr/local/*)
      bname=$(basename "$dep")
      if [[ -f "$OUT_DIR/$bname" ]]; then
        install_name_tool -change "$dep" "@executable_path/$bname" "$OUT_DIR/openconnect" 2>/dev/null || true
      fi
      ;;
  esac
done

# Rewrite dylib refs in each bundled dylib
for dylib in "$OUT_DIR/"*.dylib; do
  [[ -f "$dylib" ]] || continue
  bname=$(basename "$dylib")
  install_name_tool -id "@executable_path/$bname" "$dylib" 2>/dev/null || true
  otool -L "$dylib" 2>/dev/null | while read -r line; do
    dep=$(echo "$line" | sed -n 's/^[[:space:]]*\([^ ]*\).*/\1/p')
    [[ -z "$dep" ]] && continue
    case "$dep" in
      /opt/homebrew/*|/usr/local/*)
        dep_bname=$(basename "$dep")
        if [[ -f "$OUT_DIR/$dep_bname" ]]; then
          install_name_tool -change "$dep" "@executable_path/$dep_bname" "$dylib" 2>/dev/null || true
        fi
        ;;
    esac
  done
done

# install_name_tool mutates Mach-O load commands, which invalidates the source
# code signature. On Apple Silicon and recent macOS releases an invalid Mach-O
# signature can cause the kernel to SIGKILL the binary at exec time. Re-sign the
# staged runtime ad-hoc so the bundled openconnect can run from Electron.
echo "Ad-hoc signing staged Mach-O files..."
for binary in "$OUT_DIR/"*.dylib "$OUT_DIR/openconnect"; do
  [[ -f "$binary" ]] || continue
  chmod +w "$binary"
  codesign --force --sign - "$binary" >/dev/null
  codesign --verify --strict "$binary" >/dev/null
done

echo ""
echo "Staged contents:"
find "$OUT_DIR" -type f | sort | while read -r f; do
  size=$(stat -f%z "$f" 2>/dev/null || stat -c%s "$f" 2>/dev/null)
  echo "  $(echo "$f" | sed "s|$OUT_DIR/||") ($(( size / 1024 ))KB)"
done

echo ""
echo "Done. Legacy diagnostic runtime staged to $OUT_DIR"
echo "Production packaging denies these legacy payloads from extraResources/bin."

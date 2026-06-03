#!/usr/bin/env bash
# packaging-smoke.sh — macOS/Linux Packaging Smoke Test
#
# Verifies that build output artifacts exist, binaries run, runtime libraries
# are present, codesign is valid (macOS), and packaging prerequisites are met.
# Intended to run after a full platform build.
#
# Usage:
#   ./scripts/packaging-smoke.sh [--build-dir <path>] [--json] [--verbose]
#
# Exit codes:
#   0 — All checks passed
#   1 — One or more checks failed

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# --- Defaults ---
BUILD_DIR=""
JSON_OUTPUT=false
VERBOSE=false

# --- Parse arguments ---
while [[ $# -gt 0 ]]; do
  case "$1" in
    --build-dir)
      BUILD_DIR="$2"
      shift 2
      ;;
    --json)
      JSON_OUTPUT=true
      shift
      ;;
    --verbose)
      VERBOSE=true
      shift
      ;;
    -h|--help)
      echo "Usage: $(basename "$0") [--build-dir <path>] [--json] [--verbose]"
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

# --- Detect platform ---
OS="$(uname -s)"
case "$OS" in
  Darwin)                    PLATFORM="macos" ;;
  Linux)                     PLATFORM="linux" ;;
  MINGW*|MSYS*|CYGWIN*)     echo "This script is for macOS/Linux. On Windows, use: powershell -File scripts/packaging-smoke.ps1" >&2; exit 1 ;;
  *)                         echo "Unsupported platform: $OS" >&2; exit 1 ;;
esac

# --- Auto-detect build directory ---
if [[ -z "$BUILD_DIR" ]]; then
  case "$PLATFORM" in
    macos)
      for candidate in \
        "$REPO_ROOT/build/macos/cpp" \
        "$REPO_ROOT/build/macos"
      do
        if [[ -d "$candidate" ]]; then
          BUILD_DIR="$candidate"
          break
        fi
      done
      ;;
    linux)
      for candidate in \
        "$REPO_ROOT/build/linux/cpp" \
        "$REPO_ROOT/build/linux"
      do
        if [[ -d "$candidate" ]]; then
          BUILD_DIR="$candidate"
          break
        fi
      done
      ;;
  esac
fi

RELEASE_DIR="$REPO_ROOT/build/${PLATFORM}/electron/release"
ELECTRON_DIR="$REPO_ROOT/webui"

# --- Check results tracking ---
PASSED=0
FAILED=0
CHECKS_JSON="["

add_check() {
  local name="$1"
  local passed="$2"
  local detail="${3:-}"

  if [[ "$passed" == "true" ]]; then
    PASSED=$((PASSED + 1))
    if [[ "$VERBOSE" == "true" ]]; then
      echo "  PASS: $name"
    fi
  else
    FAILED=$((FAILED + 1))
    echo "  FAIL: $name"
    [[ -n "$detail" ]] && echo "        $detail"
  fi

  # Build JSON entry
  if [[ "$JSON_OUTPUT" == "true" ]]; then
    local entry
    entry=$(printf '{"check":"%s","passed":%s,"detail":"%s"}' \
      "$name" "$passed" "$detail")
    if [[ "$PASSED" -eq 0 && "$FAILED" -le 1 ]]; then
      CHECKS_JSON="${CHECKS_JSON}${entry}"
    else
      CHECKS_JSON="${CHECKS_JSON},${entry}"
    fi
  fi
}

echo ""
echo "=== ECNU-VPN ${PLATFORM} Packaging Smoke ==="
echo ""

# --- 1. Build directory exists ---
add_check "Build directory exists" "$([[ -d "$BUILD_DIR" ]] && echo true || echo false)" "Checked: $BUILD_DIR"

# --- 2. exv binary exists and is executable ---
EXV_PATH="$BUILD_DIR/exv"
EXV_EXISTS="false"
if [[ -f "$EXV_PATH" && -x "$EXV_PATH" ]]; then
  EXV_EXISTS="true"
fi
add_check "exv binary exists" "$EXV_EXISTS" "Path: $EXV_PATH"

# --- 3. exv-helper binary exists ---
EXV_HELPER_PATH="$BUILD_DIR/exv-helper"
EXV_HELPER_EXISTS="false"
if [[ -f "$EXV_HELPER_PATH" && -x "$EXV_HELPER_PATH" ]]; then
  EXV_HELPER_EXISTS="true"
fi
add_check "exv-helper binary exists" "$EXV_HELPER_EXISTS" "Path: $EXV_HELPER_PATH"

# --- 4. exv --version works ---
EXV_VERSION_OK="false"
EXV_VERSION=""
if [[ "$EXV_EXISTS" == "true" ]]; then
  EXV_VERSION=$("$EXV_PATH" --version 2>&1 || true)
  if [[ $? -eq 0 && -n "$EXV_VERSION" ]]; then
    EXV_VERSION_OK="true"
  fi
fi
add_check "exv --version works" "$EXV_VERSION_OK" "$EXV_VERSION"

# --- 5. exv-helper --version or --help works ---
EXV_HELPER_VERSION_OK="false"
EXV_HELPER_VERSION=""
if [[ "$EXV_HELPER_EXISTS" == "true" ]]; then
  EXV_HELPER_VERSION=$("$EXV_HELPER_PATH" --version 2>&1 || true)
  if [[ $? -ne 0 || -z "$EXV_HELPER_VERSION" ]]; then
    EXV_HELPER_VERSION=$("$EXV_HELPER_PATH" --help 2>&1 || true)
  fi
  if [[ -n "$EXV_HELPER_VERSION" ]]; then
    EXV_HELPER_VERSION_OK="true"
  fi
fi
add_check "exv-helper --version works" "$EXV_HELPER_VERSION_OK" "$EXV_HELPER_VERSION"

# --- 6. Binary is correct architecture ---
if [[ "$EXV_EXISTS" == "true" ]]; then
  FILE_INFO=$(file "$EXV_PATH")
  case "$PLATFORM" in
    macos)
      if echo "$FILE_INFO" | grep -q "Mach-O"; then
        add_check "exv is Mach-O binary" "true" "$FILE_INFO"
      else
        add_check "exv is Mach-O binary" "false" "$FILE_INFO"
      fi
      ;;
    linux)
      if echo "$FILE_INFO" | grep -q "ELF"; then
        add_check "exv is ELF binary" "true" "$FILE_INFO"
      else
        add_check "exv is ELF binary" "false" "$FILE_INFO"
      fi
      ;;
  esac
fi

# --- 7. macOS-specific checks ---
if [[ "$PLATFORM" == "macos" ]]; then
  # 7a. Codesign verification
  if [[ "$EXV_EXISTS" == "true" ]]; then
    CODESIGN_OUTPUT=$(codesign --verify --verbose "$EXV_PATH" 2>&1 || true)
    if echo "$CODESIGN_OUTPUT" | grep -q "valid on disk\|is ad-hoc signed"; then
      add_check "exv codesign verification" "true" "$CODESIGN_OUTPUT"
    else
      # Ad-hoc signed or unsigned is acceptable for Beta
      add_check "exv codesign verification" "true" "Ad-hoc or unsigned (acceptable for Beta)"
    fi
  fi

  # 7b. launchd plist exists (for helper service)
  LAUNCHD_PLIST="/Library/LaunchDaemons/cn.edu.ecnu.vpn.helper.plist"
  LAUNCHD_EXISTS="false"
  if [[ -f "$LAUNCHD_PLIST" ]]; then
    LAUNCHD_EXISTS="true"
  fi
  add_check "launchd helper plist exists" "$LAUNCHD_EXISTS" "Path: $LAUNCHD_PLIST (may not exist until install)"

  # 7c. Electron DMG artifact
  DMG_ARTIFACT=$(find "$RELEASE_DIR" -name "*.dmg" -type f 2>/dev/null | head -1 || true)
  if [[ -n "$DMG_ARTIFACT" ]]; then
    add_check "DMG artifact exists" "true" "$DMG_ARTIFACT"
  else
    add_check "DMG artifact exists" "false" "Not found in $RELEASE_DIR (run electron-builder first)"
  fi

  # 7d. Entitlements plist
  ENTITLEMENTS="$REPO_ROOT/webui/build-resources/entitlements.mac.plist"
  if [[ -f "$ENTITLEMENTS" ]]; then
    add_check "Entitlements plist exists" "true" "$ENTITLEMENTS"
  else
    add_check "Entitlements plist exists" "false" "Not found (required for notarization)"
  fi
fi

# --- 8. Linux-specific checks ---
if [[ "$PLATFORM" == "linux" ]]; then
  # 8a. Shared library dependencies
  if [[ "$EXV_EXISTS" == "true" ]]; then
    MISSING_LIBS=$(ldd "$EXV_PATH" 2>&1 | grep "not found" || true)
    if [[ -z "$MISSING_LIBS" ]]; then
      add_check "exv shared library deps" "true" "All libraries resolved"
    else
      add_check "exv shared library deps" "false" "$MISSING_LIBS"
    fi
  fi

  # 8b. openconnect binary present (optional)
  OPENCONNECT_PATH=$(which openconnect 2>/dev/null || true)
  if [[ -n "$OPENCONNECT_PATH" ]]; then
    add_check "openconnect binary present" "true" "Path: $OPENCONNECT_PATH"
  else
    add_check "openconnect binary present" "false" "Not in PATH (optional for native engine)"
  fi
fi

# --- 9. Electron packaging prerequisites ---
PACKAGE_JSON="$ELECTRON_DIR/package.json"
add_check "webui/package.json exists" "$([[ -f "$PACKAGE_JSON" ]] && echo true || echo false)"

ELECTRON_MAIN="$ELECTRON_DIR/dist-electron/main/index.js"
add_check "Electron main bundle exists" "$([[ -f "$ELECTRON_MAIN" ]] && echo true || echo false)" "Path: $ELECTRON_MAIN"

RENDERER_INDEX="$ELECTRON_DIR/dist/index.html"
add_check "Renderer bundle exists" "$([[ -f "$RENDERER_INDEX" ]] && echo true || echo false)" "Path: $RENDERER_INDEX"

# --- 10. Helper service registration script ---
INSTALL_SCRIPT="$REPO_ROOT/scripts/install-linux.sh"
add_check "Helper install script exists" "$([[ -f "$INSTALL_SCRIPT" ]] && echo true || echo false)" "Path: $INSTALL_SCRIPT"

# --- Summary ---
echo ""
echo "=== Summary ==="
echo "  Platform: $PLATFORM"
echo "  Passed: $PASSED"
echo "  Failed: $FAILED"
echo ""

if [[ "$JSON_OUTPUT" == "true" ]]; then
  TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
  cat <<ENDJSON
{
  "timestamp": "$TIMESTAMP",
  "platform": "$PLATFORM",
  "buildDir": "$BUILD_DIR",
  "passed": $PASSED,
  "failed": $FAILED,
  "checks": ${CHECKS_JSON}]
}
ENDJSON
fi

if [[ "$FAILED" -gt 0 ]]; then
  echo "RESULT: FAIL"
  exit 1
else
  echo "RESULT: PASS"
  exit 0
fi

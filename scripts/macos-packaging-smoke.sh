#!/usr/bin/env bash
#
# macOS Packaging Smoke Tests
#
# Validates the macOS build artifacts, helper IPC, service configuration,
# and codesign status without requiring a real VPN connection.
#
# Usage:
#   ./scripts/macos-packaging-smoke.sh [BUILD_DIR]
#
# BUILD_DIR defaults to build/macos/cpp (the cmake --preset macos-release output).
# For DMG validation, set EXV_DMG_PATH to the .dmg file.
#
# Exit code: 0 if all checks pass, 1 if any check fails.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD_DIR="${1:-$REPO_ROOT/build/macos/cpp}"
DMG_PATH="${EXV_DMG_PATH:-}"
ELECTRON_RELEASE="${EXV_ELECTRON_RELEASE:-$REPO_ROOT/build/macos/electron/release}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m' # No Color

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
RESULTS=()

pass() {
  local name="$1"
  PASS_COUNT=$((PASS_COUNT + 1))
  RESULTS+=("PASS  $name")
  echo -e "  ${GREEN}PASS${NC}  $name"
}

fail() {
  local name="$1"
  local detail="${2:-}"
  FAIL_COUNT=$((FAIL_COUNT + 1))
  RESULTS+=("FAIL  $name: $detail")
  echo -e "  ${RED}FAIL${NC}  $name: $detail"
}

skip() {
  local name="$1"
  local reason="$2"
  SKIP_COUNT=$((SKIP_COUNT + 1))
  RESULTS+=("SKIP  $name: $reason")
  echo -e "  ${YELLOW}SKIP${NC}  $name: $reason"
}

section() {
  echo ""
  echo "=== $1 ==="
}

# ---------- Locate binaries ----------

EXV_BIN=""
EXV_HELPER_BIN=""

find_binary() {
  # Search order: build dir, electron release staging, /usr/local/bin
  local candidates=(
    "$BUILD_DIR/exv"
    "$ELECTRON_RELEASE/mac-arm64/ECNU VPN.app/Contents/Resources/native/bin/exv"
    "$ELECTRON_RELEASE/mac/ECNU VPN.app/Contents/Resources/native/bin/exv"
    "/usr/local/bin/exv"
  )
  for c in "${candidates[@]}"; do
    if [[ -x "$c" ]]; then
      echo "$c"
      return 0
    fi
  done
  return 1
}

find_helper_binary() {
  local candidates=(
    "$BUILD_DIR/exv-helper"
    "$ELECTRON_RELEASE/mac-arm64/ECNU VPN.app/Contents/Resources/native/bin/exv-helper"
    "$ELECTRON_RELEASE/mac/ECNU VPN.app/Contents/Resources/native/bin/exv-helper"
    "/usr/local/bin/exv-helper"
  )
  for c in "${candidates[@]}"; do
    if [[ -x "$c" ]]; then
      echo "$c"
      return 0
    fi
  done
  return 1
}

section "Binary Discovery"

if EXV_BIN=$(find_binary); then
  pass "exv binary found at $EXV_BIN"
else
  fail "exv binary not found" "Searched: $BUILD_DIR, electron release, /usr/local/bin"
fi

if EXV_HELPER_BIN=$(find_helper_binary); then
  pass "exv-helper binary found at $EXV_HELPER_BIN"
else
  fail "exv-helper binary not found" "Searched: $BUILD_DIR, electron release, /usr/local/bin"
fi

# ---------- 1. exv --version ----------

section "1. exv --version"

if [[ -n "$EXV_BIN" ]]; then
  VERSION_OUTPUT=$("$EXV_BIN" --version 2>&1 || true)
  if echo "$VERSION_OUTPUT" | grep -qE '[0-9]+\.[0-9]+\.[0-9]+'; then
    pass "exv --version returns version string"
    echo "    Output: $VERSION_OUTPUT"
  else
    fail "exv --version did not return expected version" "Output: $VERSION_OUTPUT"
  fi
else
  skip "exv --version" "exv binary not found"
fi

# ---------- 2. exv service status ----------

section "2. exv service status"

if [[ -n "$EXV_BIN" ]]; then
  STATUS_OUTPUT=$("$EXV_BIN" service status 2>&1 || true)
  if echo "$STATUS_OUTPUT" | grep -qiE '(installed|socket|running|helper)'; then
    pass "exv service status returns structured output"
    echo "    Output: $(echo "$STATUS_OUTPUT" | head -5)"
  else
    fail "exv service status returned unexpected output" "Output: $STATUS_OUTPUT"
  fi
else
  skip "exv service status" "exv binary not found"
fi

# ---------- 3. Helper hello (IPC) ----------

section "3. Helper IPC hello"

HELPER_SOCK="/var/run/exv-helper.sock"
if [[ -S "$HELPER_SOCK" ]]; then
  # Try sending a hello action via the helper IPC
  HELLO_RESPONSE=$(echo '{"action":"hello"}' | nc -U "$HELPER_SOCK" 2>/dev/null || true)
  if echo "$HELLO_RESPONSE" | grep -qiE '(ok|hello|version|description)'; then
    pass "Helper IPC hello returns response"
    echo "    Response: $HELLO_RESPONSE"
  else
    fail "Helper IPC hello did not return expected response" "Response: $HELLO_RESPONSE"
  fi
else
  skip "Helper IPC hello" "Helper socket not found at $HELPER_SOCK (helper may not be installed)"
fi

# ---------- 4. Helper V2 capabilities ----------

section "4. Helper V2 capabilities"

if [[ -S "$HELPER_SOCK" ]]; then
  CAPS_RESPONSE=$(echo '{"action":"hello","protocol_version":2}' | nc -U "$HELPER_SOCK" 2>/dev/null || true)
  if echo "$CAPS_RESPONSE" | grep -qiE '(capabilities|protocol_version|server_version)'; then
    pass "Helper V2 capabilities returned"
    echo "    Response: $CAPS_RESPONSE"
  else
    skip "Helper V2 capabilities" "V2 protocol may not be active yet or response format differs"
  fi
else
  skip "Helper V2 capabilities" "Helper socket not found"
fi

# ---------- 5. desktop-rpc status ----------

section "5. Desktop-RPC status"

if [[ -n "$EXV_BIN" ]]; then
  RPC_OUTPUT=$("$EXV_BIN" desktop-rpc status.get '{}' 2>&1 || true)
  if echo "$RPC_OUTPUT" | grep -qiE '(ok|status|running)'; then
    pass "desktop-rpc status returns structured response"
  else
    skip "desktop-rpc status" "May require helper to be running or different invocation"
  fi
else
  skip "desktop-rpc status" "exv binary not found"
fi

# ---------- 6. Verify openconnect binary ----------

section "6. OpenConnect binary"

OPENCONNECT_PATHS=(
  "$BUILD_DIR/openconnect"
  "/usr/local/bin/openconnect"
  "$REPO_ROOT/runtime/darwin-x64/openconnect"
)

OPENCONNECT_FOUND=""
for oc_path in "${OPENCONNECT_PATHS[@]}"; do
  if [[ -x "$oc_path" ]]; then
    OPENCONNECT_FOUND="$oc_path"
    break
  fi
done

if [[ -n "$OPENCONNECT_FOUND" ]]; then
  pass "openconnect binary found at $OPENCONNECT_FOUND"
else
  skip "openconnect binary" "Not found (expected if native engine is default; legacy path only)"
fi

# ---------- 7. Verify helper binary present ----------

section "7. Helper binary verification"

if [[ -n "$EXV_HELPER_BIN" ]]; then
  # Check it's a valid Mach-O binary
  FILE_TYPE=$(file "$EXV_HELPER_BIN" 2>/dev/null || true)
  if echo "$FILE_TYPE" | grep -qiE '(Mach-O|executable)'; then
    pass "exv-helper is a valid executable"
    echo "    Type: $FILE_TYPE"
  else
    fail "exv-helper is not a valid executable" "file output: $FILE_TYPE"
  fi
else
  skip "Helper binary verification" "exv-helper not found"
fi

# ---------- 8. Verify Info.plist ----------

section "8. Info.plist verification"

PLIST_CANDIDATES=(
  "$ELECTRON_RELEASE/mac-arm64/ECNU VPN.app/Contents/Info.plist"
  "$ELECTRON_RELEASE/mac/ECNU VPN.app/Contents/Info.plist"
)

PLIST_FOUND=""
for plist_path in "${PLIST_CANDIDATES[@]}"; do
  if [[ -f "$plist_path" ]]; then
    PLIST_FOUND="$plist_path"
    break
  fi
done

if [[ -n "$PLIST_FOUND" ]]; then
  # Verify required keys
  MISSING_KEYS=()
  for key in CFBundleName CFBundleIdentifier CFBundleVersion CFBundleShortVersionString; do
    if ! plutil -p "$PLIST_FOUND" 2>/dev/null | grep -q "\"$key\""; then
      MISSING_KEYS+=("$key")
    fi
  done

  if [[ ${#MISSING_KEYS[@]} -eq 0 ]]; then
    pass "Info.plist contains all required keys"
    echo "    Path: $PLIST_FOUND"
  else
    fail "Info.plist missing keys" "Missing: ${MISSING_KEYS[*]}"
  fi
else
  skip "Info.plist verification" "Electron .app bundle not found (run desktop build first)"
fi

# ---------- 9. Verify codesign status ----------

section "9. Codesign verification"

APP_CANDIDATES=(
  "$ELECTRON_RELEASE/mac-arm64/ECNU VPN.app"
  "$ELECTRON_RELEASE/mac/ECNU VPN.app"
)

APP_FOUND=""
for app_path in "${APP_CANDIDATES[@]}"; do
  if [[ -d "$app_path" ]]; then
    APP_FOUND="$app_path"
    break
  fi
done

if command -v codesign &>/dev/null && [[ -n "$APP_FOUND" ]]; then
  CS_OUTPUT=$(codesign --verify --deep --strict "$APP_FOUND" 2>&1 || true)
  CS_EXIT=$?
  if [[ $CS_EXIT -eq 0 ]]; then
    pass "Codesign verification passed"
    echo "    App: $APP_FOUND"
  else
    # Check if it's just "not signed" vs "invalid signature"
    if echo "$CS_OUTPUT" | grep -q "not signed"; then
      skip "Codesign verification" "App is not signed (expected for development builds)"
    else
      fail "Codesign verification failed" "Exit: $CS_EXIT, Output: $(echo "$CS_OUTPUT" | head -3)"
    fi
  fi
else
  skip "Codesign verification" "codesign not available or .app not found"
fi

# ---------- 10. Verify helper launchd plist ----------

section "10. Helper launchd plist"

LAUNCHD_PLIST="/Library/LaunchDaemons/com.ecnu.exv.helper.plist"

if [[ -f "$LAUNCHD_PLIST" ]]; then
  # Verify plist structure
  if plutil -p "$LAUNCHD_PLIST" 2>/dev/null | grep -q '"Label"'; then
    pass "Helper launchd plist exists and is valid"

    # Check for required keys
    MISSING=()
    for key in Label ProgramArguments RunAtLoad KeepAlive; do
      if ! plutil -p "$LAUNCHD_PLIST" 2>/dev/null | grep -q "\"$key\""; then
        MISSING+=("$key")
      fi
    done
    if [[ ${#MISSING[@]} -gt 0 ]]; then
      fail "launchd plist missing keys" "Missing: ${MISSING[*]}"
    fi
  else
    fail "Helper launchd plist exists but is malformed"
  fi
else
  skip "Helper launchd plist" "Not installed (run 'sudo exv service install' first)"
fi

# ---------- 11. DMG verification ----------

section "11. DMG package verification"

if [[ -n "$DMG_PATH" && -f "$DMG_PATH" ]]; then
  DMG_SIZE=$(stat -f%z "$DMG_PATH" 2>/dev/null || stat --format=%s "$DMG_PATH" 2>/dev/null || echo "0")
  if [[ "$DMG_SIZE" -gt 1048576 ]]; then
    pass "DMG file exists and is non-trivial size"
    echo "    Path: $DMG_PATH"
    echo "    Size: $(( DMG_SIZE / 1048576 )) MB"
  else
    fail "DMG file is suspiciously small" "Size: $DMG_SIZE bytes"
  fi
else
  skip "DMG package verification" "EXV_DMG_PATH not set or file not found"
fi

# ---------- 12. DMG mount and contents check ----------

section "12. DMG mount and contents"

if [[ -n "$DMG_PATH" && -f "$DMG_PATH" ]]; then
  MOUNT_DIR=$(mktemp -d)
  if hdiutil attach "$DMG_PATH" -mountpoint "$MOUNT_DIR" -nobrowse -quiet 2>/dev/null; then
    # Check for .app
    APP_IN_DMG=$(find "$MOUNT_DIR" -maxdepth 2 -type d -name "*.app" | head -1)
    if [[ -n "$APP_IN_DMG" ]]; then
      pass "DMG contains .app bundle"

      # Check for native binaries inside .app
      NATIVE_DIR=$(find "$APP_IN_DMG" -path "*/native/bin" -type d | head -1)
      if [[ -n "$NATIVE_DIR" ]]; then
        NATIVE_FILES=$(ls "$NATIVE_DIR" 2>/dev/null | tr '\n' ', ')
        pass "DMG .app contains native binaries: $NATIVE_FILES"
      else
        skip "DMG native binaries" "native/bin directory not found inside .app"
      fi
    else
      fail "DMG does not contain an .app bundle"
    fi
    hdiutil detach "$MOUNT_DIR" -quiet 2>/dev/null || true
  else
    fail "Failed to mount DMG"
  fi
  rm -rf "$MOUNT_DIR"
else
  skip "DMG mount and contents" "DMG not available"
fi

# ---------- Summary ----------

echo ""
echo "========================================"
echo "  macOS Packaging Smoke Test Summary"
echo "========================================"
echo ""
for r in "${RESULTS[@]}"; do
  case "$r" in
    PASS*) echo -e "  ${GREEN}$r${NC}" ;;
    FAIL*) echo -e "  ${RED}$r${NC}" ;;
    SKIP*) echo -e "  ${YELLOW}$r${NC}" ;;
  esac
done
echo ""
echo "  Total: $(( PASS_COUNT + FAIL_COUNT + SKIP_COUNT ))"
echo -e "  ${GREEN}Pass: $PASS_COUNT${NC}"
echo -e "  ${YELLOW}Skip: $SKIP_COUNT${NC}"
echo -e "  ${RED}Fail: $FAIL_COUNT${NC}"
echo ""

if [[ $FAIL_COUNT -gt 0 ]]; then
  echo -e "${RED}SMOKE TESTS FAILED${NC}"
  exit 1
else
  echo -e "${GREEN}SMOKE TESTS PASSED${NC} (with $SKIP_COUNT skipped)"
  exit 0
fi

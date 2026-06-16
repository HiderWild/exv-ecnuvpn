#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LOG_DIR="$REPO_ROOT/build/webview-acceptance/macos"
mkdir -p "$LOG_DIR"

require_tool() {
  local tool="$1"
  local install_hint="$2"
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    echo "$install_hint" >&2
    exit 70
  fi
}

require_tool ninja "Install Ninja before running macOS acceptance, for example: brew install ninja"

if ! command -v clang-scan-deps >/dev/null 2>&1 && ! xcrun --find clang-scan-deps >/dev/null 2>&1; then
  echo "missing required C++20 module scanner: clang-scan-deps" >&2
  echo "Install an LLVM toolchain that provides clang-scan-deps and make CMake use that compiler before running macOS acceptance." >&2
  exit 70
fi

run_logged() {
  local log_name="$1"
  shift
  local log_path="$LOG_DIR/$log_name"
  (
    cd "$REPO_ROOT"
    "$@"
  ) 2>&1 | tee "$log_path"
  local status=${PIPESTATUS[0]}
  if [[ "$status" -ne 0 ]]; then
    echo "Command failed with exit $status. See $log_path" >&2
    exit "$status"
  fi
}

run_logged contracts.log python3 scripts/generate_contracts.py --check
run_logged configure.log cmake --preset macos-release -DEXV_BUILD_UI_SHELL=ON
run_logged build.log cmake --build build/macos/cpp --config Release
run_logged ctest.log ctest --test-dir build/macos/cpp -C Release --output-on-failure
run_logged package.log scripts/build-macos.sh desktop
run_logged smoke.log scripts/macos-packaging-smoke.sh
run_logged diff-check.log git diff --check

#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LOG_DIR="$REPO_ROOT/build/webview-acceptance/linux"
mkdir -p "$LOG_DIR"

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
run_logged configure.log cmake --preset linux-release -DEXV_BUILD_UI_SHELL=ON
run_logged build.log cmake --build build/linux/cpp --config Release
run_logged ctest.log ctest --test-dir build/linux/cpp -C Release --output-on-failure
run_logged package.log scripts/build-linux.sh desktop
run_logged diff-check.log git diff --check

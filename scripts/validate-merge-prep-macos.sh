#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SKIP_DESKTOP=0
DESKTOP_SMOKE=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-desktop)
      SKIP_DESKTOP=1
      ;;
    --desktop-smoke)
      DESKTOP_SMOKE=1
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
  shift
done

cd "$REPO_ROOT"

echo "[merge-prep] Build frontend assets for native embedding..."
(
  cd "$REPO_ROOT/webui"
  npm run build
)

echo "[merge-prep] Configure and build native targets..."
cmake --preset macos-release
cmake --build --preset macos-release --target exv platform_status_models_test vpn_runtime_test tunnel_script_contract_test app_api_runtime_policy_test crypto_roundtrip_test

echo "[merge-prep] Run focused native regression tests..."
ctest --preset macos-release -R 'platform_status_models_test|vpn_runtime_test|tunnel_script_contract_test|app_api_runtime_policy_test|crypto_roundtrip_test' --output-on-failure

if [[ "$SKIP_DESKTOP" -eq 0 ]]; then
  echo "[merge-prep] Compile Electron main/preload and native staging..."
  (
    cd "$REPO_ROOT/webui"
    npm run build:electron
    npm run prepare:native
  )
fi

if [[ "$DESKTOP_SMOKE" -eq 1 ]]; then
  echo "[merge-prep] Run desktop debug smoke build and launch..."
  "$SCRIPT_DIR/build-macos.sh" debug-run
fi

echo "[merge-prep] macOS validation complete."

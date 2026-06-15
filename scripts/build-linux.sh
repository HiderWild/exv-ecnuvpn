#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD_ROOT="$REPO_ROOT/build/linux"
ACTION="${1:-all}"

export ECNUVPN_BUILD_PLATFORM=linux
export ECNUVPN_WEBUI_DIST_DIR="$BUILD_ROOT/webview/dist"

usage() {
  cat <<EOF
Usage: $(basename "$0") [cpp|test|webview|desktop|all|clean]

  cpp      Configure and build the native Linux targets into build/linux/cpp
  test     Run the focused native regression tests from build/linux/cpp
  webview  Build native targets, run tests, then package the native WebView shell
  desktop  Build native targets, run tests, then package the native WebView shell
  all      Build native targets, run tests, then package the native WebView shell
  clean    Remove build/linux
EOF
}

build_cpp() {
  local ui_shell="${1:-off}"
  (
    cd "$REPO_ROOT"
    if [[ "$ui_shell" == "on" ]]; then
      cmake --preset linux-release -DEXV_BUILD_UI_SHELL=ON
    else
      cmake --preset linux-release
    fi
    cmake --build --preset linux-release --target exv exv-helper exv-ui platform_status_models_test backend_resolver_test native_packaging_policy_test ui_shell_contract_test ui_shell_core_rpc_client_test ui_shell_cmake_policy_test linux_webkitgtk_runtime_test
  )
}

run_tests() {
  (
    cd "$REPO_ROOT"
    ctest --preset linux-release -R 'platform_status_models_test|backend_resolver_test|native_packaging_policy_test|ui_shell_contract_test|ui_shell_core_rpc_client_test|ui_shell_cmake_policy_test|linux_webkitgtk_runtime_test'
  )
}

run_webui_renderer() {
  (
    cd "$REPO_ROOT/webui"
    pnpm run webview:compile
  )
}

package_webview() {
  (
    cd "$REPO_ROOT/webui"
    pnpm run webview:package
  )
}

case "$ACTION" in
  cpp)
    build_cpp
    ;;
  test)
    run_tests
    ;;
  webview)
    run_webui_renderer
    build_cpp on
    run_tests
    package_webview
    ;;
  desktop)
    run_webui_renderer
    build_cpp on
    run_tests
    package_webview
    ;;
  all)
    run_webui_renderer
    build_cpp on
    run_tests
    package_webview
    ;;
  clean)
    rm -rf "$BUILD_ROOT"
    ;;
  -h|--help|help)
    usage
    ;;
  *)
    usage
    exit 1
    ;;
esac

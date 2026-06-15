#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD_ROOT="$REPO_ROOT/build/macos"
ACTION="${1:-all}"

export ECNUVPN_BUILD_PLATFORM=macos
export ECNUVPN_WEBUI_DIST_DIR="$BUILD_ROOT/webview/dist"

usage() {
  cat <<EOF
Usage: $(basename "$0") [cpp|test|webview|electron|debug|debug-run|desktop|all|clean]

  cpp      Configure and build the native macOS targets into build/macos/cpp
  test     Run the focused native regression tests from build/macos/cpp
  webview  Build native targets, run tests, then package the native WebView shell
  electron Build the Electron migration adapter outputs into build/macos/electron
  debug    Build native targets, run tests, then create an unpacked Electron debug app
  debug-run Build the unpacked debug app, then launch the Electron UI
  desktop  Build native targets, run tests, then package the native WebView shell
  all      Build native targets, run tests, then package the native WebView shell
  clean    Remove build/macos
EOF
}

build_cpp() {
  (
    cd "$REPO_ROOT"
    cmake --preset macos-release
    cmake --build --preset macos-release --target exv exv-helper exv-ui platform_status_models_test backend_resolver_test vpn_runtime_test native_packaging_policy_test ui_shell_contract_test ui_shell_core_rpc_client_test ui_shell_cmake_policy_test darwin_wkwebview_runtime_test
  )
}

run_tests() {
  (
    cd "$REPO_ROOT"
    ctest --preset macos-release -R 'platform_status_models_test|backend_resolver_test|vpn_runtime_test|native_packaging_policy_test|ui_shell_contract_test|ui_shell_core_rpc_client_test|ui_shell_cmake_policy_test|darwin_wkwebview_runtime_test'
  )
}

run_webui_renderer() {
  (
    cd "$REPO_ROOT/webui"
    pnpm run webview:compile
  )
}

compile_electron() {
  (
    cd "$REPO_ROOT/webui"
    pnpm run desktop:compile
  )
}

package_webview() {
  (
    cd "$REPO_ROOT/webui"
    pnpm run webview:package
  )
}

package_desktop_debug() {
  (
    cd "$REPO_ROOT/webui"
    pnpm run desktop:package:dir
  )
}

clean_desktop_release() {
  rm -rf "$BUILD_ROOT/electron/release"
}

launch_desktop_debug() {
  local release_root="$BUILD_ROOT/electron/release"
  local app_path=""

  for candidate in \
    "$release_root/mac-arm64/ECNU VPN.app" \
    "$release_root/mac/ECNU VPN.app"
  do
    if [[ -d "$candidate" ]]; then
      app_path="$candidate"
      break
    fi
  done

  if [[ -z "$app_path" ]]; then
    app_path=$(find "$release_root" -maxdepth 2 -type d -name 'ECNU VPN.app' | head -n 1)
  fi

  if [[ -z "$app_path" ]]; then
    echo "Error: unpacked debug app not found under $release_root" >&2
    exit 1
  fi

  echo "Launching debug app: $app_path"
  open -n "$app_path"
}

case "$ACTION" in
  cpp)
    build_cpp
    ;;
  test)
    run_tests
    ;;
  electron)
    compile_electron
    ;;
  webview)
    run_webui_renderer
    build_cpp
    run_tests
    package_webview
    ;;
  debug)
    build_cpp
    run_tests
    compile_electron
    clean_desktop_release
    package_desktop_debug
    ;;
  debug-run)
    build_cpp
    run_tests
    compile_electron
    clean_desktop_release
    package_desktop_debug
    launch_desktop_debug
    ;;
  desktop)
    run_webui_renderer
    build_cpp
    run_tests
    package_webview
    ;;
  all)
    run_webui_renderer
    build_cpp
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

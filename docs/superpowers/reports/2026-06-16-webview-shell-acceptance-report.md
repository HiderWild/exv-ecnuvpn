# WebView Shell Acceptance Report

Date: 2026-06-16

## Summary

Windows acceptance was executed on the current Windows workspace. macOS
acceptance was executed on SSH host `macmini` using a clean temporary worktree
at `/tmp/ecnu-vpn-webview-clean-1781576703` with the current branch diff
applied. The synced macOS workspace at
`/Users/tomli/Development/Projects/CPP/ECNU-VPN` was intentionally not cleaned
because it contains unrelated dirty sync state. Linux WebKitGTK host code has
replaced the stub, but Linux acceptance still needs a Linux host with WebKitGTK
development packages. A later focused macOS verification on `macmini` also
covered the POSIX core process transport and `exv-ui` build from a clean
temporary worktree with Homebrew LLVM.

Reusable acceptance scripts now live in the repository:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\accept-webview-shell-windows.ps1
ssh macmini "cd /Users/tomli/Development/Projects/CPP/ECNU-VPN && bash scripts/accept-webview-shell-macos.sh"
bash scripts/accept-webview-shell-linux.sh
```

Each script writes logs under `build/webview-acceptance/<platform>/`.

## Verification Matrix

| Platform | Configure | Build | CTest | Package | Smoke | Result |
| --- | --- | --- | --- | --- | --- | --- |
| Windows | `build/webview-acceptance/windows/configure.log` exit 0 | `build/webview-acceptance/windows/build.log` exit 0 | `build/webview-acceptance/windows/ctest.log` exit 0, 87/87 passed | `build/webview-acceptance/windows/package.log` exit 0 | `build/webview-acceptance/windows/smoke.log` exit 0, 12 pass / 0 fail / 5 skip | PASS_WITH_ENV_SKIPS |
| macOS | `build/webview-acceptance/macos/configure.log` exit 0 on `macmini` with Homebrew LLVM | `build/webview-acceptance/macos/build.log` exit 0 | `build/webview-acceptance/macos/ctest.log` exit 0, 85/85 passed | `build/webview-acceptance/macos/package.log` exit 0 | `build/webview-acceptance/macos/smoke.log` exit 0, 10 pass / 0 fail / 6 skip | PASS_WITH_ENV_SKIPS |
| Linux | Not run on this Windows host | Not run on this Windows host | Not run on this Windows host | Not run on this Windows host | Not run on this Windows host | NOT_RUN |

## Windows Evidence

Commands executed through `scripts\accept-webview-shell-windows.ps1`:

```powershell
python scripts/generate_contracts.py --check
cmake --preset windows-release -DEXV_BUILD_UI_SHELL=ON -DWEBVIEW2_SDK_DIR="<repo>\build\deps\webview2\1.0.4022.49"
cmake --build build-windows\cpp --config Release
ctest --test-dir build-windows\cpp -C Release --output-on-failure
python scripts\package_ui_shell.py
powershell -ExecutionPolicy Bypass -File scripts\windows-packaging-smoke.ps1
git diff --check
```

Additional focused checks executed during Phase 7:

```powershell
cd webui
pnpm run test:host
pnpm exec tsc -p tsconfig.json --noEmit
cd ..
cmake --build build --config Debug --target native_packaging_policy_test
ctest --test-dir build -C Debug -R "native_packaging_policy_test|ui_shell_cmake_policy_test" --output-on-failure
powershell -NoProfile -ExecutionPolicy Bypass -File tests\start_script_contract.ps1
```

## Package Payload Check

- Windows package contains no `electron.exe` or `chromium.pak`.
- Windows package contains `exv-ui.exe`, `WebView2Loader.dll`, `bin/exv.exe`,
  `bin/exv-helper.exe`, `webui/index.html`, and `exv-ui.args`.
- MinGW runtime DLLs are copied to both package root and `bin/`.
- `wintun.dll` was not present in this local workspace and was reported as an
  optional runtime-asset skip by `windows-packaging-smoke.ps1`.

## macOS Evidence

Command executed against the clean macOS validation worktree:

```powershell
ssh macmini "cd /tmp/ecnu-vpn-webview-clean-1781576703 && bash scripts/accept-webview-shell-macos.sh"
```

Observed result:

- Homebrew `llvm` is selected by `scripts/accept-webview-shell-macos.sh`, so
  CMake has both `clang++` and `clang-scan-deps` for C++20 modules.
- The script installs WebUI dependencies with `pnpm --dir webui install
  --frozen-lockfile` before packaging, so it works from a clean worktree.
- CMake configure/build completed successfully.
- Full CTest completed successfully: 85/85 passed.
- `build-macos.sh desktop` produced
  `build/macos/webview/package/ECNU VPN`.
- `macos-packaging-smoke.sh` reported 10 pass / 0 fail / 6 environment skips.
- `git diff --check` completed successfully.

Notable environment skips:

- No `.app` bundle / codesign / DMG verification was available in this package
  layout.
- `openconnect` was absent, which is expected when the native engine is the
  default path and the legacy binary is optional.

## Remaining Platform Gates

The cross-platform migration is not globally accepted until Linux acceptance
passes on a Linux host and the result is recorded in a follow-up report:

```bash
# Linux
bash scripts/accept-webview-shell-linux.sh
```

The scripts prove configure/build/CTest/package-smoke health. macOS now has a
real WKWebView host implementation. Linux WebKitGTK host implementation has
replaced the previous stub, but it still needs real Linux configure/build/CTest
and package-smoke evidence before Linux can be marked accepted.

`src/app/ui_shell/core_process_manager.cpp` now returns a POSIX
`CoreRpcTransport` on macOS/Linux through posix_spawn and stdin/stdout JSON lines.
Focused macOS validation built `exv-ui`, `ui_shell_core_rpc_client_test`,
`ui_shell_cmake_policy_test`, and `darwin_wkwebview_runtime_test` in a clean
temporary worktree on `macmini`.

## Current Acceptance State

- Windows native WebView shell package path is accepted with local environment
  skips noted above.
- Electron production packaging artifacts have been removed from `webui`.
- Active root/build/user docs describe native WebView as the default desktop
  shell.
- macOS native WebView package path is accepted on `macmini` with environment
  skips noted above.
- Linux scripted acceptance remains pending real host execution.
- Linux host parity implementation is present but unaccepted until a Linux host
  runs `scripts/accept-webview-shell-linux.sh`.
- Non-Windows core process transport is implemented; full Linux acceptance
  remains the final platform gate.

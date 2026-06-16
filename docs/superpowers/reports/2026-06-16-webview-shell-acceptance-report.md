# WebView Shell Acceptance Report

Date: 2026-06-16

## Summary

Windows acceptance was executed on the current Windows workspace. macOS
acceptance was executed on SSH host `macmini` using a clean temporary worktree
at `/tmp/ecnu-vpn-webview-clean-1781576703` with the current branch diff
applied. The synced macOS workspace at
`/Users/tomli/Development/Projects/CPP/ECNU-VPN` was intentionally not cleaned
because it contains unrelated dirty sync state. A later focused macOS
verification on `macmini` also covered the POSIX core process transport and
`exv-ui` build from a clean temporary worktree with Homebrew LLVM. Linux
acceptance was executed in WSL `Ubuntu-24.04` with Node 22, CMake 3.28,
Clang 18, and WebKitGTK 4.1 development packages.

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
| Linux | `build/webview-acceptance/linux/configure.log` exit 0 in WSL `Ubuntu-24.04` | `build/webview-acceptance/linux/build.log` exit 0 | `build/webview-acceptance/linux/ctest.log` exit 0, 81/81 passed | `build/webview-acceptance/linux/package.log` exit 0 | `build/webview-acceptance/linux/diff-check.log` exit 0, focused package tests 7/7 passed | PASS |

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

## Linux Evidence

Command executed in WSL `Ubuntu-24.04`:

```powershell
wsl -d Ubuntu-24.04 -u root -- bash -lc "cd /mnt/d/Development/Projects/cpp/ECNU-VPN && bash scripts/accept-webview-shell-linux.sh"
```

Observed result:

- Environment: Node v22.22.3, pnpm 10.33.2, CMake 3.28.3,
  `clang-scan-deps-18`, WebKitGTK 2.52.3.
- Full CMake configure/build completed successfully with `EXV_BUILD_UI_SHELL=ON`.
- Full CTest completed successfully: 81/81 passed.
- `pnpm --dir webui install --frozen-lockfile` completed successfully.
- `scripts/build-linux.sh desktop` produced
  `build/linux/webview/package/ECNU VPN`.
- Focused package verification inside `scripts/build-linux.sh desktop`
  completed successfully: 7/7 passed.
- `git -c core.autocrlf=true diff --check` completed successfully. The
  explicit EOL setting avoids false positives when the Linux script runs from a
  Windows-mounted WSL worktree.

`src/app/ui_shell/core_process_manager.cpp` returns a POSIX `CoreRpcTransport`
on macOS/Linux through posix_spawn and stdin/stdout JSON lines. Focused macOS
validation built `exv-ui`, `ui_shell_core_rpc_client_test`,
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
- Linux native WebView package path is accepted in WSL `Ubuntu-24.04`.
- Non-Windows core process transport is implemented and covered by macOS and
  Linux acceptance.

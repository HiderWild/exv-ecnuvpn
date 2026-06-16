# WebView Shell Acceptance Report

Date: 2026-06-16

## Summary

Windows acceptance was executed on the current Windows workspace. macOS
acceptance was attempted on SSH host `macmini` at
`/Users/tomli/Development/Projects/CPP/ECNU-VPN`. Linux acceptance still needs a
Linux host with WebKitGTK development packages.

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
| macOS | `build/webview-acceptance/macos/configure.log` exit 1 on `macmini` | Not reached | Not reached | Not reached | Not reached | FAIL_TOOLCHAIN |
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

Command executed:

```powershell
ssh macmini "cd /Users/tomli/Development/Projects/CPP/ECNU-VPN && bash scripts/accept-webview-shell-macos.sh"
```

Observed result:

- Initial run failed because the `macos-release` CMake preset requires Ninja and
  `ninja` was not installed.
- `ninja` was installed on `macmini` with Homebrew.
- The next run passed contract generation and compiler detection, then failed
  during CMake generation because AppleClang did not provide C++20 module import
  graph scanning. `clang-scan-deps` was not present on the host.
- The acceptance script now preflights this condition and fails early with an
  actionable `clang-scan-deps` message until the macOS module toolchain is
  repaired.

Relevant log:

```text
CMake Error in CMakeLists.txt:
  The target named "exv-base-types-module" has C++ sources that may use
  modules, but the compiler does not provide a way to discover the import graph
  dependencies.
```

Required repair before macOS WebView acceptance can continue:

- Install or select an LLVM toolchain that provides `clang-scan-deps`.
- Update the macOS preset or environment so CMake uses that compiler/module
  scanner consistently.
- Re-run `scripts/accept-webview-shell-macos.sh` on `macmini`.

## Remaining Platform Gates

The cross-platform migration is not globally accepted until these scripts pass
on the corresponding hosts and results are recorded in a follow-up report:

```bash
# macOS
ssh macmini "cd /Users/tomli/Development/Projects/CPP/ECNU-VPN && bash scripts/accept-webview-shell-macos.sh"
```

```bash
# Linux
bash scripts/accept-webview-shell-linux.sh
```

The scripts prove configure/build/CTest/package-smoke health. They do not by
themselves close platform host parity while
`src/platform/darwin/ui_shell/wk_webview_host_darwin.mm` and
`src/platform/linux/ui_shell/webkitgtk_host_linux.cpp` still return stub exit
code `70` from `run(...)`.

## Current Acceptance State

- Windows native WebView shell package path is accepted with local environment
  skips noted above.
- Electron production packaging artifacts have been removed from `webui`.
- Active root/build/user docs describe native WebView as the default desktop
  shell.
- macOS/Linux scripted acceptance remains pending real host execution.
- macOS/Linux host parity remains incomplete because the platform `run(...)`
  methods still return the migration stub exit code `70`.

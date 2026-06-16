# WebView Shell Acceptance Report

Date: 2026-06-16

## Summary

Windows acceptance was executed on the current Windows workspace. macOS and
Linux acceptance were not executed in this workspace because the current host is
Windows and the CMake presets are host-conditional.

## Verification Matrix

| Platform | Configure | Build | CTest | Package | Smoke | Result |
| --- | --- | --- | --- | --- | --- | --- |
| Windows | `build/webview-acceptance/windows/configure.log` exit 0 | `build/webview-acceptance/windows/build.log` exit 0 | `build/webview-acceptance/windows/ctest.log` exit 0, 87/87 passed | `build/webview-acceptance/windows/package.log` exit 0 | `build/webview-acceptance/windows/smoke.log` exit 0, 12 pass / 0 fail / 5 skip | PASS_WITH_ENV_SKIPS |
| macOS | Not run on this Windows host | Not run on this Windows host | Not run on this Windows host | Not run on this Windows host | Not run on this Windows host | NOT_RUN |
| Linux | Not run on this Windows host | Not run on this Windows host | Not run on this Windows host | Not run on this Windows host | Not run on this Windows host | NOT_RUN |

## Windows Evidence

Commands executed:

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

## Remaining Platform Gates

The cross-platform migration is not globally accepted until these commands are
run on the corresponding hosts and recorded in a follow-up report:

```bash
# macOS
python3 scripts/generate_contracts.py --check
cmake --preset macos-release -DEXV_BUILD_UI_SHELL=ON
cmake --build build/macos/cpp --config Release
ctest --test-dir build/macos/cpp -C Release --output-on-failure
scripts/build-macos.sh desktop
scripts/macos-packaging-smoke.sh
git diff --check
```

```bash
# Linux
python3 scripts/generate_contracts.py --check
cmake --preset linux-release -DEXV_BUILD_UI_SHELL=ON
cmake --build build/linux/cpp --config Release
ctest --test-dir build/linux/cpp -C Release --output-on-failure
scripts/build-linux.sh desktop
git diff --check
```

## Current Acceptance State

- Windows native WebView shell package path is accepted with local environment
  skips noted above.
- Electron production packaging artifacts have been removed from `webui`.
- Active root/build/user docs describe native WebView as the default desktop
  shell.
- macOS/Linux acceptance remains pending real host execution.

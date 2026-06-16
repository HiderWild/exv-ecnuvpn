# Build Guide

This guide describes the active production build path. The desktop UI package is the native WebView shell, not a bundled browser runtime.

## Output Layout

All generated desktop packages use the same layout:

```text
build/<platform>/webview/package/ECNU VPN
```

Platform-specific examples:

- Windows: `build\windows\webview\package\ECNU VPN`
- macOS: `build/macos/webview/package/ECNU VPN`
- Linux: `build/linux/webview/package/ECNU VPN`

The package root contains:

- `exv-ui` or `exv-ui.exe`
- `exv-ui.args`
- `bin/exv`
- `bin/exv-helper`
- `webui/index.html`
- Windows only: `WebView2Loader.dll`

## Windows

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 desktop
```

Focused package smoke:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows-packaging-smoke.ps1
```

Merge-prep validation:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\validate-merge-prep-windows.ps1
```

## macOS

```bash
./scripts/build-macos.sh desktop
```

Focused package smoke:

```bash
./scripts/macos-packaging-smoke.sh
```

Merge-prep validation:

```bash
./scripts/validate-merge-prep-macos.sh
```

## Linux

```bash
./scripts/build-linux.sh desktop
```

Linux requires WebKitGTK development packages when `EXV_BUILD_UI_SHELL=ON`.

## Native-Only Builds

Use CMake presets for core/helper-only work:

```bash
cmake --preset <platform>-release
cmake --build --preset <platform>-release
ctest --preset <platform>-release --output-on-failure
```

Use the platform desktop scripts when the renderer, `exv-ui`, core, helper, and package layout all need to be validated together.

## WebView Runtime Notes

- Windows uses Microsoft Edge WebView2 Evergreen Runtime. The native shell detects a missing runtime and offers the controlled Evergreen bootstrap flow.
- macOS uses WKWebView from the OS.
- Linux uses WebKitGTK from system packages.

The package script also supports verifying an existing package:

```bash
python scripts/package_ui_shell.py --verify-launch-targets-only --package-dir "build/<platform>/webview/package/ECNU VPN"
```

That check validates `exv-ui.args` targets and rejects bundled Electron or Chromium payloads.

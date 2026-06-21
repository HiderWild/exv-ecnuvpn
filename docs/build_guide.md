# Build Guide

This guide describes the active production build path. The desktop UI package is the native WebView shell, not a bundled browser runtime.

## Output Layout

All generated desktop packages use the same layout:

```text
build/<platform>/webview/package/EXV
```

Platform-specific examples:

- Windows: `build\windows\webview\package\EXV`
- macOS: `build/macos/webview/package/EXV`
- Linux: `build/linux/webview/package/EXV`

The package root contains:

- `exv-ui` or `exv-ui.exe`
- `exv-ui.args`
- `bin/exv`
- `bin/exv-helper`
- `webui/index.html`
- Windows only: `WebView2Loader.dll`

## Windows

Build the Windows desktop package:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 desktop
```

Build Windows release artifacts:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\package-windows-release.ps1
```

The product version is read from `project(exv VERSION ...)` in `CMakeLists.txt`.
Use `-BuildLabel` for local, beta, or channel-specific artifact names without
changing the installed-app version:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\package-windows-release.ps1 -BuildLabel local-zh
```

Release output examples:

- `build\windows\release\EXV-3.3.2-windows-x64-portable.zip`
- `build\windows\release\EXV-3.3.2-windows-x64-setup.exe`
- `build\windows\release\EXV-3.3.2-local-zh-windows-x64-setup.exe`

The setup executable is built with NSIS and requires `makensis.exe`. Put `makensis.exe` on `PATH`, set `NSIS_MAKENSIS`, or pass `-NsisPath`:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\package-windows-release.ps1 -NsisPath "C:\Program Files (x86)\NSIS\makensis.exe"
```

The release script verifies the already-built package, creates the portable zip, expands that zip into a temporary directory, and runs `windows-packaging-smoke.ps1` against the extracted `EXV` directory.

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
python scripts/package_ui_shell.py --verify-launch-targets-only --package-dir "build/<platform>/webview/package/EXV"
```

That check validates `exv-ui.args` targets and rejects bundled Electron or Chromium payloads.

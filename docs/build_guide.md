# Build Guide

This repository now routes generated artifacts into platform-specific subtrees
under `build/` so the `windows` and `macos` branches stop fighting over shared
outputs before merge.

## Layout

- `build-windows/cpp` — Windows CMake configure/build/test output
- `build/windows/electron/dist` — Windows renderer bundle
- `build/windows/electron/dist-electron` — Windows Electron main/preload bundle
- `build/windows/electron/native/bin` — staged native binaries and runtime assets
- `build/windows/electron/release` — Windows packaged desktop artifacts
- `build/macos/cpp` — macOS CMake configure/build/test output
- `build/macos/electron/dist` — macOS renderer bundle
- `build/macos/electron/dist-electron` — macOS Electron main/preload bundle
- `build/macos/electron/native/bin` — staged native binaries and runtime assets
- `build/macos/electron/release` — macOS packaged desktop artifacts

## Native C++

### Windows

```powershell
cmake --preset windows-release
cmake --build --preset windows-release --target exv exv-helper platform_status_models_test backend_resolver_test vpn_runtime_test
ctest --preset windows-release -R 'platform_status_models_test|backend_resolver_test|vpn_runtime_test'
```

### macOS

```bash
cmake --preset macos-release
cmake --build --preset macos-release --target exv platform_status_models_test vpn_runtime_test
ctest --preset macos-release -R 'platform_status_models_test|vpn_runtime_test'
```

## Electron

The `webui` build scripts now follow the same platform split automatically.
Set `ECNUVPN_BUILD_PLATFORM` only when you need to override the default host
platform selection.

### Compile renderer + main/preload + staged native payload

```bash
cd webui
pnpm run desktop:compile
```

### Package desktop artifacts

```bash
cd webui
pnpm run desktop:build
```

### Create an unpacked debug desktop build

```bash
cd webui
pnpm run desktop:debug
```

## Platform Wrapper Scripts

### Windows

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Action all
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Action debug
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Action debug-run
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Action desktop
```

Actions:

- `cpp` — configure/build native Windows targets into `build-windows/cpp`
- `test` — run focused native tests from `build-windows/cpp`
- `electron` — build renderer/main and stage native assets into `build/windows/electron`
- `debug` — native build + tests + unpacked Electron debug app under `build/windows/electron/release/win-unpacked`
- `debug-run` — same as `debug`, then launch the unpacked Windows Electron UI
- `desktop` — native build + tests + packaged Electron output
- `all` — native build + tests + Electron compile/stage
- `clean` — remove `build/windows` and `build-windows`

### macOS

```bash
./scripts/build-macos.sh all
./scripts/build-macos.sh debug
./scripts/build-macos.sh debug-run
./scripts/build-macos.sh desktop
```

Actions:

- `cpp` — configure/build native macOS targets into `build/macos/cpp`
- `test` — run focused native tests from `build/macos/cpp`
- `electron` — build renderer/main and stage native assets into `build/macos/electron`
- `debug` — native build + tests + unpacked Electron debug app under `build/macos/electron/release/mac*`
- `debug-run` — same as `debug`, then launch the unpacked macOS Electron UI
- `desktop` — native build + tests + packaged Electron output
- `all` — native build + tests + Electron compile/stage
- `clean` — remove `build/macos`

The `debug` action now clears the platform release directory before running
`electron-builder --dir`, so stale DMG / installer artifacts from earlier
`desktop` runs do not remain mixed into the debug output.

## Merge-Prep Validation Wrappers

Use these wrappers when validating merge-prep lanes so every reviewer runs the
same focused checks.

### Windows

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\validate-merge-prep-windows.ps1
```

Optional flags:

- `-SkipDesktop` — skip Electron compile + native staging checks
- `-DesktopSmoke` — run an additional debug-run desktop smoke flow

### macOS

```bash
./scripts/validate-merge-prep-macos.sh
```

Optional flags:

- `--skip-desktop` — skip Electron compile + native staging checks
- `--desktop-smoke` — run an additional debug-run desktop smoke flow

## Native Runtime Assets

Production packages use the native VPN implementation by default.

- Windows production packages stage the native `exv`/`exv-helper` binaries,
  required MinGW runtime DLLs, and `wintun.dll`. `wintun.dll` is the Windows
  native runtime asset.
- macOS production packages stage the native `exv` binary and helper
  integration. Homebrew OpenConnect is not required for native production
  packages.

Native production mode does not support arbitrary OpenConnect-style
`extra_args`. Use the supported EXV config keys and route commands instead.

### Legacy Diagnostic Fallback

Legacy OpenConnect runtime files are retained only for development and
diagnostic comparisons with the legacy backend. They are not required before
production packaging.

- Windows legacy diagnostic assets can be prepared under `runtime/win32-x64/`
  with `scripts/stage-openconnect-runtime-win.ps1`.
- macOS legacy diagnostic assets can be prepared under `runtime/darwin-<arch>/`
  with `scripts/stage-openconnect-runtime-mac.sh`.

Those scripts may copy legacy OpenConnect executables, GnuTLS-related DLLs, and
other compatibility files used by the old backend. Do not treat those files as
native production package requirements.

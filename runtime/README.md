# Bundled OpenConnect Runtime

Place desktop runtime assets here before running `npm run desktop:build`.

Supported layouts:

- `runtime/win32-x64/`
- `runtime/win32-arm64/`
- `runtime/darwin-x64/`
- `runtime/darwin-arm64/`
- `runtime/win32/`
- `runtime/darwin/`

`webui/scripts/prepare-native.cjs` copies the first matching directory into
`webui/native/bin/`. You can override the source directory with
`ECNUVPN_RUNTIME_DIR`.

Helper staging scripts:

- Windows: `scripts/stage-openconnect-runtime-win.ps1`
- macOS: `scripts/stage-openconnect-runtime-mac.sh`

Recommended Windows contents:

- `openconnect.exe`
- `libopenconnect-5.dll`
- OpenConnect dependency DLLs
- `wintun.dll`
- Optional TAP assets such as `tap-windows-installer.exe` or `tap/OemVista.inf`

Recommended macOS contents:

- `openconnect`
- required `.dylib` dependencies adjacent to the binary

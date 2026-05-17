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

## Required vs Optional Files

### Windows

| File | Category | Behavior when missing |
|------|----------|----------------------|
| `openconnect.exe` | **REQUIRED** | VPN cannot connect. The app returns `runtime_missing` error with message "OpenConnect runtime is not available" and recommended action "Ensure the desktop package contains the bundled OpenConnect runtime". |
| `libopenconnect-5.dll` | **REQUIRED** | Same as above — openconnect.exe cannot start without its core library. |
| `wintun.dll` | **REQUIRED for Wintun mode** | If `windows_tunnel_driver` is set to `wintun` (the default), VPN connect returns `runtime_missing` with message "Wintun is selected but bundled wintun.dll is missing." The app falls back gracefully — the user can switch to TAP mode if TAP assets are available. |
| GnuTLS / libxml2 DLLs | **REQUIRED** | openconnect.exe cannot start without its dependency DLLs. The `stage-openconnect-runtime-win.ps1` script copies all DLLs from the source directory. |
| `tap-windows-installer.exe` or `tap/OemVista.inf` | **OPTIONAL** | TAP tunnel mode is a legacy fallback. If not staged, the user simply cannot select TAP as the tunnel driver. Wintun mode (the default) does not require TAP assets. |
| `LICENSE*`, `COPYING*` | **OPTIONAL** | License compliance files. Not required for functionality. |

### macOS

| File | Category | Behavior when missing |
|------|----------|----------------------|
| `openconnect` | **REQUIRED** | VPN cannot connect. Same `runtime_missing` error as Windows. |
| Required `.dylib` dependencies | **REQUIRED** | openconnect cannot start without its dynamic libraries. |

## Runtime-Missing Behavior in the Desktop App

When a required runtime file is missing, the native `exv` binary returns a
structured JSON error:

```json
{
  "ok": false,
  "error_type": "runtime_missing",
  "message": "OpenConnect runtime is not available.",
  "recoverable": true,
  "recommended_action": "Ensure the desktop package contains the bundled OpenConnect runtime"
}
```

The `/runtime.status` API endpoint also returns actionable guidance fields when
the runtime is unavailable:

```json
{
  "available": false,
  "missing_what": "openconnect binary",
  "recommended_action": "Reinstall the desktop package with the bundled OpenConnect runtime assets.",
  "effect_on_connect": "VPN connection will fail with runtime_missing error.",
  "wintun_missing": true,
  "tap_missing": true
}
```

The desktop app's Vue frontend uses these fields to display a yellow warning
card in the Settings page, showing the `missing_what` and `recommended_action`
text so the user immediately knows what is wrong and how to fix it.

This is a recoverable error — the user can resolve it by reinstalling the
desktop package with the runtime properly staged.

## Driver-Missing Behavior in the Desktop App

When Wintun or TAP driver assets are missing, the `/drivers.status` API endpoint
returns detailed guidance fields alongside the existing status fields:

```json
{
  "wintun_missing": true,
  "wintun_missing_reason": "bundled wintun.dll not found",
  "wintun_recommended_action": "Reinstall the desktop package to restore bundled wintun.dll.",
  "tap_missing": true,
  "tap_missing_reason": "no TAP adapters detected and no bundled installer",
  "tap_recommended_action": "Install TAP driver from Settings, or switch tunnel driver to Wintun.",
  "effective_driver_status": "degraded"
}
```

The `effective_driver_status` field summarizes overall driver readiness:
- `"ready"` — at least one driver works and the preferred driver is available
- `"degraded"` — the preferred driver is missing but a fallback is available
- `"unavailable"` — no driver can work; VPN connections will fail

The Settings page shows:
- Wintun: green "Ready" badge or yellow "Missing" badge with `wintun_recommended_action`
- TAP: green "Installed", yellow "Can install", or red "Not available" badge with `tap_recommended_action`
- An overall driver readiness line: "Driver ready" / "Driver degraded" / "Driver unavailable"

When the user installs a driver, the response includes a `takes_effect` field:
- Wintun: `"takes_effect": "next_connect"` — wintun.dll is loaded at connection time
- TAP: `"takes_effect": "immediately"` — the adapter is available right away, but
  existing VPN connections should be disconnected and reconnected to use the new adapter

## Staging Before Packaging

**Both the NSIS installer and the portable build carry the same native runtime
bundle.** The runtime must be staged before running `npm run desktop:build`,
otherwise the packaged app will lack runtime files and VPN connections will
fail with the `runtime_missing` error.

```powershell
# Stage from an openconnect-gui installation
powershell -ExecutionPolicy Bypass -File scripts\stage-openconnect-runtime-win.ps1 `
  -SourceDir "C:\Program Files\OpenConnect-GUI" `
  -WintunDllPath C:\Downloads\wintun\bin\amd64\wintun.dll

# Stage without TAP assets (Wintun-only build)
powershell -ExecutionPolicy Bypass -File scripts\stage-openconnect-runtime-win.ps1 `
  -SourceDir "C:\Program Files\OpenConnect-GUI" `
  -WintunDllPath C:\Downloads\wintun\bin\amd64\wintun.dll `
  -SkipTap

# Stage without Wintun (TAP-only build)
powershell -ExecutionPolicy Bypass -File scripts\stage-openconnect-runtime-win.ps1 `
  -SourceDir "C:\Program Files\OpenConnect-GUI" `
  -TapAssetPath C:\Downloads\tap-windows-installer.exe `
  -SkipWintun

# Then build
cd webui && npm run desktop:build
```

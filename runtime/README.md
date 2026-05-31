# Native Production Runtime

Production desktop packages are native-first. They must not require or bundle
OpenConnect, libopenconnect, GnuTLS, or staged macOS OpenConnect dylibs.

`webui/scripts/prepare-native.cjs` always stages the native binaries built by
CMake:

- `exv` / `exv.exe`
- `exv-helper` / `exv-helper.exe`
- required MinGW runtime DLLs on Windows

Optional production runtime assets can be placed in the first matching
directory below, or supplied with `ECNUVPN_RUNTIME_DIR`:

- `runtime/win32-x64/`
- `runtime/win32-arm64/`
- `runtime/win32/`

The only Windows production asset currently copied from those directories is
`wintun.dll`. The production prepare step copies allowed assets by name; it
does not copy runtime directories wholesale.

## Legacy Diagnostic OpenConnect Runtime

The OpenConnect staging scripts are preserved only for explicit legacy diagnostic
work. They are not part of production packaging.

Preserved legacy payloads live outside production runtime roots:

- `runtime/legacy-openconnect/win32-x64/`
- `runtime/legacy-openconnect/darwin-arm64/`

Set `ECNUVPN_LEGACY_OPENCONNECT_RUNTIME=1` before invoking either script:

```powershell
$env:ECNUVPN_LEGACY_OPENCONNECT_RUNTIME = "1"
powershell -File scripts/stage-openconnect-runtime-win.ps1 -SourceDir <dir> -Arch x64
```

```bash
ECNUVPN_LEGACY_OPENCONNECT_RUNTIME=1 \
  bash ./scripts/stage-openconnect-runtime-mac.sh /opt/homebrew/bin/openconnect arm64
```

Those scripts may stage `openconnect`, `openconnect.exe`, libopenconnect,
GnuTLS, and adjacent dependency files for diagnostics. Production packaging
filters still deny those legacy payloads from `extraResources/bin`.

`webui/scripts/prepare-native.cjs` also uses those legacy diagnostic paths only
when `ECNUVPN_LEGACY_OPENCONNECT_RUNTIME=1` is set. To point it at an external
diagnostic payload directory, set `ECNUVPN_LEGACY_OPENCONNECT_RUNTIME_DIR`.

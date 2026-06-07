# Native Windows Automated Release Gate — Evidence (2026-05-31)

**Task:** F1 — Windows Automated Native Release Gate
**Plan:** `docs/superpowers/plans/2026-05-31-native-openconnect-extraction-completion-plan.md`
**Host:** Windows 10.0.26200, x64, MinGW gcc 15.2.0, node v24.14.0 / npm 11.9.0, electron-builder 26.8.1, electron 39.8.10

Evidence-only gate: this records the automated build/test/package outcome on a
clean-room native build. Any product fix discovered here is routed back to the
P/C/D workstreams, not patched in this document.

## Commands and Results

### 1. Configure

```powershell
cmake --preset windows-release
```

```
-- Configuring done (0.1s)
-- Generating done (0.1s)
-- Build files have been written to: build-windows/cpp
```

Exit 0.

### 2. Build native binaries

```powershell
cmake --build --preset windows-release --target exv exv-helper
```

Exit 0 (`ninja: no work to do` — up to date from the preceding clean build; both
`exv.exe` and `exv-helper.exe` link successfully in this preset, see C2 evidence).

### 3. Full native test suite

```powershell
ctest --preset windows-release --output-on-failure
```

```
25/25 Test #25: win32_native_tls_stream_test .........   Passed    0.01 sec
100% tests passed, 0 tests failed out of 25
Total Test time (real) = 10.13 sec
```

Exit 0. All 25 native tests pass.

### 4. Frontend + Electron + desktop package

```powershell
cd webui
npm run build          # vue-tsc -b && vite build
npm run build:electron # main/preload bundle
npm run desktop:build  # prepare-native + electron-builder
cd ..
```

All exit 0 (captured in `docs/validation/native-packaging-composition-2026-05-31.md`).
electron-builder produced the unpacked app, the NSIS installer
(`ECNU VPN Setup 0.0.0.exe`), and the portable build
(`ECNU-VPN-0.0.0-portable.exe`).

### 5. No OpenConnect process running

```powershell
Get-Process openconnect -ErrorAction SilentlyContinue
```

Result: `(none)` — no OpenConnect process present.

### 6. Forbidden-asset scan of the package

```powershell
Get-ChildItem -Recurse build\windows\electron\release\win-unpacked `
  -Include "openconnect*","libopenconnect*","*gnutls*","libssl*","libcrypto*" |
  Select-Object FullName
```

Result: `(none)` — no OpenConnect/GnuTLS/libssl/libcrypto artifacts in the
package.

## Verdict

PASS (automated gate). Configure, native build, full test suite (25/25),
frontend, Electron, and desktop package builds all exit 0. The produced package
contains native binaries plus required runtime assets (`exv.exe`,
`exv-helper.exe`, MinGW runtime DLLs, `wintun.dll`) and no forbidden OpenConnect /
GnuTLS payload. No OpenConnect process is involved.

Live packaged connect/disconnect against ECNU (Task F2) requires an Administrator
session plus ECNU reachability and credentials and is **ENV-BLOCKED** on this
host — recorded explicitly rather than claimed as a pass.

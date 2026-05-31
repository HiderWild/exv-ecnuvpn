# Native Packaging Composition Proof — Windows (2026-05-31)

**Task:** C2 — Verify Production Packaging Excludes OpenConnect/GnuTLS
**Plan:** `docs/superpowers/plans/2026-05-31-native-openconnect-extraction-completion-plan.md`
**Host:** Windows 10.0.26200, x64, MinGW gcc 15.2.0, node v24.14.0 / npm 11.9.0, electron-builder 26.8.1, electron 39.8.10

## Goal

Prove the built desktop package contains native binaries + native runtime assets
only, with no OpenConnect / GnuTLS / libssl / libcrypto production payload. Legacy
diagnostic OpenConnect staging stays gated behind
`ECNUVPN_LEGACY_OPENCONNECT_RUNTIME=1` and is never part of the native production
package.

## Commands and Results

### 1. Packaging policy unit test

```powershell
cmake --build --preset windows-release --target native_packaging_policy_test
ctest --preset windows-release -R native_packaging_policy_test --output-on-failure
```

Result:

```
Start 13: native_packaging_policy_test
1/1 Test #13: native_packaging_policy_test .....   Passed    0.02 sec
100% tests passed, 0 tests failed out of 1
```

The policy test statically verifies that production packaging scripts
(`webui/scripts/prepare-native.cjs`, `webui/electron-builder.config.cjs`,
`runtime/README.md`, etc.) do not stage OpenConnect/GnuTLS into production paths,
and that any OpenConnect references are gated behind the legacy diagnostic env
flag or are explicit denial/filter rules.

### 2. Full desktop package build

```powershell
cd webui
npm run desktop:build
```

Result: exited 0. electron-builder produced:

- `build/windows/electron/release/win-unpacked/` (unpacked app)
- `build/windows/electron/release/ECNU VPN Setup 0.0.0.exe` (NSIS installer)
- `build/windows/electron/release/ECNU-VPN-0.0.0-portable.exe` (portable)

The `prepare-native` step logged native-only staging:

```
Copied runtime DLL: libgcc_s_seh-1.dll
Copied runtime DLL: libstdc++-6.dll
Copied runtime DLL: libwinpthread-1.dll
Copied native runtime asset(s): wintun.dll from runtime/win32-x64
[validation] Native binary present: exv.exe
[validation] Wintun DLL present: wintun.dll
```

### 3. Forbidden-asset scan

```powershell
Get-ChildItem -Recurse build\windows\electron\release\win-unpacked `
  -Include "openconnect*","libopenconnect*","*gnutls*","libssl*","libcrypto*" |
  Select-Object FullName
```

Result: **no output** — zero OpenConnect / libopenconnect / GnuTLS / libssl /
libcrypto artifacts in the packaged tree.

### 4. Required-asset scan

```powershell
Get-ChildItem build\windows\electron\release\win-unpacked\resources\bin
```

Result:

| File | Size (bytes) |
| --- | --- |
| `exv.exe` | 1,213,440 |
| `exv-helper.exe` | 781,824 |
| `libgcc_s_seh-1.dll` | 920,973 |
| `libstdc++-6.dll` | 2,376,223 |
| `libwinpthread-1.dll` | 91,221 |
| `wintun.dll` | 427,552 |

The package contains the native CLI (`exv.exe`), native helper
(`exv-helper.exe`), the three required MinGW runtime DLLs, and the Wintun adapter
driver (`wintun.dll`) — and nothing from the OpenConnect/GnuTLS stack.

## Verdict

PASS. The production Windows desktop package is composed entirely of native
binaries plus the Wintun native runtime asset. No OpenConnect, libopenconnect,
GnuTLS, libssl, or libcrypto payload is present. Legacy OpenConnect staging
remains gated behind `ECNUVPN_LEGACY_OPENCONNECT_RUNTIME=1` and is excluded from
the native production package.

## Notes

- During the first `desktop:build` attempt, electron-builder failed to empty the
  previous `win-unpacked` directory because a stale elevated `exv-helper.exe`
  process (from earlier manual testing) held a lock on the prior copy. The stale
  output directory was renamed aside and the build re-run cleanly. This is a host
  housekeeping artifact, not a packaging-composition issue.

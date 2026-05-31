# macOS Automated Validation — Native OpenConnect Extraction (2026-05-31)

Covers plan tasks **B4v** (macOS TLS stream validation) and the automated portion of
**G1** (macOS release gate: build + test + package + forbidden-asset scan). Executed on the
remote macmini host (`ssh macmini`), project dir
`/Users/tomli/Development/Projects/CPP/ECNU-VPN`, kept in real-time sync with the Windows
workspace.

## Environment

- Host: macmini, macOS 26.3.1 (Darwin 25.3.0), Apple Silicon (arm64)
- Toolchain: cmake 4.3.2, clang (Command Line Tools / MacOSX.sdk), node v25.6.1, npm 11.9.0,
  electron 39.8.10, electron-builder 26.8.1
- Preset: `macos-release` (Unix Makefiles), build dir `build/macos/cpp`

## Build fix applied during this gate

Clean configure + build initially failed on the latest macOS SDK:

```
src/platform/darwin/native_utun.cpp:106  error: use of undeclared identifier 'SYSPROTO_CONTROL'
src/platform/darwin/native_utun.cpp:140  error: use of undeclared identifier 'AF_SYS_CONTROL'
src/platform/darwin/native_utun.cpp:149  error: use of undeclared identifier 'SYSPROTO_CONTROL'
```

Root cause: newer macOS SDKs no longer transitively expose `SYSPROTO_CONTROL` /
`AF_SYS_CONTROL` via `<sys/kern_control.h>`. Fix: added `#include <sys/sys_domain.h>` to
`src/platform/darwin/native_utun.cpp`. After the fix the build completes to 100%.

## Results

### Native build (`cmake --build --preset macos-release`)

```
[100%] Linking CXX executable exv-helper
[100%] Built target exv-helper
[100%] Linking CXX executable exv
[100%] Built target exv
```

### Native tests (`ctest --preset macos-release --output-on-failure`)

```
100% tests passed, 0 tests failed out of 25
Total Test time (real) = 9.76 sec
```

Darwin-specific native tests (covers **B4v**):

```
22/25 darwin_native_utun_test ............. Passed
23/25 darwin_native_route_config_test ..... Passed
24/25 darwin_native_packet_device_test .... Passed
25/25 darwin_native_tls_stream_test ....... Passed
```

### Desktop compile + native staging (`npm run desktop:compile`)

- `vue-tsc -b && vite build` → success
- `build:electron` → success
- `prepare:native` staged native binaries only:
  - `build/macos/cpp/exv` → `build/macos/electron/native/bin/exv`
  - `build/macos/cpp/exv-helper` → `build/macos/electron/native/bin/exv-helper`
  - "No optional native runtime assets found" (runtime/darwin-arm64, runtime/darwin) — i.e. no
    OpenConnect runtime bundled.

### Clean repackage (`npm run desktop:package:dir`)

A prior `release/mac-arm64` app (pre-extraction leftover) still contained
`libgnutls.30.dylib`, `libopenconnect.5.dylib`, and `openconnect`. The stale `release/` dir was
removed and the app repackaged from the current config.

### Forbidden-asset scan (clean-room confirmation)

Dynamic link deps of the native binaries:

```
otool -L build/macos/cpp/exv        → NO_FORBIDDEN_DYLIBS (no openconnect/gnutls/libxml/nettle/gmp)
otool -L build/macos/cpp/exv-helper → NO_FORBIDDEN_DYLIBS_HELPER
```

Freshly packaged app bundle:

```
build/macos/electron/release/mac-arm64/ECNU VPN.app/Contents/Resources/bin/
  exv
  exv-helper

find ... -iname "*openconnect*" -o -iname "*gnutls*" -o -iname "*nettle*" -o -iname "*libxml2*"
  → (empty) NONE
```

The shipped macOS application contains only the native `exv` / `exv-helper` binaries and links
no OpenConnect/GnuTLS dependencies.

## Status

- **B4v**: PASS — darwin native TLS stream test (and full darwin native suite) green on real macOS.
- **G1 (automated)**: PASS — macOS build, 25/25 tests, desktop compile/package, and forbidden-asset
  scan all clean.
- **G1 (live) / G2**: still requires ECNU network reachability + credentials + admin/sudo on the
  macmini; tracked separately. Not faked here.

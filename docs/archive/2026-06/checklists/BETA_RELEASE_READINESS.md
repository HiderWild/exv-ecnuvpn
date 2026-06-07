# Beta Release Readiness

> **Target version:** 3.3.0
> **Last updated:** 2026-06-03
> **Status:** IN PROGRESS

---

## 1. Release Artifacts Inventory

| Artifact | Platform | Build Method | Status |
|----------|----------|-------------|--------|
| Windows NSIS installer | Windows x64 | electron-builder `nsis` target | Planned |
| Windows portable zip | Windows x64 | electron-builder `portable` target | Planned |
| macOS DMG (arm64) | macOS Apple Silicon | electron-builder `dmg` target | Planned |
| macOS DMG (x64) | macOS Intel | electron-builder `dmg` target | Planned |
| `exv` CLI binary | Windows / macOS / Linux | CMake build (`cmake --preset <platform>-release`) | Built |
| `exv-helper` CLI binary | Windows / macOS / Linux | CMake build | Built |
| Wintun DLL | Windows x64 | Bundled from `runtime/win32-x64/wintun.dll` | Present |
| WebView2 / Electron UI | Cross-platform | Vite + Electron (`webui/`) | Built |
| Helper service registration | Windows | `scripts/install-windows.bat` | Present |
| Helper launchd plist | macOS | Embedded in Electron packaging | Planned |
| OpenConnect runtime (legacy) | Windows / macOS | `scripts/stage-openconnect-runtime-*.sh` | Legacy diagnostic only |

### Artifact Details

**Windows NSIS Installer** (configured in `webui/electron-builder.config.cjs`):
- Target: `nsis`, architecture `x64`
- One-click: `false` (allows choosing install directory)
- Per-machine install: `true`
- Creates desktop and start menu shortcuts
- Custom installer script: `webui/build-resources/installer.nsh` (if present)
- Output artifact: `ECNU-VPN-<version>.exe`

**Windows Portable** (configured in `webui/electron-builder.config.cjs`):
- Target: `portable`, architecture `x64`
- No install required, runs from any directory
- Output artifact: `ECNU-VPN-<version>-portable.zip`

**macOS DMG** (configured in `webui/electron-builder.config.cjs`):
- Target: `dmg`, architectures `x64` and `arm64`
- Category: `public.app-category.utilities`
- Hardened runtime: `true`
- Gatekeeper assess: `false` (ad-hoc signing for Beta)
- Output artifact: `ECNU-VPN-<version>-mac-<arch>.dmg`

**CLI Binaries:**
- `exv` — main CLI and core engine entry point
- `exv-helper` — privileged helper service
- Both compile with `ECNUVPN_VERSION="${PROJECT_VERSION}"` (currently `3.3.0`)
- Version can be verified with `exv --version` and `exv-helper --version`

**Electron Desktop App:**
- Vue 3 + Vite renderer in `webui/`
- Electron main process in `webui/src/` (compiled to `dist-electron/`)
- Native binary (`exv`) bundled as `extraResources/bin/exv[.exe]`
- ASAR packaging enabled with maximum compression
- appId: `cn.edu.ecnu.vpn`

---

## 2. Beta Release Checklist

### CI & Build Gates

- [ ] **CI release-blocking pass** — `ctest -L release-blocking` passes on all three platforms (Windows, macOS, Linux)
- [ ] **Architecture guardrails pass** — `scripts/architecture-guardrails.ps1` (Windows) and `scripts/architecture-guardrails.sh` (macOS/Linux) both pass
- [ ] **WebUI typecheck pass** — `cd webui && npm run typecheck` (or `vue-tsc -b`) exits 0
- [ ] **WebUI build pass** — `cd webui && npm run build` exits 0
- [ ] **Electron compile pass** — `cd webui && npm run desktop:compile` exits 0

### Platform Packaging

- [ ] **Windows NSIS installer builds** — `electron-builder --config electron-builder.config.cjs` produces valid `.exe` installer
- [ ] **Windows portable builds** — electron-builder produces valid portable archive
- [ ] **macOS DMG (arm64) builds** — DMG produced for Apple Silicon
- [ ] **macOS DMG (x64) builds** — DMG produced for Intel (if target platform available)
- [ ] **Linux binary package** — `cmake --build --preset linux-release` produces `exv` and `exv-helper`

### Real Platform Validation

- [ ] **Windows real connect pass** — Successful VPN connect/disconnect on Windows 11
- [ ] **macOS real connect pass** — Successful VPN connect/disconnect on macOS (Apple Silicon or Intel)
- [ ] **Windows install/uninstall pass** — NSIS install, helper service install, uninstall, no residual service
- [ ] **macOS DMG install pass** — DMG mount, drag to Applications, launch, helper install, uninstall

### Security & Credential

- [ ] **No known credential leakage** — `no_secret_in_argv_test` and `no_secret_in_logs_test` pass; no passwords/tokens/cookies in helper code
- [ ] **Credential store works** — `credential_store_model_test` passes on Windows (Win Credential Store) and macOS (Keychain)
- [ ] **Architecture boundary intact** — Helper does not include `vpn_engine/protocol`; core does not have platform `#ifdef`

### Documentation & Rollback

- [ ] **Known issues documented** — `docs/KNOWN_ISSUES_PHASE7.md` is up to date
- [ ] **Rollback instructions ready** — See Section 5 below
- [ ] **User guide updated** — `docs/user_guide.md` reflects current Beta state

### Packaging Smoke

- [ ] **Windows smoke pass** — `scripts/packaging-smoke.ps1` exits 0
- [ ] **macOS/Linux smoke pass** — `scripts/packaging-smoke.sh` exits 0

---

## 3. Version Tracking

### Version Number

The canonical version is defined in `CMakeLists.txt`:

```cmake
project(exv VERSION 3.3.0 LANGUAGES CXX)
```

It is propagated to all binaries via compile definition:

```cmake
target_compile_definitions(exv PRIVATE ECNUVPN_VERSION="${PROJECT_VERSION}")
```

**How to update:**
1. Edit `VERSION 3.3.0` in `CMakeLists.txt` (line 2)
2. Update `webui/package.json` version field if Electron packaging reads it
3. The electron-builder config uses `${version}` placeholders that resolve from `package.json`

### Commit Hash

Commit hash is not currently embedded at build time. To add it for Beta:

```cmake
execute_process(
  COMMAND git rev-parse --short HEAD
  WORKING_DIRECTORY ${CMAKE_SOURCE_DIR}
  OUTPUT_VARIABLE ECNUVPN_GIT_COMMIT
  OUTPUT_STRIP_TRAILING_WHITESPACE
)
target_compile_definitions(exv PRIVATE ECNUVPN_GIT_COMMIT="${ECNUVPN_GIT_COMMIT}")
```

**Recommendation:** Add commit hash embedding before Beta release so users and support can identify exact build.

### Build Channel

Build channel is determined by the build type:
- `CMAKE_BUILD_TYPE=Release` — production release
- `CMAKE_BUILD_TYPE=Debug` — development build
- Electron packaging adds `electron-builder` channel (latest, beta, alpha)

For Beta releases, use:
- Git tag: `v3.3.0-beta.1`
- Electron-builder publish channel: `beta`

---

## 4. Recommended Release Channel

**GitHub Releases (pre-release)** is the recommended channel for Beta:

1. Create a Git tag: `git tag v3.3.0-beta.1`
2. Push tag: `git push origin v3.3.0-beta.1`
3. Create a GitHub Release marked as **pre-release**
4. Upload artifacts:
   - `ECNU-VPN-<version>.exe` (NSIS installer)
   - `ECNU-VPN-<version>-portable.zip` (portable)
   - `ECNU-VPN-<version>-mac-arm64.dmg` (Apple Silicon)
   - `ECNU-VPN-<version>-mac-x64.dmg` (Intel)
   - `exv-linux-x64` and `exv-helper-linux-x64` (Linux binaries, if distributing)
5. Release notes should include:
   - Known issues (link to `docs/KNOWN_ISSUES_PHASE7.md`)
   - Installation instructions
   - Rollback instructions
   - Feedback channel (GitHub Issues)

**Why GitHub Releases:**
- Built-in artifact hosting
- Pre-release flag signals Beta status
- Issue tracker integration
- No infrastructure to maintain
- Supports partial rollout via tag naming

---

## 5. Rollback Instructions

### Windows (NSIS Install)

1. Open **Settings > Apps > Installed apps**
2. Find **ECNU VPN** and click **Uninstall**
3. Or run the uninstaller from `C:\Program Files\ECNU-VPN\uninstall.exe`
4. Verify helper service is removed: `sc query exv-helper` should fail
5. If residual service remains: `sc delete exv-helper` (requires admin)
6. Delete install directory if residual: `rmdir /s "C:\Program Files\ECNU-VPN"`

### Windows (Portable)

1. Close the application
2. Delete the portable directory
3. If helper was installed via portable: `sc delete exv-helper` (admin)
4. Clean up any residual routes/DNS: reboot or `route delete` / `netsh int ip set dns`

### macOS

1. Quit the application
2. Drag **ECNU VPN** from Applications to Trash
3. Remove helper: the helper uninstall script runs during app uninstall, or manually:
   ```bash
   sudo /Library/PrivilegedHelperTools/cn.edu.ecnu.vpn.helper --uninstall
   ```
4. Remove launchd plist if residual:
   ```bash
   sudo rm /Library/LaunchDaemons/cn.edu.ecnu.vpn.helper.plist
   ```
5. Remove application support data if needed:
   ```bash
   rm -rf ~/Library/Application\ Support/ECNU\ VPN
   ```

### Nuclear Option (All Platforms)

1. Uninstall via platform method above
2. Reboot the machine
3. Verify: no `exv` or `exv-helper` processes running
4. Verify: no residual VPN routes or DNS settings

---

## 6. Packaging Smoke Script Locations

| Script | Platform | Purpose |
|--------|----------|---------|
| `scripts/packaging-smoke.ps1` | Windows | Verify build artifacts, binaries, DLLs, installer |
| `scripts/packaging-smoke.sh` | macOS/Linux | Verify build artifacts, binaries, codesign, launchd |

Run these after a full build to validate release readiness before manual testing.

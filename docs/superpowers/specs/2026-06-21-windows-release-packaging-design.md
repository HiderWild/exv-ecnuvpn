# Windows Release Packaging Design

## Context

EXV already builds a native Windows WebView desktop package at `build\windows\webview\package\EXV`. That directory is a runnable package layout for development and validation, but it is not yet a user-facing release artifact. The Windows release flow needs to turn that package directory into two distributable formats:

- a portable `.zip`
- an NSIS `.exe` installer

The existing `scripts\package_ui_shell.py` remains the owner of the package directory layout. The release packaging layer must consume that directory instead of duplicating binary, renderer, WebView2, MinGW runtime, or Wintun collection logic.

## Goals

- Produce `EXV-<version>-windows-x64-portable.zip`.
- Produce `EXV-<version>-windows-x64-setup.exe`.
- Keep the native WebView package free of Electron, Chromium, and legacy OpenConnect runtime payloads.
- Preserve the existing first-run helper installation flow instead of forcing installer elevation.
- Add release-focused verification for directory, portable archive, and installer outputs.
- Document how maintainers build and validate Windows release artifacts.

## Non-Goals

- Do not add MSI/WiX packaging in this phase.
- Do not bundle Microsoft Edge WebView2 Evergreen Runtime. The app continues to detect and guide runtime installation.
- Do not require the installer itself to run elevated by default.
- Do not make the uninstaller remove user profile data, saved settings, logs, credentials, or the stable helper service unless a later design explicitly adds that option.
- Do not change the core runtime package layout produced by `scripts\package_ui_shell.py` except where verification exposes a packaging defect.

## Recommended Approach

Use `NSIS .exe + portable .zip`.

This fits the current repository better than WiX/MSI because the project already uses CMake, Python, PowerShell, and script-driven packaging. NSIS gives a simple user-facing installer without requiring a heavier Windows Installer authoring model. The portable archive is the same app package directory zipped with a deterministic release filename.

## Release Artifacts

Windows release outputs should live under:

```text
build\windows\release\
```

Expected files:

```text
EXV-<version>-windows-x64-portable.zip
EXV-<version>-windows-x64-setup.exe
```

The release script should derive `<version>` from one explicit source. The preferred source is a script parameter such as `-Version`, with a fallback to the project version already compiled into the app if the codebase exposes a stable value. The script must fail if it cannot determine a non-empty version.

## Source Package Layout

Both release artifacts use this source directory:

```text
build\windows\webview\package\EXV
```

Required contents are inherited from the existing package policy:

- `exv-ui.exe`
- `exv-ui.args`
- `bin\exv.exe`
- `bin\exv-helper.exe`
- `webui\index.html`
- `WebView2Loader.dll`
- required MinGW runtime DLLs
- optional `wintun.dll` when available from the configured runtime asset path

The release layer must not search build directories for app binaries directly. It should validate and package the already-built `EXV` directory.

## Portable ZIP

The portable artifact is a compressed copy of the `EXV` package directory. The archive root should contain a single top-level `EXV\` directory, not loose files. Users should be able to extract the zip anywhere and run:

```text
EXV\exv-ui.exe
```

Portable behavior:

- no registry writes
- no shortcut creation
- no service installation during extraction
- existing in-app helper install/start flow handles privileged helper setup
- app profile/config paths stay the existing platform defaults, such as `%LOCALAPPDATA%\EXV\profile\default`

## NSIS Installer

The installer should install the same `EXV` package contents to a per-user default location:

```text
%LocalAppData%\Programs\EXV
```

Rationale: a per-user default avoids making the installer itself an elevation boundary. The app already owns the privileged helper flow and can request elevation only when helper operations need it.

Installer behavior:

- install all files from the source `EXV` package directory
- create a Start Menu shortcut for `exv-ui.exe`
- register an uninstall entry for the current user
- write an uninstaller into the install directory
- support silent install and uninstall switches through NSIS defaults
- preserve existing app data and helper state on uninstall

Uninstaller behavior:

- remove installed application files
- remove Start Menu shortcuts
- remove the current-user uninstall entry
- leave `%LOCALAPPDATA%\EXV\profile\default` intact
- leave the stable helper service/binary intact unless the user removes it through the app's existing service commands

## Scripts

Add a Windows release script:

```text
scripts\package-windows-release.ps1
```

Responsibilities:

1. Accept release parameters:
   - `-Version <semver-or-release-version>`
   - `-SkipBuild`
   - `-PackageRoot <path>`
   - `-OutputDir <path>`
   - `-NsisPath <path>`
2. By default, call:
   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 desktop
   ```
3. Validate the package directory through the existing package verifier:
   ```powershell
   python scripts\package_ui_shell.py --verify-launch-targets-only --package-dir build\windows\webview\package\EXV
   ```
4. Generate the portable zip.
5. Invoke NSIS to compile the installer.
6. Print absolute paths for both generated release artifacts.

Add an NSIS script under a packaging-focused directory, for example:

```text
distribution\windows\exv.nsi
```

The NSIS script should be template-friendly so the PowerShell release script can pass version, source package directory, output file, and install directory defaults through `/D` defines instead of rewriting the `.nsi` file.

## Verification

Directory package verification remains:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows-packaging-smoke.ps1
```

Portable verification should:

- create a temporary extraction directory outside the synced source tree
- expand `EXV-<version>-windows-x64-portable.zip`
- assert the archive has exactly one top-level `EXV` directory
- run `scripts\windows-packaging-smoke.ps1 -PackageRoot <extracted>\EXV`
- delete the temporary extraction directory after the check

Installer static verification should:

- assert the NSIS output exists and has a non-zero size
- assert the generated filename matches `EXV-<version>-windows-x64-setup.exe`
- assert `distribution\windows\exv.nsi` contains install, shortcut, uninstall, and current-user registry sections

Installer live verification can be an optional manual or CI gate:

```powershell
EXV-<version>-windows-x64-setup.exe /S /D=%TEMP%\EXV-install-test
powershell -ExecutionPolicy Bypass -File scripts\windows-packaging-smoke.ps1 -PackageRoot %TEMP%\EXV-install-test
%TEMP%\EXV-install-test\Uninstall.exe /S
```

This live check should not be mandatory for normal developer builds because it touches local installed state.

## Tests

Add focused tests before implementation:

- Extend `tests\native_packaging_policy_test.cpp` so active release scripts are scanned and installer packaging cannot reintroduce denied Electron/OpenConnect runtime payloads.
- Add a host/package policy test or script-level test that confirms `package-windows-release.ps1` calls the existing WebView package build and verifier rather than copying binaries from build directories itself.
- Add a PowerShell verification path for portable zip extraction and smoke checks.

The first implementation task should add failing tests for script presence and expected release policy, then add the scripts.

## Documentation

Update active documentation:

- `README.md`: mention Windows installer and portable release artifact names.
- `docs\build_guide.md`: add commands for Windows release packaging and verification.
- `docs\runtime-assets.md`: keep Wintun/WebView2 runtime guidance aligned with release packaging.

## Risks

- NSIS may not be installed on all developer machines. The release script should fail with a clear message when `makensis.exe` cannot be found and allow `-NsisPath`.
- The optional `wintun.dll` may be missing from `runtime\win32-x64` or `EXV_RUNTIME_DIR`. The existing package script currently warns rather than fails for this optional asset; release verification should preserve current policy unless product policy changes.
- Silent installer verification can modify local installed state. Keep it opt-in until CI has an isolated Windows runner.
- Per-user installation means each Windows account gets its own install location. This is intentional for this phase because helper elevation is owned by the app flow, not the installer.

## Acceptance Criteria

- A maintainer can run one PowerShell command to build both Windows release artifacts.
- The portable zip extracts to one `EXV` directory and passes package smoke checks.
- The NSIS installer output is generated with the expected release filename.
- Existing package policy checks still reject Electron, Chromium, and legacy OpenConnect runtime payloads.
- Documentation explains prerequisites, output paths, and verification commands.

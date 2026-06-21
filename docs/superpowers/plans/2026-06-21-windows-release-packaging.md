# Windows Release Packaging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Windows user-facing release artifacts: a portable zip and an NSIS setup executable from the existing native WebView package directory.

**Architecture:** Keep `scripts/package_ui_shell.py` as the only owner of the runnable package layout, then add a thin Windows release layer that consumes `build\windows\webview\package\EXV`. PowerShell coordinates build, package verification, portable zip creation, portable extraction smoke, NSIS compilation, and installer static checks; `distribution\windows\exv.nsi` owns installer file layout, shortcuts, and uninstall metadata.

**Tech Stack:** PowerShell 5.1+, Python package verifier, NSIS `makensis.exe`, C++20 policy tests, Node `node:test` host policy tests, existing Windows WebView package smoke script.

---

## File Structure

- Modify `tests/native_packaging_policy_test.cpp`
  - Add release packaging files to production payload scanning.
  - Assert the release script and NSIS manifest exist and use the native WebView package path.
- Modify `webui/host/__tests__/webview-package-policy.test.ts`
  - Add script-level policy tests for release artifact names, build invocation, verifier invocation, portable smoke, NSIS defines, per-user install default, and docs.
- Create `scripts/package-windows-release.ps1`
  - Build or reuse the existing Windows WebView package.
  - Verify package launch targets.
  - Create `EXV-<version>-windows-x64-portable.zip`.
  - Extract and smoke-test the portable zip.
  - Compile `EXV-<version>-windows-x64-setup.exe` with NSIS.
  - Verify installer output exists and is non-empty.
- Create `distribution/windows/exv.nsi`
  - Install package files to `$LOCALAPPDATA\Programs\EXV` by default.
  - Create Start Menu shortcut and current-user uninstall entry.
  - Remove app install files and shortcuts on uninstall while preserving profile and helper state.
- Modify `README.md`
  - Mention Windows release artifacts and output paths.
- Modify `docs/build_guide.md`
  - Add Windows release packaging command, NSIS prerequisite, output files, and verification details.
- Modify `docs/runtime-assets.md`
  - Clarify that release packaging still uses the package script allowlist and does not bundle WebView2 Evergreen Runtime.

## Task 1: Add Failing Release Packaging Policy Tests

**Files:**
- Modify: `tests/native_packaging_policy_test.cpp`
- Modify: `webui/host/__tests__/webview-package-policy.test.ts`

- [ ] **Step 1: Extend C++ production file scan targets**

In `tests/native_packaging_policy_test.cpp`, update `production_files()` so it includes the Windows release script and NSIS script:

```cpp
std::vector<fs::path> production_files() {
  return {
      "webui/package.json",
      "webui/scripts/build-layout.cjs",
      "scripts/package_ui_shell.py",
      "scripts/package-windows-release.ps1",
      "scripts/embed_assets.py",
      "scripts/build-windows.ps1",
      "scripts/build-macos.sh",
      "distribution/windows/exv.nsi",
      "docs/runtime-assets.md",
  };
}
```

- [ ] **Step 2: Add C++ release script policy check**

In `tests/native_packaging_policy_test.cpp`, add this function after `check_production_build_scripts()`:

```cpp
bool check_windows_release_packaging_scripts() {
  bool ok = true;
  const FileText release = read_file("scripts/package-windows-release.ps1");
  const FileText nsis = read_file("distribution/windows/exv.nsi");

  ok = expect(contains(release, "scripts\\build-windows.ps1") &&
                  contains(release, "desktop"),
              "Windows release packaging should build the native WebView "
              "desktop package by default") &&
       ok;
  ok = expect(contains(release, "package_ui_shell.py") &&
                  contains(release, "--verify-launch-targets-only") &&
                  contains(release, "build\\windows\\webview\\package\\EXV"),
              "Windows release packaging should verify the existing native "
              "WebView package directory before creating release artifacts") &&
       ok;
  ok = expect(contains(release, "Compress-Archive") &&
                  contains(release, "Expand-Archive") &&
                  contains(release, "windows-packaging-smoke.ps1"),
              "Windows release packaging should create and smoke-test a "
              "portable zip") &&
       ok;
  ok = expect(contains(release, "makensis.exe") &&
                  contains(release, "distribution\\windows\\exv.nsi") &&
                  contains(release, "EXV-$Version-windows-x64-setup.exe"),
              "Windows release packaging should compile the NSIS setup "
              "artifact with the expected filename") &&
       ok;
  ok = expect(contains(nsis, "RequestExecutionLevel user") &&
                  contains(nsis, "$LOCALAPPDATA\\Programs\\EXV") &&
                  contains(nsis, "WriteRegStr HKCU") &&
                  contains(nsis, "CreateShortCut") &&
                  contains(nsis, "WriteUninstaller") &&
                  contains(nsis, "DeleteRegKey HKCU"),
              "NSIS installer should be per-user, create shortcuts, and "
              "register a current-user uninstaller") &&
       ok;
  ok = expect(!contains(nsis, "$PROGRAMFILES") &&
                  !contains(nsis, "RequestExecutionLevel admin") &&
                  !contains(nsis, "RMDir /r \"$LOCALAPPDATA\\EXV\""),
              "NSIS installer should not force machine-wide elevation or "
              "remove user profile data") &&
       ok;

  return ok;
}
```

- [ ] **Step 3: Call the C++ release script policy check**

In `tests/native_packaging_policy_test.cpp`, add this line in `main()` after `check_production_build_scripts()`:

```cpp
  ok = check_windows_release_packaging_scripts() && ok;
```

- [ ] **Step 4: Add TypeScript host policy test for release packaging**

In `webui/host/__tests__/webview-package-policy.test.ts`, add this `it(...)` block after the existing `"keeps active packaging smoke and merge-prep scripts on WebView package paths"` test:

```ts
  it('keeps Windows release packaging on native WebView package outputs', () => {
    const releaseScriptPath = join(repoRoot, 'scripts', 'package-windows-release.ps1')
    const nsisPath = join(repoRoot, 'distribution', 'windows', 'exv.nsi')

    assert.equal(existsSync(releaseScriptPath), true)
    assert.equal(existsSync(nsisPath), true)

    const releaseScript = readFileSync(releaseScriptPath, 'utf8')
    const nsis = readFileSync(nsisPath, 'utf8')
    const readme = readFileSync(join(repoRoot, 'README.md'), 'utf8')
    const buildGuide = readFileSync(join(repoRoot, 'docs', 'build_guide.md'), 'utf8')
    const runtimeAssets = readFileSync(join(repoRoot, 'docs', 'runtime-assets.md'), 'utf8')

    assert.match(releaseScript, /\[string\]\$Version/)
    assert.match(releaseScript, /\[switch\]\$SkipBuild/)
    assert.match(releaseScript, /\[string\]\$PackageRoot/)
    assert.match(releaseScript, /\[string\]\$OutputDir/)
    assert.match(releaseScript, /\[string\]\$NsisPath/)
    assert.match(releaseScript, /scripts\\build-windows\.ps1/)
    assert.match(releaseScript, /desktop/)
    assert.match(releaseScript, /package_ui_shell\.py/)
    assert.match(releaseScript, /--verify-launch-targets-only/)
    assert.match(releaseScript, /build\\windows\\webview\\package\\EXV/)
    assert.match(releaseScript, /EXV-\$Version-windows-x64-portable\.zip/)
    assert.match(releaseScript, /EXV-\$Version-windows-x64-setup\.exe/)
    assert.match(releaseScript, /Compress-Archive/)
    assert.match(releaseScript, /Expand-Archive/)
    assert.match(releaseScript, /windows-packaging-smoke\.ps1/)
    assert.match(releaseScript, /makensis\.exe/)
    assert.match(releaseScript, /distribution\\windows\\exv\.nsi/)
    assert.doesNotMatch(releaseScript, /build-windows\\cpp\\exv\.exe|build-windows\\cpp\\exv-helper\.exe|build-windows\\cpp\\exv-ui\.exe/)
    assert.doesNotMatch(releaseScript, /desktop:package|build:electron|electron\\release|dist-electron/i)

    assert.match(nsis, /RequestExecutionLevel user/)
    assert.match(nsis, /\$LOCALAPPDATA\\Programs\\EXV/)
    assert.match(nsis, /SetShellVarContext current/)
    assert.match(nsis, /File \/r "\$\{SOURCE_DIR\}\\\*\.\*"/)
    assert.match(nsis, /CreateShortCut/)
    assert.match(nsis, /WriteUninstaller/)
    assert.match(nsis, /WriteRegStr HKCU/)
    assert.match(nsis, /DeleteRegKey HKCU/)
    assert.doesNotMatch(nsis, /RequestExecutionLevel admin/)
    assert.doesNotMatch(nsis, /\$PROGRAMFILES/)
    assert.doesNotMatch(nsis, /RMDir \/r "\$LOCALAPPDATA\\EXV"/)

    assert.match(readme, /EXV-<version>-windows-x64-portable\.zip/)
    assert.match(readme, /EXV-<version>-windows-x64-setup\.exe/)
    assert.match(buildGuide, /package-windows-release\.ps1 -Version/)
    assert.match(buildGuide, /makensis\.exe/)
    assert.match(buildGuide, /windows-packaging-smoke\.ps1/)
    assert.match(runtimeAssets, /Windows release packaging/)
    assert.match(runtimeAssets, /WebView2 Evergreen Runtime/)
  })
```

- [ ] **Step 5: Run the TypeScript host test and verify RED**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts
```

Expected: FAIL because `scripts\package-windows-release.ps1` and `distribution\windows\exv.nsi` do not exist yet, and README/build guide/runtime asset docs do not mention the new release artifacts.

- [ ] **Step 6: Run the C++ policy test and verify RED**

If `build-windows\cpp\native_packaging_policy_test.exe` already exists, run:

```powershell
.\build-windows\cpp\native_packaging_policy_test.exe
```

Expected: FAIL with messages that `scripts/package-windows-release.ps1` and `distribution/windows/exv.nsi` cannot be read or do not satisfy release packaging policy.

If the executable does not exist, configure and build just this target, then run it:

```powershell
cmake --preset windows-release
cmake --build --preset windows-release --target native_packaging_policy_test
.\build-windows\cpp\native_packaging_policy_test.exe
```

Expected: FAIL for the same missing release packaging files.

- [ ] **Step 7: Commit the failing tests**

```powershell
git add tests\native_packaging_policy_test.cpp webui\host\__tests__\webview-package-policy.test.ts
git commit -m "test: define windows release packaging policy"
```

## Task 2: Implement Windows Release Script and NSIS Installer

**Files:**
- Create: `scripts/package-windows-release.ps1`
- Create: `distribution/windows/exv.nsi`

- [ ] **Step 1: Add the Windows release PowerShell script**

Create `scripts/package-windows-release.ps1` with this content:

```powershell
param(
  [Parameter(Mandatory = $true)]
  [ValidatePattern('^[0-9A-Za-z][0-9A-Za-z._+-]*$')]
  [string]$Version,
  [switch]$SkipBuild,
  [string]$PackageRoot = "",
  [string]$OutputDir = "",
  [string]$NsisPath = $env:NSIS_MAKENSIS
)

$ErrorActionPreference = 'Stop'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDir

function Resolve-AbsolutePath {
  param([Parameter(Mandatory = $true)][string]$Path)

  if ([System.IO.Path]::IsPathRooted($Path)) {
    return [System.IO.Path]::GetFullPath($Path)
  }

  return [System.IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}

function Invoke-Step {
  param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Arguments
  )

  & $FilePath @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "Command failed: $FilePath $($Arguments -join ' ')"
  }
}

function Resolve-MakeNsis {
  param([string]$RequestedPath)

  $candidates = New-Object System.Collections.Generic.List[string]

  if (-not [string]::IsNullOrWhiteSpace($RequestedPath)) {
    if (Test-Path -LiteralPath $RequestedPath -PathType Container) {
      [void]$candidates.Add((Join-Path $RequestedPath 'makensis.exe'))
    }
    else {
      [void]$candidates.Add($RequestedPath)
    }
  }

  $pathCommand = Get-Command makensis.exe -ErrorAction SilentlyContinue
  if ($pathCommand) {
    [void]$candidates.Add($pathCommand.Source)
  }

  if ($env:ProgramFiles) {
    [void]$candidates.Add((Join-Path $env:ProgramFiles 'NSIS\makensis.exe'))
  }
  if (${env:ProgramFiles(x86)}) {
    [void]$candidates.Add((Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe'))
  }

  foreach ($candidate in $candidates) {
    if (-not [string]::IsNullOrWhiteSpace($candidate) -and
        (Test-Path -LiteralPath $candidate -PathType Leaf)) {
      return (Resolve-Path -LiteralPath $candidate).Path
    }
  }

  throw 'makensis.exe was not found. Install NSIS, add makensis.exe to PATH, set NSIS_MAKENSIS, or pass -NsisPath.'
}

function Assert-PackageRoot {
  param([Parameter(Mandatory = $true)][string]$Root)

  if (-not (Test-Path -LiteralPath $Root -PathType Container)) {
    throw "Package root does not exist: $Root"
  }

  $required = @(
    'exv-ui.exe',
    'exv-ui.args',
    'bin\exv.exe',
    'bin\exv-helper.exe',
    'webui\index.html',
    'WebView2Loader.dll'
  )

  $missing = @()
  foreach ($relative in $required) {
    $candidate = Join-Path $Root $relative
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
      $missing += $relative
    }
  }

  if ($missing.Count -gt 0) {
    throw "Package root is missing required file(s): $($missing -join ', ')"
  }
}

function Invoke-PackageVerifier {
  param([Parameter(Mandatory = $true)][string]$Root)

  $verifyScript = Join-Path $repoRoot 'scripts\package_ui_shell.py'
  Invoke-Step python $verifyScript --verify-launch-targets-only --platform windows --package-dir $Root
}

function New-PortableZip {
  param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$Destination
  )

  if (Test-Path -LiteralPath $Destination) {
    Remove-Item -LiteralPath $Destination -Force
  }

  Compress-Archive -Path $Root -DestinationPath $Destination -CompressionLevel Optimal
  if (-not (Test-Path -LiteralPath $Destination -PathType Leaf)) {
    throw "Portable zip was not created: $Destination"
  }
}

function Test-PortableZip {
  param(
    [Parameter(Mandatory = $true)][string]$Archive,
    [Parameter(Mandatory = $true)][string]$RuntimeDir
  )

  $tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("exv-portable-" + [System.Guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $tempRoot | Out-Null

  try {
    Expand-Archive -LiteralPath $Archive -DestinationPath $tempRoot -Force
    $children = @(Get-ChildItem -LiteralPath $tempRoot)
    if ($children.Count -ne 1 -or -not $children[0].PSIsContainer -or $children[0].Name -ne 'EXV') {
      $names = $children | ForEach-Object { $_.Name }
      throw "Portable zip must contain exactly one top-level EXV directory. Found: $($names -join ', ')"
    }

    $extractedPackage = Join-Path $tempRoot 'EXV'
    $smoke = Join-Path $repoRoot 'scripts\windows-packaging-smoke.ps1'
    Invoke-Step powershell -NoProfile -ExecutionPolicy Bypass -File $smoke -PackageRoot $extractedPackage -RuntimeDir $RuntimeDir
  }
  finally {
    if (Test-Path -LiteralPath $tempRoot) {
      Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
  }
}

function Invoke-Nsis {
  param(
    [Parameter(Mandatory = $true)][string]$MakeNsis,
    [Parameter(Mandatory = $true)][string]$SourceDir,
    [Parameter(Mandatory = $true)][string]$OutputFile,
    [Parameter(Mandatory = $true)][string]$AppVersion
  )

  $scriptPath = Join-Path $repoRoot 'distribution\windows\exv.nsi'
  if (-not (Test-Path -LiteralPath $scriptPath -PathType Leaf)) {
    throw "NSIS script not found: $scriptPath"
  }

  $defaultInstallDir = Join-Path $env:LOCALAPPDATA 'Programs\EXV'
  Invoke-Step $MakeNsis `
    /V2 `
    "/DAPP_VERSION=$AppVersion" `
    "/DSOURCE_DIR=$SourceDir" `
    "/DOUTPUT_FILE=$OutputFile" `
    "/DDEFAULT_INSTALL_DIR=$defaultInstallDir" `
    $scriptPath
}

function Assert-InstallerOutput {
  param([Parameter(Mandatory = $true)][string]$Installer)

  if (-not (Test-Path -LiteralPath $Installer -PathType Leaf)) {
    throw "Installer was not created: $Installer"
  }

  $item = Get-Item -LiteralPath $Installer
  if ($item.Length -le 0) {
    throw "Installer is empty: $Installer"
  }
}

$defaultPackageRoot = Join-Path $repoRoot 'build\windows\webview\package\EXV'
$resolvedPackageRoot = if ([string]::IsNullOrWhiteSpace($PackageRoot)) {
  $defaultPackageRoot
} else {
  Resolve-AbsolutePath $PackageRoot
}

$resolvedOutputDir = if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  Join-Path $repoRoot 'build\windows\release'
} else {
  Resolve-AbsolutePath $OutputDir
}

if (-not $SkipBuild) {
  $buildScript = Join-Path $repoRoot 'scripts\build-windows.ps1'
  Invoke-Step powershell -NoProfile -ExecutionPolicy Bypass -File $buildScript desktop
}

New-Item -ItemType Directory -Path $resolvedOutputDir -Force | Out-Null

Assert-PackageRoot $resolvedPackageRoot
Invoke-PackageVerifier $resolvedPackageRoot

$portableZip = Join-Path $resolvedOutputDir "EXV-$Version-windows-x64-portable.zip"
$installerExe = Join-Path $resolvedOutputDir "EXV-$Version-windows-x64-setup.exe"

New-PortableZip -Root $resolvedPackageRoot -Destination $portableZip
Test-PortableZip -Archive $portableZip -RuntimeDir $resolvedPackageRoot

$makeNsis = Resolve-MakeNsis $NsisPath
Invoke-Nsis -MakeNsis $makeNsis -SourceDir $resolvedPackageRoot -OutputFile $installerExe -AppVersion $Version
Assert-InstallerOutput $installerExe

Write-Host ''
Write-Host 'Windows release artifacts:' -ForegroundColor Cyan
Write-Host "  Portable: $portableZip"
Write-Host "  Installer: $installerExe"
```

- [ ] **Step 2: Add the NSIS installer script**

Create `distribution/windows/exv.nsi` with this content:

```nsis
Unicode true
ManifestDPIAware true
RequestExecutionLevel user

!ifndef APP_VERSION
  !error "APP_VERSION define is required"
!endif

!ifndef SOURCE_DIR
  !error "SOURCE_DIR define is required"
!endif

!ifndef OUTPUT_FILE
  !error "OUTPUT_FILE define is required"
!endif

!ifndef DEFAULT_INSTALL_DIR
  !define DEFAULT_INSTALL_DIR "$LOCALAPPDATA\Programs\EXV"
!endif

!define APP_NAME "EXV"
!define COMPANY_NAME "EXV"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\EXV"

Name "${APP_NAME}"
Caption "${APP_NAME} Setup"
OutFile "${OUTPUT_FILE}"
InstallDir "${DEFAULT_INSTALL_DIR}"
InstallDirRegKey HKCU "Software\EXV" "InstallDir"
Icon "..\..\assets\icons\icon.ico"
UninstallIcon "..\..\assets\icons\icon.ico"

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "Install"
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  File /r "${SOURCE_DIR}\*.*"

  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\EXV" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${COMPANY_NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\exv-ui.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1

  CreateDirectory "$SMPROGRAMS\EXV"
  CreateShortCut "$SMPROGRAMS\EXV\EXV.lnk" "$INSTDIR\exv-ui.exe" "" "$INSTDIR\exv-ui.exe"
  CreateShortCut "$SMPROGRAMS\EXV\Uninstall EXV.lnk" "$INSTDIR\Uninstall.exe" "" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  Delete "$SMPROGRAMS\EXV\EXV.lnk"
  Delete "$SMPROGRAMS\EXV\Uninstall EXV.lnk"
  RMDir "$SMPROGRAMS\EXV"

  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey /ifempty HKCU "Software\EXV"

  RMDir /r "$INSTDIR"
SectionEnd
```

- [ ] **Step 3: Run the TypeScript host test and verify GREEN for script policy**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts
```

Expected: It may still FAIL on README/build guide/runtime asset assertions until Task 4, but the new release script and NSIS assertions should pass. If any release script or NSIS assertion fails, adjust the two new files before continuing.

- [ ] **Step 4: Run the C++ policy test and verify script policy**

Run:

```powershell
cmake --build --preset windows-release --target native_packaging_policy_test
.\build-windows\cpp\native_packaging_policy_test.exe
```

Expected: It may still FAIL only on documentation assertions from the TypeScript test path, not in this C++ executable. The C++ executable should pass release script existence, native WebView package policy, and denied payload scans.

- [ ] **Step 5: Commit release script and NSIS implementation**

```powershell
git add scripts\package-windows-release.ps1 distribution\windows\exv.nsi
git commit -m "feat: add windows release packaging scripts"
```

## Task 3: Add Portable and Installer Verification Hooks

**Files:**
- Modify: `webui/host/__tests__/webview-package-policy.test.ts`

This task tightens release verification after the initial scripts exist. The implementation in Task 2 already includes the verification functions; this task adds explicit assertions that keep those functions from being removed or weakened.

- [ ] **Step 1: Add host assertions for portable smoke and installer static checks**

In the `"keeps Windows release packaging on native WebView package outputs"` test in `webui/host/__tests__/webview-package-policy.test.ts`, add these assertions after `assert.match(releaseScript, /windows-packaging-smoke\.ps1/)`:

```ts
    assert.match(releaseScript, /function Test-PortableZip/)
    assert.match(releaseScript, /Portable zip must contain exactly one top-level EXV directory/)
    assert.match(releaseScript, /function Assert-InstallerOutput/)
    assert.match(releaseScript, /Installer is empty/)
    assert.match(releaseScript, /Install NSIS, add makensis\.exe to PATH, set NSIS_MAKENSIS, or pass -NsisPath/)
```

- [ ] **Step 2: Run host test to verify GREEN for verification policy**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts
```

Expected: It may still FAIL only on README/build guide/runtime asset assertions until Task 4. The added release verification assertions should pass.

- [ ] **Step 3: Run a no-build negative check for missing package root**

Run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package-windows-release.ps1 -Version 0.0.0-test -SkipBuild -PackageRoot build\windows\webview\package\DOES-NOT-EXIST -OutputDir build\windows\release-test
```

Expected: FAIL with `Package root does not exist`. This proves the script validates the source package before attempting zip or NSIS output.

- [ ] **Step 4: Commit verification assertions**

```powershell
git add webui\host\__tests__\webview-package-policy.test.ts
git commit -m "test: require windows release verification hooks"
```

## Task 4: Document Windows Release Packaging

**Files:**
- Modify: `README.md`
- Modify: `docs/build_guide.md`
- Modify: `docs/runtime-assets.md`

- [ ] **Step 1: Update README release artifact guidance**

In `README.md`, after the package contents list that ends with `WebView2Loader.dll`, add:

````markdown
Windows release packaging produces user-facing artifacts under:

```text
build\windows\release\
```

Expected Windows release files:

- `EXV-<version>-windows-x64-portable.zip`: portable archive containing a single `EXV\` directory.
- `EXV-<version>-windows-x64-setup.exe`: per-user NSIS installer for `%LOCALAPPDATA%\Programs\EXV`.

The installer does not bundle Microsoft Edge WebView2 Evergreen Runtime and does not install the privileged helper during setup. WebView2 runtime detection and helper installation remain part of the app's controlled first-run flow.
````

- [ ] **Step 2: Update build guide Windows section**

In `docs/build_guide.md`, replace the Windows section with:

````markdown
## Windows

Build the native WebView desktop package:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 desktop
```

Build Windows release artifacts:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\package-windows-release.ps1 -Version 0.1.0
```

Release outputs:

```text
build\windows\release\EXV-0.1.0-windows-x64-portable.zip
build\windows\release\EXV-0.1.0-windows-x64-setup.exe
```

The installer build requires NSIS `makensis.exe`. Put `makensis.exe` on `PATH`, set `NSIS_MAKENSIS`, or pass `-NsisPath`:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\package-windows-release.ps1 -Version 0.1.0 -NsisPath "C:\Program Files (x86)\NSIS\makensis.exe"
```

Focused package smoke:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows-packaging-smoke.ps1
```

The release script also expands the portable zip into a temporary directory and runs `windows-packaging-smoke.ps1` against the extracted `EXV` directory.

Merge-prep validation:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\validate-merge-prep-windows.ps1
```
````

- [ ] **Step 3: Update runtime asset policy**

In `docs/runtime-assets.md`, after the allowlist section, add:

````markdown
Windows release packaging uses the same allowlist through `scripts/package_ui_shell.py`.
The release script packages the already-built `build\windows\webview\package\EXV`
directory into a portable zip and an NSIS installer; it must not copy runtime
directories directly.

The Windows installer does not bundle Microsoft Edge WebView2 Evergreen Runtime.
`exv-ui.exe` keeps responsibility for detecting a missing WebView2 Evergreen
Runtime and guiding the controlled bootstrap flow.
````

- [ ] **Step 4: Run host test and verify docs GREEN**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts
```

Expected: PASS for the Windows release packaging policy test. If other unrelated host policy assertions fail, inspect the failure and fix only if caused by this task.

- [ ] **Step 5: Commit documentation**

```powershell
git add README.md docs\build_guide.md docs\runtime-assets.md
git commit -m "docs: document windows release packaging"
```

## Task 5: End-to-End Release Verification

**Files:**
- No source files expected unless verification exposes a defect.

- [ ] **Step 1: Run focused policy tests**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts
cmake --build --preset windows-release --target native_packaging_policy_test
.\build-windows\cpp\native_packaging_policy_test.exe
```

Expected:

- Host package policy test passes.
- `native_packaging_policy_test.exe` exits 0.

- [ ] **Step 2: Build Windows release artifacts**

Run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package-windows-release.ps1 -Version 0.0.0-local
```

Expected:

- Existing `scripts\build-windows.ps1 desktop` completes.
- Existing package verifier prints `verified native WebView shell package`.
- Portable zip is created at `build\windows\release\EXV-0.0.0-local-windows-x64-portable.zip`.
- Portable zip extraction smoke runs and exits 0, allowing skipped helper-service checks if the helper service is not installed or running.
- NSIS setup is created at `build\windows\release\EXV-0.0.0-local-windows-x64-setup.exe`.
- Script prints both absolute artifact paths.

- [ ] **Step 3: Run static artifact checks**

Run:

```powershell
Test-Path build\windows\release\EXV-0.0.0-local-windows-x64-portable.zip
Test-Path build\windows\release\EXV-0.0.0-local-windows-x64-setup.exe
(Get-Item build\windows\release\EXV-0.0.0-local-windows-x64-portable.zip).Length -gt 0
(Get-Item build\windows\release\EXV-0.0.0-local-windows-x64-setup.exe).Length -gt 0
```

Expected: all four commands print `True`.

- [ ] **Step 4: Run whitespace check**

Run:

```powershell
git diff --check
```

Expected: no output and exit code 0.

- [ ] **Step 5: Remove generated release artifacts after verification**

Because `docs\agent_rules.md` asks agents to keep synced build outputs light, remove generated release test artifacts after recording the result:

```powershell
if (Test-Path build\windows\release) { Remove-Item -LiteralPath build\windows\release -Recurse -Force }
```

Expected: `build\windows\release` no longer exists.

- [ ] **Step 6: Commit any verification fixes**

If verification required source fixes, stage only source changes:

```powershell
git add scripts\package-windows-release.ps1 distribution\windows\exv.nsi tests\native_packaging_policy_test.cpp webui\host\__tests__\webview-package-policy.test.ts README.md docs\build_guide.md docs\runtime-assets.md
git commit -m "fix: harden windows release packaging verification"
```

If no source fixes were needed after Task 4, skip this commit step.

## Optional Manual Installer Live Check

Run this only on an isolated Windows account or VM because it writes current-user install and uninstall state:

```powershell
$installRoot = Join-Path $env:TEMP 'EXV-install-test'
& build\windows\release\EXV-0.0.0-local-windows-x64-setup.exe /S "/D=$installRoot"
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-packaging-smoke.ps1 -PackageRoot $installRoot
& "$installRoot\Uninstall.exe" /S
if (Test-Path $installRoot) { Remove-Item -LiteralPath $installRoot -Recurse -Force }
```

Expected:

- Silent install creates `$installRoot\exv-ui.exe`.
- Smoke passes or passes with skipped helper-service checks.
- Silent uninstall removes installed application files.

## Final Verification Checklist

- `pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts` passes.
- `.\build-windows\cpp\native_packaging_policy_test.exe` passes.
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package-windows-release.ps1 -Version 0.0.0-local` produces both release artifacts when NSIS is available.
- `git diff --check` passes.
- Generated `build\windows\release` artifacts are removed after verification unless the user asks to keep them.

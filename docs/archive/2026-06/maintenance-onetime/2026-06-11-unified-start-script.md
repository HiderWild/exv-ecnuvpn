# Unified Start Script Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make root `start.ps1` the single no-argument debug entrypoint that kills leftovers, cleans stale outputs, rebuilds from zero, deploys native artifacts to Electron runtime locations, validates binary consistency, and launches the debug app.

**Architecture:** Consolidate orchestration in root `start.ps1`. Keep `scripts/restart-debug-electron.ps1` as a thin compatibility wrapper that forwards all arguments to `start.ps1`. Add a static PowerShell contract test so future changes cannot silently remove full rebuild, native staging, packaging modes, cleanup, or binary consistency checks.

**Tech Stack:** PowerShell 7+/Windows PowerShell-compatible script, CMake presets, pnpm-only WebUI workflow, Electron dev scripts, static PowerShell contract test.

---

### Task 1: Add script contract regression test

**Files:**
- Create: `tests/start_script_contract.ps1`

- [x] **Step 1: Write the failing test**

Create a PowerShell test that reads `start.ps1` and `scripts/restart-debug-electron.ps1` and verifies the required contract:

```powershell
# tests/start_script_contract.ps1
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$startPath = Join-Path $repoRoot 'start.ps1'
$restartPath = Join-Path $repoRoot 'scripts/restart-debug-electron.ps1'
$start = Get-Content -LiteralPath $startPath -Raw
$restart = Get-Content -LiteralPath $restartPath -Raw
$failures = New-Object System.Collections.Generic.List[string]
function Require-Contains($Text, $Needle, $Description) {
  if ($Text -notlike "*$Needle*") { $failures.Add($Description) }
}
Require-Contains $start 'param(' 'start.ps1 must define parameterized modes'
Require-Contains $start '[switch]$Quick' 'start.ps1 must expose -Quick'
Require-Contains $start '[switch]$NoLaunch' 'start.ps1 must expose -NoLaunch'
Require-Contains $start '[switch]$PackageDir' 'start.ps1 must expose -PackageDir'
Require-Contains $start '[switch]$Package' 'start.ps1 must expose -Package'
Require-Contains $start '[switch]$CleanOnly' 'start.ps1 must expose -CleanOnly'
Require-Contains $start '[switch]$Status' 'start.ps1 must expose -Status'
Require-Contains $start 'cmake --preset windows-release' 'start.ps1 must configure C++ from preset'
Require-Contains $start 'cmake --build --preset windows-release' 'start.ps1 must build C++ from preset'
Require-Contains $start 'pnpm run prepare:native' 'start.ps1 must stage native binaries through prepare:native'
Require-Contains $start 'pnpm run desktop:package:dir' 'start.ps1 must expose package-dir mode'
Require-Contains $start 'pnpm run desktop:package' 'start.ps1 must expose package mode'
Require-Contains $start 'build-windows' 'start.ps1 must clean/regenerate build-windows'
Require-Contains $start 'build/windows/electron/bin' 'start.ps1 must manage legacy Electron bin deployment'
Require-Contains $start 'build/windows/electron/native/bin' 'start.ps1 must validate Electron native bin deployment'
Require-Contains $start 'webui/native/bin' 'start.ps1 must clean stale legacy native bin outputs'
Require-Contains $start 'Get-FileHash' 'start.ps1 must print binary hashes for consistency checks'
Require-Contains $start 'desktop-rpc service.status' 'start.ps1 must probe runtime-selected native binary behavior'
Require-Contains $restart '..\start.ps1' 'restart-debug-electron.ps1 must delegate to root start.ps1'
if ($restart -match 'cmake\s+--preset|pnpm\s+run\s+desktop:dev') {
  $failures.Add('restart-debug-electron.ps1 must not keep independent build/launch logic')
}
if ($start -match '\bnpm\b|\bnpx\b') {
  $failures.Add('start.ps1 must use pnpm only, not npm/npx')
}
if ($failures.Count -gt 0) {
  Write-Host 'start script contract failed:' -ForegroundColor Red
  foreach ($failure in $failures) { Write-Host " - $failure" -ForegroundColor Red }
  exit 1
}
Write-Host 'start script contract passed.' -ForegroundColor Green
```

- [x] **Step 2: Run test to verify it fails**

Run: `pwsh -NoProfile -File tests/start_script_contract.ps1`
Expected: FAIL because current `start.ps1` is a legacy launch-only script and `restart-debug-electron.ps1` still contains independent logic.

### Task 2: Replace `start.ps1` with canonical orchestrator

**Files:**
- Modify: `start.ps1`

- [ ] **Step 1: Implement parameterized modes**

Add switches: `-Quick`, `-NoFrontendBuild`, `-NoLaunch`, `-PackageDir`, `-Package`, `-CleanOnly`, `-Status`, `-Port`.

- [ ] **Step 2: Implement cleanup**

Stop service/processes and release port. In full mode remove stale generated locations: `build-windows`, `build/windows/electron/bin`, `build/windows/electron/native/bin`, `webui/native/bin`, `webui/dist`, `webui/dist-electron`, and debug logs.

- [ ] **Step 3: Implement rebuild/deploy**

Run CMake configure/build, then WebUI `pnpm install`, `pnpm run build`, `pnpm run build:electron`, `pnpm run prepare:native`. Copy native binaries to compatibility `build/windows/electron/bin` after staging.

- [ ] **Step 4: Implement validation**

Print hashes and run selected `exv.exe desktop-rpc service.status "{}"` probes before launch. Fail if expected `exv.exe`/`exv-helper.exe` are absent.

- [ ] **Step 5: Implement launch/package modes**

`-PackageDir` runs `pnpm run desktop:package:dir`; `-Package` runs `pnpm run desktop:package`; default launches `pnpm run desktop:dev` and waits for Vite/Electron.

### Task 3: Collapse old restart script into wrapper

**Files:**
- Modify: `scripts/restart-debug-electron.ps1`

- [ ] Replace body with a forwarding wrapper:

```powershell
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$startScript = Join-Path $repoRoot 'start.ps1'
& $startScript @args
exit $LASTEXITCODE
```

### Task 4: Verify

**Files:**
- Test: `tests/start_script_contract.ps1`

- [ ] Run `pwsh -NoProfile -File tests/start_script_contract.ps1` and expect PASS.
- [ ] Run PowerShell parser checks for both scripts.
- [ ] Run `./start.ps1 -Status` and expect it only prints state.
- [ ] Do not run full no-arg launch unless the user confirms because it kills processes and rebuilds.

### Self-review

- Spec coverage: covers unified entrypoint, full cleanup, rebuild, deployment, packaging modes, validation, wrapper compatibility.
- Placeholder scan: no TBD/TODO placeholders.
- Type consistency: PowerShell switch names match across test, plan, and implementation.
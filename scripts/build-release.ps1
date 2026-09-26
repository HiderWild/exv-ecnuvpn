# build-release.ps1 - One-click EXV release build (assumes toolchain is ready).
#
# Orchestrates the full Windows release chain from a clean temporary git worktree:
#   frontend npm ci + vite build -> assemble runtime trio -> build setup tools
#   -> package installer -> verify payload (inspect-windows-setup.py).
#
# Environment prerequisites (NOT installed by this script):
#   git, cargo/rustc, node/npm, cmake + Ninja, and Python 3 for payload inspection.
# The script builds from the committed HEAD only; uncommitted changes are ignored.

[CmdletBinding()]
param(
  [string]$Version = '',
  [string]$OutDir = '',
  [ValidateRange(1, 64)]
  [int]$Jobs = 4
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot

# ---------------------------------------------------------------------------
# Environment checks: fail fast with a clear list instead of deep inside cargo.
# ---------------------------------------------------------------------------
$required = @('git', 'cargo', 'rustc', 'node', 'npm', 'cmake', 'ninja')
$missing = @()
foreach ($name in $required) {
  if (-not (Get-Command $name -ErrorAction SilentlyContinue)) { $missing += $name }
}
if ($missing.Count -gt 0) {
  throw "missing required tools (install them first, this script does not set up environments): $($missing -join ', ')"
}
$python = @('python', 'python3', 'py') | Where-Object { Get-Command $_ -ErrorAction SilentlyContinue } | Select-Object -First 1
if (-not $python) { throw 'missing python3 (required for payload inspection)' }
$pythonArgs = @()
if ($python -eq 'py') { $pythonArgs = @('-3') }

# Resolve the version from the committed tauri.conf.json BEFORE any build step,
# and pass it explicitly downstream, so version resolution never depends on the
# state of the tree mid-build.
if ([string]::IsNullOrWhiteSpace($Version)) {
  $confPath = Join-Path $repoRoot 'src\platform\win32\rust\tauri\app\tauri.conf.json'
  $conf = Get-Content -LiteralPath $confPath -Raw | ConvertFrom-Json
  $Version = [string]$conf.version
  if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
    throw "product version must use major.minor.patch: $Version"
  }
}
Write-Host "EXV release version: $Version"

if ([string]::IsNullOrWhiteSpace($OutDir)) {
  $OutDir = Join-Path $repoRoot 'build\release'
}
$OutDir = [IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
if (-not (Test-Path -LiteralPath $OutDir -PathType Container)) { throw "cannot create output dir: $OutDir" }

$worktree = Join-Path $repoRoot ('build\.release-worktree-' + [Guid]::NewGuid().ToString('N'))
$created = $false
try {
  # -------------------------------------------------------------------------
  # Temporary worktree: the packaging scripts refuse to run in the main
  # worktree, and a clean checkout guarantees we build committed state only.
  # -------------------------------------------------------------------------
  & git -C $repoRoot worktree add --detach $worktree HEAD
  if ($LASTEXITCODE -ne 0) { throw 'git worktree add failed' }
  $created = $true

  $frontend = Join-Path $worktree 'src\platform\win32\rust\tauri\frontend'
  & npm ci --prefer-offline --no-audit --no-fund --prefix $frontend
  if ($LASTEXITCODE -ne 0) { throw 'npm ci failed (frontend)' }

  $trio = Join-Path $worktree 'build\release-ui-merged\runtime-trio-current'
  $assembleArgs = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
    (Join-Path $worktree 'scripts\assemble-runtime-trio.ps1'), '-OutDir', $trio)
  & powershell @assembleArgs
  if ($LASTEXITCODE -ne 0) { throw 'assemble-runtime-trio failed' }

  $setupArgs = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
    (Join-Path $worktree 'scripts\build-rust-setup.ps1'),
    '-Jobs', [string]$Jobs, '-Version', $Version)
  & powershell @setupArgs
  if ($LASTEXITCODE -ne 0) { throw 'build-rust-setup failed' }

  # Packaging requires its output inside the worktree it runs from.
  $pkgOut = Join-Path $worktree 'build\release'
  $packageArgs = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
    (Join-Path $worktree 'scripts\package-rust-setup.ps1'),
    '-OutDir', $pkgOut, '-Version', $Version, '-KeepInstallers', '1')
  & powershell @packageArgs
  if ($LASTEXITCODE -ne 0) { throw 'package-rust-setup failed' }

  # -------------------------------------------------------------------------
  # Payload inspection: installer CRC/structure plus byte-match against the
  # freshly built trio and the bundled support script.
  # -------------------------------------------------------------------------
  $installer = Get-ChildItem -LiteralPath $pkgOut -Filter 'EXV-*-windows-x64-setup.exe' |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
  if (-not $installer) { throw 'installer not found after packaging' }
  $verify = Join-Path ([IO.Path]::GetTempPath()) ('exv-verify-' + [Guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path (Join-Path $verify 'support') -Force | Out-Null
  Copy-Item (Join-Path $trio '*') $verify -Force
  Copy-Item (Join-Path $worktree 'scripts\clear-local-user-config.ps1') (Join-Path $verify 'support') -Force
  $inspection = Join-Path $pkgOut 'inspection.json'
  & $python @pythonArgs (Join-Path $worktree 'scripts\inspect-windows-setup.py') `
    --installer $installer.FullName --source-dir $verify --json-output $inspection
  if ($LASTEXITCODE -ne 0) { throw 'inspect-windows-setup failed' }
  $report = Get-Content -LiteralPath $inspection -Raw | ConvertFrom-Json
  if (-not $report.verified) { throw "payload verification failed, see $inspection" }
  Remove-Item -LiteralPath $verify -Recurse -Force -ErrorAction SilentlyContinue

  # Deliver artifacts to the caller's output directory.
  New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
  Copy-Item -LiteralPath $installer.FullName -Destination $OutDir -Force
  Copy-Item -LiteralPath $inspection -Destination $OutDir -Force
  $delivered = Join-Path $OutDir $installer.Name

  $hash = (Get-FileHash -LiteralPath $delivered -Algorithm SHA256).Hash
  Write-Host ''
  Write-Host ('installer: ' + $delivered)
  Write-Host ('sha256:    ' + $hash)
  Write-Host ('inspection:' + (Join-Path $OutDir 'inspection.json'))
  Write-Host 'build-release completed successfully.'
} finally {
  if ($created) {
    & git -C $repoRoot worktree remove --force $worktree 2>$null
    if (Test-Path -LiteralPath $worktree) {
      Write-Warning "worktree cleanup incomplete: $worktree"
    }
  }
}

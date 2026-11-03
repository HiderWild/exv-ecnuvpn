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
. (Join-Path $PSScriptRoot 'product-version.ps1')
$Version = Get-ExvProductVersion $repoRoot $Version

# ---------------------------------------------------------------------------
# Environment checks: fail fast with a clear list instead of deep inside cargo.
# ---------------------------------------------------------------------------
$required = @('git', 'cargo', 'rustc', 'sccache', 'node', 'npm', 'cmake', 'ninja')
$missing = @()
foreach ($name in $required) {
  if (-not (Get-Command $name -ErrorAction SilentlyContinue)) { $missing += $name }
}
if ($missing.Count -gt 0) {
  throw "missing required tools (install them first, this script does not set up environments): $($missing -join ', ')"
}
$env:RUSTC_WRAPPER = 'sccache'
& (Join-Path $PSScriptRoot 'clear-repository-build-artifacts.ps1') -KeepLatestInstaller
$python = @('python', 'python3', 'py') | Where-Object { Get-Command $_ -ErrorAction SilentlyContinue } | Select-Object -First 1
if (-not $python) { throw 'missing python3 (required for payload inspection)' }
$pythonArgs = @()
if ($python -eq 'py') { $pythonArgs = @('-3') }

# The root product metadata is checked before any build and passed downstream.
Write-Host "EXV release version: $Version"

# T7（连接痕迹计划）：engine crate 版本必须与产品版本一致——
# version_marker 的「版本升级首连强制预检」以 engine CARGO_PKG_VERSION 为比较基准；
# 发布不 bump crate 版本则该触发器永不触发，发布流程在此强制校验。
$engineManifest = Join-Path $repoRoot 'src\platform\win32\rust\crates\exv-vpn-win32-engine\Cargo.toml'
$engineVersion = (Select-String -LiteralPath $engineManifest -Pattern '^\s*version\s*=\s*"([^"]+)"' |
  Select-Object -First 1).Matches[0].Groups[1].Value
if ($engineVersion -ne $Version) {
  throw "engine crate version ($engineVersion) must equal product version ($Version); bump src\platform\win32\rust\crates\exv-vpn-win32-engine\Cargo.toml before releasing"
}

if ([string]::IsNullOrWhiteSpace($OutDir)) {
  . (Join-Path $PSScriptRoot 'shared-artifact-root.ps1')
  $OutDir = Get-ExvSharedArtifactRoot -RepositoryRoot $repoRoot
}
$OutDir = [IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
if (-not (Test-Path -LiteralPath $OutDir -PathType Container)) { throw "cannot create output dir: $OutDir" }

# Worktree discipline (AGENTS.md): every worktree must live under .worktrees/
# of the MAIN repository, not under build\.  Resolve the main repo root from
# the git common dir (this script may itself run inside a linked worktree);
# when resolution fails, fall back to this checkout's own root, which keeps
# the worktree inside a .worktrees\ directory either way.
$gitCommonDir = $null
try {
  $gitCommonDir = (& git -C $repoRoot rev-parse --path-format=absolute --git-common-dir 2>$null |
    Select-Object -First 1)
} catch { $gitCommonDir = $null }
$mainRepoRoot = $repoRoot
if ($gitCommonDir -and -not [string]::IsNullOrWhiteSpace([string]$gitCommonDir)) {
  $candidate = Split-Path -Parent ([string]$gitCommonDir).Trim()
  if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Container)) { $mainRepoRoot = $candidate }
}
$worktreesRoot = Join-Path $mainRepoRoot '.worktrees'
New-Item -ItemType Directory -Path $worktreesRoot -Force | Out-Null
# Builds are serial. A stable source path lets sccache reuse compilation inputs
# across versions; an existing worktree is reported by git rather than removed.
$worktree = Join-Path $worktreesRoot '.release-build'
$created = $false
$originalEncodedRustFlags = [Environment]::GetEnvironmentVariable('CARGO_ENCODED_RUSTFLAGS', 'Process')
try {
  # -------------------------------------------------------------------------
  # Temporary worktree: the packaging scripts refuse to run in the main
  # worktree, and a clean checkout guarantees we build committed state only.
  # -------------------------------------------------------------------------
  # sccache guard (2026-10-08): the persistent sccache server inherits TMP
  # from wherever it started.  A server surviving an earlier throwaway
  # worktree keeps compiling against that worktree's deleted TMP and this
  # build dies with icu compile failures + link.exe LNK1104.  Stop any
  # running server before and after the worktree's lifetime; the disk cache
  # survives and the server restarts on demand (this is NOT a sccache
  # bypass).  Hosts without sccache on PATH (public tree) skip this.  Do not
  # run parallel builds on the same host while this script runs.
  $sccache = Get-Command sccache -CommandType Application -ErrorAction SilentlyContinue
  if ($null -ne $sccache) {
    try { & $sccache.Source --stop-server 2>$null } catch { Write-Host "sccache pre-clear: $($_.Exception.Message)" }
    Write-Host 'sccache stale-server guard: stop requested (pre-worktree)'
  }
  & git -C $repoRoot worktree add --detach $worktree HEAD
  if ($LASTEXITCODE -ne 0) { throw 'git worktree add failed' }
  $created = $true
  $mappedFlags = & $python @pythonArgs (Join-Path $worktree 'scripts/release.py') --root $worktree build-flags
  if ($LASTEXITCODE -ne 0) { throw 'cannot prepare release compiler path mappings' }
  $env:CARGO_ENCODED_RUSTFLAGS = [string]$mappedFlags

  # .cargo/config.toml force-pins TMP/TEMP to rust\.build-tmp（untracked+ignored，
  # worktree 带不出来）；link.exe 的 lnk*.tmp 落在该目录，不存在即 LNK1104。
  # 已注册工作树由清理脚本预备；本临时 worktree 在此自备。
  $buildTmp = Join-Path $worktree 'src\platform\win32\rust\.build-tmp'
  New-Item -ItemType Directory -Path $buildTmp -Force | Out-Null

  $frontend = Join-Path $worktree 'src\platform\win32\rust\tauri\frontend'
  & npm ci --prefer-offline --no-audit --no-fund --prefix $frontend
  if ($LASTEXITCODE -ne 0) { throw 'npm ci failed (frontend)' }

  # Cross-platform settings tests import Darwin frontend source during vue-tsc.
  # Install its locked dependencies beside that source in the clean worktree.
  $darwinFrontend = Join-Path $worktree 'src\platform\darwin\rust\tauri\frontend'
  & npm ci --prefer-offline --no-audit --no-fund --prefix $darwinFrontend
  if ($LASTEXITCODE -ne 0) { throw 'npm ci failed (Darwin frontend)' }

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
  # S3a：载荷新增 support\install_manifest.tsv（文件清单库），核验 staging 必须同步
  # 提供同源副本，否则 source_match=false 拖垮整体 verified（W5 集成实录）。
  $manifest = Join-Path $worktree 'src\platform\win32\windows_setup_rust\install_manifest.tsv'
  if (Test-Path -LiteralPath $manifest) {
    Copy-Item $manifest (Join-Path $verify 'support') -Force
  }
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
  [Environment]::SetEnvironmentVariable('CARGO_ENCODED_RUSTFLAGS', $originalEncodedRustFlags, 'Process')
  if ($created) {
    & git -C $repoRoot worktree remove --force $worktree 2>$null
    if (Test-Path -LiteralPath $worktree) {
      Write-Warning "worktree cleanup incomplete: $worktree"
    }
    # Post-worktree sccache guard: the worktree (and the rust\.build-tmp TMP
    # root the build ran against) is gone; make sure no server survives
    # holding that now-dead TMP for the next build on this host.
    $sccache = Get-Command sccache -CommandType Application -ErrorAction SilentlyContinue
    if ($null -ne $sccache) {
      try { & $sccache.Source --stop-server 2>$null } catch { Write-Host "sccache post-clear: $($_.Exception.Message)" }
      Write-Host 'sccache stale-server guard: stop requested (post-worktree)'
    }
  }
  & (Join-Path $PSScriptRoot 'clear-repository-build-artifacts.ps1') -KeepLatestInstaller
}
